//! Pinned language-server installs under the data dir (Go `lsp/managed`):
//!
//! - gopls: `go install` (no official multi-arch tarballs)
//! - rust-analyzer, marksman, ruff, taplo: GitHub release assets checked
//!   against pinned SHA-256 digests
//! - npm servers: `npm install --prefix` (needs node and npm)

mod binaries;
mod gopls;
mod npm;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub use binaries::{
    MARKSMAN_RELEASE, RUFF_RELEASE, RUST_ANALYZER_RELEASE, TAPLO_RELEASE, marksman_asset,
    marksman_sha256, ruff_asset, ruff_sha256, rust_analyzer_asset, rust_analyzer_sha256,
    taplo_asset, taplo_sha256,
};
pub use gopls::{GOPLS_MODULE, GOPLS_RELEASE};
pub use npm::{
    BASED_PYRIGHT_RELEASE, BASH_LANGUAGE_SERVER_RELEASE, NpmBinPin, SVELTE_LANGUAGE_SERVER_RELEASE,
    TYPESCRIPT_LANGUAGE_SERVER_RELEASE, TYPESCRIPT_RELEASE, VSCODE_LANGSERVERS_EXTRACTED_RELEASE,
    VUE_LANGUAGE_SERVER_RELEASE, YAML_LANGUAGE_SERVER_RELEASE, install_npm_bin,
};

use crate::servers::{is_executable, look_path};

/// Env overrides: an absolute executable path returned without installing.
pub const ENV_GOPLS: &str = "RECUR_LSP_GOPLS";
pub const ENV_RUST_ANALYZER: &str = "RECUR_LSP_RUST_ANALYZER";
pub const ENV_MARKSMAN: &str = "RECUR_LSP_MARKSMAN";
pub const ENV_TYPESCRIPT: &str = "RECUR_LSP_TYPESCRIPT_LANGUAGE_SERVER";
pub const ENV_BASED_PYRIGHT: &str = "RECUR_LSP_BASEDPYRIGHT";
pub const ENV_RUFF: &str = "RECUR_LSP_RUFF";
pub const ENV_TAPLO: &str = "RECUR_LSP_TAPLO";
pub const ENV_JSON: &str = "RECUR_LSP_JSON";
pub const ENV_HTML: &str = "RECUR_LSP_HTML";
pub const ENV_CSS: &str = "RECUR_LSP_CSS";
pub const ENV_YAML: &str = "RECUR_LSP_YAML";
pub const ENV_BASH: &str = "RECUR_LSP_BASH_LANGUAGE_SERVER";
pub const ENV_SVELTE: &str = "RECUR_LSP_SVELTE_LANGUAGE_SERVER";
pub const ENV_VUE: &str = "RECUR_LSP_VUE_LANGUAGE_SERVER";

pub(crate) const INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Serializes installs so concurrent first-use cannot race on a shared
/// prefix (vscode-langservers-extracted).
static ENSURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// An absolute path to a usable language server for `managed_id`, falling
/// back to PATH when the install fails. `data_dir` empty means PATH only.
pub async fn ensure(data_dir: &str, managed_id: &str) -> Result<String, String> {
    let _guard = ENSURE.lock().await;
    match managed_id.trim().to_lowercase().as_str() {
        "gopls" => gopls::ensure_gopls(data_dir).await,
        "rust-analyzer" => binaries::ensure_rust_analyzer(data_dir).await,
        "marksman" => binaries::ensure_marksman(data_dir).await,
        "ruff" => binaries::ensure_ruff(data_dir).await,
        "taplo" => binaries::ensure_taplo(data_dir).await,
        id => match npm::pin_for(id) {
            Some(pin) => npm::ensure_npm_bin(data_dir, &pin).await,
            None => Err(format!(
                "lsp managed: unknown id {}",
                serde_json::to_string(managed_id).unwrap_or_default()
            )),
        },
    }
}

/// `<data>/bin/lsp/<id>`.
pub fn managed_bin_dir(data_dir: &str, id: &str) -> PathBuf {
    Path::new(data_dir).join("bin").join("lsp").join(id)
}

/// The `@vue/language-server` package dir that hosts
/// `@vue/typescript-plugin`; empty when not installed.
// simplify: managed npm prefix only; PATH/env Vue binaries skip pairing until
// that prefix exists.
pub fn vue_plugin_path(data_dir: &str) -> String {
    if data_dir.is_empty() {
        return String::new();
    }
    let p = managed_bin_dir(data_dir, "vue-language-server")
        .join("node_modules")
        .join("@vue")
        .join("language-server");
    if p.is_dir() {
        p.to_string_lossy().into_owned()
    } else {
        String::new()
    }
}

