//! Working-tree status letters and in-flight coalescing of status work.
//!
//! An Observatory switch fires tree, status, changed-files and land-state
//! refreshes in parallel, and each used to shell its own merge-base / diff /
//! porcelain. Concurrent git on one worktree serializes on `index.lock`, so
//! the parallel refresh was often slower than one pass. In-flight coalescing
//! shares one computation among concurrent callers. Results are not cached
//! after the flight lands: the next refresh must see disk.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use recur_domain::path;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::command::command;

/// Path → status letter: `m` modified, `a` added, `d` deleted, `u` untracked.
pub type StatusLetters = HashMap<String, String>;

/// Budget for one coalesced status computation. The work is detached from
/// its callers: a caller that goes away does not cancel it for the others.
const STATUS_WORK_TIMEOUT: Duration = Duration::from_secs(15);

type Flight = watch::Receiver<Option<Option<Arc<StatusLetters>>>>;

#[derive(Default)]
pub(crate) struct StatusFlights {
    calls: Mutex<HashMap<String, Flight>>,
}

pub(crate) static CHANGE_STATUS_FLIGHTS: LazyLock<StatusFlights> =
    LazyLock::new(StatusFlights::default);
static STATUS_MAP_FLIGHTS: LazyLock<StatusFlights> = LazyLock::new(StatusFlights::default);

impl StatusFlights {
    /// Joins the in-flight computation for `key`, or starts one running
    /// `work` with a fresh [`STATUS_WORK_TIMEOUT`] deadline.
    pub(crate) async fn run<W, F>(&'static self, key: String, work: W) -> Option<StatusLetters>
    where
        W: FnOnce(Instant) -> F,
        F: Future<Output = Option<StatusLetters>> + Send + 'static,
    {
        let mut rx = {
            let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
            match calls.get(&key) {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    calls.insert(key.clone(), rx.clone());
                    let fut = work(Instant::now() + STATUS_WORK_TIMEOUT);
                    tokio::spawn(async move {
                        let val = fut.await.map(Arc::new);
                        hooks::fire("flight-hold", &key).await;
                        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
                        calls.remove(&key);
                        let _ = tx.send(Some(val));
                    });
                    rx
                }
            }
        };
        let landed = rx.wait_for(Option::is_some).await.ok()?;
        landed.as_ref().and_then(|v| v.as_deref().cloned())
    }
}

/// `git status --porcelain` in `dir` as status letters, matching diff status
/// letters plus `u` for untracked. Best-effort: `None` when `dir` is not a
/// work tree or git fails. Concurrent callers share one git run.
pub async fn status_map(dir: &str) -> Option<StatusLetters> {
    if dir.is_empty() {
        return None;
    }
    let owned = dir.to_string();
    STATUS_MAP_FLIGHTS
        .run(dir.to_string(), move |deadline| {
            status_map_work(owned, Some(deadline))
        })
        .await
}

/// Always runs git. Discard must not join an in-flight poll whose snapshot
/// predates the caller's own write: a coalesced "path absent" would make
/// discard a silent no-op, and a stale `u` would delete a tracked file.
pub(crate) async fn status_map_now(dir: &str) -> Option<StatusLetters> {
    if dir.is_empty() {
        return None;
    }
    status_map_work(dir.to_string(), None).await
}

async fn status_map_work(dir: String, deadline: Option<Instant>) -> Option<StatusLetters> {
    hooks::fire("porcelain", &dir).await;
    let mut cmd = command(["-C", dir.as_str(), "status", "--porcelain", "-uall"]);
    if let Some(d) = deadline {
        cmd = cmd.deadline(d);
    }
    let out = cmd.output().await.ok()?;
    parse_status_porcelain(&String::from_utf8_lossy(&out))
}

/// Parses `git status --porcelain` into path → `m|a|d|u`. Renames and copies
/// mark the destination added and the source deleted. Ignored (`!!`) entries
/// are skipped.
pub fn parse_status_porcelain(out: &str) -> Option<StatusLetters> {
    let out = out.strip_suffix('\n').unwrap_or(out);
    if out.is_empty() {
        return None;
    }
    let mut m = StatusLetters::new();
    for line in out.split('\n') {
        if line.len() < 3 || !line.is_char_boundary(2) || !line.is_char_boundary(3) {
            continue;
        }
        let xy = &line[..2];
        if xy == "!!" {
            continue;
        }
        let rest = &line[3..];
        if rest.is_empty() {
            continue;
        }
        let (from, to) = split_status_path(rest);
        let to = to.strip_suffix('/').unwrap_or(&to).to_string();
        if to.is_empty() {
            continue;
        }
        if !from.is_empty() {
            let from = from.strip_suffix('/').unwrap_or(&from);
            if !from.is_empty() {
                m.insert(from.to_string(), "d".to_string());
            }
            m.insert(to, "a".to_string());
            continue;
        }
        let st = status_letter(xy.as_bytes());
        if st.is_empty() {
            continue;
        }
        m.insert(to, st.to_string());
    }
    (!m.is_empty()).then_some(m)
}

