//! Locates, runs and installs ripgrep. Resolution order: `$RECUR_RG`,
//! `<dataDir>/bin/rg`, then `PATH`. Managed installs are the pinned release
//! below, verified against its SHA-256.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::gopath::{self, errno_text};
use crate::root::path_str;

/// Overrides ripgrep resolution (dev, tests).
pub const ENV_PATH: &str = "RECUR_RG";

/// The executable name under `<dataDir>/bin/`.
pub const BIN_NAME: &str = "rg";

/// The ripgrep release Recur installs into `<dataDir>/bin/rg`. Bumping it
/// replaces a managed install whose `rg.version` sidecar differs; `PATH` and
/// `$RECUR_RG` binaries are left alone.
pub const RELEASE_VERSION: &str = "15.2.0";

/// Sits next to the managed binary to detect stale installs.
pub const VERSION_FILE_NAME: &str = "rg.version";

const RELEASE_BASE: &str = "https://github.com/BurntSushi/ripgrep/releases/download/";

/// Pinned digests of the release tarballs (from upstream's `.sha256` assets;
/// Linux uses the static musl builds). Bump with [`RELEASE_VERSION`].
const RELEASE_SHA256: &[(&str, &str)] = &[
    (
        "ripgrep-15.2.0-aarch64-apple-darwin.tar.gz",
        "3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4",
    ),
    (
        "ripgrep-15.2.0-x86_64-apple-darwin.tar.gz",
        "af7825fcc69a2afc7a7aea55fc9af90e26421d8f20fe59df32e233c0b8a231c1",
    ),
    (
        "ripgrep-15.2.0-aarch64-unknown-linux-musl.tar.gz",
        "800b1e7206afe799dfb5a6901f23147cfaabe0e52210538100f61e86e1740915",
    ),
    (
        "ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz",
        "33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c",
    ),
];

/// Excludes heavy trees from `rg --files -uu` listings. Hidden files stay
/// (`-uu`); only these globs are dropped.
pub const DEFAULT_GLOBS: [&str; 4] = ["!.git", "!.git/**", "!node_modules", "!node_modules/**"];

/// An executable ripgrep, or an error listing what was tried.
pub fn locate(data_dir: &Path) -> Result<PathBuf> {
    let mut tried: Vec<String> = Vec::new();
    let mut add = |p: &str| -> Option<PathBuf> {
        if p.is_empty() {
            return None;
        }
        if is_executable_file(p) {
            return Some(PathBuf::from(p));
        }
        tried.push(p.to_string());
        None
    };
    if let Some(p) = add(&std::env::var(ENV_PATH).unwrap_or_default()) {
        return Ok(p);
    }
    let data_dir = path_str(data_dir);
    if !data_dir.is_empty()
        && let Some(p) = add(&gopath::join(&[&data_dir, "bin", BIN_NAME]))
    {
        return Ok(p);
    }
    match look_path(BIN_NAME) {
        Some(p) => {
            if let Some(p) = add(&p) {
                return Ok(p);
            }
        }
        None => tried.push("PATH".into()),
    }
    Err(Error::msg(format!(
        "ripgrep (rg) not found (looked in: {}) — set {ENV_PATH} or install into <dataDir>/bin/rg",
        tried.join(", ")
    )))
}

pub(crate) fn is_executable_file(p: &str) -> bool {
    fs::metadata(p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
}

/// Go `exec.LookPath` for a bare name: the first executable regular file in
/// `$PATH`. Matches in relative `PATH` entries are refused (Go's `ErrDot`).
pub(crate) fn look_path(name: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = gopath::join(&[dir, name]);
        if fs::metadata(&p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0) {
            if !gopath::is_abs(&p) {
                return None;
            }
            return Some(p);
        }
    }
    None
}

/// The argv for a recursive file listing under `root`.
pub fn files_args(root: &str, extra_globs: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["--files", "-uu", "--no-messages", "--color", "never"]
        .map(String::from)
        .to_vec();
    for g in DEFAULT_GLOBS {
        args.extend(["--glob".into(), g.into()]);
    }
    for g in extra_globs.iter().filter(|g| !g.is_empty()) {
        args.extend(["--glob".into(), g.clone()]);
    }
    args.push(root.into());
    args
}

