//! Recur-owned background processes. The workspace owns them, not a chat,
//! a model or a terminal tab: `run_background` starts one in a terminal
//! session, `stop_terminal` ends it, and when one exits on its own the chat
//! that started it gets a hidden notice turn with the exit code and the last
//! lines of output (exits within [`EXIT_COALESCE`] share one turn).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_agent_tools::{BackgroundRequest, HostError};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::terminals::{SpawnOptions, TerminalSession};
use crate::text::truncate;

/// The display label every process notice starts with (the transcript's
/// "Process" row).
pub const PROC_NOTICE_PREFIX: &str = "Background process";
pub const EXIT_COALESCE: Duration = Duration::from_secs(2);
const NOTICE_TAIL_LINES: i64 = 40;
const NOTICE_TITLE_MAX: usize = 60;

/// Exit notices waiting out the coalescing window, processes started with
/// `notify_on_exit: false`, and each terminal's output mark as of the agent's
/// last read (read_terminal's `until` only matches output past it).
#[derive(Default)]
pub(crate) struct Procs {
    quiet: HashSet<String>,
    pending: HashMap<(String, String), Vec<ProcExit>>,
    read_marks: HashMap<String, u64>,
}

/// What run_background leaves beside the process log until the process ends
/// while Recur runs. One still on disk for a process that is no longer live
/// was stopped by Recur quitting (`stopped_at` set) or by a crash, and the
/// starting chat is told on its next turn.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ProcRecord {
    pub id: String,
    pub title: String,
    pub command: String,
    pub started_by: String,
    pub stopped_at: i64,
}

/// How long run_background waits for a process's first output.
const START_OUTPUT_WAIT: Duration = Duration::from_secs(3);
const START_OUTPUT_POLL: Duration = Duration::from_millis(250);

struct ProcExit {
    id: String,
    title: String,
    code: i32,
    tail: String,
}

fn host_err(e: impl std::fmt::Display) -> HostError {
    HostError(e.to_string())
}

impl Engine {
    fn procs_dir(&self, workspace_id: &str) -> PathBuf {
        self.data_dir
            .join("workspaces")
            .join(workspace_id)
            .join("procs")
    }

    fn proc_record_path(&self, workspace_id: &str, id: &str) -> PathBuf {
        self.procs_dir(workspace_id).join(format!("{id}.json"))
    }

    fn write_proc_record(&self, workspace_id: &str, r: &ProcRecord) {
        let path = self.proc_record_path(workspace_id, &r.id);
        let res = std::fs::create_dir_all(self.procs_dir(workspace_id))
            .and_then(|()| std::fs::write(&path, serde_json::to_vec(r).unwrap_or_default()));
        if let Err(e) = res {
            tracing::warn!("background process record {}: {e}", path.display());
        }
    }