/// Handles `path`, `old -> new` and quoted forms.
fn split_status_path(rest: &str) -> (String, String) {
    let rest = rest.trim();
    match rest.find(" -> ") {
        Some(i) => (unquote_path(&rest[..i]), unquote_path(&rest[i + 4..])),
        None => (String::new(), unquote_path(rest)),
    }
}

fn unquote_path(p: &str) -> String {
    let mut p = p.trim().to_string();
    if p.len() >= 2 && p.starts_with('"') && p.ends_with('"') {
        // Porcelain quotes are C-style; unescape the common cases.
        p = p[1..p.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
            .replace("\\t", "\t")
            .replace("\\n", "\n");
    }
    let p = p.replace('\\', "/");
    if p == "." || p.is_empty() {
        return String::new();
    }
    path::clean(&p)
}

/// Collapses the XY pair into one letter. Priority: deleted > added >
/// modified > untracked.
fn status_letter(xy: &[u8]) -> &'static str {
    if xy == b"??" {
        return "u";
    }
    let rank = |c: u8| match c {
        b'D' => 4,
        b'A' | b'C' => 3,
        b'M' | b'R' | b'T' | b'U' => 2,
        b'?' => 1,
        _ => 0,
    };
    let (x, y) = (xy[0], xy[1]);
    let pick = if rank(y) > rank(x) { y } else { x };
    match pick {
        b'D' => "d",
        b'A' | b'C' => "a",
        b'M' | b'R' | b'T' | b'U' => "m",
        b'?' => "u",
        _ => "",
    }
}

/// Test hooks that run inside the real git work, not on waiters, so tests
/// can count coalescing and hold a flight open.
pub(crate) mod hooks {
    #[cfg(test)]
    pub(crate) use imp::*;

    #[cfg(test)]
    mod imp {
        use std::collections::HashMap;
        use std::pin::Pin;
        use std::sync::{Arc, LazyLock, Mutex, PoisonError};

        pub(crate) type Hook =
            Arc<dyn Fn(&'static str) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

        static HOOKS: LazyLock<Mutex<HashMap<String, Hook>>> = LazyLock::new(Default::default);

        /// Installs `hook` for status work whose key starts with `dir`.
        pub(crate) fn set(dir: &str, hook: Hook) {
            HOOKS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(dir.to_string(), hook);
        }

        pub(crate) fn clear(dir: &str) {
            HOOKS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(dir);
        }

        pub(crate) async fn fire(kind: &'static str, key: &str) {
            let hook = {
                let hooks = HOOKS.lock().unwrap_or_else(PoisonError::into_inner);
                hooks
                    .iter()
                    .find(|(dir, _)| key.starts_with(dir.as_str()))
                    .map(|(_, h)| h.clone())
            };
            if let Some(h) = hook {
                h(kind).await;
            }
        }
    }

