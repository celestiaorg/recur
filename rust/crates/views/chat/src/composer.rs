//! The composer's behavior (`AgentChat.svelte`'s composer half and `lib/composerDom.ts`): the
//! `@` / `/` / prompt-argument menus driven by the caret, chip picks, the Discord-style prompt
//! chip with its slot inputs, editing keys, paste and drops of images and files, undo, and the
//! platform text input (IME) over the [`ComposerDoc`].

use recur_ui::guard::Guarded;
use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardEntry, Context, EntityInputHandler, KeyDownEvent,
    Pixels, Point, UTF16Selection, Window, point, px,
};
use recur_engine::view::chat::{ChatAttachmentDraft, new_attachment_id};
use recur_engine::view::composer::{PromptChipSpec, Segment, parse_prompt_chip_values};
use recur_engine::view::mentions::{
    format_file_mention_chip, mention_query_at, parse_file_mention_chip,
    prepare_file_mention_insert, repair_file_mention_boundaries, skill_query_at,
};
use recur_engine::view::prompts::{
    params_from_args, prompt_arg_slot_optional, prompt_arg_specs, prompt_required_args_filled,
    prompt_takes_args,
};
use recur_services::api::{PromptActionInfo, PromptCompleteHitDto, PromptCompleteRequestDto};
use recur_ui::logic::fuzzy::{fuzzy_filter_prepared, prepare_fuzzy};
use recur_ui::widgets::toasts::ToastKind;
use recur_view_core::chat::{WorkspaceArtifact, WorkspaceTerminalTab};
use recur_view_core::file_drag::{classify_os_drop, encode_chat_drop_id, is_chipable_file_path};
use recur_view_core::toast;

use crate::chat::{
    AgentChat, ChatEvent, Escaped, MentionHit, OpenArtifact, PromptArgHit, SkillHit, SlashHit,
    args_query, js_trim, now_ms,
};
use crate::composer_doc::{ChipKind, ComposerDoc};
use crate::file_index::search_files;
use crate::flow::ATOM;
use crate::prompt_actions::{PromptCatalog, complete_prompt_args, prompt_action};

/// `setTimeout(…, 200)` before a complete script runs.
const COMPLETE_DEBOUNCE: Duration = Duration::from_millis(200);

/// Composer min/max: two lines grow through six, then scroll (`resizeComposerHeight`).
pub const COMPOSER_LINE_PX: f32 = 12.5 * 1.4;
pub const COMPOSER_PAD_PX: f32 = 16.;
pub fn composer_min_height() -> f32 {
    (COMPOSER_PAD_PX + COMPOSER_LINE_PX * 2.).round()
}
pub fn composer_max_height() -> f32 {
    (COMPOSER_PAD_PX + COMPOSER_LINE_PX * 6.).round()
}

fn artifact_title_rank(title: &str, q: &str) -> u8 {
    let t = title.to_lowercase();
    if t == q {
        0
    } else if t.starts_with(q) {
        1
    } else if t.contains(q) {
        2
    } else {
        3
    }
}

/// `matchingTerminalHits`.
pub(crate) fn matching_terminal_hits(
    tabs: &[WorkspaceTerminalTab],
    query: &str,
) -> Vec<MentionHit> {
    let q = js_trim(query).to_lowercase();
    if tabs.is_empty() {
        return Vec::new();
    }
    let show_all = q.is_empty() || q == "term" || q == "terminal" || "terminal".starts_with(&q);
    tabs.iter()
        .filter(|t| {
            show_all || t.title.to_lowercase().contains(&q) || t.id.to_lowercase().starts_with(&q)
        })
        .take(8)
        .map(|t| MentionHit::Terminal {
            id: t.id.clone(),
            title: t.title.clone(),
            active: t.active,
            exited: t.exited,
        })
        .collect()
}

/// `matchingArtifactHits`.
pub(crate) fn matching_artifact_hits(items: &[WorkspaceArtifact], query: &str) -> Vec<MentionHit> {
    let q = js_trim(query).to_lowercase();
    if items.is_empty() {
        return Vec::new();
    }
    let show_all = q.is_empty() || (q.chars().count() >= 3 && "artifact".starts_with(&q));
    let mut hits: Vec<&WorkspaceArtifact> = items
        .iter()
        .filter(|a| {
            show_all
                || a.title.to_lowercase().contains(&q)
                || a.id.to_lowercase().starts_with(&q)
                || a.kind.to_lowercase().contains(&q)
        })
        .collect();
    if !show_all {
        hits.sort_by_key(|a| artifact_title_rank(&a.title, &q));
    }
    hits.into_iter()
        .take(8)
        .map(|a| MentionHit::Artifact {
            id: a.id.clone(),
            title: a.title.clone(),
            kind: a.kind.clone(),
        })
        .collect()
}

impl AgentChat {
    // ---- indexes ----