pub(crate) fn managed_version(data_dir: &str, id: &str) -> String {
    std::fs::read_to_string(managed_bin_dir(data_dir, id).join(format!("{id}.version")))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

pub fn write_managed_version(data_dir: &str, id: &str, version: &str) -> std::io::Result<()> {
    let dir = managed_bin_dir(data_dir, id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{id}.version")), format!("{version}\n"))
}

fn look_path_fallback(name: &str) -> Option<String> {
    look_path(name).filter(|p| is_executable(Path::new(p)))
}

/// The env override, an installed binary at the pinned version, a fresh
/// install, or PATH, in that order.
pub(crate) async fn ensure_pinned_binary<F, Fut>(
    data_dir: &str,
    id: &str,
    version: &str,
    env_key: &str,
    path_name: &str,
    bin_name: &str,
    install: F,
) -> Result<String, String>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let env = std::env::var(env_key).unwrap_or_default();
    if !env.is_empty() && is_executable(Path::new(&env)) {
        return Ok(env);
    }
    let bin_name = if bin_name.is_empty() {
        path_name
    } else {
        bin_name
    };
    if !data_dir.is_empty() {
        let dest = managed_bin_dir(data_dir, id).join(bin_name);
        if is_executable(&dest) && managed_version(data_dir, id) == version {
            return Ok(dest.to_string_lossy().into_owned());
        }
        if let Err(e) = install(dest.clone()).await {
            return look_path_fallback(path_name).ok_or(e);
        }
        return Ok(dest.to_string_lossy().into_owned());
    }
    look_path_fallback(path_name)
        .ok_or_else(|| format!("lsp managed: no data directory and {path_name} not on PATH"))
}

/// Go's `runtime.GOOS/runtime.GOARCH` for this build.
pub fn host_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    format!("{os}/{arch}")
}

pub(crate) fn unsupported(platform: &str, what: &str) -> String {
    format!("lsp managed: unsupported platform {platform} for {what}")
}

/// Downloads `url` to `dest` through a `.partial` staging file, checking
/// the SHA-256 digest when one is pinned.
pub(crate) async fn download_verified(
    url: &str,
    want_sha256: &str,
    dest: &Path,
) -> Result<(), String> {
    let client = recur_http::client(|b| b.timeout(INSTALL_TIMEOUT));
    let mut res = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("lsp managed: download: {e}"))?;
    let status = res.status();
    if status != reqwest::StatusCode::OK {
        let reason = status.canonical_reason().unwrap_or_default();
        return Err(format!(
            "lsp managed: download: HTTP {} {reason}",
            status.as_u16()
        ));
    }
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    let staging = PathBuf::from(format!("{}.partial", dest.display()));
    let mut f = tokio::fs::File::create(&staging)
        .await
        .map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let copied: Result<(), String> = async {
        while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
            hasher.update(&chunk);
            f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        f.flush().await.map_err(|e| e.to_string())
    }
    .await;
    drop(f);
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("lsp managed: download: {e}"));
    }
    let got = hex::encode(hasher.finalize());
    if !want_sha256.is_empty() && got != want_sha256 {
        let _ = std::fs::remove_file(&staging);
        return Err(format!(
            "lsp managed: SHA-256 mismatch: got {got} want {want_sha256}"
        ));
    }
    std::fs::rename(&staging, dest).map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        e.to_string()
    })
}

fn create_exec(dest: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(dest)
}

pub(crate) fn gunzip_file(src: &Path, dest: &Path) -> Result<(), String> {
    let input = std::fs::File::open(src).map_err(|e| e.to_string())?;
    let mut gz = flate2::read::GzDecoder::new(input);
    let mut out = create_exec(dest).map_err(|e| e.to_string())?;
    std::io::copy(&mut gz, &mut out)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Extracts the first regular file named `want_base` from a `.tar.gz`.
pub(crate) fn extract_tar_gz_named(
    archive: &Path,
    dest: &Path,
    want_base: &str,
) -> Result<(), String> {
    let input = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut tr = tar::Archive::new(flate2::read::GzDecoder::new(input));
    for entry in tr.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let name = entry.path().map_err(|e| e.to_string())?.into_owned();
        if name.file_name().and_then(|n| n.to_str()) != Some(want_base) {
            continue;
        }
        let mut out = create_exec(dest).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut out, &buf).map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err(format!("lsp managed: {want_base} not found in archive"))
}

pub(crate) fn copy_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    let mut out = create_exec(dest)?;
    std::io::copy(&mut input, &mut out).map(|_| ())
}

pub(crate) fn chmod_exec(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

/// Runs an installer command with the install timeout; the error text is
/// its combined output (or the spawn error).
pub(crate) async fn run_install(mut cmd: tokio::process::Command) -> Result<(), String> {
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(INSTALL_TIMEOUT, cmd.output())
        .await
        .map_err(|_| "signal: killed".to_owned())?
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let mut msg = String::from_utf8_lossy(&out.stdout).into_owned();
    msg.push_str(&String::from_utf8_lossy(&out.stderr));
    let msg = msg.trim();
    Err(if msg.is_empty() {
        format!("{}", out.status)
    } else {
        msg.to_owned()
    })
}

#[cfg(test)]
mod tests;
