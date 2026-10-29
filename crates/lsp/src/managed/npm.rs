//! npm language servers installed with `npm install --prefix`.

use std::path::{Path, PathBuf};

use super::{
    ENV_BASED_PYRIGHT, ENV_BASH, ENV_CSS, ENV_HTML, ENV_JSON, ENV_SVELTE, ENV_TYPESCRIPT, ENV_VUE,
    ENV_YAML, chmod_exec, copy_file, ensure_pinned_binary, managed_bin_dir, managed_version,
    run_install, write_managed_version,
};
use crate::servers::look_path;
use crate::uri::is_file;

pub const TYPESCRIPT_LANGUAGE_SERVER_RELEASE: &str = "5.3.0";
/// The typescript package (tsserver host) installed alongside.
pub const TYPESCRIPT_RELEASE: &str = "5.9.2";
pub const BASED_PYRIGHT_RELEASE: &str = "1.39.9";
pub const YAML_LANGUAGE_SERVER_RELEASE: &str = "1.24.0";
pub const BASH_LANGUAGE_SERVER_RELEASE: &str = "5.6.0";
pub const SVELTE_LANGUAGE_SERVER_RELEASE: &str = "0.18.3";
pub const VUE_LANGUAGE_SERVER_RELEASE: &str = "3.3.9";
pub const VSCODE_LANGSERVERS_EXTRACTED_RELEASE: &str = "4.10.0";

/// One npm-installed binary. `prefix_id` shares one install across several
/// bins (vscode-langservers-extracted serves json, html and css).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NpmBinPin {
    pub id: String,
    pub version: String,
    pub env_key: String,
    pub path_name: String,
    pub bin_name: String,
    pub pkgs: Vec<String>,
    pub prefix_id: String,
}

fn pin(id: &str, version: &str, env: &str, bin: &str, pkgs: &[String], prefix: &str) -> NpmBinPin {
    NpmBinPin {
        id: id.into(),
        version: version.into(),
        env_key: env.into(),
        path_name: bin.into(),
        bin_name: bin.into(),
        pkgs: pkgs.to_vec(),
        prefix_id: prefix.into(),
    }
}

pub(super) fn pin_for(id: &str) -> Option<NpmBinPin> {
    let at = |pkg: &str, v: &str| format!("{pkg}@{v}");
    let vscode = |bin: &str, env: &str| {
        pin(
            bin,
            VSCODE_LANGSERVERS_EXTRACTED_RELEASE,
            env,
            bin,
            &[at(
                "vscode-langservers-extracted",
                VSCODE_LANGSERVERS_EXTRACTED_RELEASE,
            )],
            "vscode-langservers-extracted",
        )
    };
    Some(match id {
        "typescript-language-server" => pin(
            id,
            TYPESCRIPT_LANGUAGE_SERVER_RELEASE,
            ENV_TYPESCRIPT,
            "typescript-language-server",
            &[
                at(
                    "typescript-language-server",
                    TYPESCRIPT_LANGUAGE_SERVER_RELEASE,
                ),
                at("typescript", TYPESCRIPT_RELEASE),
            ],
            "",
        ),
        "basedpyright" => pin(
            id,
            BASED_PYRIGHT_RELEASE,
            ENV_BASED_PYRIGHT,
            "basedpyright-langserver",
            &[at("basedpyright", BASED_PYRIGHT_RELEASE)],
            "",
        ),
        "yaml-language-server" => pin(
            id,
            YAML_LANGUAGE_SERVER_RELEASE,
            ENV_YAML,
            "yaml-language-server",
            &[at("yaml-language-server", YAML_LANGUAGE_SERVER_RELEASE)],
            "",
        ),
        "bash-language-server" => pin(
            id,
            BASH_LANGUAGE_SERVER_RELEASE,
            ENV_BASH,
            "bash-language-server",
            &[at("bash-language-server", BASH_LANGUAGE_SERVER_RELEASE)],
            "",
        ),
        "svelte-language-server" => pin(
            id,
            SVELTE_LANGUAGE_SERVER_RELEASE,
            ENV_SVELTE,
            "svelteserver",
            &[at("svelte-language-server", SVELTE_LANGUAGE_SERVER_RELEASE)],
            "",
        ),
        "vue-language-server" => pin(
            id,
            VUE_LANGUAGE_SERVER_RELEASE,
            ENV_VUE,
            "vue-language-server",
            &[at("@vue/language-server", VUE_LANGUAGE_SERVER_RELEASE)],
            "",
        ),
        "vscode-json-language-server" => vscode(id, ENV_JSON),
        "vscode-html-language-server" => vscode(id, ENV_HTML),
        "vscode-css-language-server" => vscode(id, ENV_CSS),
        _ => return None,
    })
}

pub(super) async fn ensure_npm_bin(data_dir: &str, pin: &NpmBinPin) -> Result<String, String> {
    let bin = if pin.bin_name.is_empty() {
        &pin.path_name
    } else {
        &pin.bin_name
    };
    ensure_pinned_binary(
        data_dir,
        &pin.id,
        &pin.version,
        &pin.env_key,
        &pin.path_name,
        bin,
        |dest| install_npm_bin(data_dir, dest, pin),
    )
    .await
}

/// Installs (or reuses) the pinned npm prefix and links `dest` to its
/// `node_modules/.bin` shim.
pub async fn install_npm_bin(data_dir: &str, dest: PathBuf, pin: &NpmBinPin) -> Result<(), String> {
    let prefix_id = if pin.prefix_id.is_empty() {
        &pin.id
    } else {
        &pin.prefix_id
    };
    let prefix = managed_bin_dir(data_dir, prefix_id);
    std::fs::create_dir_all(&prefix).map_err(|e| e.to_string())?;
    let need_install = managed_version(data_dir, prefix_id) != pin.version
        || npm_bin_path(&prefix, &pin.bin_name).is_err();
    if need_install {
        let npm = look_path("npm").ok_or_else(|| {
            format!(
                "lsp managed: npm not on PATH (needed to install {}): exec: \"npm\": executable file not found in $PATH",
                pin.id
            )
        })?;
        let mut cmd = tokio::process::Command::new(npm);
        cmd.args([
            "install",
            "--omit=dev",
            "--no-fund",
            "--no-audit",
            "--prefix",
        ])
        .arg(&prefix)
        .args(&pin.pkgs);
        run_install(cmd)
            .await
            .map_err(|msg| format!("lsp managed: npm install {}: {msg}", pin.id))?;
        write_managed_version(data_dir, prefix_id, &pin.version).map_err(|e| e.to_string())?;
    }
    let installed = npm_bin_path(&prefix, &pin.bin_name)?;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    match std::fs::remove_file(&dest) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
        _ => {}
    }
    if std::os::unix::fs::symlink(&installed, &dest).is_err() {
        copy_file(&installed, &dest).map_err(|e| format!("lsp managed: link {}: {e}", pin.id))?;
    }
    chmod_exec(&dest)?;
    if prefix_id != &pin.id {
        write_managed_version(data_dir, &pin.id, &pin.version).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn npm_bin_path(prefix: &Path, bin_name: &str) -> Result<PathBuf, String> {
    let p = prefix.join("node_modules").join(".bin").join(bin_name);
    if is_file(&p) {
        return Ok(p);
    }
    Err(format!(
        "lsp managed: npm install succeeded but {bin_name} missing under {}",
        prefix.display()
    ))
}
