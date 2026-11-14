//! The chat tab strip: list, create, close, reopen, delete, select, rotate, reorder, fork,
//! turn diff and reset, plus the active-tab memory in local storage.

use std::time::Duration;

use gpui_kit::{App, Context, Entity, PromptLevel, SharedString, Window};
use recur_domain::WorkspaceChatTab;
use recur_engine::view::chat_tab_session::{ACTIVE_CHAT_TAB_KEY, ActiveChatTabs};
use recur_ui::widgets::ToastKind;
use recur_view_chat::heap::ChatHeapCache;
use recur_view_chat::strip::{ChatStripCache, ChatStripSnapshot};
use recur_view_chat::transcript_ui::TranscriptUiCache;
use recur_view_core::{AppState, call, toast};

use super::{DELETE_ARM_MS, WorkspaceView};
use crate::tab_order::apply_chat_tab_order;

/// `restoredChatCode`: the saved tab when it is still open, else the first, else `PA`.
pub(crate) fn restored_code(
    state: &Entity<AppState>,
    ws_id: &str,
    tabs: &[WorkspaceChatTab],
    cx: &App,
) -> String {
    let saved = ActiveChatTabs::parse(state.read(cx).storage().get(ACTIVE_CHAT_TAB_KEY));
    let codes: Vec<&str> = tabs.iter().map(|t| t.code.as_str()).collect();
    saved.restored_chat_code(ws_id, &codes)
}

/// `saveActiveChatCode`.
pub(crate) fn save_active_code(state: &Entity<AppState>, ws_id: &str, code: &str, cx: &mut App) {
    state.update(cx, |s, _| {
        let mut all = ActiveChatTabs::parse(s.storage().get(ACTIVE_CHAT_TAB_KEY));
        if !all.save(ws_id, code) {
            return;
        }
        match all.encode() {
            Some(v) => s.storage_mut().set(ACTIVE_CHAT_TAB_KEY, v),
            None => s.storage_mut().remove(ACTIVE_CHAT_TAB_KEY),
        }
    });
}

fn new_chat_tab_stub() -> WorkspaceChatTab {
    WorkspaceChatTab {
        code: "PA".into(),
        title: "New chat".into(),
        ..Default::default()
    }
}

impl WorkspaceView {
    pub(crate) fn set_tabs(&mut self, list: Vec<WorkspaceChatTab>, cx: &mut Context<Self>) {
        self.tabs = Some(list);
        self.put_strip(cx);
        self.touch(cx);
    }

    /// `putChatStrip` whenever the strip changes.
    pub(crate) fn put_strip(&mut self, cx: &mut Context<Self>) {
        let Some(tabs) = self.tabs.clone().filter(|t| !t.is_empty()) else {
            return;
        };
        let prev = ChatStripCache::global(cx).get(&self.ws_id);
        let snap = ChatStripSnapshot {
            tabs,
            closed: self.closed_tabs.clone(),
            active_code: self.active.clone().unwrap_or_default(),
            tab_scroll_left: prev.and_then(|p| p.tab_scroll_left),
        };
        ChatStripCache::global(cx).put(&self.ws_id, snap);
    }

