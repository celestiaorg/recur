//! GitHub release binaries checked against pinned digests: rust-analyzer,
//! marksman, ruff, taplo (Go `lsp/managed/{rust_analyzer,marksman,ruff,taplo}.go`).

use std::path::{Path, PathBuf};

use super::{
    ENV_MARKSMAN, ENV_RUFF, ENV_RUST_ANALYZER, ENV_TAPLO, chmod_exec, copy_file, download_verified,
    ensure_pinned_binary, extract_tar_gz_named, gunzip_file, host_platform, unsupported,
    write_managed_version,
};

pub const RUST_ANALYZER_RELEASE: &str = "2026-08-10.1";
const RUST_ANALYZER_BASE: &str = "https://github.com/rust-lang/rust-analyzer/releases/download/";

pub const MARKSMAN_RELEASE: &str = "2026-02-08";
const MARKSMAN_BASE: &str = "https://github.com/artempyanykh/marksman/releases/download/";

/// The pinned ruff release (native `ruff server` LSP).
pub const RUFF_RELEASE: &str = "0.16.2";
const RUFF_BASE: &str = "https://github.com/astral-sh/ruff/releases/download/";

/// The pinned taplo CLI release (`taplo lsp stdio`).
pub const TAPLO_RELEASE: &str = "0.10.0";
const TAPLO_BASE: &str = "https://github.com/tamasfe/taplo/releases/download/";

/// Digests from the GitHub release asset listings.
pub fn rust_analyzer_sha256(asset: &str) -> Option<&'static str> {
    match asset {
        "rust-analyzer-aarch64-apple-darwin.gz" => {
            Some("954c1dce00ab43d5a35caac07aff12a787b006b50dd3f4471bae4b13512343ef")
        }
        "rust-analyzer-x86_64-apple-darwin.gz" => {
            Some("b84eba6670c7e55869d191f23cc23e8953ed18141db41de0834d7b9fb47a0090")
        }
        "rust-analyzer-aarch64-unknown-linux-gnu.gz" => {
            Some("a7eb3a97865dd189ad4fff526fbe73559bece876fdbd641e0f50f39271043542")
        }
        "rust-analyzer-x86_64-unknown-linux-gnu.gz" => {
            Some("d42908a7dc7b89250ae881a0919e477296843665c98574ecc8fe16ba60cecefb")
        }
        _ => None,
    }
}

pub fn marksman_sha256(asset: &str) -> Option<&'static str> {
    match asset {
        "marksman-macos" => {
            Some("6a801c17b5ac0dba69787c5282b3b3bd416e66c96253fae098d311c6bbd1833b")
        }
        "marksman-linux-x64" => {
            Some("be5098e8213219269c47fc0d916a66fa31ce0602ec967475c722260aabf26087")
        }
        "marksman-linux-arm64" => {
            Some("db8e124527f7f8048e3e6c91821b9c52ef173d92c01e47d221bf1337afd962fb")
        }
        "marksman.exe" => Some("a6d05beb08ebe41b0a9f09c98a438540421436fa5531424c22e0bb1d22529705"),
        _ => None,
    }
}

pub fn ruff_sha256(asset: &str) -> Option<&'static str> {
    match asset {
        "ruff-aarch64-apple-darwin.tar.gz" => {
            Some("fbf6cbc23d254b0bc03a6fb2b1b04efb917fe5ce068d027e735ce7ed65b9bed6")
        }
        "ruff-x86_64-apple-darwin.tar.gz" => {
            Some("6648fa7a7c95b087c5b9d269d8b9a567fae091bdef3993f77cc7531a01bd7266")
        }
        "ruff-aarch64-unknown-linux-gnu.tar.gz" => {
            Some("b2a2a2573455cc33af98f8a8fb49294c02d4e2e4a7f9e81844411f0a57f30318")
        }
        "ruff-x86_64-unknown-linux-gnu.tar.gz" => {
            Some("3d2c355e641ceb5b608a158c603768fcc908c5009c56c6e78da7487da033b92a")
        }
        _ => None,
    }
}

