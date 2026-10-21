//! Redacted bridge stderr: a ring for error tails and an on-disk log (Go
//! `cursor/bridge_log.go`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde_json::Value;

use super::READY_PREFIX;

/// The stderr capture under the log dir; rotated past [`BRIDGE_LOG_MAX_BYTES`].
pub(crate) const BRIDGE_LOG_FILE: &str = "cursor-bridge.log";

pub(crate) const BRIDGE_LOG_MAX_BYTES: u64 = 2 << 20;

/// Lines kept for agent errors: enough context for "[internal] internal
/// error", not enough to bloat the result's stderr.
pub(crate) const BRIDGE_RECENT_LINES: usize = 40;

/// A fixed-size ring of the most recent lines.
struct Ring {
    lines: Vec<String>,
    next: usize,
    full: bool,
}

impl Ring {
    fn new(n: usize) -> Self {
        Self {
            lines: vec![String::new(); n.max(1)],
            next: 0,
            full: false,
        }
    }

    fn add(&mut self, line: String) {
        self.lines[self.next] = line;
        self.next = (self.next + 1) % self.lines.len();
        if self.next == 0 {
            self.full = true;
        }
    }

    fn tail(&self) -> String {
        let ordered: Vec<&String> = if self.full {
            self.lines[self.next..]
                .iter()
                .chain(&self.lines[..self.next])
                .collect()
        } else {
            self.lines[..self.next].iter().collect()
        };
        ordered
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

struct Inner {
    recent: Ring,
    file: Option<File>,
    path: Option<PathBuf>,
}

/// One bridge's stderr record. Safe for concurrent use.
pub(crate) struct BridgeLog {
    inner: Mutex<Inner>,
}

impl BridgeLog {
    /// Opens (appending, rotating when oversized) `<log_dir>/cursor-bridge.log`.
    /// An empty dir or an unopenable file keeps the ring only: logging never
    /// fails a bridge start.
    pub(crate) fn open(log_dir: &Path) -> Self {
        let (file, path) = match open_bridge_log(log_dir) {
            Some(f) => (Some(f), Some(log_dir.join(BRIDGE_LOG_FILE))),
            None => (None, None),
        };
        Self {
            inner: Mutex::new(Inner {
                recent: Ring::new(BRIDGE_RECENT_LINES),
                file,
                path,
            }),
        }
    }

    /// Records one line, redacted, to the ring and the log file.
    pub(crate) fn note(&self, line: &str) {
        let safe = redact_bridge_line(line);
        let mut g = self.inner.lock();
        if let Some(f) = g.file.as_mut() {
            let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
            let _ = writeln!(f, "{ts} {safe}");
        }
        g.recent.add(safe);
    }

    /// The recent lines, plus the log path when one is open.
    pub(crate) fn tail(&self) -> String {
        let g = self.inner.lock();
        let tail = g.recent.tail();
        match &g.path {
            Some(p) if !tail.is_empty() => format!("{tail}\n(bridge log: {})", p.display()),
            _ => tail,
        }
    }

    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.inner.lock().path.clone()
    }

    pub(crate) fn close(&self) {
        self.inner.lock().file = None;
    }
}

fn open_bridge_log(log_dir: &Path) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    if log_dir.as_os_str().is_empty() {
        return None;
    }
    std::fs::create_dir_all(log_dir).ok()?;
    let path = log_dir.join(BRIDGE_LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= BRIDGE_LOG_MAX_BYTES) {
        let mut old = path.clone().into_os_string();
        old.push(".old");
        let _ = std::fs::rename(&path, old);
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .ok()
}

/// Strips secrets from a stderr line before it reaches disk or the ring: the
/// ready line carries `authToken` in plaintext JSON. Re-encodes like Go's
/// `json.Marshal(map[string]any)` (sorted keys).
pub(crate) fn redact_bridge_line(line: &str) -> String {
    let Some(idx) = line.find(READY_PREFIX) else {
        return line.to_owned();
    };
    let head = &line[..idx + READY_PREFIX.len()];
    let mut d = match serde_json::from_str::<Option<BTreeMap<String, Value>>>(
        &line[idx + READY_PREFIX.len()..],
    ) {
        Ok(Some(d)) => d,
        Ok(None) => return format!("{head}null"),
        Err(_) => return format!("{head}[redacted-discovery]"),
    };
    if let Some(v) = d.get_mut("authToken") {
        *v = Value::String("[redacted]".into());
    }
    let obj = Value::Object(d.into_iter().collect());
    format!("{head}{}", crate::gojson::marshal_any(&obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_strips_auth_token() {
        let line = format!(
            r#"{READY_PREFIX}{{"schemaVersion":1,"authToken":"secret-token","url":"http://127.0.0.1:1"}}"#
        );
        let got = redact_bridge_line(&line);
        assert!(!got.contains("secret-token"), "{got}");
        assert!(got.contains("[redacted]"), "{got}");
        assert!(got.contains("http://127.0.0.1:1"), "{got}");
        assert_eq!(
            got,
            format!(
                r#"{READY_PREFIX}{{"authToken":"[redacted]","schemaVersion":1,"url":"http://127.0.0.1:1"}}"#
            )
        );
        assert_eq!(
            redact_bridge_line(&format!("{READY_PREFIX}not json")),
            format!("{READY_PREFIX}[redacted-discovery]")
        );
    }

    #[test]
    fn redact_leaves_ordinary_lines() {
        let line = "some bridge warning: thing failed";
        assert_eq!(redact_bridge_line(line), line);
    }

    #[test]
    fn ring_keeps_last_lines_in_order() {
        let mut r = Ring::new(3);
        assert_eq!(r.tail(), "");
        for l in ["a", "b", "c", "d"] {
            r.add(l.into());
        }
        assert_eq!(r.tail(), "b\nc\nd");
    }
}