    /// The strip's scroll offset into the cache (`captureStripScrollLeft` on scroll); parked
    /// shells keep their last live value.
    pub(crate) fn save_strip_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.shell_active {
            return;
        }
        let left = -f32::from(self.strip_scroll.offset().x);
        let Some(prev) = ChatStripCache::global(cx).get(&self.ws_id) else {
            return;
        };
        if prev.tab_scroll_left == Some(left) {
            return;
        }
        ChatStripCache::global(cx).put(
            &self.ws_id,
            ChatStripSnapshot {
                tab_scroll_left: Some(left),
                ..prev
            },
        );
    }

    pub(crate) fn set_active(&mut self, code: &str, cx: &mut Context<Self>) {
        self.active = Some(code.to_string());
        self.put_strip(cx);
        self.touch(cx);
    }

    /// `ListWorkspaceChats` on mount and after a workspace scan settles; an empty or failed
    /// list falls back to one fresh PA tab.
    pub(crate) fn load_tabs(&mut self, cx: &mut Context<Self>) {
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let res = svc.workspaces.list_workspace_chats(&id).await;
            let _ = this.update(cx, |v, cx| {
                let list = match res {
                    Ok(tabs) if !tabs.is_empty() => tabs,
                    Ok(_) => vec![new_chat_tab_stub()],
                    Err(_) if v.tabs.is_some() => return,
                    Err(_) => vec![new_chat_tab_stub()],
                };
                if v.active.as_deref().is_none_or(str::is_empty) {
                    v.active = Some(restored_code(&v.state, &v.ws_id, &list, cx));
                }
                v.set_tabs(list, cx);
            });
        })
        .detach();
    }

    pub(crate) fn refresh_closed_tabs(&mut self, cx: &mut Context<Self>) {
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let list = svc
                .workspaces
                .list_closed_workspace_chats(&id)
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                v.closed_tabs = list;
                if v.closed_tabs.is_empty() {
                    v.recents_open = false;
                }
                v.put_strip(cx);
                v.touch(cx);
            });
        })
        .detach();
    }

    /// Refetch the strip and resync the active tab's caches (`resyncActivatedChats`).
    pub(crate) fn refetch_tabs(&mut self, cx: &mut Context<Self>) {
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let Ok(tabs) = svc.workspaces.list_workspace_chats(&id).await else {
                return;
            };
            if tabs.is_empty() {
                return;
            }
            let _ = this.update(cx, |v, cx| {
                let codes: Vec<String> = tabs.iter().map(|t| t.code.clone()).collect();
                v.set_tabs(tabs, cx);
                v.resync_activated_chats(&codes, None, &[], cx);
            });
        })
        .detach();
    }

    /// Park keyboard focus in the agent composer after a tab switch, new or close.
    pub(crate) fn focus_chat_input(&mut self, cx: &mut Context<Self>) {
        self.focus_composer_pending = true;
        cx.notify();
    }

    /// Opens a coding chat tab; its code, or `None` when creation failed.
    pub(crate) fn new_chat_tab(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Task<Option<String>> {
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let tab = svc.workspaces.create_workspace_chat(&id).await.ok()?;
            if tab.code.is_empty() {
                return None;
            }
            let list = svc.workspaces.list_workspace_chats(&id).await.ok();
            this.update(cx, |v, cx| {
                let list = match list {
                    Some(l) if !l.is_empty() => l,
                    _ => {
                        let mut l = v.tabs().to_vec();
                        l.push(tab.clone());
                        l
                    }
                };
                v.tabs = Some(list);
                v.set_active(&tab.code, cx);
                v.focus_chat_input(cx);
            })
            .ok()?;
            Some(tab.code)
        })
    }

    /// Create a sibling workspace with this chat copied through the fork point.
    pub(crate) fn fork_chat_to_workspace(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let (id, code) = (self.ws_id.clone(), self.active_code());
        if message_id.is_empty() || self.forking {
            return;
        }
        self.forking = true;
        let (svc, mid) = (self.services.clone(), message_id.to_string());
        cx.spawn(async move |this, cx| {
            let dest = call(
                cx,
                svc.workspaces
                    .fork_workspace_chat_to_workspace(&id, &code, &mid),
                "Could not fork to a new workspace",
            )
            .await;
            let _ = this.update(cx, |v, cx| {
                v.forking = false;
                if let Some(dest) = dest.filter(|d| !d.id.is_empty()) {
                    let dest_id = dest.id.clone();
                    v.state.update(cx, |s, cx| {
                        s.upsert_workspace(dest, cx);
                        s.open_workspace(&dest_id, false, cx);
                    });
                }
            });
        })
        .detach();
    }

    /// Branch the open chat at a finished turn and switch to the new tab.
    pub(crate) fn fork_chat_tab(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let (id, code) = (self.ws_id.clone(), self.active_code());
        if message_id.is_empty() || self.forking {
            return;
        }
        self.forking = true;
        let (svc, mid) = (self.services.clone(), message_id.to_string());
        cx.spawn(async move |this, cx| {
            let tab = svc
                .workspaces
                .fork_workspace_chat(&id, &code, &mid)
                .await
                .ok()
                .filter(|t| !t.code.is_empty());
            let Some(tab) = tab else {
                let _ = this.update(cx, |v, cx| {
                    v.forking = false;
                    toast(ToastKind::Error, "Could not fork this chat", cx);
                });
                return;
            };
            let list = svc.workspaces.list_workspace_chats(&id).await.ok();
            let _ = this.update(cx, |v, cx| {
                v.forking = false;
                let list = match list {
                    Some(l) if !l.is_empty() => l,
                    _ => {
                        let mut l = v.tabs().to_vec();
                        l.push(tab.clone());
                        l
                    }
                };
                v.tabs = Some(list);
                v.set_active(&tab.code, cx);
                v.focus_chat_input(cx);
            });
        })
        .detach();
    }

    /// Show what the agent changed between a turn's checkpoints in the review surface.
    pub(crate) fn show_turn_diff(
        &mut self,
        message_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, code) = (self.ws_id.clone(), self.active_code());
        if message_id.is_empty() {
            return;
        }
        let (svc, mid) = (self.services.clone(), message_id.to_string());
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let turn = call(
                cx,
                svc.workspaces.workspace_chat_turn_diff(&id, &code, &mid),
                "Could not load this turn's changes",
            )
            .await;
            let Some(turn) = turn else { return };
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |v, cx| {
                    // An empty scope would fall back to `git show <hash>`.
                    if turn.files.as_ref().is_none_or(Vec::is_empty) {
                        toast(
                            ToastKind::Info,
                            "The agent didn't change any files during this turn",
                            cx,
                        );
                        return;
                    }
                    v.observatory
                        .update(cx, |o, cx| o.open_turn_review(turn, window, cx));
                });
            });
        })
        .detach();
    }

    /// Drop messages after a turn and, when later turns edited files, restore the worktree
    /// to that turn's end checkpoint. Confirms with a native dialog.
    pub(crate) fn reset_chat_to(
        &mut self,
        message_id: &str,
        restores_files: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (id, code) = (self.ws_id.clone(), self.active_code());
        if message_id.is_empty()
            || self
                .sending_by_key
                .get(&self.active_key())
                .copied()
                .unwrap_or(false)
        {
            return;
        }
        let detail = if restores_files {
            "Files in the worktree go back to how they were at the end of this turn, and every later message in this chat is removed. This cannot be undone."
        } else {
            "Every later message in this chat is removed. Files in the worktree are not changed. This cannot be undone."
        };
        let answer = window.prompt(
            PromptLevel::Info,
            "Reset to this point?",
            Some(detail),
            &["Cancel", "Reset"],
            cx,
        );
        let (svc, mid) = (self.services.clone(), message_id.to_string());
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(1) {
                return;
            }
            let ok = call(
                cx,
                svc.workspaces.reset_workspace_chat(&id, &code, &mid),
                "Could not reset this chat",
            )
            .await
            .is_some();
            if !ok {
                return;
            }
            let _ = this.update(cx, |v, cx| {
                v.resync_chat(&id, &code, cx);
                toast(ToastKind::Success, "Reset to this point", cx);
            });
        })
        .detach();
    }

    /// Close a coding chat tab: the neighbour that slid into its slot takes over.
    pub(crate) fn close_chat_tab(&mut self, code: &str, cx: &mut Context<Self>) {
        let id = self.ws_id.clone();
        let idx = self.tabs().iter().position(|t| t.code == code);
        let was_active = self.active.as_deref() == Some(code);
        let (svc, code) = (self.services.clone(), code.to_string());
        cx.spawn(async move |this, cx| {
            let Ok(list) = svc.workspaces.close_workspace_chat(&id, &code).await else {
                return;
            };
            if list.is_empty() {
                return;
            }
            let _ = this.update(cx, |v, cx| {
                if was_active {
                    let i = idx.unwrap_or(0).min(list.len() - 1);
                    v.active = Some(list[i].code.clone());
                } else if !list
                    .iter()
                    .any(|t| Some(t.code.as_str()) == v.active.as_deref())
                {
                    v.active = Some(list[0].code.clone());
                }
                v.set_tabs(list, cx);
                let key = v.key(&code);
                TranscriptUiCache::global(cx).drop_key(&key);
                v.chat_by_key.remove(&key);
                v.usage_by_key.remove(&key);
                if was_active {
                    v.focus_chat_input(cx);
                }
                v.refresh_closed_tabs(cx);
            });
        })
        .detach();
    }

    pub(crate) fn close_active_chat_tab(&mut self, cx: &mut Context<Self>) {
        let code = self.active_code();
        if !code.is_empty() {
            self.close_chat_tab(&code, cx);
        }
    }

    pub(crate) fn reopen_closed_chat(&mut self, code: &str, cx: &mut Context<Self>) {
        self.recents_open = false;
        self.touch(cx);
        let (svc, id, code) = (self.services.clone(), self.ws_id.clone(), code.to_string());
        cx.spawn(async move |this, cx| {
            let Ok(list) = svc.workspaces.reopen_workspace_chat(&id, &code).await else {
                return;
            };
            if list.is_empty() {
                return;
            }
            let closed = svc
                .workspaces
                .list_closed_workspace_chats(&id)
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                v.closed_tabs = closed;
                v.set_tabs(list, cx);
                v.select_chat_tab(&code, cx);
            });
        })
        .detach();
    }

    /// Every client-side trace of a permanently deleted chat.
    pub(crate) fn drop_chat_client_state(&mut self, key: &str, cx: &mut Context<Self>) {
        TranscriptUiCache::global(cx).drop_key(key);
        self.chat_by_key.remove(key);
        self.usage_by_key.remove(key);
        self.input_by_key.remove(key);
        self.attachments_by_key.remove(key);
        self.sending_by_key.remove(key);
        self.send_gen_by_key.remove(key);
        self.hoist_send_gen_by_key.remove(key);
        self.pending_by_key.remove(key);
        self.hold_pending_by_key.remove(key);
        self.turn_started_at_by_key.remove(key);
        self.stale_idle_armed_by_key.remove(key);
        self.turn_got_text_by_key.remove(key);
        self.stream_seq_by_key
            .retain(|k, _| !(k == key || k.starts_with(&format!("{key}:"))));
        self.pickers_by_key.remove(key);
        self.simplify_by_key.remove(key);
        self.pickers_fetched.remove(key);
        self.simplify_fetched.remove(key);
        self.pickers_loading.remove(key);
        *self.picker_gen.entry(key.to_string()).or_default() += 1;
        *self.simplify_gen.entry(key.to_string()).or_default() += 1;
        *self.transcript_gen.entry(key.to_string()).or_default() += 1;
    }

    pub(crate) fn clear_armed_delete(&mut self) {
        self.delete_arm = None;
        self.armed_delete = None;
    }

    /// The recents row's trash: arm on the first click, delete on the second within 3s.
    pub(crate) fn on_delete_closed_chat(&mut self, code: &str, cx: &mut Context<Self>) {
        if self.armed_delete.as_deref() != Some(code) {
            self.armed_delete = Some(code.to_string());
            let armed = code.to_string();
            self.delete_arm = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(DELETE_ARM_MS))
                    .await;
                let _ = this.update(cx, |v, cx| {
                    v.delete_arm = None;
                    if v.armed_delete.as_deref() == Some(armed.as_str()) {
                        v.armed_delete = None;
                    }
                    v.touch(cx);
                });
            }));
            self.touch(cx);
            return;
        }
        self.clear_armed_delete();
        self.delete_closed_chat(code, cx);
    }

    fn delete_closed_chat(&mut self, code: &str, cx: &mut Context<Self>) {
        if self.tabs().iter().any(|t| t.code == code) {
            self.refresh_closed_tabs(cx);
            return;
        }
        let prev = self.closed_tabs.clone();
        self.closed_tabs.retain(|t| t.code != code);
        let menu_was_open = self.recents_open;
        if self.closed_tabs.is_empty() {
            self.recents_open = false;
        }
        self.touch(cx);
        let (svc, id, code) = (self.services.clone(), self.ws_id.clone(), code.to_string());
        cx.spawn(async move |this, cx| {
            let res = svc
                .workspaces
                .delete_closed_workspace_chat(&id, &code)
                .await;
            let _ = this.update(cx, |v, cx| {
                let Ok(list) = res else {
                    v.closed_tabs = prev;
                    v.recents_open = menu_was_open;
                    v.touch(cx);
                    toast(ToastKind::Error, "Could not delete this chat", cx);
                    return;
                };
                v.closed_tabs = list;
                if v.closed_tabs.is_empty() {
                    v.recents_open = false;
                }
                let key = v.key(&code);
                v.drop_chat_client_state(&key, cx);
                ChatHeapCache::global(cx).drop_key(&id, &key);
                v.put_strip(cx);
                v.touch(cx);
            });
        })
        .detach();
    }

    pub(crate) fn toggle_recents(&mut self, cx: &mut Context<Self>) {
        self.recents_open = !self.recents_open;
        if !self.recents_open {
            self.clear_armed_delete();
        }
        self.touch(cx);
    }

    pub(crate) fn close_recents(&mut self, cx: &mut Context<Self>) {
        if self.recents_open {
            self.recents_open = false;
            self.clear_armed_delete();
            self.touch(cx);
        }
    }

    /// Select a chat tab: focus it on the engine, clear its unseen chip, focus the composer.
    pub fn select_chat_tab(&mut self, code: &str, cx: &mut Context<Self>) {
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        let c = code.to_string();
        cx.background_executor()
            .spawn(async move {
                let _ = svc.workspaces.set_workspace_chat_focus(&id, &c).await;
            })
            .detach();
        if self.active.as_deref() == Some(code) {
            self.focus_chat_input(cx);
            return;
        }
        if let Some(tabs) = self.tabs.as_mut()
            && tabs.iter().any(|t| t.code == code && t.unseen)
        {
            for t in tabs.iter_mut().filter(|t| t.code == code) {
                t.unseen = false;
            }
        }
        self.set_active(code, cx);
        self.focus_chat_input(cx);
    }

    /// ⌥⌘← / ⌥⌘→ while the agent dock is focused.
    pub(crate) fn rotate_chat_tab(&mut self, dir: i32, cx: &mut Context<Self>) {
        let tabs = self.tabs();
        if tabs.len() < 2 {
            return;
        }
        let active = self.active_code();
        let n = tabs.len() as i32;
        let next = match tabs.iter().position(|t| t.code == active) {
            None => 0,
            Some(i) => (i as i32 + dir + n) % n,
        };
        let code = tabs[next as usize].code.clone();
        self.select_chat_tab(&code, cx);
    }

    /// ⌘9 while the agent dock is focused.
    pub(crate) fn select_last_chat_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(code) = self.tabs().last().map(|t| t.code.clone()) {
            self.select_chat_tab(&code, cx);
        }
    }

    /// A strip drop: show the new order at once, persist it, roll back on failure.
    pub(crate) fn reorder_tabs(&mut self, order: Vec<SharedString>, cx: &mut Context<Self>) {
        let snapshot = self.tabs().to_vec();
        let codes: Vec<String> = order.iter().map(|s| s.to_string()).collect();
        let next: Vec<WorkspaceChatTab> = codes
            .iter()
            .filter_map(|c| snapshot.iter().find(|t| &t.code == c).cloned())
            .collect();
        if next.len() != snapshot.len() {
            return;
        }
        self.reorder_committing = Some(codes.clone());
        self.set_tabs(next, cx);
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let ok = call(
                cx,
                svc.workspaces.reorder_workspace_chats(&id, &codes),
                "Could not reorder chats",
            )
            .await
            .is_some();
            let _ = this.update(cx, |v, cx| {
                v.reorder_committing = None;
                if !ok {
                    let current = v.tabs().to_vec();
                    let restored = recur_ui::widgets::tab_strip::restore_strip_order(
                        &current,
                        &snapshot,
                        |t| t.code.clone(),
                    );
                    v.set_tabs(restored, cx);
                }
            });
        })
        .detach();
    }

    /// Live titles after a turn (provisional on send, LLM upgrade shortly after). While a
    /// drop is committing the dropped order wins.
    pub(crate) fn refresh_tab_titles(&mut self, cx: &mut Context<Self>) {
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let Ok(tabs) = svc.workspaces.list_workspace_chats(&id).await else {
                return;
            };
            if tabs.is_empty() {
                return;
            }
            let _ = this.update(cx, |v, cx| {
                let list = match &v.reorder_committing {
                    Some(order) if v.tabs.as_ref().is_some_and(|t| !t.is_empty()) => {
                        apply_chat_tab_order(&tabs, order)
                    }
                    _ => tabs,
                };
                v.set_tabs(list, cx);
            });
        })
        .detach();
    }
}