    /// For the next turn of `chat`: its background processes that Recur's last
    /// quit (or a crash) stopped. Each is reported once, then forgotten.
    pub(crate) fn stopped_procs_prompt(&self, workspace_id: &str, chat: &str) -> String {
        let Ok(dir) = std::fs::read_dir(self.procs_dir(workspace_id)) else {
            return String::new();
        };
        let live: HashSet<String> = self
            .terminals
            .as_ref()
            .map(|t| t.list(workspace_id).into_iter().map(|s| s.id).collect())
            .unwrap_or_default();
        let mut lines = Vec::new();
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Ok(r) = std::fs::read(&path)
                .map_err(|_| ())
                .and_then(|b| serde_json::from_slice::<ProcRecord>(&b).map_err(|_| ()))
            else {
                continue;
            };
            if r.started_by != chat || live.contains(&r.id) {
                continue;
            }
            let _ = std::fs::remove_file(&path);
            let when = chrono::DateTime::from_timestamp_millis(r.stopped_at)
                .filter(|_| r.stopped_at > 0)
                .map(|t| {
                    format!(
                        " when Recur quit at {}",
                        t.with_timezone(&chrono::Local).format("%-I:%M %p")
                    )
                })
                .unwrap_or_else(|| " when Recur last closed".into());
            let name = if r.title.is_empty() {
                &r.command
            } else {
                &r.title
            };
            lines.push(format!(
                "- {name} (`{}`, terminal {}): stopped{when}",
                r.command, r.id
            ));
        }
        if lines.is_empty() {
            return String::new();
        }
        lines.sort();
        format!(
            "Background processes you started with mcp__recur__run_background are no longer running because Recur quit since then (they stop when the app quits; closing a terminal tab never stops them, and your earlier reports about them were accurate at the time):\n{}\nStart them again with mcp__recur__run_background if they are still needed.",
            lines.join("\n")
        )
    }

    /// The terminal's output mark as of the agent's last read; 0 before any.
    pub(crate) fn terminal_read_mark(&self, id: &str) -> u64 {
        self.procs.lock().read_marks.get(id).copied().unwrap_or(0)
    }

    pub(crate) fn set_terminal_read_mark(&self, id: &str, mark: u64) {
        self.procs.lock().read_marks.insert(id.to_owned(), mark);
    }

    /// Host `run_background`. Bypass runs, ask goes to the human, deny refuses.
    pub async fn run_background_from_chat(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        chat: &str,
        req: BackgroundRequest,
    ) -> Result<String, HostError> {
        let src = self
            .terminals
            .as_ref()
            .ok_or_else(|| HostError::new("no Observatory terminals in this session"))?;
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| HostError::new("workspace not found"))?;
        if ws.worktree_path.is_empty() {
            return Err(HostError::new("this workspace has no worktree"));
        }
        let cwd = Path::new(&ws.worktree_path).join(req.cwd.trim());
        if !cwd.is_dir() {
            return Err(HostError(format!("cwd {:?} is not a directory", req.cwd)));
        }
        self.machine_permission(
            cancel,
            workspace_id,
            chat,
            recur_agent_tools::names::mcp::RUN_BACKGROUND,
            serde_json::json!({"command": req.command, "cwd": req.cwd}),
            "background processes",
        )
        .await?;
        let proj = self.store.project(&ws.project_id).unwrap_or_default();
        let env = recur_project::wsenv::Vars::from_workspace(&ws, &proj).environ(None);
        let info = src
            .spawn(
                workspace_id,
                &cwd.to_string_lossy(),
                &env,
                SpawnOptions {
                    title: req.title.trim().into(),
                    command: req.command.clone(),
                    background: true,
                    started_by: chat.into(),
                    log_dir: Some(self.procs_dir(workspace_id)),
                    ..SpawnOptions::default()
                },
            )
            .map_err(HostError)?;
        self.write_proc_record(
            workspace_id,
            &ProcRecord {
                id: info.id.clone(),
                title: info.title.clone(),
                command: req.command.clone(),
                started_by: chat.into(),
                stopped_at: 0,
            },
        );
        if !req.notify_on_exit {
            self.procs.lock().quiet.insert(info.id.clone());
        }
        let mut now = info.clone();
        let deadline = tokio::time::Instant::now() + START_OUTPUT_WAIT;
        while !now.exited && tokio::time::Instant::now() < deadline {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(START_OUTPUT_POLL) => {}
            }
            match src.list(workspace_id).into_iter().find(|s| s.id == info.id) {
                Some(s) => now = s,
                None => break,
            }
        }
        let output = src.read_plain(&info.id, 0).unwrap_or_default();
        self.set_terminal_read_mark(&info.id, src.output_written(&info.id));
        let mut out = serde_json::json!({
            "id": info.id,
            "title": info.title,
            "cwd": info.cwd,
            "notifyOnExit": req.notify_on_exit,
            "output": output.trim_end(),
        });
        if now.exited {
            // The result already reports the exit: no notice turn follows.
            self.procs.lock().quiet.insert(info.id.clone());
            out["exited"] = true.into();
            out["exitCode"] = now.exit_code.unwrap_or_default().into();
        }
        Ok(out.to_string())
    }

    /// Host `stop_terminal`: only background processes, never a user's tab.
    pub fn stop_terminal_from_chat(
        &self,
        workspace_id: &str,
        id: &str,
    ) -> Result<String, HostError> {
        let src = self
            .terminals
            .as_ref()
            .ok_or_else(|| HostError::new("no Observatory terminals in this session"))?;
        let Some(info) = src.list(workspace_id).into_iter().find(|s| s.id == id) else {
            return Err(HostError(format!(
                "terminal {id:?} not found: call list_terminals"
            )));
        };
        if !info.background {
            return Err(HostError(format!(
                "terminal {id} is a tab the user opened: only run_background processes can be stopped"
            )));
        }
        src.stop(id).map_err(host_err)?;
        let mut p = self.procs.lock();
        p.quiet.remove(id);
        p.read_marks.remove(id);
        drop(p);
        Ok(match info.exit_code {
            Some(code) if info.exited => {
                format!("background process {id} had already exited ({code}); closed its tab")
            }
            _ => format!("stopped background process {id} ({})", info.title),
        })
    }

    /// The terminal exit hook: queues the notice for the chat that started a
    /// background process that exited on its own. Runs on the PTY reader
    /// thread, so the send is spawned on `rt`.
    pub(crate) fn on_terminal_exit(
        &self,
        info: &TerminalSession,
        stopped: bool,
        rt: &tokio::runtime::Handle,
    ) {
        if info.background {
            let path = self.proc_record_path(&info.workspace_id, &info.id);
            if self.is_shutting_down() {
                if let Some(mut r) = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<ProcRecord>(&b).ok())
                {
                    r.stopped_at = self.now().timestamp_millis();
                    self.write_proc_record(&info.workspace_id, &r);
                }
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        if !info.background || stopped || info.started_by.is_empty() {
            self.procs.lock().quiet.remove(&info.id);
            return;
        }
        let tail = self
            .terminals
            .as_ref()
            .and_then(|t| t.read_plain(&info.id, NOTICE_TAIL_LINES).ok())
            .unwrap_or_default();
        let key = (info.workspace_id.clone(), info.started_by.clone());
        let first = {
            let mut p = self.procs.lock();
            let q = p.pending.entry(key.clone()).or_default();
            q.push(ProcExit {
                id: info.id.clone(),
                title: info.title.clone(),
                code: info.exit_code.unwrap_or(-1),
                tail,
            });
            q.len() == 1
        };
        if first {
            let e = self.arc();
            rt.spawn(async move {
                tokio::time::sleep(EXIT_COALESCE).await;
                e.flush_proc_exits(&key.0, &key.1).await;
            });
        }
    }

    async fn flush_proc_exits(&self, workspace_id: &str, chat: &str) {
        let exits: Vec<ProcExit> = {
            let mut p = self.procs.lock();
            let q = p
                .pending
                .remove(&(workspace_id.to_owned(), chat.to_owned()))
                .unwrap_or_default();
            q.into_iter().filter(|x| !p.quiet.remove(&x.id)).collect()
        };
        if exits.is_empty() || self.store.agent(workspace_id, chat).is_none() {
            return;
        }
        self.send_hidden_notice(
            workspace_id,
            chat,
            &proc_exit_notice(&exits),
            &proc_exit_label(&exits),
        )
        .await;
    }

    /// Sends an engine-to-agent notice as its own turn on a chat tab: hidden
    /// input, `label` as the transcript row, queued behind a live turn rather
    /// than steered into it. Returns the reply.
    pub(crate) async fn send_hidden_notice(
        &self,
        workspace_id: &str,
        chat: &str,
        text: &str,
        label: &str,
    ) -> String {
        let (model, flags, mode) = self.workspace_chat_selection(workspace_id, chat);
        self.send_workspace_message(
            workspace_id,
            chat,
            text,
            &model,
            Some(&flags),
            &[],
            &mode,
            &format!("{DISPLAY_HIDDEN_SENTINEL}{label}"),
        )
        .await
    }
}

fn proc_exit_notice(exits: &[ProcExit]) -> String {
    let mut b = String::new();
    for x in exits {
        b.push_str(&format!(
            "{PROC_NOTICE_PREFIX} {} ({:?}) exited with code {}.\n",
            x.id, x.title, x.code
        ));
        if x.tail.trim().is_empty() {
            b.push_str("It printed nothing.\n\n");
        } else {
            b.push_str(&format!("Last lines:\n```\n{}\n```\n\n", x.tail.trim_end()));
        }
    }
    b.push_str("read_terminal reads more of its output; run_background starts it again.");
    b
}

fn proc_exit_label(exits: &[ProcExit]) -> String {
    let each = |x: &ProcExit| format!("{} ({})", truncate(&x.title, NOTICE_TITLE_MAX), x.code);
    match exits {
        [x] => format!(
            "{PROC_NOTICE_PREFIX} {} exited ({})",
            truncate(&x.title, NOTICE_TITLE_MAX),
            x.code
        ),
        _ => format!(
            "{PROC_NOTICE_PREFIX}es exited: {}",
            exits.iter().map(each).collect::<Vec<_>>().join(", ")
        ),
    }
}
