use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

// 依存ライブラリの名前
const LIB_NAME: &str = "aom";
const LINK_NAME: &str = "aom";

// シンボル書き換え用のプレフィックス
//
// prebuilt で配布する際、他のライブラリが同じ libaom シンボル (aom_codec_encode 等) を
// 使っていると衝突する。この定数のプレフィックスを付けることで回避する。
//
// 変換例:
//   aom_codec_encode → shiguredo_aom_codec_encode (aom_ を shiguredo_aom_ に置換)
//   av1_internal_func → shiguredo_aom_av1_internal_func (内部シンボルは単純にプレフィックス付与)
const SYMBOL_PREFIX: &str = "shiguredo_aom";

fn main() {
    // Cargo.toml / build.rs / docs.rs 用ダミー bindings が更新されたら、依存ライブラリを再ビルドする。
    // build/dummy_bindings.rs は DOCS_RS ビルド専用だが、変更を確実に反映するため
    // 通常ビルドでも再ビルドを誘発する。
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build/dummy_bindings.rs");
    println!("cargo::rerun-if-env-changed=CARGO_FEATURE_SOURCE_BUILD");
    println!("cargo::rerun-if-env-changed=LIBAOM_TARGET");
    println!("cargo::rerun-if-env-changed=DOCS_RS");
    println!("cargo::rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
    println!("cargo::rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo::rerun-if-env-changed=ANDROID_NDK_HOME");
    println!("cargo::rerun-if-env-changed=ANDROID_PLATFORM");
    // docs.rs 向けダミー bindings か実 bindings かで発火する lint が異なるため、
    // sys.rs の lint 抑止を切り替える cfg を用意する
    println!("cargo::rustc-check-cfg=cfg(docs_rs)");

    // 各種変数やビルドディレクトリのセットアップ
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("infallible"));
    let output_metadata_path = out_dir.join("metadata.rs");
    let output_bindings_path = out_dir.join("bindings.rs");

    // 各種メタデータを書き込む
    let (git_url, version) = get_git_url_and_version();
    fs::write(
        output_metadata_path,
        format!(
            concat!(
                "pub const BUILD_METADATA_REPOSITORY: &str={:?};\n",
                "pub const BUILD_METADATA_VERSION: &str={:?};\n",
            ),
            git_url, version
        ),
    )
    .expect("failed to write metadata file");

    if env::var("DOCS_RS").is_ok() {
        // Docs.rs 向けのビルドでは git clone ができないので build.rs の処理はスキップして、
        // 代わりに、src/lib.rs が参照するシンボルを網羅したダミー bindings を出力している。
        //
        // ダミー bindings は build/dummy_bindings.rs に置き、source-build で生成した
        // bindings.rs から src/lib.rs が使用するシンボルとその依存を抽出したもの。
        // 定数値は libaom v3.15.1 の実値を反映している。
        //
        // 参照: https://docs.rs/about/builds
        //
        // sys.rs の lint 抑止をダミー bindings 用に切り替える
        println!("cargo::rustc-cfg=docs_rs");
        fs::write(
            output_bindings_path,
            include_str!("build/dummy_bindings.rs"),
        )
        .expect("write file error");
        return;
    }

    let output_lib_dir = if should_use_prebuilt() {
        download_prebuilt(&out_dir)
    } else {
        build_from_source(&out_dir, &output_bindings_path)
    };

    println!("cargo::rustc-link-search={}", output_lib_dir.display());
    println!("cargo::rustc-link-lib=static={LINK_NAME}");

    // libaom は C++ の標準ライブラリに依存する場合がある
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo::rustc-link-lib=c++");
    } else if target_os == "linux" {
        println!("cargo::rustc-link-lib=stdc++");
        println!("cargo::rustc-link-lib=m");
        println!("cargo::rustc-link-lib=pthread");
    }
}

// source-build feature が有効でなければ prebuilt を使う
fn should_use_prebuilt() -> bool {
    if env::var("CARGO_FEATURE_SOURCE_BUILD").is_ok() {
        return false;
    }
    true
}

