//! Absolute paths the user dropped onto the app. `/chat-image` and
//! `/external-file` only serve paths from this set, so the webview cannot
//! fetch arbitrary files by crafting a URL.
//!
//! Native drops persist under the data dir so a relaunch can still serve a
//! transcript chip or a restored editor tab. Nothing the webview can call
//! grants a path; editor-session restore is not evidence of a user drop.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::LazyLock;

use parking_lot::Mutex;

use crate::gopath;

const MAX_EXTERNAL_ALLOWED_DEFAULT: usize = 512;

struct State {
    allowed: HashSet<String>,
    /// Oldest first; eviction drops index 0.
    order: Vec<String>,
    file: String,
    max: usize,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        allowed: HashSet::new(),
        order: Vec::new(),
        file: String::new(),
        max: MAX_EXTERNAL_ALLOWED_DEFAULT,
    })
});

/// Points the persist file at `path` and merges previously written entries
/// into the in-memory set. `""` disables persistence. Called once at startup
/// after the data dir is known.
pub fn load_external_allow_file(path: &str) {
    STATE.lock().file = path.to_string();
    if path.is_empty() {
        return;
    }
    let Ok(data) = fs::read(path) else {
        return;
    };
    let Ok(paths) = serde_json::from_slice::<Option<Vec<String>>>(&data) else {
        return;
    };
    let mut st = STATE.lock();
    for p in paths.unwrap_or_default() {
        st.remember(&p);
    }
}

/// Records an absolute path from a native file drop and persists the
/// allowlist when a persist file is configured.
pub fn allow_external_path(abs: &str) {
    let mut st = STATE.lock();
    if st.remember(abs) {
        st.persist();
    }
}

/// Clears the in-memory allowlist (tests). Does not delete the persist file;
/// call `load_external_allow_file("")` to disable persistence.
pub fn reset_external_paths() {
    let mut st = STATE.lock();
    st.allowed.clear();
    st.order.clear();
}

#[cfg(test)]
pub(crate) fn set_max_external_allowed(n: usize) {
    STATE.lock().max = if n == 0 {
        MAX_EXTERNAL_ALLOWED_DEFAULT
    } else {
        n
    };
}

pub(crate) fn external_path_allowed(abs: &str) -> bool {
    STATE.lock().allowed.contains(&gopath::clean(abs))
}

impl State {
    fn remember(&mut self, abs: &str) -> bool {
        let clean = gopath::clean(abs.trim());
        if clean.is_empty() || !gopath::is_abs(&clean) || self.allowed.contains(&clean) {
            return false;
        }
        if self.allowed.len() >= self.max && !self.order.is_empty() {
            let oldest = self.order.remove(0);
            self.allowed.remove(&oldest);
        }
        self.allowed.insert(clean.clone());
        self.order.push(clean);
        true
    }

    fn persist(&self) {
        if self.file.is_empty() {
            return;
        }
        let Ok(data) = serde_json::to_string(&self.order) else {
            return;
        };
        let tmp = format!("{}.tmp", self.file);
        let written = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| std::io::Write::write_all(&mut f, data.as_bytes()));
        if written.is_ok() {
            let _ = fs::rename(&tmp, &self.file);
        }
    }
}

/// Serializes tests that touch the process-wide allowlist.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());
