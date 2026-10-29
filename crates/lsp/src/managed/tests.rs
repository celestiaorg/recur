//! Go `lsp/managed/managed_test.go` and `assets_test.go`.

use super::binaries::release_url;
use super::*;

fn write_exec(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "#!/bin/sh\n").unwrap();
    chmod_exec(p).unwrap();
}

#[tokio::test]
async fn ensure_gopls_reuses_pinned_install() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_string_lossy().into_owned();
    let bin = managed_bin_dir(&dir, "gopls").join("gopls");
    write_exec(&bin);
    write_managed_version(&dir, "gopls", GOPLS_RELEASE).unwrap();
    assert_eq!(ensure(&dir, "gopls").await.unwrap(), bin.to_string_lossy());
}

#[tokio::test]
async fn install_npm_bin_creates_dest_dir_for_shared_prefix() {
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_string_lossy().into_owned();
    let prefix = managed_bin_dir(&dir, "vscode-langservers-extracted");
    let shim = prefix.join("node_modules/.bin/vscode-json-language-server");
    write_exec(&shim);
    write_managed_version(&dir, "vscode-langservers-extracted", "1").unwrap();
    let dest =
        managed_bin_dir(&dir, "vscode-json-language-server").join("vscode-json-language-server");
    let pin = NpmBinPin {
        id: "vscode-json-language-server".into(),
        version: "1".into(),
        bin_name: "vscode-json-language-server".into(),
        prefix_id: "vscode-langservers-extracted".into(),
        ..Default::default()
    };
    install_npm_bin(&dir, dest.clone(), &pin).await.unwrap();
    let st = std::fs::symlink_metadata(&dest).unwrap();
    assert!(st.file_type().is_symlink(), "dest is not a symlink");
    assert_eq!(std::fs::read_link(&dest).unwrap(), shim);
    assert_eq!(managed_version(&dir, "vscode-json-language-server"), "1");
}

#[tokio::test]
async fn unknown_id_and_no_data_dir() {
    assert_eq!(
        ensure("", "nope").await.unwrap_err(),
        "lsp managed: unknown id \"nope\""
    );
    if look_path("marksman").is_none() {
        assert_eq!(
            ensure("", "marksman").await.unwrap_err(),
            "lsp managed: no data directory and marksman not on PATH"
        );
    }
}

#[test]
fn vue_plugin_path_cases() {
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(vue_plugin_path(""), "");
    assert_eq!(vue_plugin_path(&empty.path().to_string_lossy()), "");
    let data = tempfile::tempdir().unwrap();
    let dir = data
        .path()
        .join("bin/lsp/vue-language-server/node_modules/@vue/language-server");
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(
        vue_plugin_path(&data.path().to_string_lossy()),
        dir.to_string_lossy()
    );
}

#[test]
fn host_assets_have_digests() {
    let p = host_platform();
    if let Ok(a) = rust_analyzer_asset(&p) {
        assert!(rust_analyzer_sha256(a).is_some(), "{a}");
        match p.as_str() {
            "darwin/arm64" => assert_eq!(a, "rust-analyzer-aarch64-apple-darwin.gz"),
            "darwin/amd64" => assert_eq!(a, "rust-analyzer-x86_64-apple-darwin.gz"),
            _ => {}
        }
    }
    for (asset, sha) in [
        (
            marksman_asset(&p),
            marksman_sha256 as fn(&str) -> Option<&'static str>,
        ),
        (ruff_asset(&p), ruff_sha256),
        (taplo_asset(&p), taplo_sha256),
    ] {
        if let Ok(a) = asset {
            assert!(sha(a).is_some(), "{a}");
        }
    }
    assert_eq!(
        ruff_asset("plan9/386").unwrap_err(),
        "lsp managed: unsupported platform plan9/386 for ruff"
    );
}

#[test]
fn release_urls() {
    assert_eq!(
        release_url("ruff", "ruff-x86_64-apple-darwin.tar.gz"),
        "https://github.com/astral-sh/ruff/releases/download/0.16.2/ruff-x86_64-apple-darwin.tar.gz"
    );
}

#[tokio::test]
async fn download_verified_checks_digest_and_status() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let resp = if head.starts_with("GET /missing") {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"
                        .to_owned()
                };
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("sub/file");
    let hello = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    download_verified(&format!("{base}/ok"), hello, &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
    let err = download_verified(&format!("{base}/ok"), "00", &dir.path().join("bad"))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!("lsp managed: SHA-256 mismatch: got {hello} want 00")
    );
    assert!(!dir.path().join("bad.partial").exists() && !dir.path().join("bad").exists());
    let err = download_verified(&format!("{base}/missing"), "", &dir.path().join("m"))
        .await
        .unwrap_err();
    assert_eq!(err, "lsp managed: download: HTTP 404 Not Found");
}

#[test]
fn archives_extract_named_binary() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let gz = dir.path().join("a.gz");
    let mut enc = flate2::write::GzEncoder::new(
        std::fs::File::create(&gz).unwrap(),
        flate2::Compression::default(),
    );
    enc.write_all(b"binary").unwrap();
    enc.finish().unwrap();
    let out = dir.path().join("out/bin");
    gunzip_file(&gz, &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"binary");
    assert!(is_executable(&out));

    let tgz = dir.path().join("a.tar.gz");
    let enc = flate2::write::GzEncoder::new(
        std::fs::File::create(&tgz).unwrap(),
        flate2::Compression::default(),
    );
    let mut b = tar::Builder::new(enc);
    for (name, body) in [("ruff-x/README", "doc"), ("ruff-x/ruff", "exe")] {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, name, body.as_bytes()).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap();
    let dest = dir.path().join("ruff");
    extract_tar_gz_named(&tgz, &dest, "ruff").unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"exe");
    assert_eq!(
        extract_tar_gz_named(&tgz, &dest, "nope").unwrap_err(),
        "lsp managed: nope not found in archive"
    );
}

/// Go `TestEnsureGoplsInstallsWhenGoPresent`: downloads gopls with `go
/// install`. Network; run with `--ignored`.
#[tokio::test]
#[ignore]
async fn ensure_gopls_installs_when_go_present() {
    if look_path("go").is_none() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let dir = data.path().to_string_lossy().into_owned();
    let got = ensure(&dir, "gopls").await.unwrap();
    assert!(is_executable(Path::new(&got)), "{got}");
    assert_eq!(managed_version(&dir, "gopls"), GOPLS_RELEASE);
}
