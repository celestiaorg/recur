//! `recur config`: read and write app settings without the GUI.
//!
//! While Recur runs it owns the settings: the request goes over the open-IPC
//! socket and the app applies it to its own copy (and reloads its UI), so it
//! never saves a stale copy over the change. While it is closed the CLI takes
//! the data dir's flock, so a launch waits, and edits `recur.db` through the
//! store.

use std::path::Path;
use std::time::{Duration, Instant};

use recur_platform::open_boot::SettingsMap;
use recur_platform::openipc::{self, IpcError, Request, SettingsRequest};
use recur_store::Store;
use serde_json::Value;

use crate::paths;

/// How long to wait out an app that holds the data dir but is not answering
/// (starting up or quitting).
const RETRY_FOR: Duration = Duration::from_secs(30);
const RETRY_WAIT: Duration = Duration::from_millis(250);

/// A command-line value: JSON when it parses, else the string itself.
pub fn parse_value(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

/// Strings print bare, everything else as compact JSON.
pub fn format_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

/// Applies `req` (a set when it carries a value) and returns every config
/// key with its value afterwards.
pub fn apply(req: SettingsRequest) -> Result<SettingsMap, String> {
    let root = paths::data_dir()?;
    let root = Path::new(&root);
    let deadline = Instant::now() + RETRY_FOR;
    loop {
        match openipc::try_lock_data_dir(root) {
            Ok(_lock) => return apply_closed(root, req),
            Err(IpcError::Held) => {}
            Err(e) => return Err(e.to_string()),
        }
        let call = Request {
            settings: Some(req.clone()),
            ..Request::default()
        };
        match openipc::call(&openipc::sock_path(root), &call) {
            Ok(res) if res.ok => {
                return res.settings.ok_or_else(|| {
                    "the running Recur does not support `recur config`; restart it".to_string()
                });
            }
            Ok(res) if !openipc::retryable(&res) => return Err(res.error),
            Err(e) if !e.is_dial() => return Err(e.to_string()),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err("Recur holds the data dir but is not answering; retry".to_string());
        }
        std::thread::sleep(RETRY_WAIT);
    }
}

/// The app is closed and the flock is ours.
fn apply_closed(root: &Path, req: SettingsRequest) -> Result<SettingsMap, String> {
    let db = recur_store::paths::db_path(root);
    let open = || Store::open_without_secrets(&db).map_err(|e| format!("open recur.db: {e}"));
    let store = open()?;
    let mut set = store.settings();
    if let Some(value) = req.value {
        set = set.with_config_value(&req.key, value)?;
        store.set_settings(set.clone());
        store.close().map_err(|e| e.to_string())?;
        // set_settings does not report a failed write; read it back.
        if open()?.settings() != set {
            return Err("could not save settings to recur.db".to_string());
        }
    }
    Ok(set
        .config_values()
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect())
}