pub fn taplo_sha256(asset: &str) -> Option<&'static str> {
    match asset {
        "taplo-darwin-aarch64.gz" => {
            Some("713734314c3e71894b9e77513c5349835eefbd52908445a0d73b0c7dc469347d")
        }
        "taplo-darwin-x86_64.gz" => {
            Some("898122cde3a0b1cd1cbc2d52d3624f23338218c91b5ddb71518236a4c2c10ef2")
        }
        "taplo-linux-aarch64.gz" => {
            Some("033681d01eec8376c3fd38fa3703c79316f5e14bb013d859943b60a07bccdcc3")
        }
        "taplo-linux-x86_64.gz" => {
            Some("8fe196b894ccf9072f98d4e1013a180306e17d244830b03986ee5e8eabeb6156")
        }
        _ => None,
    }
}

/// The release asset for a Go `GOOS/GOARCH` platform.
pub fn rust_analyzer_asset(platform: &str) -> Result<&'static str, String> {
    match platform {
        "darwin/arm64" => Ok("rust-analyzer-aarch64-apple-darwin.gz"),
        "darwin/amd64" => Ok("rust-analyzer-x86_64-apple-darwin.gz"),
        "linux/arm64" => Ok("rust-analyzer-aarch64-unknown-linux-gnu.gz"),
        "linux/amd64" => Ok("rust-analyzer-x86_64-unknown-linux-gnu.gz"),
        _ => Err(unsupported(platform, "rust-analyzer")),
    }
}

pub fn marksman_asset(platform: &str) -> Result<&'static str, String> {
    match platform {
        // Upstream ships a single (universal) macOS binary.
        "darwin/arm64" | "darwin/amd64" => Ok("marksman-macos"),
        "linux/amd64" => Ok("marksman-linux-x64"),
        "linux/arm64" => Ok("marksman-linux-arm64"),
        "windows/amd64" | "windows/arm64" => Ok("marksman.exe"),
        _ => Err(unsupported(platform, "marksman")),
    }
}

pub fn ruff_asset(platform: &str) -> Result<&'static str, String> {
    match platform {
        "darwin/arm64" => Ok("ruff-aarch64-apple-darwin.tar.gz"),
        "darwin/amd64" => Ok("ruff-x86_64-apple-darwin.tar.gz"),
        "linux/arm64" => Ok("ruff-aarch64-unknown-linux-gnu.tar.gz"),
        "linux/amd64" => Ok("ruff-x86_64-unknown-linux-gnu.tar.gz"),
        _ => Err(unsupported(platform, "ruff")),
    }
}

pub fn taplo_asset(platform: &str) -> Result<&'static str, String> {
    match platform {
        "darwin/arm64" => Ok("taplo-darwin-aarch64.gz"),
        "darwin/amd64" => Ok("taplo-darwin-x86_64.gz"),
        "linux/arm64" => Ok("taplo-linux-aarch64.gz"),
        "linux/amd64" => Ok("taplo-linux-x86_64.gz"),
        _ => Err(unsupported(platform, "taplo")),
    }
}

/// The host asset, its pinned digest, and a staging dir next to `dest`.
fn prepare(
    asset: Result<&'static str, String>,
    sha: fn(&str) -> Option<&'static str>,
    dest: &Path,
    prefix: &str,
) -> Result<(&'static str, &'static str, tempfile::TempDir), String> {
    let asset = asset?;
    let want = sha(asset).ok_or_else(|| format!("lsp managed: no pinned SHA-256 for {asset}"))?;
    let dir = dest.parent().map(PathBuf::from).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let staging = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(&dir)
        .map_err(|e| e.to_string())?;
    Ok((asset, want, staging))
}