/// Runs `rg --files` and calls `emit` with each root-relative forward-slash
/// path (also when rg prints absolute paths). `extra_globs` are more
/// `--glob`s, typically negated `files.exclude` patterns. An `emit` error
/// stops rg and is returned; cancelling kills rg.
pub async fn files(
    cancel: &CancellationToken,
    rg_path: &Path,
    root: &Path,
    extra_globs: &[String],
    mut emit: impl FnMut(String) -> Result<()>,
) -> Result<()> {
    if rg_path.as_os_str().is_empty() {
        return Err(Error::msg("rg: empty binary path"));
    }
    let root_abs = gopath::abs(&path_str(root)).map_err(|e| Error::msg(e.to_string()))?;
    let mut child = Command::new(rg_path)
        .args(files_args(&root_abs, extra_globs))
        .current_dir(&root_abs)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            Error::msg(format!(
                "rg start: fork/exec {}: {}",
                rg_path.display(),
                errno_text(&e)
            ))
        })?;
    let Some(stdout) = child.stdout.take() else {
        return Err(Error::msg("rg: no stdout"));
    };
    let mut lines = BufReader::with_capacity(64 * 1024, stdout);
    let mut buf = Vec::new();
    let mut fail: Option<Error> = None;
    loop {
        buf.clear();
        let read = tokio::select! {
            r = lines.read_until(b'\n', &mut buf) => r,
            () = cancel.cancelled() => {
                let _ = child.kill().await;
                return Err(Error::Canceled);
            }
        };
        match read {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                fail = Some(Error::msg(errno_text(&e)));
                break;
            }
        }
        if buf.len() > 1024 * 1024 {
            fail = Some(Error::msg("bufio.Scanner: token too long"));
            let _ = child.start_kill();
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rel = rel_slash(&root_abs, line);
        if rel.is_empty() || rel == "." {
            continue;
        }
        if let Err(e) = emit(rel) {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(e);
        }
    }
    let waited = child.wait().await;
    if let Some(e) = fail {
        return Err(e);
    }
    if cancel.is_cancelled() {
        return Err(Error::Canceled);
    }
    // A non-zero exit (empty tree, everything filtered) is still a listing.
    waited.map(|_| ()).map_err(|e| Error::msg(errno_text(&e)))
}

fn rel_slash(root: &str, path: &str) -> String {
    let p = if gopath::is_abs(path) {
        path.to_string()
    } else {
        gopath::join(&[root, path])
    };
    match gopath::rel(root, &p) {
        Ok(rel) if rel == ".." || rel.starts_with("../") => String::new(),
        Ok(rel) => rel,
        Err(_) => path.to_string(),
    }
}

/// A usable rg. A managed install under `data_dir/bin` is (re)installed when
/// missing or when its sidecar version differs from [`RELEASE_VERSION`];
/// `$RECUR_RG` and a `PATH` rg are never replaced. A failed install falls
/// back to whatever [`locate`] still finds.
pub async fn ensure(data_dir: &Path) -> Result<PathBuf> {
    let env = std::env::var(ENV_PATH).unwrap_or_default();
    if !env.is_empty() && is_executable_file(&env) {
        return Ok(PathBuf::from(env));
    }
    let dir = path_str(data_dir);
    if !dir.is_empty() {
        let managed = gopath::join(&[&dir, "bin", BIN_NAME]);
        if is_executable_file(&managed) && managed_version(&dir) == RELEASE_VERSION {
            return Ok(PathBuf::from(managed));
        }
        if let Err(e) = install(&dir).await {
            return locate(data_dir).map_err(|_| e);
        }
        return Ok(PathBuf::from(managed));
    }
    match look_path(BIN_NAME) {
        Some(p) if is_executable_file(&p) => Ok(PathBuf::from(p)),
        _ => Err(Error::msg(
            "rg ensure: no data directory and ripgrep not on PATH",
        )),
    }
}

pub(crate) fn managed_version(data_dir: &str) -> String {
    fs::read_to_string(gopath::join(&[data_dir, "bin", VERSION_FILE_NAME]))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub(crate) fn write_managed_version(data_dir: &str) -> Result<()> {
    let dir = gopath::join(&[data_dir, "bin"]);
    gopath::mkdir_all(&dir)?;
    let p = gopath::join(&[&dir, VERSION_FILE_NAME]);
    fs::write(&p, format!("{RELEASE_VERSION}\n")).map_err(|e| Error::path("open", &p, e))
}

pub(crate) fn release_sha256(asset: &str) -> Option<&'static str> {
    RELEASE_SHA256
        .iter()
        .find(|(a, _)| *a == asset)
        .map(|(_, sum)| *sum)
}