// prebuilt バイナリをダウンロードして展開する
fn download_prebuilt(out_dir: &Path) -> PathBuf {
    let target = get_target_platform();
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is not set");
    let base_url = format!(
        "https://github.com/shiguredo/aom-rs/releases/download/{}",
        version
    );
    let archive_name = format!("libaom-{}.tar.gz", target);
    let archive_url = format!("{}/{}", base_url, archive_name);
    let sha256_url = format!("{}/{}.sha256", base_url, archive_name);

    let archive_path = out_dir.join("prebuilt.tar.gz");
    let sha256_path = out_dir.join("prebuilt.sha256");
    let prebuilt_dir = out_dir.join("prebuilt");
    fs::create_dir_all(&prebuilt_dir).expect("failed to create prebuilt directory");

    // curl でアーカイブをダウンロード
    eprintln!("downloading prebuilt library: {}", archive_url);
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&archive_path)
        .arg(&archive_url)
        .status()
        .expect("failed to execute curl. Ensure curl is installed");
    if !status.success() {
        panic!("failed to download prebuilt library: {}", archive_url);
    }

    // curl で SHA256 チェックサムをダウンロード
    let status = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&sha256_path)
        .arg(&sha256_url)
        .status()
        .expect("failed to execute curl");
    if !status.success() {
        panic!("failed to download SHA256 checksum: {}", sha256_url);
    }

    // SHA256 を検証
    verify_sha256(&archive_path, &sha256_path);

    // tar で展開
    let status = Command::new("tar")
        .args(["xzf"])
        .arg(&archive_path)
        .arg("-C")
        .arg(&prebuilt_dir)
        .status()
        .expect("failed to execute tar. Ensure tar is installed");
    if !status.success() {
        panic!("failed to extract prebuilt archive");
    }

    // ライブラリファイルを OUT_DIR/lib/ にコピー
    //
    // prebuilt アーカイブにはリリースビルド時にシンボル書き換え済みのライブラリと
    // #[link_name] 属性付きの bindings.rs が含まれているため、そのままコピーするだけでよい。
    //
    // MSVC ビルドでは aom.lib、その他では libaom.a として出力されるため、
    // アーカイブ内の実際のファイル名に応じてコピーする。
    let lib_dir = out_dir.join("lib");
    fs::create_dir_all(&lib_dir).expect("failed to create lib directory");
    let prebuilt_lib_dir = prebuilt_dir.join("lib");
    if prebuilt_lib_dir.join("aom.lib").exists() {
        fs::copy(prebuilt_lib_dir.join("aom.lib"), lib_dir.join("aom.lib"))
            .expect("failed to copy aom.lib");
    } else {
        fs::copy(prebuilt_lib_dir.join("libaom.a"), lib_dir.join("libaom.a"))
            .expect("failed to copy libaom.a");
    }

    // bindings.rs を OUT_DIR/ にコピー
    fs::copy(
        prebuilt_dir.join("bindings.rs"),
        out_dir.join("bindings.rs"),
    )
    .expect("failed to copy bindings.rs");

    lib_dir
}

// SHA256 チェックサムを検証する
fn verify_sha256(file_path: &Path, sha256_path: &Path) {
    let expected = fs::read_to_string(sha256_path)
        .expect("failed to read SHA256 checksum file")
        .split_whitespace()
        .next()
        .expect("SHA256 checksum file is empty")
        .to_lowercase();

    let actual = compute_sha256(file_path);
    if actual != expected {
        panic!(
            "SHA256 checksum mismatch:\n  expected: {}\n  actual:   {}",
            expected, actual
        );
    }
    eprintln!("SHA256 checksum verified: {}", actual);
}