    pub(crate) fn ensure_skill_index(&mut self, cx: &mut Context<Self>) {
        if !self.skill_index.is_empty() || self.skill_index_loading {
            return;
        }
        self.skill_index_loading = true;
        let services = self.services.clone();
        let (p, w) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        self.skill_task = Some(cx.spawn(async move |this, cx| {
            let res = services.code.list_all_skills(&p, &w).await;
            let _ = this.update(cx, |c, cx| {
                c.skill_index_loading = false;
                match res {
                    Ok(list) => {
                        c.skill_index = list
                            .into_iter()
                            .filter(|s| s.user_invocable)
                            .map(|s| SkillHit {
                                name: s.name,
                                description: s.description,
                                scope: s.scope,
                            })
                            .collect();
                    }
                    Err(err) => toast(
                        ToastKind::Error,
                        format!(
                            "List skills failed: {}",
                            recur_view_core::bridge_error_message(&err)
                        ),
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn ensure_prompt_index(&mut self, cx: &mut Context<Self>) {
        let epoch = self.state.read(cx).bundles_epoch;
        if let Some(list) = PromptCatalog::peek(epoch, cx)
            && !std::rc::Rc::ptr_eq(&list, &self.prompt_index)
            && !list.is_empty()
        {
            self.prompt_index = list;
            self.rehydrate_for_specs(cx);
            return;
        }
        if !self.prompt_index.is_empty() || self.prompt_index_loading {
            return;
        }
        self.prompt_index_loading = true;
        let load = PromptCatalog::load(&self.services, epoch, cx);
        self.prompt_task = Some(cx.spawn(async move |this, cx| {
            let list = load.await;
            let _ = this.update(cx, |c, cx| {
                c.prompt_index_loading = false;
                c.prompt_index = list;
                c.rehydrate_for_specs(cx);
                c.sync_mention_from_caret(cx);
                cx.notify();
            });
        }));
    }

    /// A seeded `/prompt args` chips as a prompt once the lazy index lands.
    fn rehydrate_for_specs(&mut self, cx: &mut Context<Self>) {
        if self.doc.prompt_chip().is_some() || self.prompt_chip_specs(cx).is_empty() {
            return;
        }
        let next = self.hydrate_wire(&self.input.clone(), cx);
        if next.prompt_chip().is_some() {
            self.doc = next;
            self.rebuild_prompt_slots(Some(0), cx);
        }
    }

    pub(crate) fn prefetch_artifacts(&mut self, cx: &mut Context<Self>) {
        let ws = self.props.workspace_id.clone();
        if ws.is_empty() {
            return;
        }
        let services = self.services.clone();
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            if let Ok(list) = services.artifacts.list_artifacts(&ws).await {
                let rows = list
                    .into_iter()
                    .map(|a| WorkspaceArtifact {
                        title: if a.title.is_empty() {
                            a.id.clone()
                        } else {
                            a.title
                        },
                        id: a.id,
                        kind: a.kind.as_str().to_string(),
                        path: a.path,
                    })
                    .collect();
                state.update(cx, |s, cx| s.set_workspace_artifacts(&ws, rows, cx));
            }
        })
        .detach();
    }

    fn list_terminals_once(&mut self, cx: &mut Context<Self>) {
        if self.listed_terminals_for_mention || self.props.workspace_id.is_empty() {
            return;
        }
        self.listed_terminals_for_mention = true;
        let ws = self.props.workspace_id.clone();
        let services = self.services.clone();
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let Ok(list) = services.terminal.list_terminals(&ws).await else {
                return;
            };
            state.update(cx, |s, cx| {
                let prev = s.workspace_terminals(&ws).to_vec();
                let next = list
                    .into_iter()
                    .map(|info| {
                        let old = prev.iter().find(|t| t.id == info.id);
                        WorkspaceTerminalTab {
                            title: old
                                .map(|o| o.title.clone())
                                .filter(|t| !t.is_empty())
                                .or_else(|| (!info.title.is_empty()).then(|| info.title.clone()))
                                .unwrap_or_else(|| "zsh".into()),
                            exited: old.map_or(info.exited, |o| o.exited),
                            active: old.is_some_and(|o| o.active),
                            id: info.id,
                        }
                    })
                    .collect();
                s.set_workspace_terminals(&ws, next, cx);
            });
        })
        .detach();
    }

    fn list_artifacts_once(&mut self, cx: &mut Context<Self>) {
        if self.listed_artifacts_for_mention || self.props.workspace_id.is_empty() {
            return;
        }
        self.listed_artifacts_for_mention = true;
        self.prefetch_artifacts(cx);
    }

    // ---- derived menus ----

    pub(crate) fn enabled_skill_index(&self, cx: &App) -> Vec<SkillHit> {
        let s = self.state.read(cx);
        let proj = s.project(&self.props.project_id);
        let global_disabled = s
            .settings
            .as_ref()
            .map(|s| s.disabled_skills.clone())
            .unwrap_or_default();
        self.skill_index
            .iter()
            .filter(|sk| {
                if proj.is_some_and(|p| p.disabled_skills.contains(&sk.name)) {
                    return false;
                }
                if proj.is_some_and(|p| p.enabled_skills.contains(&sk.name)) {
                    return true;
                }
                !global_disabled.contains(&sk.name)
            })
            .cloned()
            .collect()
    }

    /// `slashHits`: prompts first (alphabetical via the list), then skills; prompts win
    /// same-name collisions.
    pub(crate) fn slash_hits(&self, cx: &App) -> Vec<SlashHit> {
        let Some(m) = &self.skill_mention else {
            return Vec::new();
        };
        let prompts: Vec<PromptActionInfo> = if self.prompt_gate_open && self.attachments.is_empty()
        {
            let index = self.enabled_prompt_index(cx);
            let prepared = prepare_fuzzy(&index, |p| {
                format!("{} {} {}", p.slash, p.name, p.description)
            });
            fuzzy_filter_prepared(&prepared, &m.query, 12)
                .into_iter()
                .map(|h| h.item)
                .collect()
        } else {
            Vec::new()
        };
        let skills = self.enabled_skill_index(cx);
        let prepared = prepare_fuzzy(&skills, |s| format!("{} {}", s.name, s.description));
        let skill_hits = fuzzy_filter_prepared(&prepared, &m.query, 12);
        let names: HashSet<&str> = prompts.iter().map(|p| p.slash.as_str()).collect();
        let mut out: Vec<SlashHit> = prompts
            .iter()
            .map(|p| {
                let a = prompt_action(p);
                SlashHit::Prompt {
                    slash: p.slash.clone(),
                    name: p.name.clone(),
                    description: p.description.clone(),
                    takes_args: prompt_takes_args(Some(&a)),
                    args: prompt_arg_specs(Some(&a))
                        .into_iter()
                        .map(|s| s.name)
                        .collect(),
                    mode: p.mode.clone(),
                }
            })
            .collect();
        out.extend(
            skill_hits
                .into_iter()
                .filter(|h| !names.contains(h.item.name.as_str()))
                .map(|h| SlashHit::Skill(h.item)),
        );
        out
    }

    pub(crate) fn prompt_arg_info(&self, cx: &App) -> Option<PromptActionInfo> {
        let q = self.prompt_args.as_ref()?;
        self.enabled_prompt_index(cx)
            .into_iter()
            .find(|p| p.slash == q.slash)
    }

    pub(crate) fn prompt_arg_hits(&self, cx: &App) -> Vec<PromptArgHit> {
        let Some(q) = &self.prompt_args else {
            return Vec::new();
        };
        if self.args_menu_escaped || !self.attachments.is_empty() {
            return Vec::new();
        }
        let info = self.prompt_arg_info(cx);
        let args = js_trim(&q.args).to_string();
        let mut rows = vec![PromptArgHit::Run {
            slash: q.slash.clone(),
            name: info
                .as_ref()
                .map(|i| i.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| q.slash.clone()),
            description: if args.is_empty() {
                info.as_ref()
                    .map(|i| i.arg_placeholder.clone())
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "Type an argument, then Enter".into())
            } else {
                info.as_ref()
                    .map(|i| i.description.clone())
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "Run this prompt".into())
            },
            args,
        }];
        rows.extend(
            self.complete_hits
                .iter()
                .cloned()
                .map(PromptArgHit::Suggest),
        );
        rows
    }

    /// The prompt chip's slot values, or the typed args.
    fn prompt_values_now(&self, cx: &App) -> Vec<String> {
        if self.doc.prompt_chip().is_some() {
            return self.slot_values(cx);
        }
        self.prompt_args
            .as_ref()
            .map(|q| vec![q.args.clone()])
            .unwrap_or_default()
    }

    pub(crate) fn prompt_arg_can_invoke(&self, cx: &App) -> bool {
        let info = self.prompt_arg_info(cx).map(|i| prompt_action(&i));
        prompt_required_args_filled(info.as_ref(), &self.prompt_values_now(cx))
    }

    pub(crate) fn prompt_arg_allow_empty(&self, cx: &App) -> bool {
        let info = self.prompt_arg_info(cx).map(|i| prompt_action(&i));
        let arg = self
            .prompt_args
            .as_ref()
            .map(|q| q.arg.clone())
            .unwrap_or_default();
        prompt_arg_slot_optional(info.as_ref(), &arg)
    }

    /// The menu rows in play: files, then prompt args, then `/`.
    pub(crate) fn active_menu_len(&self, cx: &App) -> usize {
        if self.mention.is_some() {
            self.mention_hits.len()
        } else if self.prompt_args.is_some() {
            self.prompt_arg_hits(cx).len()
        } else {
            self.slash_hits(cx).len()
        }
    }

    /// A slash, `@` or prompt-argument menu is open (`.chat-mention`): it owns Tab and Escape
    /// over a host dialog's (`composerOwnsDialogTab`).
    pub fn menu_open(&self) -> bool {
        self.mention.is_some() || self.skill_mention.is_some() || self.prompt_args.is_some()
    }

    /// Re-arms the hover gate when a menu opens, closes or changes rows, and keeps the
    /// selection in range.
    pub(crate) fn after_menu_change(&mut self, cx: &App) {
        let len = self.active_menu_len(cx);
        if self.mention_sel >= len {
            self.mention_sel = len.saturating_sub(1);
        }
        let key = format!(
            "{:?}|{:?}|{:?}|{len}",
            self.mention, self.skill_mention, self.prompt_args
        );
        if key != self.last_menu_key {
            self.last_menu_key = key;
            self.hover_gate.reset();
        }
    }

    // ---- caret → menus ----

    /// `syncMentionFromCaret`.
    pub(crate) fn sync_mention_from_caret(&mut self, cx: &mut Context<Self>) {
        if self.app_surface() {
            self.mention = None;
            self.skill_mention = None;
            self.prompt_args = None;
            return self.menus_changed(cx);
        }
        // A chip's wire form must not reopen its menu when the caret is flush after it.
        if self.doc.prompt_chip().is_some() {
            self.mention = None;
            self.skill_mention = None;
            self.prompt_gate_open = false;
            if self.args_menu_escaped {
                self.prompt_args = None;
                return self.menus_changed(cx);
            }
            let Some(Segment::Prompt { spec, .. }) = self
                .doc
                .prompt_chip()
                .and_then(|i| self.doc.chips.get(i))
                .cloned()
            else {
                return;
            };
            let values = self.slot_values(cx);
            let i = self.prompt_slot_focus.unwrap_or(0);
            let args = values.get(i).cloned().unwrap_or_else(|| values.join(" "));
            let args = recur_engine::view::js::trim_end(&args).to_string();
            let arg = spec.args.get(i).map(|a| a.name.clone()).unwrap_or_default();
            if self
                .prompt_args
                .as_ref()
                .is_none_or(|p| p.slash != spec.slash || p.args != args || p.arg != arg)
            {
                self.mention_sel = 0;
            }
            self.prompt_args = Some(recur_engine::view::prompts::PromptArgsQuery {
                args_start: spec.slash.len() + 2,
                slash: spec.slash.clone(),
                args,
                arg,
            });
            return self.menus_changed(cx);
        }
        if matches!(
            self.kind_before_caret(),
            Some(ChipKind::File | ChipKind::Skill | ChipKind::Terminal | ChipKind::Artifact)
        ) {
            self.mention = None;
            self.skill_mention = None;
            self.prompt_args = None;
            return self.menus_changed(cx);
        }
        let caret = self.caret_wire();
        let input = self.input.clone();
        let file_q = mention_query_at(&input, caret);
        let skill_q = skill_query_at(&input, caret);
        if let Some(fq) = file_q {
            if self
                .mention_escaped
                .as_ref()
                .is_some_and(|e| !e.skill && e.start == fq.start && e.query == fq.query)
            {
                self.mention = None;
                self.skill_mention = None;
                self.prompt_args = None;
                return self.menus_changed(cx);
            }
            self.mention_escaped = None;
            self.skill_mention = None;
            self.prompt_args = None;
            if self.mention.as_ref() == Some(&fq) {
                return self.menus_changed(cx);
            }
            if self
                .mention
                .as_ref()
                .is_none_or(|m| m.query != fq.query || m.start != fq.start)
            {
                self.mention_sel = 0;
            }
            let opened = self.mention.is_none();
            self.mention = Some(fq);
            if opened {
                self.list_terminals_once(cx);
                self.list_artifacts_once(cx);
            }
            self.run_file_search(cx);
            return self.menus_changed(cx);
        }
        self.mention = None;
        self.listed_terminals_for_mention = false;
        self.listed_artifacts_for_mention = false;
        if let Some(sq) = skill_q {
            if self
                .mention_escaped
                .as_ref()
                .is_some_and(|e| e.skill && e.start == sq.start && e.query == sq.query)
            {
                self.skill_mention = None;
                self.prompt_gate_open = false;
                self.prompt_args = None;
                return self.menus_changed(cx);
            }
            self.mention_escaped = None;
            self.prompt_gate_open =
                crate::chat::prompt_gate(&input, caret) && self.attachments.is_empty();
            self.prompt_args = None;
            if self.skill_mention.as_ref() == Some(&sq) {
                return self.menus_changed(cx);
            }
            if self
                .skill_mention
                .as_ref()
                .is_none_or(|m| m.query != sq.query || m.start != sq.start)
            {
                self.mention_sel = 0;
                self.ensure_skill_index(cx);
                if self.prompt_gate_open {
                    self.ensure_prompt_index(cx);
                }
            }
            self.skill_mention = Some(sq);
            return self.menus_changed(cx);
        }
        self.mention_escaped = None;
        self.skill_mention = None;
        self.prompt_gate_open = false;
        self.ensure_prompt_index(cx);
        let index = self.enabled_prompt_index(cx);
        let loaded = !self.prompt_index.is_empty();
        let takes = |slash: &str| match index.iter().find(|p| p.slash == slash) {
            Some(info) => prompt_takes_args(Some(&prompt_action(info))),
            None => !loaded,
        };
        if let Some(aq) = args_query(&input, caret, &takes)
            && self.attachments.is_empty()
        {
            if self.args_menu_escaped {
                self.prompt_args = None;
                return self.menus_changed(cx);
            }
            if self
                .prompt_args
                .as_ref()
                .is_none_or(|p| p.slash != aq.slash || p.args != aq.args)
            {
                self.mention_sel = 0;
            }
            self.prompt_args = Some(aq);
            return self.menus_changed(cx);
        }
        self.prompt_args = None;
        self.menus_changed(cx)
    }

    fn menus_changed(&mut self, cx: &mut Context<Self>) {
        self.after_menu_change(cx);
        self.schedule_complete(cx);
        self.notify_dock(cx);
    }

    /// The prompt-argument complete script, 200ms after the args settle.
    fn schedule_complete(&mut self, cx: &mut Context<Self>) {
        let info = self.prompt_arg_info(cx);
        let Some(q) = self.prompt_args.clone() else {
            self.complete_hits.clear();
            self.complete_message.clear();
            self.complete_task = None;
            return;
        };
        let Some(info) = info.filter(|i| i.has_complete && !self.args_menu_escaped) else {
            self.complete_hits.clear();
            self.complete_message.clear();
            self.complete_task = None;
            return;
        };
        self.complete_gen += 1;
        let cgen = self.complete_gen;
        let action = prompt_action(&info);
        let specs = prompt_arg_specs(Some(&action));
        let params = if self.doc.prompt_chip().is_some() {
            let values = self.slot_values(cx);
            specs
                .iter()
                .enumerate()
                .map(|(i, s)| (s.name.clone(), values.get(i).cloned().unwrap_or_default()))
                .collect()
        } else {
            params_from_args(Some(&action), &q.args)
        };
        let arg = if q.arg.is_empty() {
            specs.first().map(|s| s.name.clone()).unwrap_or_default()
        } else {
            q.arg.clone()
        };
        let req = PromptCompleteRequestDto {
            args: js_trim(&q.args).to_string(),
            arg,
            params,
            workspace_id: self.props.workspace_id.clone(),
        };
        let services = self.services.clone();
        self.complete_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COMPLETE_DEBOUNCE).await;
            let Ok(task) = this.update(cx, |_, cx| complete_prompt_args(&info, req, &services, cx))
            else {
                return;
            };
            let res = task.await;
            let _ = this.update(cx, |c, cx| {
                if cgen != c.complete_gen {
                    return;
                }
                match res {
                    Ok(r) => {
                        c.complete_hits = r.hits.unwrap_or_default();
                        c.complete_message = r.message;
                    }
                    Err(_) => {
                        c.complete_hits.clear();
                        c.complete_message.clear();
                    }
                }
                c.after_menu_change(cx);
                c.notify_dock(cx);
            });
        }));
    }

    /// The `SearchFiles` effect; terminal and artifact rows splice in without a refetch.
    pub(crate) fn run_file_search(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.mention.clone() else {
            self.mention_hits.clear();
            self.file_index_loading = false;
            return;
        };
        if self.props.workspace_id.is_empty() && self.props.project_id.is_empty() {
            self.mention_hits.clear();
            self.file_index_loading = false;
            return;
        }
        self.mention_search_gen += 1;
        let my = self.mention_search_gen;
        let terms = matching_terminal_hits(&self.live_terminals(cx), &m.query);
        let arts = matching_artifact_hits(&self.live_artifacts(cx), &m.query);
        self.file_index_loading = self
            .mention_hits
            .iter()
            .all(|h| !matches!(h, MentionHit::File { .. }))
            && terms.is_empty()
            && arts.is_empty();
        if self.props.project_id.is_empty() {
            self.file_index_loading = false;
            return;
        }
        let task = search_files(
            &self.services,
            &self.props.project_id,
            &self.props.workspace_id,
            &m.query,
            50,
            cx,
        );
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |c, cx| {
                if my != c.mention_search_gen {
                    return;
                }
                let Some(m) = c.mention.clone() else { return };
                let files: Vec<MentionHit> = res
                    .hits
                    .iter()
                    .map(|h| MentionHit::File {
                        item: h.item.clone(),
                        label: h.label.clone(),
                        score: h.score,
                        dir: h.dir,
                    })
                    .collect();
                let terms = matching_terminal_hits(&c.live_terminals(cx), &m.query);
                let arts = matching_artifact_hits(&c.live_artifacts(cx), &m.query);
                c.file_index_loading =
                    res.building && files.is_empty() && terms.is_empty() && arts.is_empty();
                let mut next = terms;
                next.extend(arts);
                next.extend(files);
                c.set_mention_hits(next);
                c.after_menu_change(cx);
                c.notify_dock(cx);
            });
        }));
    }

    /// Live terminal / artifact rows (OSC titles, focus) without restarting the search.
    pub(crate) fn splice_live_mention_rows(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.mention.clone() else {
            return;
        };
        let mut next = matching_terminal_hits(&self.live_terminals(cx), &m.query);
        next.extend(matching_artifact_hits(&self.live_artifacts(cx), &m.query));
        next.extend(
            self.mention_hits
                .iter()
                .filter(|h| matches!(h, MentionHit::File { .. }))
                .cloned(),
        );
        self.set_mention_hits(next);
        self.after_menu_change(cx);
    }

    /// `setMentionHits`: identical rows keep the array; otherwise the selection follows its
    /// row.
    fn set_mention_hits(&mut self, next: Vec<MentionHit>) {
        let prev = &self.mention_hits;
        if prev.len() == next.len() && prev.iter().zip(&next).all(|(a, b)| a.same(b)) {
            return;
        }
        let keep = prev.get(self.mention_sel).map(MentionHit::key);
        self.mention_hits = next;
        let Some(keep) = keep else { return };
        match self.mention_hits.iter().position(|h| h.key() == keep) {
            Some(i) => self.mention_sel = i,
            None if self.mention_sel >= self.mention_hits.len() => {
                self.mention_sel = self.mention_hits.len().saturating_sub(1);
            }
            None => {}
        }
    }

    // ---- picks ----

    /// Replaces the active query with a chip (`pickMention` / `pickTerminalMention` /
    /// `pickArtifactMention` / `pickSkill`).
    fn replace_query_with_chip(
        &mut self,
        q: recur_engine::view::mentions::MentionQuery,
        chip: Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.input.clone();
        let before = &input[..q.start];
        let after = &input[q.end..];
        let needs_space = !after.starts_with(' ') && !after.starts_with('\n');
        let mut doc = self.hydrate_wire(&format!("{before}{after}"), cx);
        doc.caret = doc.doc_offset(before.len());
        doc.anchor = doc.caret;
        doc.insert_chip(chip, needs_space);
        self.doc = doc;
        self.mention = None;
        self.skill_mention = None;
        self.prompt_gate_open = false;
        self.mention_escaped = None;
        self.sync_input_from_doc(true, cx);
        self.focus_composer(window, cx);
        self.menus_changed(cx);
    }

    pub(crate) fn pick_mention_hit(
        &mut self,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hit) = self.mention_hits.get(i).cloned() else {
            return;
        };
        let Some(q) = self.mention.clone() else {
            return;
        };
        let chip = match hit {
            MentionHit::Terminal { id, title, .. } => Segment::Terminal { id, title },
            MentionHit::Artifact { id, title, .. } => Segment::Artifact { id, title },
            MentionHit::File { item, dir, .. } => {
                let path = if dir {
                    recur_engine::view::mentions::dir_file_mention_path(&item)
                } else {
                    item
                };
                Segment::File {
                    dir: recur_engine::view::composer::is_dir_chip_path(&path),
                    path,
                }
            }
        };
        self.replace_query_with_chip(q, chip, window, cx);
    }

    pub(crate) fn pick_skill(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(q) = self.skill_mention.clone() else {
            return;
        };
        if name.is_empty() {
            return;
        }
        self.replace_query_with_chip(q, Segment::Skill(name.to_string()), window, cx);
    }

    /// `pickPrompt`: takes-args prompts become a chip; others clear the query and invoke.
    pub(crate) fn pick_prompt(&mut self, slash: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.skill_mention.is_none()
            || slash.is_empty()
            || !self.hooks.prompt_action
            || self.props.prompt_busy
            || self.props.sending
        {
            return;
        }
        let info = self
            .enabled_prompt_index(cx)
            .into_iter()
            .find(|p| p.slash == slash);
        if prompt_takes_args(info.as_ref().map(prompt_action).as_ref()) {
            let existing = recur_engine::view::prompts::parse_committed_prompt_line(&self.input)
                .filter(|(s, _)| s == slash)
                .map(|(_, a)| a)
                .unwrap_or_default();
            return self.enter_prompt_args(slash, &existing, window, cx);
        }
        self.doc = ComposerDoc::default();
        self.mention = None;
        self.skill_mention = None;
        self.prompt_gate_open = false;
        self.mention_escaped = None;
        self.sync_input_from_doc(true, cx);
        self.near_bottom = true;
        self.focus_composer(window, cx);
        self.menus_changed(cx);
        cx.emit(ChatEvent::PromptAction {
            slash: slash.to_string(),
            args: String::new(),
            alt: false,
        });
    }

    /// `enterPromptArgs`: `/slash args` as a chip with the first slot focused.
    pub(crate) fn enter_prompt_args(
        &mut self,
        slash: &str,
        args: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rest = js_trim(args);
        let next = if rest.is_empty() {
            format!("/{slash} ")
        } else {
            format!("/{slash} {rest}")
        };
        self.doc = self.hydrate_wire(&next, cx);
        self.mention = None;
        self.skill_mention = None;
        self.prompt_gate_open = false;
        self.args_menu_escaped = false;
        self.mention_escaped = None;
        let focus_slot = self.doc.prompt_chip().map(|_| 0);
        self.rebuild_prompt_slots(focus_slot, cx);
        self.sync_input_from_doc(true, cx);
        if let Some(i) = focus_slot {
            self.focus_prompt_slot(i, window, cx);
        } else {
            self.doc.caret = self.doc.text.len();
            self.doc.anchor = self.doc.caret;
            self.focus_composer(window, cx);
        }
        self.sync_mention_from_caret(cx);
    }

    /// `promptChipArgs`: the chip's args plus any text after it.
    pub(crate) fn prompt_chip_args(&self, slash: &str, fallback: &str) -> String {
        let Some(i) = self.doc.prompt_chip() else {
            return fallback.to_string();
        };
        let wire = self.doc.chips[i].wire();
        if !wire.starts_with(&format!("/{slash}")) {
            return fallback.to_string();
        }
        recur_engine::view::composer::prompt_chip_args_with_trailing(&self.doc.lines())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// `applyPromptArgs`.
    pub(crate) fn apply_prompt_args(
        &mut self,
        slash: &str,
        args: &str,
        invoke: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.hooks.prompt_action {
            return;
        }
        let remainder = self.prompt_chip_args(slash, args);
        if invoke {
            self.mention = None;
            self.skill_mention = None;
            self.prompt_gate_open = false;
            self.prompt_args = None;
            self.mention_escaped = None;
            self.near_bottom = true;
            cx.emit(ChatEvent::PromptAction {
                slash: slash.to_string(),
                args: remainder,
                alt: false,
            });
            return self.menus_changed(cx);
        }
        let next = if remainder.is_empty() {
            format!("/{slash} ")
        } else {
            format!("/{slash} {remainder}")
        };
        self.doc = self.hydrate_wire(&next, cx);
        self.mention = None;
        self.skill_mention = None;
        self.prompt_gate_open = false;
        self.prompt_args = Some(recur_engine::view::prompts::PromptArgsQuery {
            slash: slash.to_string(),
            args: remainder,
            args_start: slash.len() + 2,
            arg: String::new(),
        });
        self.mention_escaped = None;
        let focus_slot = self.doc.prompt_chip().map(|_| 0);
        self.rebuild_prompt_slots(focus_slot, cx);
        self.sync_input_from_doc(true, cx);
        match focus_slot {
            Some(i) => self.focus_prompt_slot(i, window, cx),
            None => self.focus_composer(window, cx),
        }
        self.menus_changed(cx);
    }

    /// `pickPromptCompleteHit`.
    pub(crate) fn pick_prompt_complete_hit(
        &mut self,
        hit: &PromptCompleteHitDto,
        invoke: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = js_trim(&hit.action).to_string();
        if let Some(section) = action.strip_prefix("settings:") {
            let section = section.to_string();
            self.state
                .update(cx, |s, cx| s.open_settings(Some(&section), cx));
            return;
        }
        let insert = js_trim(if hit.insert.is_empty() {
            &hit.label
        } else {
            &hit.insert
        })
        .to_string();
        if insert.is_empty() {
            return;
        }
        let info = self.prompt_arg_info(cx).map(|i| prompt_action(&i));
        if self.doc.prompt_chip().is_some() && !self.prompt_slots.is_empty() {
            let i = self
                .prompt_slot_focus
                .unwrap_or(0)
                .min(self.prompt_slots.len() - 1);
            let slot = self.prompt_slots[i].clone();
            slot.update(cx, |s, cx| s.set_value(insert.clone(), window, cx));
            self.on_slot_changed(cx);
            let values = self.slot_values(cx);
            if !invoke || !prompt_required_args_filled(info.as_ref(), &values) {
                if invoke {
                    let specs = prompt_arg_specs(info.as_ref());
                    let n = self.prompt_slots.len();
                    for step in 1..n {
                        let idx = (i + step) % n;
                        let optional = specs.get(idx).is_some_and(|s| s.optional);
                        if !optional
                            && js_trim(values.get(idx).map_or("", String::as_str)).is_empty()
                        {
                            self.focus_prompt_slot(idx, window, cx);
                            break;
                        }
                    }
                }
                self.sync_mention_from_caret(cx);
                return;
            }
        }
        if let Some(q) = self.prompt_args.clone() {
            let remainder = if self.doc.prompt_chip().is_some() {
                self.prompt_chip_args(&q.slash, &insert)
            } else {
                insert.clone()
            };
            let spec = PromptChipSpec {
                slash: q.slash.clone(),
                args: prompt_arg_specs(info.as_ref()),
            };
            let values = parse_prompt_chip_values(&remainder, &spec);
            let fill = invoke && prompt_required_args_filled(info.as_ref(), &values);
            self.apply_prompt_args(&q.slash, &insert, fill, window, cx);
        }
    }

    // ---- the prompt chip's slot inputs ----

    /// Rebuilds the slot inputs from the doc's prompt chip.
    pub(crate) fn rebuild_prompt_slots(&mut self, focus: Option<usize>, cx: &mut Context<Self>) {
        self.prompt_slots.clear();
        self.prompt_slot_subs.clear();
        self.prompt_slot_focus = focus;
        let Some(Segment::Prompt { spec, values }) = self
            .doc
            .prompt_chip()
            .and_then(|i| self.doc.chips.get(i))
            .cloned()
        else {
            return;
        };
        let window = self.window_handle;
        // Inside a window update (a host handler, a test step) the window is borrowed: build
        // the slots once it is free.
        if window.update(cx, |_, _, _| ()).is_err() {
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = this.update(cx, |c, cx| {
                    c.rebuild_prompt_slots(focus, cx);
                    c.notify_dock(cx);
                });
            });
            return;
        }
        let args: Vec<(String, String)> = if spec.args.is_empty() {
            vec![("args".into(), String::new())]
        } else {
            spec.args
                .iter()
                .map(|a| (a.name.clone(), a.placeholder.clone()))
                .collect()
        };
        for (i, (_, placeholder)) in args.iter().enumerate() {
            let value = values.get(i).cloned().unwrap_or_default();
            let placeholder = placeholder.clone();
            let slot = window
                .update(cx, |_, window, cx| {
                    cx.new(|cx| {
                        let mut s = InputState::new(window, cx).placeholder(placeholder);
                        s.set_value(value, window, cx);
                        s
                    })
                })
                .ok();
            let Some(slot) = slot else { continue };
            let sub = cx.guarded_subscribe(
                &slot,
                move |this: &mut Self, _, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => this.on_slot_changed(cx),
                    InputEvent::Focus => {
                        this.prompt_slot_focus = Some(i);
                        this.sync_mention_from_caret(cx);
                    }
                    InputEvent::Blur => this.notify_dock(cx),
                    InputEvent::PressEnter { .. } => {}
                },
            );
            self.prompt_slots.push(slot);
            self.prompt_slot_subs.push(sub);
        }
    }

    pub(crate) fn slot_values(&self, cx: &App) -> Vec<String> {
        self.prompt_slots
            .iter()
            .map(|s| s.read(cx).value().to_string())
            .collect()
    }

    fn on_slot_changed(&mut self, cx: &mut Context<Self>) {
        let values = self.slot_values(cx);
        if let Some(i) = self.doc.prompt_chip()
            && let Some(Segment::Prompt { spec, .. }) = self.doc.chips.get(i).cloned()
        {
            self.doc.set_chip(i, Segment::Prompt { spec, values });
        }
        self.args_menu_escaped = false;
        self.sync_input_from_doc(true, cx);
        self.sync_mention_from_caret(cx);
    }

    pub(crate) fn focus_prompt_slot(
        &mut self,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self
            .prompt_slots
            .get(i.min(self.prompt_slots.len().saturating_sub(1)))
            .cloned()
        else {
            return;
        };
        self.prompt_slot_focus = Some(i);
        slot.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.notify_dock(cx);
    }

    pub(crate) fn focused_slot(&self, window: &Window, cx: &App) -> Option<usize> {
        self.prompt_slots
            .iter()
            .position(|s| gpui_kit::Focusable::focus_handle(s.read(cx), cx).is_focused(window))
    }

    /// Keys a focused slot gives the chat (`onChipDeleteKey` for slots and the args menu).
    /// True when handled.
    pub(crate) fn on_slot_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(i) = self.focused_slot(window, cx) else {
            return false;
        };
        let key = ev.keystroke.key.as_str();
        if key == "backspace" && !ev.keystroke.modifiers.modified() {
            let slot = self.prompt_slots[i].clone();
            let empty = slot.read(cx).value().is_empty();
            if !empty {
                return false;
            }
            if i > 0 {
                self.focus_prompt_slot(i - 1, window, cx);
                return true;
            }
            if let Some(ci) = self.doc.prompt_chip() {
                self.doc.remove_chip(ci);
            }
            self.prompt_args = None;
            self.rebuild_prompt_slots(None, cx);
            self.sync_input_from_doc(true, cx);
            self.focus_composer(window, cx);
            self.menus_changed(cx);
            return true;
        }
        if matches!(key, "up" | "down" | "escape" | "enter" | "tab") {
            return self.on_mention_key(ev, window, cx);
        }
        if ev.keystroke.modifiers.platform && key == "." && self.hooks.cycle_mode {
            cx.emit(ChatEvent::CycleMode);
            return true;
        }
        false
    }

    // ---- keyboard ----

    /// `onMentionKeydown`: true when a menu handled the key.
    pub(crate) fn on_mention_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = ev.keystroke.key.as_str();
        if self.prompt_args.is_some() && self.mention.is_none() && self.skill_mention.is_none() {
            let hits = self.prompt_arg_hits(cx);
            match key {
                "down" => {
                    if !hits.is_empty() {
                        self.mention_sel = (self.mention_sel + 1).min(hits.len() - 1);
                    }
                    self.notify_dock(cx);
                    return true;
                }
                "up" => {
                    self.mention_sel = self.mention_sel.saturating_sub(1);
                    self.notify_dock(cx);
                    return true;
                }
                "escape" => {
                    self.args_menu_escaped = true;
                    self.prompt_args = None;
                    self.menus_changed(cx);
                    return true;
                }
                "enter" | "tab" => {
                    let tab = key == "tab";
                    let q = self.prompt_args.clone().expect("prompt args");
                    let Some(hit) = hits.get(self.mention_sel).cloned() else {
                        if tab {
                            return false;
                        }
                        if js_trim(&q.args).is_empty() {
                            if self.prompt_arg_can_invoke(cx) {
                                self.apply_prompt_args(&q.slash, "", true, window, cx);
                            }
                            return true;
                        }
                        return false;
                    };
                    match hit {
                        PromptArgHit::Suggest(h) => {
                            self.pick_prompt_complete_hit(&h, !tab, window, cx);
                            if tab
                                && let Some(i) = self.focused_slot(window, cx)
                                && i + 1 < self.prompt_slots.len()
                            {
                                self.focus_prompt_slot(i + 1, window, cx);
                            }
                            return true;
                        }
                        PromptArgHit::Run { slash, args, .. } => {
                            if tab {
                                if self.doc.prompt_chip().is_some() {
                                    if args.is_empty() && !self.prompt_arg_allow_empty(cx) {
                                        return true;
                                    }
                                    if let Some(i) = self.focused_slot(window, cx)
                                        && i + 1 < self.prompt_slots.len()
                                    {
                                        self.focus_prompt_slot(i + 1, window, cx);
                                    }
                                    return true;
                                }
                                return false;
                            }
                            if !self.prompt_arg_can_invoke(cx) {
                                return true;
                            }
                            self.apply_prompt_args(&slash, &args, true, window, cx);
                            return true;
                        }
                    }
                }
                _ => return false,
            }
        }
        if self.mention.is_none() && self.skill_mention.is_none() {
            return false;
        }
        let len = if self.mention.is_some() {
            self.mention_hits.len()
        } else {
            self.slash_hits(cx).len()
        };
        match key {
            "down" => {
                if len > 0 {
                    self.mention_sel = (self.mention_sel + 1).min(len - 1);
                }
                self.notify_dock(cx);
                true
            }
            "up" => {
                self.mention_sel = self.mention_sel.saturating_sub(1);
                self.notify_dock(cx);
                true
            }
            "enter" | "tab" => {
                if self.mention.is_some() {
                    if self.mention_sel < self.mention_hits.len() {
                        self.pick_mention_hit(self.mention_sel, window, cx);
                        return true;
                    }
                } else {
                    let hits = self.slash_hits(cx);
                    if let Some(hit) = hits.get(self.mention_sel).cloned() {
                        match hit {
                            SlashHit::Prompt { slash, .. } => self.pick_prompt(&slash, window, cx),
                            SlashHit::Skill(s) => self.pick_skill(&s.name, window, cx),
                        }
                        return true;
                    }
                    let q = self
                        .skill_mention
                        .as_ref()
                        .map(|m| m.query.clone())
                        .unwrap_or_default();
                    let exact = self
                        .enabled_prompt_index(cx)
                        .into_iter()
                        .find(|p| p.slash == q);
                    if let Some(p) = exact
                        && prompt_takes_args(Some(&prompt_action(&p)))
                    {
                        self.pick_prompt(&p.slash, window, cx);
                        return true;
                    }
                }
                if key == "tab" {
                    self.mention = None;
                    self.skill_mention = None;
                    self.prompt_gate_open = false;
                    self.menus_changed(cx);
                    return false;
                }
                // Enter on an open but empty menu dismisses it, never sends.
                self.mention = None;
                self.skill_mention = None;
                self.menus_changed(cx);
                true
            }
            "escape" => {
                if let Some(mq) = &self.mention {
                    self.mention_escaped = Some(Escaped {
                        skill: false,
                        start: mq.start,
                        query: mq.query.clone(),
                    });
                } else if let Some(sq) = &self.skill_mention {
                    self.mention_escaped = Some(Escaped {
                        skill: true,
                        start: sq.start,
                        query: sq.query.clone(),
                    });
                }
                self.mention = None;
                self.skill_mention = None;
                self.menus_changed(cx);
                true
            }
            _ => false,
        }
    }

    /// The composer's key handling: menus, chip deletion, undo, mode cycling, Shift+Enter
    /// line breaks and Enter to send.
    pub(crate) fn on_composer_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composer_marked.is_some() {
            return;
        }
        let k = &ev.keystroke;
        let m = k.modifiers;
        let key = k.key.as_str();
        if self.on_mention_key(ev, window, cx) {
            cx.stop_propagation();
            return;
        }
        let cmd = m.platform || m.control;
        let handled = match key {
            "z" if cmd && !m.alt => {
                self.apply_undo(m.shift, cx);
                true
            }
            "." if cmd && !m.shift && !m.alt && self.hooks.cycle_mode => {
                cx.emit(ChatEvent::CycleMode);
                true
            }
            "enter" if m.shift || (self.props.composer_only && !cmd) => {
                self.doc.break_line();
                self.after_edit(true, cx);
                true
            }
            "enter" => {
                self.send(cmd, window, cx);
                true
            }
            "backspace" => {
                if m.platform {
                    self.doc.delete_to_line_start();
                } else if m.alt {
                    self.doc.delete_word_backward();
                } else {
                    self.doc.delete_backward();
                }
                self.after_edit(true, cx);
                true
            }
            "delete" => {
                self.doc.delete_forward();
                self.after_edit(true, cx);
                true
            }
            "left" => {
                if m.platform {
                    let s = self.doc.line_start(self.doc.caret);
                    self.doc.move_to(s, m.shift);
                } else {
                    self.doc.move_left(m.shift, m.alt);
                }
                self.after_caret_move(cx);
                true
            }
            "right" => {
                if m.platform {
                    let e = self.doc.line_end(self.doc.caret);
                    self.doc.move_to(e, m.shift);
                } else {
                    self.doc.move_right(m.shift, m.alt);
                }
                self.after_caret_move(cx);
                true
            }
            "up" | "down" => {
                let down = key == "down";
                let next = if m.platform {
                    Some(if down { self.doc.text.len() } else { 0 })
                } else {
                    self.composer_layout
                        .borrow()
                        .as_ref()
                        .and_then(|l| l.vertical(self.doc.caret, down))
                        .or(Some(if down { self.doc.text.len() } else { 0 }))
                };
                if let Some(b) = next {
                    self.doc.move_to(b, m.shift);
                }
                self.after_caret_move(cx);
                true
            }
            "home" => {
                let s = self.doc.line_start(self.doc.caret);
                self.doc.move_to(s, m.shift);
                self.after_caret_move(cx);
                true
            }
            "end" => {
                let e = self.doc.line_end(self.doc.caret);
                self.doc.move_to(e, m.shift);
                self.after_caret_move(cx);
                true
            }
            "a" if cmd => {
                self.doc.select_all();
                self.after_caret_move(cx);
                true
            }
            "c" | "x" if cmd => {
                if self.doc.has_selection() {
                    let text = self.doc.selected_wire();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    if key == "x" {
                        self.doc.delete_backward();
                        self.after_edit(true, cx);
                    }
                }
                true
            }
            "v" if cmd => {
                self.paste(window, cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn apply_undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let snap = if redo {
            self.undo.redo()
        } else {
            self.undo.undo()
        };
        let Some(snap) = snap else { return };
        self.attachments = snap.atts.clone();
        let mut doc = self.hydrate_wire(&snap.text, cx);
        doc.caret = doc.doc_offset(snap.caret);
        doc.anchor = doc.caret;
        self.doc = doc;
        self.rebuild_prompt_slots(None, cx);
        self.sync_input_from_doc(false, cx);
        self.sync_mention_from_caret(cx);
        self.reveal_caret();
    }

    /// After any edit: wire text, undo, menus, layout.
    pub(crate) fn after_edit(&mut self, record: bool, cx: &mut Context<Self>) {
        self.args_menu_escaped = false;
        if self.doc.prompt_chip().is_none() && !self.prompt_slots.is_empty() {
            self.rebuild_prompt_slots(None, cx);
        }
        self.sync_input_from_doc(record, cx);
        self.sync_mention_from_caret(cx);
        self.caret_blink_epoch = std::time::Instant::now();
        self.reveal_caret();
    }

    fn after_caret_move(&mut self, cx: &mut Context<Self>) {
        self.sync_mention_from_caret(cx);
        self.caret_blink_epoch = std::time::Instant::now();
        self.reveal_caret();
        self.notify_dock(cx);
    }

    /// `scrollComposerCaretIntoView`: keep the caret 6px inside the composer's box.
    pub(crate) fn reveal_caret(&mut self) {
        let Some(l) = self.composer_layout.borrow().clone() else {
            return;
        };
        let (_, top, h) = l.caret(self.doc.caret);
        let view = composer_max_height() - COMPOSER_PAD_PX;
        let cur = -f32::from(self.composer_scroll.offset().y);
        let pad = 6.;
        let (top, bottom) = (top + 8., top + 8. + h);
        let next = if bottom > cur + view + COMPOSER_PAD_PX - pad {
            bottom - (view + COMPOSER_PAD_PX) + pad
        } else if top < cur + pad {
            (top - pad).max(0.)
        } else {
            cur
        };
        if (next - cur).abs() > 0.01 {
            self.composer_scroll
                .set_offset(point(px(0.), px(-next.max(0.))));
        }
    }

    pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_slot_focus = None;
        window.focus(&self.focus, cx);
        self.caret_blink_epoch = std::time::Instant::now();
        self.notify_dock(cx);
    }

    // ---- inserting chips and drafts ----

    /// `insertFileMention`: an `@file` chip at the caret (⌘L, drops). A 1-based loc becomes
    /// `@path:line` / `@path:start-end` / `@path:line:col-line:col`; an in-progress query is
    /// replaced and a whitespace boundary kept before the chip.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_file_mention(
        &mut self,
        path: &str,
        start_line: Option<i64>,
        end_line: Option<i64>,
        start_col: Option<i64>,
        end_col: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chip = format_file_mention_chip(
            path,
            start_line.unwrap_or(0),
            end_line.unwrap_or(0),
            start_col.unwrap_or(0),
            end_col.unwrap_or(0),
        );
        if chip.is_empty() || self.props.readonly {
            return;
        }
        self.doc.ensure_chip_boundaries();
        let text = self.doc.wire();
        let caret = self.doc.wire_caret();
        let after_chip = matches!(
            self.kind_before_caret(),
            Some(ChipKind::File | ChipKind::Skill | ChipKind::Terminal | ChipKind::Artifact)
        );
        let q = if after_chip {
            None
        } else {
            mention_query_at(&text, caret).or_else(|| skill_query_at(&text, caret))
        };
        if q.is_some() {
            let (before, after) = prepare_file_mention_insert(&text, caret, true);
            let caret_at = repair_file_mention_boundaries(&before).len();
            let mut doc = self.hydrate_wire(&format!("{before}{after}"), cx);
            doc.caret = doc.doc_offset(caret_at);
            doc.anchor = doc.caret;
            self.doc = doc;
        } else {
            let before = &text[..caret];
            if !before.is_empty() && !before.chars().next_back().is_some_and(char::is_whitespace) {
                self.doc.insert_text(" ");
            }
        }
        let wire = self.doc.wire();
        let after_caret = &wire[self.doc.wire_caret().min(wire.len())..];
        let needs_space = !after_caret.starts_with(' ') && !after_caret.starts_with('\n');
        let dir = recur_engine::view::composer::is_dir_chip_path(&chip);
        self.doc
            .insert_chip(Segment::File { path: chip, dir }, needs_space);
        self.mention = None;
        self.skill_mention = None;
        self.mention_escaped = None;
        self.sync_input_from_doc(true, cx);
        self.focus_composer(window, cx);
        self.menus_changed(cx);
        self.reveal_caret();
    }

    /// `insertDroppedDrafts`: image chips at the caret.
    pub(crate) fn insert_drafts(
        &mut self,
        added: Vec<ChatAttachmentDraft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if added.is_empty() {
            return;
        }
        for att in &added {
            self.doc.insert_chip(
                Segment::Attachment(recur_engine::view::composer::ComposerAtt {
                    id: att.id.clone(),
                    data_url: att.data_url.clone(),
                    name: att.name.clone(),
                }),
                true,
            );
        }
        self.attachments.extend(added);
        self.sync_input_from_doc(true, cx);
        self.focus_composer(window, cx);
        self.reveal_caret();
    }

    /// `onPaste`: images become drafts (not on Ask Recur); text pastes as plain text.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut drafts = Vec::new();
        for e in item.entries() {
            if let ClipboardEntry::Image(img) = e {
                let mime = img.format.mime_type();
                let id = new_attachment_id(now_ms());
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&img.bytes);
                drafts.push(ChatAttachmentDraft {
                    name: "image.png".into(),
                    mime: mime.to_string(),
                    data_url: format!("data:{mime};base64,{b64}"),
                    id,
                });
            }
        }
        if !drafts.is_empty() {
            if !self.app_surface() {
                self.insert_drafts(drafts, window, cx);
            }
            return;
        }
        let text = item
            .text()
            .unwrap_or_default()
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if text.is_empty() {
            return;
        }
        self.doc.insert_text(&text);
        self.after_edit(true, cx);
    }

    /// `applyOsDrop`: workspace files become `@file` chips, images attach.
    pub(crate) fn apply_os_drop(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.props.readonly || self.app_surface() {
            return;
        }
        let drop = classify_os_drop(&paths, &[], &self.props.root_path);
        if let Some(first) = drop.skipped.first() {
            let name = first.rsplit(['/', '\\']).next().unwrap_or(first);
            toast(
                ToastKind::Info,
                format!("Can't mention “{name}” — paths with spaces aren't supported yet"),
                cx,
            );
        }
        for p in &drop.mentions {
            self.insert_file_mention(p, None, None, None, None, window, cx);
        }
        let mut drafts = Vec::new();
        for p in &drop.image_paths {
            let Ok(bytes) = std::fs::read(p) else {
                continue;
            };
            let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
            let mime = match ext.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                "heic" => "image/heic",
                _ => "image/png",
            };
            use base64::Engine as _;
            let id = new_attachment_id(now_ms());
            drafts.push(ChatAttachmentDraft {
                name: p
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()
                    .unwrap_or("image.png")
                    .to_string(),
                mime: mime.into(),
                data_url: format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                ),
                id,
            });
        }
        if !drop.image_paths.is_empty() && drafts.is_empty() {
            toast(ToastKind::Error, "Could not attach dropped image", cx);
        }
        self.insert_drafts(drafts, window, cx);
    }

    /// An explorer drag (`fileDragPaths`) dropped on the chat.
    pub(crate) fn drop_explorer_paths(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_drop_over = false;
        if self.props.readonly || self.app_surface() {
            return;
        }
        for p in paths {
            if !is_chipable_file_path(p) {
                let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
                toast(
                    ToastKind::Info,
                    format!("Can't mention “{name}” — paths with spaces aren't supported yet"),
                    cx,
                );
                continue;
            }
            self.insert_file_mention(p, None, None, None, None, window, cx);
        }
    }

    /// `files.dropped` routed to this tab by its `data-chat-drop` id.
    pub(crate) fn on_files_dropped(
        &mut self,
        d: &recur_services::events::FilesDropped,
        cx: &mut Context<Self>,
    ) {
        if self.props.readonly || self.app_surface() || d.files.is_empty() {
            return;
        }
        let target = d
            .attributes
            .as_ref()
            .and_then(|a| a.get("data-chat-drop"))
            .cloned()
            .unwrap_or_default();
        if target != encode_chat_drop_id(&self.props.reset_key) {
            return;
        }
        let files = d.files.clone();
        let window = self.window_handle;
        let this = cx.entity();
        // Called from inside this entity's event loop update: apply once it returns.
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                this.update(cx, |c, cx| c.apply_os_drop(files, window, cx));
            });
        });
    }

    // ---- chip clicks ----

    pub(crate) fn open_file_chip(&mut self, chip_path: &str, cx: &mut Context<Self>) {
        if recur_engine::view::composer::is_dir_chip_path(chip_path) {
            return;
        }
        let r = parse_file_mention_chip(chip_path);
        if r.path.is_empty() {
            return;
        }
        let pos = |v: i64| (v > 0).then_some(v);
        cx.emit(ChatEvent::OpenFile {
            path: r.path.clone(),
            line: pos(r.start_line),
            end_line: (r.start_line > 0 && r.end_line > r.start_line).then_some(r.end_line),
            start_col: pos(r.start_col),
            end_col: pos(r.end_col),
        });
    }

    pub(crate) fn open_terminal_chip(&mut self, id: &str, cx: &mut Context<Self>) {
        let tabs = self.live_terminals(cx);
        let tid = if id.is_empty() {
            tabs.iter()
                .find(|t| t.active)
                .or(tabs.first())
                .map(|t| t.id.clone())
                .unwrap_or_default()
        } else {
            id.to_string()
        };
        let ws = self.props.workspace_id.clone();
        if tid.is_empty() || ws.is_empty() {
            return;
        }
        self.state.update(cx, |s, cx| {
            s.set_terminal_dock_open(&ws, true, cx);
            s.request_terminal_focus(&ws, &tid, cx);
        });
    }

    pub(crate) fn open_artifact_chip(&mut self, id: &str, cx: &mut Context<Self>) {
        let arts = self.live_artifacts(cx);
        let a = if id.is_empty() {
            arts.first().cloned()
        } else {
            arts.iter().find(|a| a.id == id).cloned()
        };
        let Some(a) = a else {
            if !id.is_empty() {
                toast(ToastKind::Info, "Artifact not found", cx);
            }
            return;
        };
        cx.emit(ChatEvent::OpenArtifact(OpenArtifact {
            id: a.id,
            title: a.title,
            kind: a.kind,
            path: a.path,
            created_at: String::new(),
            created_by: String::new(),
        }));
    }

    /// A chip's × (`[data-chip-remove]`).
    pub(crate) fn remove_chip(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let removed = self.doc.remove_chip(i);
        if let Some(Segment::Attachment(a)) = &removed {
            self.attachments.retain(|x| x.id != a.id);
        }
        if matches!(removed, Some(Segment::Prompt { .. })) {
            self.prompt_args = None;
            self.rebuild_prompt_slots(None, cx);
        }
        self.mention = None;
        self.skill_mention = None;
        self.sync_input_from_doc(true, cx);
        self.focus_composer(window, cx);
        self.menus_changed(cx);
    }

    /// A chip body click (`onComposerClick`).
    pub(crate) fn click_chip(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chip) = self.doc.chips.get(i).cloned() else {
            return;
        };
        match chip {
            Segment::File { path, dir } => {
                if !dir {
                    self.open_file_chip(&path, cx);
                }
            }
            Segment::Attachment(a) => self.open_lightbox(&a.data_url, &a.name, window, cx),
            Segment::Terminal { id, .. } => self.open_terminal_chip(&id, cx),
            Segment::Artifact { id, .. } => self.open_artifact_chip(&id, cx),
            Segment::Prompt { .. } => {
                let i = self.prompt_slot_focus.unwrap_or(0);
                self.focus_prompt_slot(i, window, cx);
            }
            Segment::Skill(_) | Segment::Text(_) => {}
        }
    }

    // ---- pointer ----

    pub(crate) fn composer_mouse_down(
        &mut self,
        byte: usize,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.doc.move_to(byte, shift);
        self.selecting = true;
        self.focus_composer(window, cx);
        self.sync_mention_from_caret(cx);
    }

    pub(crate) fn composer_mouse_drag(&mut self, byte: usize, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.doc.move_to(byte, true);
        self.notify_dock(cx);
    }

    pub(crate) fn utf16_range_to_bytes(&self, r: Range<usize>) -> Range<usize> {
        self.doc.byte_of_utf16(r.start)..self.doc.byte_of_utf16(r.end)
    }
}