fn rename_or_copy(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_err() {
        copy_file(from, to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) async fn ensure_rust_analyzer(data_dir: &str) -> Result<String, String> {
    ensure_pinned_binary(
        data_dir,
        "rust-analyzer",
        RUST_ANALYZER_RELEASE,
        ENV_RUST_ANALYZER,
        "rust-analyzer",
        "rust-analyzer",
        |dest| async move {
            let (asset, want, staging) = prepare(
                rust_analyzer_asset(&host_platform()),
                rust_analyzer_sha256,
                &dest,
                ".ra-staging-",
            )?;
            let archive = staging.path().join(asset);
            download_verified(
                &format!("{RUST_ANALYZER_BASE}{RUST_ANALYZER_RELEASE}/{asset}"),
                want,
                &archive,
            )
            .await?;
            let staged = staging.path().join("rust-analyzer");
            gunzip_file(&archive, &staged)
                .map_err(|e| format!("lsp managed: rust-analyzer gunzip: {e}"))?;
            chmod_exec(&staged)?;
            rename_or_copy(&staged, &dest)?;
            write_managed_version(data_dir, "rust-analyzer", RUST_ANALYZER_RELEASE)
                .map_err(|e| e.to_string())
        },
    )
    .await
}

pub(super) async fn ensure_marksman(data_dir: &str) -> Result<String, String> {
    ensure_pinned_binary(
        data_dir,
        "marksman",
        MARKSMAN_RELEASE,
        ENV_MARKSMAN,
        "marksman",
        "marksman",
        |dest| async move {
            let (asset, want, staging) = prepare(
                marksman_asset(&host_platform()),
                marksman_sha256,
                &dest,
                ".marksman-staging-",
            )?;
            let downloaded = staging.path().join(asset);
            download_verified(
                &format!("{MARKSMAN_BASE}{MARKSMAN_RELEASE}/{asset}"),
                want,
                &downloaded,
            )
            .await?;
            chmod_exec(&downloaded)?;
            rename_or_copy(&downloaded, &dest)?;
            chmod_exec(&dest)?;
            write_managed_version(data_dir, "marksman", MARKSMAN_RELEASE).map_err(|e| e.to_string())
        },
    )
    .await
}

pub(super) async fn ensure_ruff(data_dir: &str) -> Result<String, String> {
    ensure_pinned_binary(
        data_dir,
        "ruff",
        RUFF_RELEASE,
        ENV_RUFF,
        "ruff",
        "ruff",
        |dest| async move {
            let (asset, want, staging) = prepare(
                ruff_asset(&host_platform()),
                ruff_sha256,
                &dest,
                ".ruff-staging-",
            )?;
            let archive = staging.path().join(asset);
            download_verified(
                &format!("{RUFF_BASE}{RUFF_RELEASE}/{asset}"),
                want,
                &archive,
            )
            .await?;
            let base = dest
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("ruff")
                .to_owned();
            extract_tar_gz_named(&archive, &dest, &base)?;
            chmod_exec(&dest)?;
            write_managed_version(data_dir, "ruff", RUFF_RELEASE).map_err(|e| e.to_string())
        },
    )
    .await
}

pub(super) async fn ensure_taplo(data_dir: &str) -> Result<String, String> {
    ensure_pinned_binary(
        data_dir,
        "taplo",
        TAPLO_RELEASE,
        ENV_TAPLO,
        "taplo",
        "taplo",
        |dest| async move {
            let (asset, want, staging) = prepare(
                taplo_asset(&host_platform()),
                taplo_sha256,
                &dest,
                ".taplo-staging-",
            )?;
            let archive = staging.path().join(asset);
            download_verified(
                &format!("{TAPLO_BASE}{TAPLO_RELEASE}/{asset}"),
                want,
                &archive,
            )
            .await?;
            gunzip_file(&archive, &dest).map_err(|e| format!("lsp managed: taplo gunzip: {e}"))?;
            chmod_exec(&dest)?;
            write_managed_version(data_dir, "taplo", TAPLO_RELEASE).map_err(|e| e.to_string())
        },
    )
    .await
}

/// Release URL for an asset.
#[cfg(test)]
pub(crate) fn release_url(id: &str, asset: &str) -> String {
    match id {
        "rust-analyzer" => format!("{RUST_ANALYZER_BASE}{RUST_ANALYZER_RELEASE}/{asset}"),
        "marksman" => format!("{MARKSMAN_BASE}{MARKSMAN_RELEASE}/{asset}"),
        "ruff" => format!("{RUFF_BASE}{RUFF_RELEASE}/{asset}"),
        _ => format!("{TAPLO_BASE}{TAPLO_RELEASE}/{asset}"),
    }
}
