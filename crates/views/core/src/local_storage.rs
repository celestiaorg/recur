//! A `localStorage`-style key/value store for UI prefs (themes, rail, panel widths, terminal
//! prefs). Values are strings, so a pref reads back as written. Stored as one JSON object in
//! the data dir ([`LOCAL_STORAGE_FILE`]);
//! without a path (tests, galleries) it lives in memory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The file under the data dir.
pub const LOCAL_STORAGE_FILE: &str = "local-storage.json";

#[derive(Default)]
pub struct LocalStorage {
    path: Option<PathBuf>,
    items: BTreeMap<String, String>,
}

impl LocalStorage {
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Reads `path`; a missing or malformed file starts empty.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let items = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            items,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Every key (`localStorage.key(i)`), in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.items.keys().map(String::as_str)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.items.get(key).map(String::as_str)
    }

    /// `localStorage.setItem`. Writes through; a failed write is ignored.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        if self.items.get(key) == Some(&value) {
            return;
        }
        self.items.insert(key.to_string(), value);
        self.flush();
    }

    pub fn remove(&mut self, key: &str) {
        if self.items.remove(key).is_some() {
            self.flush();
        }
    }

    fn flush(&self) {
        let Some(path) = &self.path else { return };
        let Ok(data) = serde_json::to_vec_pretty(&self.items) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LOCAL_STORAGE_FILE);
        let mut s = LocalStorage::open(&path);
        assert_eq!(s.get("recur.theme"), None);
        s.set("recur.theme", "nord");
        s.set("recur.railCollapsed", "1");
        let again = LocalStorage::open(&path);
        assert_eq!(again.get("recur.theme"), Some("nord"));
        assert_eq!(again.get("recur.railCollapsed"), Some("1"));
        std::fs::write(&path, "not json").expect("write");
        assert_eq!(LocalStorage::open(&path).get("recur.theme"), None);
    }
}
