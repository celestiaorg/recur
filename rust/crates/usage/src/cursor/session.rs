//! Recur-owned Cursor Connect tokens on disk (Go `usage/cursor_session.go`).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::types::SOURCE_CURSOR_CONNECT;

const CURSOR_AUTH_FILE_NAME: &str = "cursor-usage-auth.json";

/// The Connect session plus the user API key minted from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorTok {
    pub access: String,
    pub refresh: String,
    pub email: String,
    pub api_key: String,
    pub api_key_expires_at_ms: i64,
    pub api_key_id: i64,
    pub source: String,
}

pub(crate) fn cursor_auth_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CURSOR_AUTH_FILE_NAME)
}

/// Whether Recur-owned Connect tokens exist on disk (stat only).
pub fn has_recur_cursor_auth_file(data_dir: &Path) -> bool {
    !data_dir.as_os_str().is_empty() && cursor_auth_path(data_dir).exists()
}

pub(crate) fn default_cursor_token(data_dir: Option<&Path>) -> Result<CursorTok, String> {
    let no_session = || "no Cursor usage session".to_owned();
    let dir = data_dir.ok_or_else(no_session)?;
    match read_cursor_auth_file(&cursor_auth_path(dir)) {
        Ok(mut tok) if !tok.access.is_empty() => {
            tok.source = SOURCE_CURSOR_CONNECT.into();
            Ok(tok)
        }
        _ => Err(no_session()),
    }
}

pub(crate) fn read_cursor_auth_file(path: &Path) -> Result<CursorTok, String> {
    let raw = std::fs::read(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    parse_cursor_auth_file(&raw)
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AuthFile {
    access_token: Option<String>,
    refresh_token: Option<String>,
    email: Option<String>,
    api_key: Option<String>,
    api_key_expires_at_ms: Option<i64>,
    api_key_id: Option<i64>,
}

pub(crate) fn parse_cursor_auth_file(raw: &[u8]) -> Result<CursorTok, String> {
    let file = serde_json::from_slice::<Option<AuthFile>>(raw)
        .map_err(|e| format!("cursor usage auth: {e}"))?
        .unwrap_or_default();
    let s = |v: Option<String>| v.unwrap_or_default().trim().to_owned();
    let tok = CursorTok {
        access: s(file.access_token),
        refresh: s(file.refresh_token),
        email: s(file.email),
        api_key: s(file.api_key),
        api_key_expires_at_ms: file.api_key_expires_at_ms.unwrap_or(0),
        api_key_id: file.api_key_id.unwrap_or(0),
        source: SOURCE_CURSOR_CONNECT.into(),
    };
    if tok.access.is_empty() {
        return Err("cursor usage auth is empty".into());
    }
    Ok(tok)
}

/// The file Go's `json.MarshalIndent(…, "", "  ")` writes, mode 0600.
pub(crate) fn write_cursor_auth_file(path: &Path, tok: &CursorTok) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let q = |s: &str| recur_domain::json::to_string(s).unwrap_or_default();
    let mut lines = vec![
        format!("  \"accessToken\": {}", q(&tok.access)),
        format!("  \"refreshToken\": {}", q(&tok.refresh)),
        format!("  \"email\": {}", q(&tok.email)),
    ];
    if !tok.api_key.is_empty() {
        lines.push(format!("  \"apiKey\": {}", q(&tok.api_key)));
    }
    if tok.api_key_expires_at_ms != 0 {
        lines.push(format!(
            "  \"apiKeyExpiresAtMs\": {}",
            tok.api_key_expires_at_ms
        ));
    }
    if tok.api_key_id != 0 {
        lines.push(format!("  \"apiKeyId\": {}", tok.api_key_id));
    }
    let body = format!("{{\n{}\n}}", lines.join(",\n"));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(body.as_bytes())
}

/// The user API key minted from Recur Connect, if any.
pub fn recur_cursor_api_key(data_dir: &Path) -> String {
    if data_dir.as_os_str().is_empty() {
        return String::new();
    }
    read_cursor_auth_file(&cursor_auth_path(data_dir))
        .map(|t| t.api_key)
        .unwrap_or_default()
}

pub(crate) fn remove_cursor_auth_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes Recur-owned Connect tokens. Callers that can revoke the minted
/// dashboard key should do that first (the poller does).
pub fn delete_recur_cursor_auth(data_dir: &Path) -> std::io::Result<()> {
    if data_dir.as_os_str().is_empty() {
        return Ok(());
    }
    remove_cursor_auth_file(&cursor_auth_path(data_dir))
}
