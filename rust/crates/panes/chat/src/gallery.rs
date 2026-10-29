//! Parity scenes: the `chat` list in `crates/ui/gallery/scenes.json`, built the way
//! `web/scenes/Chat.svelte` mounts AgentChat and AppChat, over `content/chat.json`.

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, div,
};
use recur_domain::{ChatItem, FeedItem, PermissionRequest, UserQuestion};
use recur_engine::view::chat::{StreamKind, StreamOpts, apply_stream, map_chat_items};
use recur_engine::view::loop_cockpit::merge_loop_cockpit;
use recur_engine::view::loops::{LoopInfo, PolicyInfo};
use recur_services::mock::{MockBridge, MockOptions};
use recur_ui::gallery::SceneSpec;
use recur_ui::style::body;
use recur_ui::widgets::select::{Select, SelectOption};
use recur_view_core::models::{CatalogModel, flags_for_model, normalize_flags};
use recur_view_core::modes::{ChatMode, resolved_default_for_mode};
use recur_view_core::nav::ViewName;
use recur_view_core::{AppState, LocalStorage, ViewContext};
use serde::Deserialize;
use serde_json::Value;

use crate::app_chat::AppChat;
use crate::chat::{AgentChat, ChatHooks, ChatProps, now_ms};
use crate::pickers::{LoopPolicyPickers, ModelPickers};

const SCENES_JSON: &str = include_str!("../../../ui/gallery/scenes.json");
const FIXTURES: &str = include_str!("../../../ui/gallery/content/chat.json");

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    chat: Vec<SceneSpec>,
}

/// Every chat scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.chat)
        .unwrap_or_default()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Fixture {
    items: Vec<ChatItem>,
    facts: Vec<FeedItem>,
    stream: String,
    sending: bool,
    draft: String,
    permission: Option<PermissionRequest>,
    question: Option<UserQuestion>,
}

#[derive(Deserialize)]
struct Named {
    id: String,
    name: String,
    hint: String,
}

fn fixtures() -> Value {
    serde_json::from_str(FIXTURES).unwrap_or(Value::Null)
}

fn named(key: &str) -> Vec<Named> {
    serde_json::from_value(fixtures()[key].clone()).unwrap_or_default()
}

/// The scene's chat with the mode toolbar, focusing the composer once the window has its
/// document focus (`focusComposer()` after mount).
struct ChatScene {
    chat: Entity<AgentChat>,
    focus_composer: bool,
}

impl ChatScene {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if std::mem::take(&mut self.focus_composer) {
            self.chat.update(cx, |c, cx| c.focus_composer(window, cx));
        }
        body(cx)
            .size_full()
            .flex()
            .flex_col()
            .child(self.chat.clone())
    }
}

impl Render for ChatScene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

struct AppScene(Entity<AppChat>);

impl AppScene {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        body(cx).size_full().flex().flex_col().child(self.0.clone())
    }
}

impl Render for AppScene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The dock toolbar on the `mode-*` scenes: the chat mode select, then the model pickers (or
/// the loop / policy pickers in Loop mode).
struct ModeToolbar {
    mode: Entity<Select>,
    models: Option<Entity<ModelPickers>>,
    loops: Option<Entity<LoopPolicyPickers>>,
}

impl ModeToolbar {
    fn new(
        mode: ChatMode,
        state: &Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let t = recur_ui::Theme::of(cx).tokens;
        let options = ChatMode::options(false, false)
            .into_iter()
            .map(|m| {
                SelectOption::new(m.key(), m.label())
                    .icon(m.icon())
                    .tint(m.tint(&t))
                    .hint(m.hint())
            })
            .collect();
        let select = cx.new(|cx| {
            let mut s = Select::new(options, window, cx)
                .label("Chat mode")
                .ghost()
                .up();
            s.set_value(mode.key(), cx);
            s
        });
        if mode == ChatMode::Loop {
            let loops: Vec<LoopInfo> = named("loops")
                .into_iter()
                .map(|n| LoopInfo {
                    id: n.id,
                    name: n.name,
                    hint: n.hint,
                })
                .collect();
            let policies: Vec<PolicyInfo> = named("policies")
                .into_iter()
                .map(|n| PolicyInfo {
                    id: n.id,
                    name: n.name,
                    hint: n.hint,
                })
                .collect();
            let pickers = cx.new(|cx| LoopPolicyPickers::new(true, true, false, window, cx));
            pickers.update(cx, |p, cx| {
                p.set(
                    &loops[0].id,
                    &policies[0].id,
                    &loops,
                    &policies,
                    false,
                    t.pane,
                    cx,
                )
            });
            return ModeToolbar {
                mode: select,
                models: None,
                loops: Some(pickers),
            };
        }
        let pickers = cx.new(|cx| ModelPickers::new(true, true, window, cx));
        let load = state.update(cx, |s, cx| s.load_model_catalog(cx));
        let p = pickers.clone();
        cx.spawn(async move |this, cx| {
            let models: Vec<CatalogModel> = load.await;
            let _ = this.update(cx, |_, cx| {
                let d = resolved_default_for_mode(mode.key(), None, &models);
                let flags = normalize_flags(&flags_for_model(&models, &d.model), Some(&d.flags));
                p.update(cx, |p, cx| p.set(models, &d.model, &flags, None, cx));
                cx.notify();
            });
        })
        .detach();
        ModeToolbar {
            mode: select,
            models: Some(pickers),
            loops: None,
        }
    }
}

impl ModeToolbar {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut row = div()
            .flex()
            .items_center()
            .gap(gpui_kit::px(6.))
            .min_w_0()
            .child(self.mode.clone());
        if let Some(m) = &self.models {
            row = row.child(m.clone());
        }
        if let Some(l) = &self.loops {
            row = row.child(l.clone());
        }
        row
    }
}