impl EntityInputHandler for AgentChat {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        recur_ui::guard::catch("composer text_for_range", || {
            let r = self.utf16_range_to_bytes(range);
            *adjusted = Some(self.doc.utf16_of_byte(r.start)..self.doc.utf16_of_byte(r.end));
            Some(self.doc.text[r].replace(ATOM, "\u{FFFC}"))
        })
        .flatten()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        recur_ui::guard::catch("composer selected_text_range", || {
            let s = self.doc.selection();
            Some(UTF16Selection {
                range: self.doc.utf16_of_byte(s.start)..self.doc.utf16_of_byte(s.end),
                reversed: self.doc.caret < self.doc.anchor,
            })
        })
        .flatten()
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        recur_ui::guard::catch("composer marked_text_range", || {
            self.composer_marked
                .as_ref()
                .map(|r| self.doc.utf16_of_byte(r.start)..self.doc.utf16_of_byte(r.end))
        })
        .flatten()
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        recur_ui::guard::catch("composer unmark_text", || {
            self.composer_marked = None;
            self.notify_dock(cx);
        });
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        recur_ui::guard::catch("composer replace_text_in_range", || {
            if self.props.readonly {
                return;
            }
            let r = range
                .map(|r| self.utf16_range_to_bytes(r))
                .or(self.composer_marked.clone())
                .unwrap_or_else(|| self.doc.selection());
            self.doc.caret = r.end;
            self.doc.anchor = r.start;
            self.doc.insert_text(text);
            self.composer_marked = None;
            self.after_edit(true, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        recur_ui::guard::catch("composer replace_and_mark_text_in_range", || {
            let r = range
                .map(|r| self.utf16_range_to_bytes(r))
                .or(self.composer_marked.clone())
                .unwrap_or_else(|| self.doc.selection());
            self.doc.caret = r.end;
            self.doc.anchor = r.start;
            self.doc.insert_text(new_text);
            let start = r.start;
            self.composer_marked = (!new_text.is_empty()).then(|| start..start + new_text.len());
            if let Some(sel) = new_selected {
                let base = self.doc.utf16_of_byte(start);
                let a = self.doc.byte_of_utf16(base + sel.start);
                let b = self.doc.byte_of_utf16(base + sel.end);
                self.doc.anchor = a;
                self.doc.caret = b;
            }
            // Composition does not commit to the wire or undo until it ends.
            self.notify_dock(cx);
        });
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        recur_ui::guard::catch("composer bounds_for_range", || {
            let l = self.composer_layout.borrow().clone()?;
            let b = self.doc.byte_of_utf16(range.start);
            let (x, y, h) = l.caret(b);
            Some(Bounds::new(
                bounds.origin + point(px(x), px(y)),
                gpui_kit::size(px(2.), px(h)),
            ))
        })
        .flatten()
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        recur_ui::guard::catch("composer character_index_for_point", || {
            Some(self.doc.utf16_of_byte(self.composer_byte_at(p)))
        })
        .flatten()
    }
}