async fn install(data_dir: &str) -> Result<()> {
    let asset = release_asset()?;
    let url = format!("{RELEASE_BASE}{RELEASE_VERSION}/{asset}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3 * 60))
        .build()
        .map_err(|e| Error::msg(format!("rg download: {e}")))?;
    let mut res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| Error::msg(format!("rg download: {e}")))?;
    if res.status() != reqwest::StatusCode::OK {
        return Err(Error::msg(format!("rg download: HTTP {}", res.status())));
    }
    let Some(want) = release_sha256(&asset) else {
        return Err(Error::msg(format!(
            "rg download: no pinned SHA-256 for {asset}"
        )));
    };
    let bin_dir = gopath::join(&[data_dir, "bin"]);
    gopath::mkdir_all(&bin_dir)?;
    let staging = make_staging_dir(&bin_dir)?;
    let result = async {
        // Buffer and hash the whole archive before extracting: hashing while
        // extracting would miss the unread archive tail.
        let archive = gopath::join(&[&staging, &asset]);
        let mut hasher = Sha256::new();
        let mut body = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|e| Error::msg(format!("rg download: {e}")))?
        {
            hasher.update(&chunk);
            body.extend_from_slice(&chunk);
        }
        fs::write(&archive, &body).map_err(|e| Error::path("open", &archive, e))?;
        let got = hex::encode(hasher.finalize());
        if got != want {
            return Err(Error::msg(format!(
                "rg download: SHA-256 mismatch for {asset}: got {got} want {want}"
            )));
        }
        let staged = gopath::join(&[&staging, BIN_NAME]);
        extract_rg(&body[..], &staged)?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .map_err(|e| Error::path("chmod", &staged, e))?;
        let dest = gopath::join(&[&bin_dir, BIN_NAME]);
        if fs::rename(&staged, &dest).is_err() {
            copy_file(&staged, &dest)?;
        }
        write_managed_version(data_dir)
            .map_err(|e| Error::msg(format!("rg install: write version: {e}")))
    }
    .await;
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Go `os.MkdirTemp(dir, ".rg-staging-*")`.
fn make_staging_dir(bin_dir: &str) -> Result<String> {
    use std::os::unix::fs::DirBuilderExt;
    let pid = std::process::id();
    for n in 0..10_000u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let p = gopath::join(&[
            bin_dir,
            &format!(
                ".rg-staging-{}",
                (nanos ^ pid ^ n.wrapping_mul(2_654_435_761)) % 1_000_000_000
            ),
        ]);
        match fs::DirBuilder::new().mode(0o700).create(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::path("mkdirtemp", &p, e)),
        }
    }
    Err(Error::msg("rg download: could not create staging dir"))
}

/// The `rg-<version>-<triple>.tar.gz` asset for this platform.
pub(crate) fn release_asset() -> Result<String> {
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        (os, arch) => {
            let os = if os == "macos" { "darwin" } else { os };
            let arch = match arch {
                "aarch64" => "arm64",
                "x86_64" => "amd64",
                a => a,
            };
            return Err(Error::msg(format!(
                "rg ensure: unsupported platform {os}/{arch} (install ripgrep on PATH)"
            )));
        }
    };
    Ok(format!("ripgrep-{RELEASE_VERSION}-{triple}.tar.gz"))
}

/// Pulls the `rg` member out of a release tarball.
pub(crate) fn extract_rg(r: impl Read, dest: &str) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(r);
    if gz.header().is_none() {
        return Err(Error::msg("rg download: gzip: gzip: invalid header"));
    }
    let mut tr = tar::Archive::new(gz);
    let entries = tr.entries().map_err(|e| Error::msg(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| Error::msg(e.to_string()))?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if gopath::base(&name) != BIN_NAME
            || entry.header().entry_type() != tar::EntryType::Regular
            || name.contains("..")
        {
            continue;
        }
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(dest)
            .map_err(|e| Error::path("open", dest, e))?;
        io::copy(&mut entry, &mut f).map_err(|e| Error::msg(e.to_string()))?;
        return Ok(());
    }
    Err(Error::msg("rg download: no rg binary in archive"))
}

