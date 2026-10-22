//! Source Control (commit staged with a generated or typed message, stage / unstage,
//! discard) and the Search panel (search and replace across the worktree).

use gpui_kit::{Context, Task, Window};
use recur_domain::SearchOpts;
use recur_ui::widgets::ToastKind;
use recur_view_core::{call, toast};
use recur_view_review::review_diff::{FileListMode, write_file_list_mode};

use crate::obs::{Observatory, OpenRequest};

impl Observatory {
    /// `stagedPaths` / `unstagedPaths` of the working status, sorted.
    pub(crate) fn working_paths(&self) -> (Vec<String>, Vec<String>) {
        let mut all: Vec<String> = self.working_status.keys().cloned().collect();
        all.sort_by(|a, b| recur_code::collate::locale_compare(a, b));
        all.into_iter()
            .partition(|p| self.staged_files.iter().any(|s| s == p))
    }

    pub(crate) fn set_scm_file_list_mode(&mut self, next: FileListMode, cx: &mut Context<Self>) {
        self.scm_file_list_mode = next;
        self.state
            .update(cx, |s, _| write_file_list_mode(s.storage_mut(), next));
        cx.notify();
    }

    pub(crate) fn toggle_scm_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.scm_collapsed.remove(path) {
            self.scm_collapsed.insert(path.to_string());
        }
        cx.notify();
    }

    /// `humanCommit` (⌘↵ in the message, the Commit button).
    pub(crate) fn human_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_git(cx) || !self.tree_writable() || self.commit_busy {
            return;
        }
        let msg = self.inputs.commit_msg(cx).trim().to_string();
        if msg.is_empty() {
            toast(ToastKind::Info, "Enter a commit message", cx);
            return;
        }
        if self.staged_files.is_empty() {
            toast(ToastKind::Info, "Stage changes to commit", cx);
            return;
        }
        self.commit_busy = true;
        cx.notify();
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let res = call(
                cx,
                svc.code.commit_staged(&pid, &wid, &msg),
                "Commit failed",
            )
            .await;
            let refresh = this
                .update_in(cx, |o, window, cx| {
                    let res = res?;
                    if !res.committed {
                        toast(ToastKind::Info, "Nothing to commit", cx);
                        return None;
                    }
                    o.inputs.set_commit_msg("", window, cx);
                    toast(
                        ToastKind::Success,
                        format!(
                            "Committed {} (+{}/−{})",
                            res.hash, res.additions, res.deletions
                        ),
                        cx,
                    );
                    Some(o.soft_refresh(vec![String::new()], cx))
                })
                .ok()
                .flatten();
            if let Some(t) = refresh {
                t.await;
            }
            let _ = this.update(cx, |o, cx| {
                o.commit_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// `generateCommitMessage` from the staged diff.
    pub(crate) fn generate_commit_msg(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_git(cx) || !self.tree_writable() || self.commit_msg_busy {
            return;
        }
        if self.staged_files.is_empty() {
            toast(
                ToastKind::Info,
                "Stage changes to generate a commit message",
                cx,
            );
            return;
        }
        self.commit_msg_busy = true;
        cx.notify();
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let msg = call(
                cx,
                svc.code.generate_commit_message(&pid, &wid),
                "Generate commit message failed",
            )
            .await;
            let _ = this.update_in(cx, |o, window, cx| {
                o.commit_msg_busy = false;
                if let Some(m) = msg {
                    let t = m.trim();
                    if t.is_empty() {
                        toast(ToastKind::Info, "No commit message generated", cx);
                    } else {
                        o.inputs.set_commit_msg(t, window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `discardPaths`: confirm, then restore each path to HEAD.
    pub(crate) fn discard_paths(
        &mut self,
        paths: Vec<String>,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_git(cx) || !self.tree_writable() || paths.is_empty() {
            return;
        }
        let msg = if paths.len() == 1 {
            format!("Discard uncommitted changes to “{}”?", paths[0])
        } else {
            format!(
                "Discard uncommitted changes to {} files in “{label}”?",
                paths.len()
            )
        };
        let confirm = self.ask_confirm("Discard changes", &msg, "Discard", window, cx);
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return;
            }
            for p in &paths {
                if call(cx, svc.code.discard_file(&pid, &wid, p), "Discard failed")
                    .await
                    .is_none()
                {
                    return;
                }
            }
            let refresh = this
                .update(cx, |o, cx| {
                    let text = if paths.len() == 1 {
                        format!("Discarded {}", paths[0])
                    } else {
                        format!("Discarded {} files", paths.len())
                    };
                    toast(ToastKind::Success, text, cx);
                    o.soft_refresh(vec![String::new()], cx)
                })
                .ok();
            if let Some(t) = refresh {
                t.await;
            }
            let _ = this.update_in(cx, |o, window, cx| {
                if paths.contains(&o.selected_path) {
                    let p = o.selected_path.clone();
                    o.open_path(OpenRequest::new(p).from_disk(true), window, cx)
                        .detach();
                }
            });
        })
        .detach();
    }

    /// `discardAllWorking`.
    pub(crate) fn discard_all_working(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.working_dirty_count();
        if !self.is_git(cx) || !self.tree_writable() || n == 0 {
            return;
        }
        let confirm = self.ask_confirm(
            "Discard all",
            &format!("Discard all {n} uncommitted change(s) in this worktree?"),
            "Discard all",
            window,
            cx,
        );
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return;
            }
            if call(cx, svc.code.discard_all(&pid, &wid), "Discard all failed")
                .await
                .is_none()
            {
                return;
            }
            let refresh = this
                .update(cx, |o, cx| {
                    toast(ToastKind::Success, "Discarded all uncommitted changes", cx);
                    o.soft_refresh(vec![String::new()], cx)
                })
                .ok();
            if let Some(t) = refresh {
                t.await;
            }
            let _ = this.update_in(cx, |o, window, cx| {
                if !o.selected_path.is_empty() {
                    let p = o.selected_path.clone();
                    o.open_path(OpenRequest::new(p).from_disk(true), window, cx)
                        .detach();
                }
            });
        })
        .detach();
    }

    /// `stagePaths` / `unstagePaths`.
    pub(crate) fn stage_paths(&mut self, paths: Vec<String>, stage: bool, cx: &mut Context<Self>) {
        if !self.is_git(cx) || !self.tree_writable() || paths.is_empty() {
            return;
        }
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let ok = if stage {
                call(cx, svc.code.stage(&pid, &wid, &paths), "Stage failed").await
            } else {
                call(cx, svc.code.unstage(&pid, &wid, &paths), "Unstage failed").await
            };
            if ok.is_none() {
                return;
            }
            if let Ok(t) = this.update(cx, |o, cx| o.refresh_working_status(cx)) {
                t.await;
            }
        })
        .detach();
    }

    pub(crate) fn stage_all(&mut self, cx: &mut Context<Self>) {
        let (_, unstaged) = self.working_paths();
        self.stage_paths(unstaged, true, cx);
    }

    pub(crate) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        let (staged, _) = self.working_paths();
        self.stage_paths(staged, false, cx);
    }

    // ---- search ----

    fn search_opts(&self, offset: i64, cx: &gpui_kit::App) -> SearchOpts {
        SearchOpts {
            regex: self.search_regex,
            case_sensitive: self.search_case,
            glob: self.inputs.glob(cx).trim().to_string(),
            limit: 200,
            offset,
        }
    }

    /// `runSearch`: `reset` starts over, else loads the next page.
    pub(crate) fn run_search(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.inputs.query(cx).trim().to_string();
        if q.is_empty() {
            return;
        }
        self.select_code_panel(recur_view_core::prefs::CodePanelKey::Search, window, cx);
        if reset {
            self.search_offset = 0;
        }
        let opts = self.search_opts(self.search_offset, cx);
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let res = call(
                cx,
                svc.code.search_advanced(&pid, &wid, &q, &opts),
                "Search failed",
            )
            .await;
            let Some(res) = res else { return };
            let _ = this.update(cx, |o, cx| {
                let page = res.hits.unwrap_or_default();
                if reset {
                    o.hits = page.clone();
                } else {
                    o.hits.extend(page.iter().cloned());
                }
                if !page.is_empty() {
                    o.search_offset = o.hits.len() as i64;
                }
                o.search_skipped_large = if reset {
                    res.skipped_large
                } else {
                    o.search_skipped_large + res.skipped_large
                };
                o.search_truncated = res.truncated;
                cx.notify();
            });
        })
        .detach();
    }

    /// `runReplace`: confirm, replace across the worktree, then search again.
    pub(crate) fn run_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.inputs.query(cx).trim().to_string();
        if q.is_empty() || !self.tree_writable() {
            return;
        }
        let confirm = self.ask_confirm(
            "Replace in files",
            &format!(
                "Replace all matches of “{q}” across the worktree? This cannot be undone (except via Discard/git)."
            ),
            "Replace all",
            window,
            cx,
        );
        let repl = self.inputs.replace(cx);
        let opts = self.search_opts(0, cx);
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let Some(res) = call(
                cx,
                svc.code.replace_in_files(&pid, &wid, &q, &repl, &opts),
                "Replace failed",
            )
            .await
            else {
                return;
            };
            let refresh: Option<Task<()>> = this
                .update(cx, |o, cx| {
                    toast(
                        ToastKind::Success,
                        format!(
                            "Replaced {} in {} file(s)",
                            res.replacements, res.files_changed
                        ),
                        cx,
                    );
                    o.soft_refresh(vec![String::new()], cx)
                })
                .ok();
            if let Some(t) = refresh {
                t.await;
            }
            let _ = this.update_in(cx, |o, window, cx| {
                o.run_search(true, window, cx);
                if !o.selected_path.is_empty() {
                    let p = o.selected_path.clone();
                    let line = o.highlight_line;
                    o.open_path(OpenRequest::new(p).line(line).from_disk(true), window, cx)
                        .detach();
                }
            });
        })
        .detach();
    }
}