// ファイルの SHA256 ハッシュを計算する
fn compute_sha256(path: &Path) -> String {
    let output = if cfg!(target_os = "macos") {
        // macOS: shasum を使用
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
            .expect("failed to execute shasum. Ensure shasum is installed")
    } else if cfg!(target_os = "windows") {
        // Windows: certutil を使用
        Command::new("certutil")
            .args(["-hashfile"])
            .arg(path)
            .arg("SHA256")
            .output()
            .expect("failed to execute certutil")
    } else {
        // Linux: sha256sum を使用
        Command::new("sha256sum")
            .arg(path)
            .output()
            .expect("failed to execute sha256sum. Ensure coreutils is installed")
    };

    if !output.status.success() {
        panic!("failed to compute SHA256 checksum");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if cfg!(target_os = "windows") {
        // certutil 出力形式:
        // SHA256 hash of <file>:
        // <hash>
        // CertUtil: -hashfile command completed successfully.
        stdout
            .lines()
            .nth(1)
            .expect("unexpected certutil output format")
            .trim()
            .to_lowercase()
    } else {
        // shasum / sha256sum 出力形式: <hash>  <filename>
        stdout
            .split_whitespace()
            .next()
            .expect("unexpected shasum/sha256sum output format")
            .to_lowercase()
    }
}

// ソースからビルドする
fn build_from_source(out_dir: &Path, output_bindings_path: &Path) -> PathBuf {
    let out_build_dir = out_dir.join("build/");
    let _ = fs::remove_dir_all(&out_build_dir);
    fs::create_dir(&out_build_dir).expect("failed to create build directory");

    // 依存ライブラリのリポジトリを取得する
    git_clone_external_lib(&out_build_dir);

    // CMAKE 環境変数を設定
    shiguredo_cmake::set_cmake_env();

    // CMake でソースからビルドする
    let src_dir = out_build_dir.join(LIB_NAME);
    let mut cmake_config = shiguredo_cmake::Config::new(&src_dir);
    cmake_config
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("ENABLE_TESTS", "OFF")
        // aomenc / aomdec は不要なため無効化する。
        // 根拠: libaom v3.15.0 で ENABLE_APPS (aomenc / aomdec 用) が追加され、
        // ENABLE_EXAMPLES はサンプルプログラムのみをビルドする意味に再定義された。
        // v3.14.1 以前は ENABLE_EXAMPLES が aomenc / aomdec も制御していた。
        .define("ENABLE_APPS", "OFF")
        .define("ENABLE_EXAMPLES", "OFF")
        .define("ENABLE_TOOLS", "OFF")
        .define("ENABLE_DOCS", "OFF")
        .define("CONFIG_AV1_HIGHBITDEPTH", "1")
        .profile("Release");

    // モバイル向けの SDK、ABI、最小 OS バージョンを指定する
    let clang_args = configure_mobile_build(&mut cmake_config);
    let install_dir = cmake_config.build();

    let input_header_dir = install_dir.join("include/aom/");
    let output_lib_dir = install_dir.join("lib/");

    // 静的ライブラリのシンボルを書き換える
    let callbacks = rewrite_symbols(&output_lib_dir, out_dir);

    // バインディングを生成する
    //
    // parse_callbacks にシンボル書き換え用の ParseCallbacks を渡すことで、
    // 生成されるバインディングに #[link_name = "書き換え後のシンボル名"] が自動付与される。
    bindgen::Builder::default()
        .header(input_header_dir.join("aomcx.h").display().to_string())
        .header(input_header_dir.join("aomdx.h").display().to_string())
        .header(input_header_dir.join("aom_encoder.h").display().to_string())
        .header(input_header_dir.join("aom_decoder.h").display().to_string())
        .clang_arg(format!("-I{}", install_dir.join("include").display()))
        .clang_args(clang_args)
        .parse_callbacks(Box::new(callbacks))
        .generate()
        .expect("failed to generate bindings")
        .write_to_file(output_bindings_path)
        .expect("failed to write bindings");

    output_lib_dir
}

/// モバイル向けの CMake と bindgen に同じ SDK、ABI、最小 OS バージョンを指定する
fn configure_mobile_build(config: &mut shiguredo_cmake::Config) -> Vec<String> {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is not set");
    let target = env::var("TARGET").expect("TARGET is not set");

    match target_os.as_str() {
        "ios" => {
            // Rust のターゲットで実機とシミュレーターを区別する。
            // 根拠: rustc book の *-apple-ios / Requirements。仕様は将来変更される可能性がある。
            let (sdk, arch, simulator_suffix, aom_target_cpu, default_deployment_target) =
                match target.as_str() {
                    "aarch64-apple-ios" => ("iphoneos", "arm64", "", "arm64", "13.0"),
                    // arm64 シミュレーターは Rust と Clang のターゲット下限が iOS 14.0。
                    // 根拠: Rust 1.93 の OSVersion::minimum_deployment_target。
                    // ターゲットの下限は将来変更される可能性がある。
                    "aarch64-apple-ios-sim" => {
                        ("iphonesimulator", "arm64", "-simulator", "arm64", "14.0")
                    }
                    _ => panic!("unsupported iOS target: {target}"),
                };
            let deployment_target = env::var("IPHONEOS_DEPLOYMENT_TARGET")
                .unwrap_or_else(|_| default_deployment_target.to_string());
            let output = Command::new("xcrun")
                .args(["--sdk", sdk, "--show-sdk-path"])
                .output()
                .expect("failed to run xcrun. Ensure Xcode is installed");
            if !output.status.success() {
                panic!("failed to find iOS SDK: {sdk}");
            }
            let sdk_path = String::from_utf8(output.stdout).expect("invalid iOS SDK path");
            let sdk_path = sdk_path.trim();

            // libaom の CMake は AOM_TARGET_SYSTEM が Darwin のときだけ ARM アセンブリの
            // コンパイラと x86_64 の NASM オブジェクト形式 (macho64) を切り替える。
            // iOS でも Darwin として構成し、SDK とアーキテクチャで対象を分ける。
            // 根拠: libaom v3.15.1 の cmake/aom_configure.cmake と cmake/aom_optimization.cmake。
            // 実装は将来変更される可能性がある。
            //
            // cc クレートの既定フラグと CMake の SDK 選択が競合しないよう、
            // コンパイラのターゲット設定は CMake に任せる。
            config
                .no_default_flags(true)
                .define("CMAKE_SYSTEM_NAME", "Darwin")
                .define("CMAKE_OSX_SYSROOT", sdk_path)
                .define("CMAKE_OSX_ARCHITECTURES", arch)
                .define("CMAKE_OSX_DEPLOYMENT_TARGET", &deployment_target)
                // Darwin ホストでは CMAKE_SYSTEM_NAME が同じでも CMAKE_SYSTEM_PROCESSOR が
                // 空になり、libaom が CPU を generic と判定して SIMD が無効になる。
                // 根拠: libaom v3.15.1 の cmake/aom_configure.cmake の CPU 判定。
                // 実装は将来変更される可能性がある。
                .define("AOM_TARGET_CPU", aom_target_cpu);

            vec![
                format!("--target={arch}-apple-ios{deployment_target}{simulator_suffix}"),
                "-isysroot".to_string(),
                sdk_path.to_string(),
            ]
        }
        "android" => {
            let ndk = PathBuf::from(
                env::var_os("ANDROID_NDK_HOME")
                    .expect("ANDROID_NDK_HOME is not set. Set it to the Android NDK directory"),
            );
            let (abi, clang_target) = match target.as_str() {
                "aarch64-linux-android" => ("arm64-v8a", "aarch64-linux-android"),
                "x86_64-linux-android" => ("x86_64", "x86_64-linux-android"),
                _ => panic!("unsupported Android target: {target}"),
            };
            let platform = env::var("ANDROID_PLATFORM").unwrap_or_else(|_| "21".to_string());
            // bindgen の Clang ターゲットには API 名ではなく数値が必要なため、
            // CMake にも同じ数値を渡して NDK の API 名解決との差異を防ぐ。
            let api_level = platform
                .strip_prefix("android-")
                .unwrap_or(&platform)
                .parse::<u32>()
                .expect("ANDROID_PLATFORM must be a numeric API level or android-<API level>");
            // NDK が下限未満の API level を引き上げると bindgen と食い違うため、ここで拒否する。
            assert!(
                api_level >= 21,
                "ANDROID_PLATFORM must be at least API level 21"
            );
            let host_tag = match env::consts::OS {
                "linux" => "linux-x86_64",
                "macos" => "darwin-x86_64",
                "windows" => "windows-x86_64",
                os => panic!("unsupported Android NDK host: {os}"),
            };
            let toolchain = ndk.join("toolchains/llvm/prebuilt").join(host_tag);
            let sysroot = toolchain.join("sysroot");

            // cmake クレートもコンパイラ種別を調べるため、PATH 上のターゲット別
            // ラッパーを探索させずに NDK の実際のコンパイラを指定する。
            let mut c_config = cc::Build::new();
            c_config.compiler(toolchain.join("bin").join(exe_name("clang")));
            let mut cxx_config = cc::Build::new();
            cxx_config.compiler(toolchain.join("bin").join(exe_name("clang++")));

            // NDK が提供するツールチェーンを使い、CMake 独自の NDK 検出を避ける。
            // 根拠: Android NDK ガイドの CMake / The CMake toolchain file。
            // SDK の構成や引数は将来変更される可能性がある。
            config
                .init_c_cfg(c_config)
                .init_cxx_cfg(cxx_config)
                .define(
                    "CMAKE_TOOLCHAIN_FILE",
                    ndk.join("build/cmake/android.toolchain.cmake"),
                )
                .define("ANDROID_ABI", abi)
                .define("ANDROID_PLATFORM", api_level.to_string())
                // libaom は補助ライブラリ aom_av1_rc に C++ ソース (av1/ratectrl_rtc.cc)
                // を含むため、NDK の C++ 標準ライブラリを指定してビルドできるようにする。
                // 成果物の libaom.a 自体には C++ のオブジェクトは含まれない。
                .define("ANDROID_STL", "c++_static")
                // Android アプリの共有ライブラリに静的リンクするため PIC を有効にする。
                // NASM のアセンブリにも -DPIC が渡り、テキスト再配置を防げる。
                .define("CONFIG_PIC", "1");

            vec![
                format!("--target={clang_target}{api_level}"),
                format!("--sysroot={}", sysroot.display()),
            ]
        }
        _ => Vec::new(),
    }
}

// --- シンボル書き換え ---
//
// 他のライブラリとのシンボル衝突を回避するため、静的ライブラリ内の全シンボルに
// プレフィックスを付与する仕組み。
//
// llvm-nm / llvm-objcopy は rustup の llvm-tools コンポーネントに含まれるものを使用する。
// rust-toolchain.toml に components = ["llvm-tools"] の記載が必要。
//
// プラットフォームごとのシンボル形式の違い:
//   - macOS / iOS (Mach-O): シンボル先頭に `_` が付く (例: _aom_codec_encode)
//   - Linux (ELF): 先頭 `_` なし (例: aom_codec_encode)
//   - Windows x64 (COFF): 先頭 `_` なし (例: aom_codec_encode)
//
// bindgen の generated_link_name_override は返した文字列に \u{1} プレフィックスを
// 自動付加する。\u{1} はコンパイラに「この名前をそのまま使え（マングリングするな）」と
// 指示するため、プラットフォーム固有のシンボル名（macOS / iOS なら _shiguredo_aom_codec_encode）を
// そのまま返す必要がある。

/// llvm-nm / llvm-objcopy のパスを保持する
struct LlvmTools {
    nm: PathBuf,
    objcopy: PathBuf,
}

/// objcopy 用と bindgen 用の 2 つのリネームマップを保持する
///
/// 2 つのマップが必要な理由:
///   - objcopy_map: ライブラリ内の実シンボル名を書き換えるため、プラットフォーム依存の名前を使う
///   - bindgen_map: Rust コードからリンクする際の名前を指定するため、C シンボル名をキーにする
struct SymbolRenameMaps {
    /// llvm-objcopy の --redefine-syms 用マップ
    ///
    /// キー: 元のシンボル名 (例: macOS / iOS なら _aom_codec_encode、Linux なら aom_codec_encode)
    /// 値: 書き換え後のシンボル名 (例: macOS / iOS なら _shiguredo_aom_codec_encode)
    objcopy_map: HashMap<String, String>,

    /// bindgen の #[link_name] 用マップ
    ///
    /// キー: C シンボル名 (プラットフォーム非依存、例: aom_codec_encode)
    /// 値: 書き換え後のシンボル名 (プラットフォーム依存、例: macOS / iOS なら _shiguredo_aom_codec_encode)
    ///
    /// bindgen は \u{1} プレフィックスを付加してマングリングを抑制するため、
    /// 値にはプラットフォーム固有のシンボル名を格納する必要がある。
    bindgen_map: HashMap<String, String>,
}

/// bindgen の ParseCallbacks 実装
///
/// バインディング生成時に、書き換え後のシンボル名を `#[link_name = "..."]` として付与する。
/// これにより lib.rs 側のコード変更なしでシンボル書き換えが透過的に動作する。
#[derive(Debug)]
struct SymbolLinkNameCallbacks {
    /// C シンボル名 → 書き換え後シンボル名のマップ
    rename_map: HashMap<String, String>,
}

impl bindgen::callbacks::ParseCallbacks for SymbolLinkNameCallbacks {
    /// bindgen がバインディングを生成する際に呼ばれるコールバック
    ///
    /// 戻り値が Some の場合、bindgen は #[link_name = "\u{1}<戻り値>"] を生成する。
    /// \u{1} プレフィックスによりコンパイラのシンボルマングリングが抑制されるため、
    /// 戻り値にはプラットフォーム固有のシンボル名を返す必要がある。
    fn generated_link_name_override(
        &self,
        item_info: bindgen::callbacks::ItemInfo<'_>,
    ) -> Option<String> {
        self.rename_map.get(item_info.name).cloned()
    }
}

/// 静的ライブラリのシンボルを書き換え、bindgen 用の ParseCallbacks を返す
///
/// 処理の流れ:
///   1. rustup の sysroot から llvm-nm / llvm-objcopy を探す
///   2. llvm-nm で静的ライブラリの定義済み外部シンボルを収集する
///   3. 収集したシンボルに対してリネームマップを生成する
///   4. マップファイルを書き出し、llvm-objcopy でライブラリ内のシンボルを書き換える
///   5. bindgen 用の ParseCallbacks を返す
fn rewrite_symbols(lib_dir: &Path, out_dir: &Path) -> SymbolLinkNameCallbacks {
    let tools = discover_llvm_tools();
    let lib_path = find_static_library(lib_dir);

    // Apple プラットフォームの Mach-O ではシンボル先頭に `_` が付くため、
    // プラットフォーム判定してリネームマップの生成時に考慮する
    let is_macho = env::var("CARGO_CFG_TARGET_VENDOR")
        .map(|v| v == "apple")
        .unwrap_or(false);

    // シンボル名の変換ルール
    //
    // aom_ プレフィックスを持つシンボル (公開 API) は aom_ を SYMBOL_PREFIX_ に置換する。
    //   例: aom_codec_encode → shiguredo_aom_codec_encode
    //
    // それ以外のシンボル (av1_* 等の内部シンボル) は先頭に SYMBOL_PREFIX_ を付与する。
    //   例: av1_internal_func → shiguredo_aom_av1_internal_func
    let rename_symbol = |name: &str| -> Option<String> {
        if let Some(rest) = name.strip_prefix("aom_") {
            Some(format!("{SYMBOL_PREFIX}_{rest}"))
        } else {
            Some(format!("{SYMBOL_PREFIX}_{name}"))
        }
    };

    // 全定義済み外部シンボルを収集してリネームマップを生成する
    let symbols = collect_defined_external_symbols(&tools.nm, &lib_path);
    let maps = build_symbol_rename_maps(&symbols, is_macho, &rename_symbol);

    // マップファイルを書き出してシンボルを書き換える
    let map_file = out_dir.join("symbol_rename_map.txt");
    write_objcopy_rename_map(&maps.objcopy_map, &map_file);
    rewrite_archive_symbols(&tools.objcopy, &lib_path, &map_file);

    SymbolLinkNameCallbacks {
        rename_map: maps.bindgen_map,
    }
}

/// 静的ライブラリのパスを探す
///
/// ビルド結果は Unix 系では libaom.a、Windows では aom.lib として出力される。
fn find_static_library(lib_dir: &Path) -> PathBuf {
    let unix_path = lib_dir.join("libaom.a");
    if unix_path.exists() {
        return unix_path;
    }
    let win_path = lib_dir.join("aom.lib");
    if win_path.exists() {
        return win_path;
    }
    panic!("static library not found in {}", lib_dir.display());
}

/// rustc --print sysroot の結果を取得する
///
/// llvm-tools は rustup が管理する sysroot 配下にインストールされるため、
/// sysroot のパスを取得して llvm-nm / llvm-objcopy の探索に使用する。
fn get_rustc_sysroot() -> PathBuf {
    let output = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .output()
        .expect("failed to run rustc --print sysroot");
    if !output.status.success() {
        panic!("rustc --print sysroot failed");
    }
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("invalid UTF-8")
            .trim(),
    )
}