fn copy_file(src: &str, dest: &str) -> Result<()> {
    let mut input = fs::File::open(src).map_err(|e| Error::path("open", src, e))?;
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(dest)
        .map_err(|e| Error::path("open", dest, e))?;
    io::copy(&mut input, &mut out).map_err(|e| Error::msg(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_args_include_uu_and_globs() {
        let args = files_args("/tmp/repo", &[]);
        for w in ["--files", "-uu", "--glob", "!.git", "!node_modules"] {
            assert!(
                args.iter().any(|a| a == w),
                "files_args = {args:?}, missing {w}"
            );
        }
        assert_eq!(args.last().map(String::as_str), Some("/tmp/repo"));
        let extra = files_args("/r", &["!dist".into(), String::new()]);
        assert_eq!(extra[extra.len() - 3..], ["--glob", "!dist", "/r"]);
    }

    #[tokio::test]
    async fn files_lists_with_system_rg() {
        let Some(rg) = look_path("rg") else {
            eprintln!("rg not on PATH");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("validators")).unwrap();
        fs::write(root.path().join("validators/hyperlane_validator2.sh"), "x").unwrap();
        fs::write(root.path().join("hypothesis.txt"), "x").unwrap();
        // Hidden files are listed with -uu.
        fs::write(root.path().join(".env.example"), "x").unwrap();
        let mut got = Vec::new();
        files(
            &CancellationToken::new(),
            Path::new(&rg),
            root.path(),
            &[],
            |rel| {
                got.push(rel);
                Ok(())
            },
        )
        .await
        .unwrap();
        for want in [
            "validators/hyperlane_validator2.sh",
            "hypothesis.txt",
            ".env.example",
        ] {
            assert!(
                got.iter().any(|p| p == want),
                "files = {got:?}, missing {want}"
            );
        }
    }

    #[tokio::test]
    async fn files_reports_start_failure_and_emit_errors() {
        let root = tempfile::tempdir().unwrap();
        let err = files(
            &CancellationToken::new(),
            Path::new("/nonexistent/rg-binary"),
            root.path(),
            &[],
            |_| Ok(()),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "rg start: fork/exec /nonexistent/rg-binary: no such file or directory"
        );
        let Some(rg) = look_path("rg") else {
            return;
        };
        fs::write(root.path().join("a"), "x").unwrap();
        let err = files(
            &CancellationToken::new(),
            Path::new(&rg),
            root.path(),
            &[],
            |_| Err(Error::msg("stop")),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "stop");
    }

    #[test]
    fn managed_version_sidecar() {
        let data = tempfile::tempdir().unwrap();
        let d = data.path().to_str().unwrap();
        assert_eq!(managed_version(d), "");
        write_managed_version(d).unwrap();
        assert_eq!(managed_version(d), RELEASE_VERSION);
    }

    #[test]
    fn release_sha256_covers_assets() {
        for arch in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-musl",
            "x86_64-unknown-linux-musl",
        ] {
            let asset = format!("ripgrep-{RELEASE_VERSION}-{arch}.tar.gz");
            let sum = release_sha256(&asset).unwrap_or_default();
            assert_eq!(sum.len(), 64, "missing/invalid SHA-256 pin for {asset}");
        }
        assert!(release_asset().is_ok());
    }

    #[test]
    fn stale_managed_needs_upgrade() {
        let data = tempfile::tempdir().unwrap();
        let d = data.path().to_str().unwrap();
        let bin = data.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        recur_exec::write_executable(bin.join(BIN_NAME), "#!/bin/sh\n", 0o755).unwrap();
        fs::write(bin.join(VERSION_FILE_NAME), "0.0.1\n").unwrap();
        assert_ne!(managed_version(d), RELEASE_VERSION);
        assert!(is_executable_file(bin.join(BIN_NAME).to_str().unwrap()));
    }

    #[test]
    fn extract_rg_from_tarball() {
        let mut tarball = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut tarball, flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            for (name, body) in [
                ("ripgrep-15.2.0/doc/rg.1", &b"man"[..]),
                ("ripgrep-15.2.0/rg", &b"binary"[..]),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                b.append_data(&mut h, name, body).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("rg");
        extract_rg(&tarball[..], dest.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"binary");
        let err = extract_rg(&b"not gzip"[..], dest.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().starts_with("rg download"), "{err}");
    }
}
