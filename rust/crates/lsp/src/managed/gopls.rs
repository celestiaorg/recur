//! gopls via `go install` (Go `lsp/managed/gopls.go`).

use std::path::PathBuf;

use super::{ENV_GOPLS, chmod_exec, ensure_pinned_binary, run_install, write_managed_version};
use crate::servers::{is_executable, look_path};

/// The pinned gopls module version installed into the data dir.
pub const GOPLS_RELEASE: &str = "v0.23.0";
pub const GOPLS_MODULE: &str = "golang.org/x/tools/gopls@v0.23.0";

pub(super) async fn ensure_gopls(data_dir: &str) -> Result<String, String> {
    ensure_pinned_binary(
        data_dir,
        "gopls",
        GOPLS_RELEASE,
        ENV_GOPLS,
        "gopls",
        "gopls",
        |dest| install_gopls(data_dir.to_owned(), dest),
    )
    .await
}

async fn install_gopls(data_dir: String, dest: PathBuf) -> Result<(), String> {
    let go = look_path("go").ok_or_else(|| {
        "lsp managed: go not on PATH (needed to install gopls): exec: \"go\": executable file not found in $PATH".to_owned()
    })?;
    let dir = dest.parent().map(PathBuf::from).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut cmd = tokio::process::Command::new(go);
    cmd.args(["install", GOPLS_MODULE]).env("GOBIN", &dir);
    run_install(cmd)
        .await
        .map_err(|msg| format!("lsp managed: go install gopls: {msg}"))?;
    if !is_executable(&dest) {
        return Err(format!(
            "lsp managed: go install succeeded but {} missing",
            dest.display()
        ));
    }
    chmod_exec(&dest)?;
    write_managed_version(&data_dir, "gopls", GOPLS_RELEASE).map_err(|e| e.to_string())
}