/// Windows 対応の実行ファイル名を生成する
///
/// Windows では実行ファイルに .exe 拡張子が必要。
fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// rustup の sysroot から llvm-nm / llvm-objcopy を探す
///
/// llvm-tools コンポーネントのバイナリは以下のパスに配置される:
///   <sysroot>/lib/rustlib/<host>/bin/llvm-nm
///   <sysroot>/lib/rustlib/<host>/bin/llvm-objcopy
///
/// rust-toolchain.toml に llvm-tools コンポーネントの記載が必要。
///
/// llvm-nm / llvm-objcopy はホスト上で実行するツールなので、クロスコンパイル時は
/// TARGET ではなく HOST のパスから探す必要がある。
/// 例: Windows CI でホストが x86_64-pc-windows-msvc、ターゲットが x86_64-pc-windows-gnu の場合、
/// llvm-tools は msvc 側にのみインストールされている。
fn discover_llvm_tools() -> LlvmTools {
    let sysroot = get_rustc_sysroot();
    // llvm-tools はホスト上で動作するため HOST を使う。
    // クロスコンパイル時に TARGET を使うと、ホスト側にインストールされた
    // llvm-tools が見つからない。
    let host = env::var("HOST").expect("HOST environment variable not set");
    let tools_dir = sysroot.join("lib/rustlib").join(host).join("bin");

    let nm = tools_dir.join(exe_name("llvm-nm"));
    let objcopy = tools_dir.join(exe_name("llvm-objcopy"));

    if !nm.exists() {
        panic!(
            "llvm-nm not found at {}. Run: rustup component add llvm-tools",
            nm.display()
        );
    }
    if !objcopy.exists() {
        panic!(
            "llvm-objcopy not found at {}. Run: rustup component add llvm-tools",
            objcopy.display()
        );
    }

    LlvmTools { nm, objcopy }
}

