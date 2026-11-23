//! Watch schedules: while Recur runs, it keeps each unpaused watch schedule's
//! command running in the project root (restarting it with backoff) and turns
//! every non-empty stdout line into an event. The scheduler tick starts one
//! occurrence per batch of pending events once the schedule has no active run,
//! through the same claim and launch as a cron occurrence.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_domain::{WatchStatus, Workspace};
use recur_project::runconfig::{Streamed, exit_status_string, spawn_streamed};
use recur_store::Store;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio_util::sync::CancellationToken;

use crate::Engine;

/// simplify: pending events live in memory, newest 100 kept, and are lost
/// when Recur quits. Persist them in a table if a backlog must survive.
const MAX_PENDING: usize = 100;
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A command that ran this long restarts after the shortest backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Watchers {
    /// The scheduler's stop token: watchers run as its children, so Shutdown
    /// stops them. `None` until the ticker starts.
    pub(crate) root: Option<CancellationToken>,
    by_id: HashMap<String, Watcher>,
}

struct Watcher {
    command: String,
    cancel: CancellationToken,
    state: Arc<Mutex<WatchState>>,
}

#[derive(Default)]
struct WatchState {
    status: WatchStatus,
    events: Vec<Value>,
}

impl WatchState {
    fn push(&mut self, v: Value) {
        self.status.last_event_at = chrono::Utc::now().timestamp_millis();
        self.events.push(v);
        if self.events.len() > MAX_PENDING {
            self.events.remove(0);
        }
    }
}

/// One stdout line as an event: its JSON value when it parses, else its text.
/// Blank lines are no event.
pub(crate) fn parse_event(line: &str) -> Option<Value> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.trim().is_empty() {
        return None;
    }
    Some(serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_owned())))
}

/// The saved prompt with the batch appended as a JSON array, one event per
/// line, oldest first. No events leaves the prompt as saved.
pub(crate) fn prompt_with_events(prompt: &str, events: &[Value]) -> String {
    if events.is_empty() {
        return prompt.to_owned();
    }
    let items: Vec<String> = events.iter().map(|e| format!("  {e}")).collect();
    format!(
        "{prompt}\n\nWatch events ({}), oldest first:\n```json\n[\n{}\n]\n```",
        events.len(),
        items.join(",\n")
    )
}

impl Engine {
    /// Starts watchers for unpaused watch schedules and stops the rest
    /// (paused, deleted, or command changed). A no-op before the ticker starts.
    pub(crate) fn sync_watchers(&self) {
        let want = match self.store.watch_scheduled_prompts() {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!("watch schedules: {e}");
                return;
            }
        };
        let mut w = self.watchers.lock();
        let Some(root) = w.root.clone() else {
            return;
        };
        w.by_id.retain(|id, x| {
            let keep = want
                .iter()
                .any(|p| p.id == *id && p.watch_command == x.command);
            if !keep {
                x.cancel.cancel();
            }
            keep
        });
        for p in want {
            if w.by_id.contains_key(&p.id) {
                continue;
            }
            let x = Watcher {
                command: p.watch_command.clone(),
                cancel: root.child_token(),
                state: Arc::default(),
            };
            self.tasks.schedules.spawn(watch_loop(
                x.command.clone(),
                Arc::clone(&self.store),
                p.project_id,
                x.cancel.clone(),
                Arc::clone(&x.state),
            ));
            w.by_id.insert(p.id, x);
        }
    }

    /// The live watcher of a watch schedule; `None` when Recur is not running one.
    pub fn watch_status(&self, id: &str) -> Option<WatchStatus> {
        let w = self.watchers.lock();
        let s = w.by_id.get(id)?.state.lock();
        Some(WatchStatus {
            pending: s.events.len(),
            ..s.status.clone()
        })
    }

    pub(crate) fn watch_pending_ids(&self) -> Vec<String> {
        let w = self.watchers.lock();
        w.by_id
            .iter()
            .filter(|(_, x)| !x.state.lock().events.is_empty())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(crate) fn take_watch_events(&self, id: &str) -> Vec<Value> {
        let w = self.watchers.lock();
        w.by_id
            .get(id)
            .map(|x| std::mem::take(&mut x.state.lock().events))
            .unwrap_or_default()
    }

    /// Puts an unclaimed batch back ahead of anything that arrived meanwhile.
    pub(crate) fn restore_watch_events(&self, id: &str, mut events: Vec<Value>) {
        let w = self.watchers.lock();
        if let Some(x) = w.by_id.get(id) {
            let mut s = x.state.lock();
            events.append(&mut s.events);
            let over = events.len().saturating_sub(MAX_PENDING);
            events.drain(..over);
            s.events = events;
        }
    }

    /// Queues lines as if a watcher printed them, with no process behind it.
    #[cfg(test)]
    pub(crate) fn push_watch_lines(&self, id: &str, lines: &[&str]) {
        let mut w = self.watchers.lock();
        let x = w.by_id.entry(id.to_owned()).or_insert_with(|| Watcher {
            command: String::new(),
            cancel: CancellationToken::new(),
            state: Arc::default(),
        });
        let mut s = x.state.lock();
        for v in lines.iter().filter_map(|l| parse_event(l)) {
            s.push(v);
        }
    }
}