    #[cfg(not(test))]
    pub(crate) async fn fire(_kind: &'static str, _key: &str) {}
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::testutil::{git, temp_dir, write};

    /// Counts hook firings of `kind` for `dir`, sleeping in each so
    /// concurrent callers overlap.
    fn count_kind(dir: &str, kind: &'static str) -> Arc<AtomicUsize> {
        let n = Arc::new(AtomicUsize::new(0));
        let counter = n.clone();
        hooks::set(
            dir,
            Arc::new(move |k| {
                let counter = counter.clone();
                Box::pin(async move {
                    if k == kind {
                        counter.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(80)).await;
                    }
                })
            }),
        );
        n
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn change_status_coalesces_concurrent_calls() {
        let root = temp_dir();
        git(&root.path, &["init", "-b", "main"]);
        write(&root.path, "base.go", "package b\n");
        git(&root.path, &["add", "."]);
        git(&root.path, &["commit", "-m", "base"]);
        git(&root.path, &["checkout", "-b", "recur/feat"]);
        write(&root.path, "feat.go", "package f\n");
        git(&root.path, &["add", "."]);
        git(&root.path, &["commit", "-m", "feat"]);

        let vs_base = count_kind(&root.path, "vs-base");
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let dir = root.path.clone();
                tokio::spawn(async move { crate::working_diff::change_status(&dir, "main").await })
            })
            .collect();
        let mut results = Vec::new();
        for t in tasks {
            results.push(t.await.expect("join"));
        }
        assert_eq!(
            vs_base.load(Ordering::SeqCst),
            1,
            "concurrent change_status ran git more than once"
        );
        for (i, st) in results.iter().enumerate() {
            let st = st.clone().unwrap_or_default();
            assert_eq!(
                st.get("feat.go").map(String::as_str),
                Some("a"),
                "task {i}: {st:?}"
            );
        }
        // After the flight lands, a later call recomputes (no stale cache).
        let _ = crate::working_diff::change_status(&root.path, "main").await;
        assert_eq!(
            vs_base.load(Ordering::SeqCst),
            2,
            "change_status after the flight did not recompute"
        );
        hooks::clear(&root.path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn status_map_coalesces_concurrent_calls() {
        let root = temp_dir();
        git(&root.path, &["init", "-b", "main"]);
        write(&root.path, "a.go", "package a\n");
        git(&root.path, &["add", "."]);
        git(&root.path, &["commit", "-m", "init"]);
        write(&root.path, "a.go", "package a\nfunc F() {}\n");

        let porcelain = count_kind(&root.path, "porcelain");
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let dir = root.path.clone();
                tokio::spawn(async move { status_map(&dir).await })
            })
            .collect();
        for (i, t) in tasks.into_iter().enumerate() {
            let st = t.await.expect("join").unwrap_or_default();
            assert_eq!(
                st.get("a.go").map(String::as_str),
                Some("m"),
                "task {i}: {st:?}"
            );
        }
        assert_eq!(
            porcelain.load(Ordering::SeqCst),
            1,
            "concurrent status_map ran git more than once"
        );
        hooks::clear(&root.path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn discard_file_does_not_join_in_flight_status_map() {
        let root = temp_dir();
        git(&root.path, &["init", "-b", "main"]);
        write(&root.path, "a.go", "package a\n");
        git(&root.path, &["add", "."]);
        git(&root.path, &["commit", "-m", "init"]);

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let entered = Arc::new(Mutex::new(Some(entered_tx)));
        let release = Arc::new(tokio::sync::Notify::new());
        {
            let (entered, release) = (entered.clone(), release.clone());
            hooks::set(
                &root.path,
                Arc::new(move |k| {
                    let (entered, release) = (entered.clone(), release.clone());
                    Box::pin(async move {
                        if k == "flight-hold" {
                            if let Some(tx) = entered
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .take()
                            {
                                let _ = tx.send(());
                            }
                            release.notified().await;
                        }
                    })
                }),
            );
        }
        let dir = root.path.clone();
        let flight = tokio::spawn(async move { status_map(&dir).await });
        tokio::time::timeout(Duration::from_secs(5), entered_rx)
            .await
            .expect("status_map never reached flight-hold")
            .expect("hook dropped");

        write(&root.path, "fresh.go", "package f\n");
        let discard = tokio::time::timeout(
            Duration::from_secs(2),
            crate::scm::discard_file(&root.path, "fresh.go"),
        )
        .await
        .expect("discard_file blocked on the coalesced status_map");
        discard.expect("discard_file");
        assert!(
            std::fs::metadata(format!("{}/fresh.go", root.path)).is_err(),
            "discard_file should remove fresh.go"
        );
        release.notify_one();
        hooks::clear(&root.path);
        let _ = flight.await;
    }

    #[test]
    fn parse_status_porcelain_letters() {
        let input = [
            " M src/main.go",
            "A  new.txt",
            " D gone.md",
            "?? scratch.go",
            "R  old.go -> renamed.go",
            "!! ignored.bin",
            "MM both.go",
            "AD staged-del.go",
        ]
        .join("\n");
        let got = parse_status_porcelain(&input).unwrap_or_default();
        let want: StatusLetters = [
            ("src/main.go", "m"),
            ("new.txt", "a"),
            ("gone.md", "d"),
            ("scratch.go", "u"),
            ("old.go", "d"),
            ("renamed.go", "a"),
            ("both.go", "m"),
            ("staged-del.go", "d"), // D beats A
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn parse_status_porcelain_quoted() {
        let got = parse_status_porcelain(r#"?? "weird name.txt""#).unwrap_or_default();
        assert_eq!(
            got.get("weird name.txt").map(String::as_str),
            Some("u"),
            "got {got:?}"
        );
    }

    #[test]
    fn parse_status_porcelain_empty() {
        assert_eq!(parse_status_porcelain(""), None);
    }
}