/// llvm-nm で静的ライブラリから定義済み外部シンボルを収集する
///
/// llvm-nm のオプション:
///   --defined-only: 定義済みシンボルのみ (未定義シンボルを除外)
///   --extern-only: 外部シンボルのみ (ローカルシンボルを除外)
///   --format=just-symbols: シンボル名のみ出力 (アドレスやタイプを省略)
///
/// 出力にはオブジェクトファイル名 (例: av1_cx_iface.c.o:) も含まれるため、
/// is_c_identifier() でフィルタリングして純粋なシンボル名のみを抽出する。
fn collect_defined_external_symbols(nm_path: &Path, lib_path: &Path) -> Vec<String> {
    let output = Command::new(nm_path)
        .arg("--defined-only")
        .arg("--extern-only")
        .arg("--format=just-symbols")
        .arg(lib_path)
        .output()
        .expect("failed to run llvm-nm");
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("llvm-nm failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).expect("llvm-nm output is not valid UTF-8");
    let mut symbols: Vec<String> = stdout
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|s| !s.is_empty() && is_c_identifier(s))
        .collect();
    symbols.sort();
    symbols.dedup();
    symbols
}

/// C 識別子として有効かどうかを判定する
///
/// llvm-nm の --format=just-symbols 出力にはオブジェクトファイル名 (av1_cx_iface.c.o: 等) も
/// 含まれるため、この関数で C 識別子のみをフィルタリングする。
///
/// Apple プラットフォームの Mach-O ではシンボル先頭に `_` が付くため、`_` で始まる文字列も受け入れる。
///
/// libaom の NASM ソースは cglobal_label を使用しておらず、ドット付きの
/// extern シンボルは生成されないため、`.` は許可しない。
fn is_c_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// objcopy 用と bindgen 用のリネームマップを生成する
///
/// 2 つのマップを生成する理由:
///
/// objcopy_map: ライブラリバイナリ内の実シンボル名を書き換えるためのマップ。
///   macOS / iOS では _aom_codec_encode → _shiguredo_aom_codec_encode のようにプラットフォーム固有の
///   `_` プレフィックスを含む形で管理する。
///
/// bindgen_map: Rust バインディングの #[link_name] に使うマップ。
///   キーは C シンボル名 (aom_codec_encode)、値はプラットフォーム固有のシンボル名
///   (_shiguredo_aom_codec_encode) を格納する。
///   bindgen は generated_link_name_override の戻り値に \u{1} を付加してマングリングを
///   抑制するため、プラットフォーム固有の名前を直接返す必要がある。
fn build_symbol_rename_maps(
    symbols: &[String],
    is_macho: bool,
    rename_symbol: &dyn Fn(&str) -> Option<String>,
) -> SymbolRenameMaps {
    let mut objcopy_map = HashMap::new();
    let mut bindgen_map = HashMap::new();

    for sym in symbols {
        // プラットフォーム固有のプレフィックスを除去して C シンボル名を取得する
        //   macOS / iOS: _aom_codec_encode → aom_codec_encode
        //   Linux/Windows: aom_codec_encode → aom_codec_encode (変化なし)
        let c_name = if is_macho {
            sym.strip_prefix('_').unwrap_or(sym)
        } else {
            sym.as_str()
        };

        if let Some(new_c_name) = rename_symbol(c_name) {
            // objcopy 用: プラットフォーム固有のプレフィックスを再付与する
            //   macOS / iOS: shiguredo_aom_codec_encode → _shiguredo_aom_codec_encode
            //   Linux/Windows: shiguredo_aom_codec_encode → shiguredo_aom_codec_encode (変化なし)
            let new_sym = if is_macho {
                format!("_{new_c_name}")
            } else {
                new_c_name.clone()
            };
            objcopy_map.insert(sym.clone(), new_sym.clone());

            // bindgen 用: generated_link_name_override は \u{1} プレフィックスを付加して
            // シンボル名をそのまま使うため、プラットフォーム固有のシンボル名で管理する
            bindgen_map.insert(c_name.to_string(), new_sym);
        }
    }

    SymbolRenameMaps {
        objcopy_map,
        bindgen_map,
    }
}

