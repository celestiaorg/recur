//! The Recur process log, `<root>/logs/recur.log`, rotated to
//! `recur.log.old` past 2 MiB (also mid-session).
//!
//! Go tees the standard `log` package to this file and points
//! `debug.SetCrashOutput` at it. In Rust the process logger is the `tracing`
//! subscriber the binary installs, so [`tee_app_log`] returns a writer for
//! it ([`AppLogWriter`] is a `MakeWriter`) plus [`GoLogFormat`], which keeps
//! Go's `2006/01/02 15:04:05 message` lines (multi-line messages and stack
//! traces stay verbatim). Panics are the crash output: a panic hook appends
//! the message and a backtrace to the file.

use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::registry::LookupSpan;

use crate::paths::logs_dir;

/// The process log file under [`crate::paths::logs_dir`].
pub const APP_LOG_FILE: &str = "recur.log";

/// The size past which the log rotates (matches cursor-bridge.log), so a
/// stuck debug loop cannot fill the data dir.
pub const APP_LOG_MAX_BYTES: u64 = 2 << 20;

/// `<root>/logs/recur.log` (the file may not exist yet).
pub fn app_log_path(root: &Path) -> PathBuf {
    root.join("logs").join(APP_LOG_FILE)
}

fn open_append(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

struct Inner {
    f: Option<std::fs::File>,
    size: u64,
}

/// An append-only log that rotates to `path.old` when a write would cross
/// max. Writes are serialized. Fail-open: a rotate error keeps the current
/// file (or drops the handle if it cannot be reopened).
pub(crate) struct RotatingFile {
    inner: Mutex<Inner>,
    path: PathBuf,
    max: u64,
}

impl RotatingFile {
    fn open(path: PathBuf, max: u64) -> Option<Self> {
        let f = open_append(&path).ok()?;
        let size = f.metadata().map(|m| m.len()).unwrap_or(0);
        Some(Self {
            inner: Mutex::new(Inner { f: Some(f), size }),
            path,
            max,
        })
    }

    fn write_locked(&self, inner: &mut Inner, p: &[u8]) -> io::Result<usize> {
        if inner.f.is_none() {
            return Ok(p.len());
        }
        if inner.size > 0 && inner.size + p.len() as u64 >= self.max {
            self.rotate_locked(inner);
        }
        let Some(f) = inner.f.as_mut() else {
            return Ok(p.len());
        };
        let n = f.write(p)?;
        inner.size += n as u64;
        Ok(n)
    }

    pub(crate) fn write(&self, p: &[u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock();
        self.write_locked(&mut inner, p)
    }

    fn rotate_locked(&self, inner: &mut Inner) {
        inner.f = None;
        let old = PathBuf::from(format!("{}.old", self.path.display()));
        if std::fs::rename(&self.path, &old).is_err() {
            match open_append(&self.path) {
                Ok(f) => {
                    inner.size = f.metadata().map(|m| m.len()).unwrap_or(0);
                    inner.f = Some(f);
                }
                Err(_) => inner.f = None,
            }
            return;
        }
        match open_append(&self.path) {
            Ok(f) => {
                inner.f = Some(f);
                inner.size = 0;
            }
            Err(_) => inner.f = None,
        }
    }

    fn close(&self) {
        self.inner.lock().f = None;
    }

    /// The crash sink: never blocks a panicking thread for long.
    fn write_crash(&self, p: &[u8]) {
        if let Some(mut inner) = self.inner.try_lock_for(Duration::from_millis(500)) {
            let _ = self.write_locked(&mut inner, p);
        }
    }
}

fn open_app_log(root: &Path) -> Option<RotatingFile> {
    if root.as_os_str().is_empty() {
        return None;
    }
    logs_dir(root).ok()?;
    let path = app_log_path(root);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= APP_LOG_MAX_BYTES) {
        let _ = std::fs::rename(&path, format!("{}.old", path.display()));
    }
    RotatingFile::open(path, APP_LOG_MAX_BYTES)
}

type Prev = Arc<Mutex<Box<dyn Write + Send>>>;

/// Writes to the previous sink (errors ignored, so a broken stderr in a
/// Dock-launched app cannot skip the on-disk write) and to the log file.
#[derive(Clone)]
pub struct AppLogWriter {
    prev: Prev,
    file: Arc<RotatingFile>,
}

impl Write for AppLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = self.prev.lock().write_all(buf);
        self.file.write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = self.prev.lock().flush();
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for AppLogWriter {
    type Writer = AppLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Undoes [`tee_app_log`]: restores the previous panic hook and closes the
/// file (later writes reach only the previous sink). Runs on drop.
pub struct AppLogGuard {
    file: Option<Arc<RotatingFile>>,
    prev_hook: Option<Arc<Hook>>,
}

impl AppLogGuard {
    pub fn cleanup(&mut self) {
        if let Some(prev) = self.prev_hook.take() {
            std::panic::set_hook(Box::new(move |info| prev(info)));
        }
        if let Some(f) = self.file.take() {
            f.close();
        }
    }
}

impl Drop for AppLogGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Tees the process log to `<root>/logs/recur.log` in addition to stderr.
/// See [`tee_app_log_to`].
pub fn tee_app_log(root: &Path) -> (Option<AppLogWriter>, AppLogGuard) {
    tee_app_log_to(root, Box::new(io::stderr()))
}

/// Opens the log file (rotating an oversized one first), routes panics to
/// it, and logs `recur: logging to <path>` through the returned writer,
/// which tees to prev and the file. Install it with
/// `tracing_subscriber::fmt().event_format(GoLogFormat).with_writer(w)`.
///
/// Fail-open: with an empty root or an unopenable file the writer is `None`,
/// the guard is a no-op, and nothing changes.
pub fn tee_app_log_to(
    root: &Path,
    prev: Box<dyn Write + Send>,
) -> (Option<AppLogWriter>, AppLogGuard) {
    let Some(file) = open_app_log(root) else {
        return (
            None,
            AppLogGuard {
                file: None,
                prev_hook: None,
            },
        );
    };
    let file = Arc::new(file);
    let mut w = AppLogWriter {
        prev: Arc::new(Mutex::new(prev)),
        file: Arc::clone(&file),
    };

    let prev_hook: Arc<Hook> = Arc::new(std::panic::take_hook());
    {
        let file = Arc::clone(&file);
        let prev_hook = Arc::clone(&prev_hook);
        std::panic::set_hook(Box::new(move |info| {
            let bt = std::backtrace::Backtrace::force_capture();
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            file.write_crash(format!("thread '{name}' {info}\n{bt}\n").as_bytes());
            prev_hook(info);
        }));
    }

    let _ =
        w.write_all(go_log_line(&format!("recur: logging to {}", file.path.display())).as_bytes());
    (
        Some(w),
        AppLogGuard {
            file: Some(file),
            prev_hook: Some(prev_hook),
        },
    )
}

/// `2006/01/02 15:04:05 msg\n` in local time, like Go's default logger.
fn go_log_line(msg: &str) -> String {
    let ts = chrono::Local::now().format("%Y/%m/%d %H:%M:%S");
    if msg.ends_with('\n') {
        format!("{ts} {msg}")
    } else {
        format!("{ts} {msg}\n")
    }
}

/// A `tracing` event format matching Go's `log.Printf` output: local
/// timestamp, then the message and fields exactly as written (no level, no
/// target, no escaping), one trailing newline.
#[derive(Debug, Clone, Copy, Default)]
pub struct GoLogFormat;

impl<S, N> FormatEvent<S, N> for GoLogFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let mut msg = String::new();
        ctx.format_fields(Writer::new(&mut msg), event)?;
        writer.write_str(&go_log_line(&msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock()).into_owned()
        }
    }

    fn log_with(w: &AppLogWriter, f: impl FnOnce()) {
        let sub = tracing_subscriber::fmt()
            .event_format(GoLogFormat)
            .with_writer(w.clone())
            .finish();
        tracing::subscriber::with_default(sub, f);
    }

    #[test]
    fn tee_writes_file_and_previous_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let prev = Buf::default();
        let (w, mut guard) = tee_app_log_to(root.path(), Box::new(prev.clone()));
        let w = w.unwrap();
        log_with(&w, || tracing::info!("hello from TeeAppLog"));
        let body = std::fs::read_to_string(app_log_path(root.path())).unwrap();
        assert!(body.contains("hello from TeeAppLog"), "{body}");
        assert!(body.contains("recur: logging to"), "{body}");
        assert!(prev.text().contains("hello from TeeAppLog"));
        guard.cleanup();
        let mut w2 = w.clone();
        w2.write_all(b"after cleanup\n").unwrap();
        assert!(prev.text().contains("after cleanup"));
        let body = std::fs::read_to_string(app_log_path(root.path())).unwrap();
        assert!(!body.contains("after cleanup"));
    }

    #[test]
    fn multiline_messages_stay_verbatim() {
        let root = tempfile::tempdir().unwrap();
        let (w, _guard) = tee_app_log_to(root.path(), Box::new(io::sink()));
        let w = w.unwrap();
        log_with(&w, || {
            tracing::warn!(
                "recur: loop {} panicked: {}\n{}",
                "L1",
                "boom",
                "goroutine 1 [running]:"
            )
        });
        let got = std::fs::read_to_string(app_log_path(root.path())).unwrap();
        assert!(
            !got.contains("msg=\"") && !got.contains("\\ngoroutine"),
            "{got}"
        );
        assert!(got.contains("\ngoroutine 1 [running]:"), "{got}");
        let line = got.lines().last().unwrap();
        assert!(!line.contains("WARN"), "{got}");
        let first = got.lines().next().unwrap();
        let ts = &first[..19];
        assert!(
            chrono::NaiveDateTime::parse_from_str(ts, "%Y/%m/%d %H:%M:%S").is_ok(),
            "{first}"
        );
    }

    #[test]
    fn rotates_oversized_file_on_open() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("logs")).unwrap();
        let path = app_log_path(root.path());
        let old = vec![b'x'; APP_LOG_MAX_BYTES as usize];
        std::fs::write(&path, &old).unwrap();
        let (w, _guard) = tee_app_log_to(root.path(), Box::new(io::sink()));
        let mut w = w.unwrap();
        w.write_all(b"after rotate\n").unwrap();
        let rotated = std::fs::read(format!("{}.old", path.display())).unwrap();
        assert_eq!(rotated, old);
        let body = std::fs::read(&path).unwrap();
        assert!(String::from_utf8_lossy(&body).contains("after rotate"));
        assert!(!body.windows(32).any(|w| w == &old[..32]));
    }

    #[test]
    fn rotating_file_rotates_mid_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(APP_LOG_FILE);
        let r = RotatingFile::open(path.clone(), 64).unwrap();
        assert_eq!(r.write(b"hello\n").unwrap(), 6);
        let second = vec![b'x'; 64];
        assert_eq!(r.write(&second).unwrap(), 64);
        assert_eq!(
            std::fs::read(format!("{}.old", path.display())).unwrap(),
            b"hello\n"
        );
        assert_eq!(std::fs::read(&path).unwrap(), second);
    }

    #[test]
    fn empty_root_is_noop() {
        let (w, _guard) = tee_app_log_to(Path::new(""), Box::new(io::sink()));
        assert!(w.is_none());
    }

    #[test]
    fn unopenable_root_fails_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("blocked");
        std::fs::write(&root, "not a dir").unwrap();
        let (w, _guard) = tee_app_log_to(&root, Box::new(io::sink()));
        assert!(w.is_none());
    }

    /// The panic hook is process-global, so this runs in its own process.
    #[test]
    fn panics_land_in_the_log() {
        let no_env: [(&str, &str); 0] = [];
        crate::testenv::run(module_path!(), "panics_land_in_the_log", &no_env, || {
            let root = tempfile::tempdir().unwrap();
            let (_w, mut guard) = tee_app_log_to(root.path(), Box::new(io::sink()));
            let _ = std::thread::Builder::new()
                .name("crasher".into())
                .spawn(|| panic!("boom for the crash log"))
                .unwrap()
                .join();
            guard.cleanup();
            let body = std::fs::read_to_string(app_log_path(root.path())).unwrap();
            assert!(
                body.contains("thread 'crasher'") && body.contains("boom for the crash log"),
                "{body}"
            );
        });
    }
}