/// The project root and its Env, read fresh so a restart picks up edits.
fn project_root_env(store: &Store, project_id: &str) -> (String, Vec<String>) {
    let proj = store.project(project_id).unwrap_or_default();
    let root_ws = Workspace {
        project_id: project_id.to_owned(),
        worktree_path: proj.repo_path.clone(),
        ..Workspace::default()
    };
    let env = recur_project::wsenv::Vars::from_workspace(&root_ws, &proj).environ(None);
    (proj.repo_path, env)
}

async fn watch_loop(
    command: String,
    store: Arc<Store>,
    project_id: String,
    cancel: CancellationToken,
    state: Arc<Mutex<WatchState>>,
) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        let started = tokio::time::Instant::now();
        let (cwd, env) = project_root_env(&store, &project_id);
        let error = match spawn_streamed(&command, &cwd, &env) {
            Err(e) => format!("could not start: {e}"),
            Ok(mut proc) => {
                {
                    let mut s = state.lock();
                    s.status.running = true;
                    s.status.error.clear();
                }
                let error = watch_proc(&mut proc, &cancel, &state).await;
                proc.stop().await;
                state.lock().status.running = false;
                error
            }
        };
        if cancel.is_cancelled() {
            return;
        }
        state.lock().status.error = error;
        if started.elapsed() >= HEALTHY_RUN {
            backoff = FIRST_BACKOFF;
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Reads events until stdout closes, then reports how the command ended.
async fn watch_proc(
    proc: &mut Streamed,
    cancel: &CancellationToken,
    state: &Mutex<WatchState>,
) -> String {
    let last_err = Arc::new(Mutex::new(String::new()));
    let stderr = proc.stderr.take().map(|e| {
        let last = Arc::clone(&last_err);
        tokio::spawn(each_line(e, move |l| {
            if !l.trim().is_empty() {
                *last.lock() = l.trim().to_owned();
            }
        }))
    });
    if let Some(out) = proc.stdout.take() {
        tokio::select! {
            () = cancel.cancelled() => return String::new(),
            () = each_line(out, |l| {
                if let Some(v) = parse_event(&l) {
                    state.lock().push(v);
                }
            }) => {}
        }
    }
    let status = tokio::select! {
        () = cancel.cancelled() => return String::new(),
        s = proc.wait() => s,
    };
    if let Some(h) = stderr {
        let _ = tokio::time::timeout(Duration::from_millis(200), h).await;
    }
    let mut msg = match status {
        Ok(s) => exit_status_string(s),
        Err(e) => e,
    };
    let last = last_err.lock();
    if !last.is_empty() {
        msg = format!("{msg}: {last}");
    }
    msg
}

/// Calls `f` with each line (lossy UTF-8) until EOF or a read error.
async fn each_line(r: impl AsyncRead + Unpin, mut f: impl FnMut(String)) {
    let mut r = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => f(String::from_utf8_lossy(&buf).into_owned()),
        }
    }
}