/// --redefine-syms 用のマップファイルを書き出す
///
/// ファイル形式は 1 行に "旧シンボル名 新シンボル名" を空白区切りで記述する。
/// llvm-objcopy の --redefine-syms オプションで使用される。
fn write_objcopy_rename_map(map: &HashMap<String, String>, path: &Path) {
    let mut lines: Vec<String> = map
        .iter()
        .map(|(old, new)| format!("{old} {new}"))
        .collect();
    // 出力を決定的にするためソートする
    lines.sort();
    fs::write(path, lines.join("\n")).expect("failed to write symbol rename map");
}

/// llvm-objcopy でアーカイブ内のシンボルを書き換える
///
/// --redefine-syms はマップファイルに従ってシンボル名を一括置換する。
/// ライブラリファイルはインプレースで更新される。
fn rewrite_archive_symbols(objcopy_path: &Path, lib_path: &Path, map_file: &Path) {
    let status = Command::new(objcopy_path)
        .arg("--redefine-syms")
        .arg(map_file)
        .arg(lib_path)
        .status()
        .expect("failed to run llvm-objcopy");
    if !status.success() {
        panic!("llvm-objcopy failed");
    }
}

// --- 既存のヘルパー関数 ---

// Rust のモバイルターゲット、または OS とアーキテクチャからプラットフォーム名を生成する
fn get_target_platform() -> String {
    if let Ok(target) = env::var("LIBAOM_TARGET") {
        return target;
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let rust_target = env::var("TARGET").expect("TARGET is not set");

    // 同じ OS とアーキテクチャでも Catalyst や別の ABI は prebuilt と互換にならない。
    // prebuilt と一致するターゲットだけを受け入れ、異なる ABI への誤リンクを防ぐ。
    if target_os == "ios" || target_os == "android" {
        return match rust_target.as_str() {
            "aarch64-apple-ios" => "ios_arm64",
            "aarch64-apple-ios-sim" => "ios-sim_arm64",
            "aarch64-linux-android" => "android_arm64",
            "x86_64-linux-android" => "android_x86_64",
            _ => panic!("unsupported mobile target: {rust_target}"),
        }
        .to_string();
    }

    match (target_os.as_str(), target_arch.as_str()) {
        ("linux", "x86_64") => format!("{}_x86_64", detect_linux_distro()),
        ("linux", "aarch64") => format!("{}_arm64", detect_linux_distro()),
        ("macos", "aarch64") => "macos_arm64".to_string(),
        ("windows", "x86_64") => "windows_x86_64".to_string(),
        _ => panic!("unsupported target: os={}, arch={}", target_os, target_arch),
    }
}

// /etc/os-release から Ubuntu バージョンを検出する
fn detect_linux_distro() -> String {
    if let Ok(content) = fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(version) = line.strip_prefix("VERSION_ID=") {
                let version = version.trim_matches('"');
                match version {
                    "22.04" | "24.04" | "26.04" => return format!("ubuntu-{}", version),
                    _ => {}
                }
            }
        }
    }
    panic!(
        "unsupported Linux distribution. \
         set LIBAOM_TARGET environment variable to specify the target explicitly"
    );
}

