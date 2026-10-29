//! CLI tests that leave the process environment alone.

use recur_cli::{Io, execute, looks_like_open_path};

fn run(args: &[&str]) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(
        &args,
        &mut Io {
            out: &mut out,
            err: &mut err,
        },
    );
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

#[test]
fn bundles_create_and_test() {
    let parent = tempfile::tempdir().unwrap();
    let parent = parent.path().to_string_lossy().into_owned();
    let (code, out, err) = run(&["bundles", "create", "DemoPack", "--dir", &parent]);
    assert_eq!(code, 0, "create: {err}");
    let dest = format!("{parent}/DemoPack");
    assert_eq!(
        out,
        format!("created {dest}\nsee {dest}/README.md\nnext: recur bundles test DemoPack\n")
    );
    for rel in [
        "bundle.toml",
        "README.md",
        "tests/uppercase.toml",
        "tests/prefix-lines.toml",
        "bin/prefix-lines.sh",
        "src/uppercase.ts",
    ] {
        assert!(
            std::path::Path::new(&format!("{dest}/{rel}")).exists(),
            "{rel} missing"
        );
    }
    let readme = std::fs::read_to_string(format!("{dest}/README.md")).unwrap();
    for want in [
        "# DemoPack",
        "bundle.toml",
        "recur bundles test",
        "typescript",
        "shebang",
    ] {
        assert!(readme.contains(want), "README missing {want:?}");
    }
    let manifest = std::fs::read_to_string(format!("{dest}/bundle.toml")).unwrap();
    assert!(
        manifest.contains("id = \"demopack\"") && manifest.contains("id = \"demopack.uppercase\"")
    );
    let (code, out, err) = run(&["bundles", "test", &dest]);
    assert_eq!(code, 0, "test: {out}{err}");
    assert!(out.contains("2 passed"), "{out}");

    let (code, _, err) = run(&["bundles", "create", "DemoPack", "--dir", &parent]);
    assert_eq!(code, 1);
    assert_eq!(
        err,
        format!("{dest} already exists (pass --force to replace)\n")
    );
    let (code, _, _) = run(&["bundles", "create", "DemoPack", "--dir", &parent, "--force"]);
    assert_eq!(code, 0);
}

#[test]
fn create_rejects_bad_name() {
    let dir = tempfile::tempdir().unwrap();
    let err =
        recur_cli::create_bundle("bad name", &dir.path().to_string_lossy(), false).unwrap_err();
    assert!(err.starts_with("invalid bundle name"), "{err}");
}

#[test]
fn looks_like_open_path_cases() {
    assert!(looks_like_open_path(".") && looks_like_open_path(".."));
    assert!(looks_like_open_path("./foo"));
    assert!(!looks_like_open_path("bundlez"));
    let dir = tempfile::tempdir().unwrap();
    assert!(looks_like_open_path(&dir.path().to_string_lossy()));
}

#[test]
fn root_unknown_command() {
    let (code, out, err) = run(&["bundlez"]);
    assert_eq!(code, 1);
    assert_eq!(out, "");
    assert_eq!(err, "unknown command \"bundlez\" for \"recur\"\n");
}

#[test]
fn is_cli_binary_cases() {
    assert!(recur_cli::open::is_cli_binary(
        "/Applications/Recur.app/Contents/MacOS/recur-cli"
    ));
    assert!(!recur_cli::open::is_cli_binary(
        "/Applications/Recur.app/Contents/MacOS/recur"
    ));
}

#[test]
fn resolve_open_arg_cases() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "x\n").unwrap();
    let file = file.to_string_lossy().into_owned();
    let got = recur_cli::open::resolve_open_arg(&file).unwrap();
    assert!(got.starts_with('/'), "{got}");
    assert!(got.ends_with("/a.txt"), "{got}");
    let missing = format!("{}/missing", dir.path().to_string_lossy());
    let err = recur_cli::open::resolve_open_arg(&missing).unwrap_err();
    assert!(err.starts_with(&format!("{missing}: ")), "{err}");
}

#[test]
fn resolve_bundle_targets_rules() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().to_string_lossy().into_owned();
    let user = format!("{root}/user");
    std::fs::create_dir_all(format!("{user}/Named")).unwrap();
    std::fs::write(format!("{user}/Named/bundle.toml"), "").unwrap();
    std::fs::create_dir_all(format!("{root}/pack/A")).unwrap();
    std::fs::write(format!("{root}/pack/A/bundle.toml"), "").unwrap();
    std::fs::create_dir_all(format!("{root}/pack/B")).unwrap();
    std::fs::create_dir_all(format!("{root}/empty")).unwrap();

    let dirs = recur_cli::resolve_bundle_targets(&["Named".into()], &user, "").unwrap();
    assert_eq!(dirs.len(), 1);
    assert!(dirs[0].ends_with("/user/Named"), "{dirs:?}");
    let dirs = recur_cli::resolve_bundle_targets(
        &[format!("{root}/pack"), format!("{root}/pack/A")],
        &user,
        "",
    )
    .unwrap();
    assert_eq!(dirs.len(), 1, "children dedupe: {dirs:?}");
    let err = recur_cli::resolve_bundle_targets(&[format!("{root}/empty")], &user, "").unwrap_err();
    assert!(err.ends_with("/empty: no bundle.toml"), "{err}");
    let err = recur_cli::resolve_bundle_targets(&["Nope".into()], &user, "").unwrap_err();
    assert_eq!(
        err,
        format!("bundle \"Nope\" not found (looked in {user}, ./bundles, shipped)")
    );
}

#[test]
fn version_and_help_exit_zero() {
    for flag in ["--version", "-v"] {
        let (code, out, _) = run(&[flag]);
        assert_eq!(code, 0);
        assert!(out.contains(recur_cli::VERSION), "{out}");
    }
    let (code, out, _) = run(&["help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Recur command-line tools."), "{out}");
    assert!(out.contains("Author and test Recur Bundles"), "{out}");
    assert!(out.contains("--bundles-dir"), "{out}");
    let (code, out, _) = run(&["bundles"]);
    assert_eq!(code, 0);
    assert!(out.contains("Scaffold a new bundle"), "{out}");
    let (code, out, _) = run(&["bundles", "create", "--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("--force") && out.contains("recur bundles create MyPack"));
}

#[test]
fn usage_errors_exit_one() {
    let (code, _, err) = run(&["bundles", "create", "X", "--force=maybe"]);
    assert_eq!(code, 1);
    assert!(err.contains("--force"), "{err}");
    let (code, _, err) = run(&["-x"]);
    assert_eq!(code, 1);
    assert!(err.contains("'-x'"), "{err}");
    let (code, _, err) = run(&["bundles", "create"]);
    assert_eq!(code, 1);
    assert!(err.contains("<Name>"), "{err}");
    let (code, _, err) = run(&["open", "a", "b"]);
    assert_eq!(code, 1);
    assert!(err.contains("'b'"), "{err}");
}

#[test]
fn completion_scripts() {
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let (code, out, err) = run(&["completion", shell]);
        assert_eq!(code, 0, "{shell}: {err}");
        assert!(out.contains("recur") && out.contains("bundles"), "{shell}");
    }
    let (code, _, _) = run(&["completion", "tcsh"]);
    assert_eq!(code, 1);
}