impl Render for ModeToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let spec = specs().into_iter().find(|s| s.name == name)?;
    // Scenes share one app: no chat chrome carries over from the previous one.
    crate::transcript_ui::TranscriptUiCache::global(cx).clear();
    let bridge = MockBridge::fresh(MockOptions::default());
    let services = bridge.services();
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", spec.theme.clone());
    let state: Entity<AppState> = AppState::install(services.clone(), storage, cx);
    let vcx = ViewContext {
        state: state.clone(),
        services,
    };
    let variant = name.split_once('/').map_or("", |(_, v)| v);
    let variant = variant.strip_suffix("-paper").unwrap_or(variant);
    if variant == "app" {
        let app = cx.new(|cx| AppChat::new(&vcx, true, window, cx));
        return Some(cx.new(|_| AppScene(app)).into());
    }
    let mode = variant
        .strip_prefix("mode-")
        .or((variant == "pickers").then_some("agent"))
        .map(ChatMode::from_key);
    let code = if mode.is_some_and(|m| m != ChatMode::Loop) {
        "PA"
    } else {
        "MA"
    };
    let fx: Fixture = serde_json::from_value(fixtures()[variant].clone()).unwrap_or_default();
    state.update(cx, |s, cx| {
        s.view = ViewName::Detail;
        s.permission_requests.extend(fx.permission.clone());
        s.user_questions.extend(fx.question.clone());
        cx.notify();
    });
    let mut ma = map_chat_items(&fx.items);
    if !fx.stream.is_empty() {
        ma = apply_stream(&ma, StreamKind::Text, &fx.stream, &StreamOpts::default());
    }
    let messages = merge_loop_cockpit(&ma, &[], &fx.facts, &[], "");
    let sending = fx.sending;
    let props = ChatProps {
        messages,
        sending,
        can_stop: sending,
        turn_started_at: now_ms(),
        compact: true,
        reset_key: format!("LOOP-2231:{code}"),
        empty_text: if code == "MA" {
            "Ask where this loop stands, or steer it — the Maestro can read the worktree, edit the plan, re-run review, and dispatch specialists; rails hold at execution."
        } else {
            "Ask the agent, or tell it what to change — the mode selector below decides whether it may edit."
        }
        .into(),
        placeholder: if sending {
            "Add a follow-up… (@ files, / skills, ⏎ to queue)"
        } else if code == "MA" {
            "Ask where things stand, or steer the loop… (@ files, / skills)"
        } else {
            "Message the agent… (@ files, / skills, ⏎ to send)"
        }
        .into(),
        root_path: "/mock/worktrees/LOOP-2231".into(),
        project_id: "storefront".into(),
        workspace_id: "LOOP-2231".into(),
        agent_code: code.into(),
        ..Default::default()
    };
    let chat = cx.new(|cx| AgentChat::new(&vcx, props, window, cx));
    let toolbar =
        mode.map(|m| -> AnyView { cx.new(|cx| ModeToolbar::new(m, &state, window, cx)).into() });
    let draft = fx.draft.clone();
    chat.update(cx, |c, cx| {
        c.set_hooks(
            ChatHooks {
                send: true,
                stop: true,
                retry: true,
                remove_queued: true,
                cycle_mode: true,
                open_file: true,
                open_artifact: true,
                fork: true,
                fork_to_workspace: true,
                prompt_action: true,
                ..Default::default()
            },
            cx,
        );
        c.set_toolbar(toolbar, cx);
        if !draft.is_empty() {
            c.set_draft(draft, Vec::new(), cx);
        }
    });
    let focus_composer = variant == "composer";
    Some(
        cx.new(|_| ChatScene {
            chat,
            focus_composer,
        })
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_has_a_fixture_or_needs_none() {
        let fx = fixtures();
        for spec in specs() {
            let v = spec.name.split_once('/').map_or("", |(_, v)| v);
            let v = v.strip_suffix("-paper").unwrap_or(v);
            let needs = [
                "transcript",
                "streaming",
                "permission",
                "question",
                "plan",
                "fanout",
                "composer",
            ];
            if needs.contains(&v) {
                assert!(
                    serde_json::from_value::<Fixture>(fx[v].clone()).is_ok(),
                    "{v}"
                );
            }
        }
        assert_eq!(named("loops").len(), 2);
    }
}