// 外部ライブラリのリポジトリを git clone する
fn git_clone_external_lib(build_dir: &Path) {
    let (git_url, version) = get_git_url_and_version();
    let success = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--branch")
        .arg(version)
        .arg(git_url)
        .current_dir(build_dir)
        .status()
        .is_ok_and(|status| status.success());
    if !success {
        panic!("failed to clone {LIB_NAME} repository");
    }
}

// Cargo.toml から依存ライブラリの Git URL とバージョンタグを取得する
fn get_git_url_and_version() -> (String, String) {
    let cargo_toml = shiguredo_toml::Value::Table(
        shiguredo_toml::from_str(include_str!("Cargo.toml")).expect("failed to parse Cargo.toml"),
    );
    if let Some((Some(git_url), Some(version))) = cargo_toml
        .get("package")
        .and_then(|v| v.get("metadata"))
        .and_then(|v| v.get("external-dependencies"))
        .and_then(|v| v.get(LIB_NAME))
        .map(|v| {
            (
                v.get("url").and_then(|s| s.as_str()),
                v.get("version").and_then(|s| s.as_str()),
            )
        })
    {
        (git_url.to_string(), version.to_string())
    } else {
        panic!(
            "Cargo.toml does not contain a valid [package.metadata.external-dependencies.{LIB_NAME}] table"
        );
    }
}
