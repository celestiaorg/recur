//! The shell with the chat's overlays registered: the floating permission card, ask toast,
//! Stop palette and typeahead overlays. (`loop-lifecycle.spec.ts` is the workspace view's.)

use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_kit::{AppContext as _, Focusable};
use recur_app::gallery::scene_seed;
use recur_app::{Shell, ShellOptions};
use recur_services::mock::{MockBridge, MockOptions};
use recur_ui::widgets::ToastStore;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewRegistry};
use serde_json::json;

use super::harness::init;

const RUNNING: &str = "LOOP-2229";

struct S<'a> {
    state: Entity<AppState>,
    cx: &'a mut VisualTestContext,
}

fn boot(cx: &mut TestAppContext, opts: MockOptions) -> S<'_> {
    init(cx);
    let bridge = MockBridge::new(scene_seed(&json!({})), opts);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let ctx = ViewContext {
        state: state.clone(),
        services,
    };
    let mut registry = ViewRegistry::default();
    crate::register(&mut registry);
    let (shell, cx) = cx.add_window_view(move |window, cx| {
        Shell::new(ctx, registry, ShellOptions::default(), window, cx)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.activate_window();
        let f = shell.focus_handle(cx);
        window.focus(&f, cx);
    });
    let mut s = S { state, cx };
    s.settle();
    s
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl S<'_> {
    fn settle(&mut self) {
        for _ in 0..3 {
            self.cx.run_until_parked();
            self.cx.update(|window, _| window.refresh());
        }
        self.cx.run_until_parked();
    }

    fn has(&mut self, sel: &str) -> bool {
        self.settle();
        self.cx.debug_bounds(leak(sel)).is_some()
    }

    fn open(&mut self, id: &str) {
        let id = id.to_string();
        self.state
            .update(self.cx, |s, cx| s.open_workspace(&id, false, cx));
        self.settle();
    }

    fn toasts(&mut self) -> Vec<String> {
        self.cx.update(|_, cx| {
            ToastStore::global(cx)
                .read(cx)
                .toasts()
                .iter()
                .map(|t| t.text.to_string())
                .collect()
        })
    }
}

// ---- the chat overlays in the shell ----

fn permission(ws: &str, code: &str, surface: &str) -> serde_json::Value {
    json!({"id": format!("perm-{code}"), "workspaceId": ws, "agentCode": code, "surface": surface,
        "tool": "Bash", "input": "{\"command\":\"rm -rf build\"}", "createdAt": 1})
}

#[gpui::test]
async fn a_loop_permission_floats_and_escape_denies_it(cx: &mut TestAppContext) {
    let mut s = boot(cx, MockOptions::default());
    let req: recur_domain::PermissionRequest =
        serde_json::from_value(permission(RUNNING, "MA", "loop")).unwrap();
    s.state.update(s.cx, |st, cx| {
        st.permission_requests.push(req);
        cx.notify();
    });
    assert!(s.has("perm-wrap"));
    s.cx.simulate_keystrokes("escape");
    s.settle();
    assert!(
        s.state
            .read_with(s.cx, |st, _| st.permission_requests.is_empty())
    );
    assert!(!s.has("perm-wrap"));
}

#[gpui::test]
async fn an_off_tab_question_toasts_once(cx: &mut TestAppContext) {
    let mut s = boot(cx, MockOptions::default());
    let q: recur_domain::UserQuestion = serde_json::from_value(json!({
        "id": "q1", "workspaceId": RUNNING, "agentCode": "PA", "question": "Ship   it\nnow?", "createdAt": 1,
    }))
    .unwrap();
    s.state.update(s.cx, |st, cx| {
        st.user_questions.push(q.clone());
        cx.notify();
    });
    s.settle();
    s.state.update(s.cx, |_, cx| cx.notify());
    s.settle();
    let asks: Vec<String> = s
        .toasts()
        .into_iter()
        .filter(|t| t.starts_with("Agent has a question"))
        .collect();
    assert_eq!(asks.len(), 1);
    assert!(asks[0].ends_with("Ship it now?"), "{}", asks[0]);
}

#[gpui::test]
async fn the_stop_palette_lists_running_commands_and_stops_the_picked_one(cx: &mut TestAppContext) {
    let mut s = boot(cx, MockOptions::default());
    s.open(RUNNING);
    s.state.update(s.cx, |st, cx| {
        st.set_run_session(RUNNING, "dev", "sess-dev", cx);
        st.set_run_session(RUNNING, "test", "sess-test", cx);
    });
    let opened = s
        .state
        .update(s.cx, |st, cx| st.open_stop_menu(RUNNING, cx));
    assert!(opened);
    assert!(s.has("stop-dev"));
    assert!(s.has("stop-test"));
    assert!(s.has("stop-__all__"));
    s.cx.simulate_input("tes");
    s.settle();
    assert!(!s.has("stop-dev"));
    assert!(s.has("stop-test"));
    s.cx.simulate_keystrokes("escape");
    s.settle();
    assert!(!s.state.read_with(s.cx, |st, _| st.overlays.stop_menu));
    assert!(!s.has("stop-test"));
}

/// A bare text field with the emoji and mention typeaheads, and the overlays painting them.
struct Field {
    input: Entity<gpui_kit::base::input::InputState>,
    emoji: Entity<crate::suggest::SuggestOverlay>,
    _attach: Entity<crate::suggest::SuggestAttach<gpui_kit::base::input::InputMode>>,
}

impl gpui_kit::Render for Field {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        gpui_kit::div()
            .size_full()
            .p(gpui_kit::px(40.))
            .child(gpui_kit::base::input::Input::new(&self.input))
            .child(self.emoji.clone())
    }
}

#[gpui::test]
async fn an_emoji_shortcode_in_an_attached_field_expands_and_suggests(cx: &mut TestAppContext) {
    init(cx);
    let (field, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| gpui_kit::base::input::InputState::new(window, cx));
        let attach = crate::suggest::attach_emoji(&input, None, window, cx);
        let emoji = cx
            .new(|cx| crate::suggest::SuggestOverlay::new(crate::suggest::SuggestKind::Emoji, cx));
        Field {
            input,
            emoji,
            _attach: attach,
        }
    });
    let input = field.read_with(cx, |f, _| f.input.clone());
    cx.update(|window, cx| {
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.run_until_parked();
    cx.simulate_input("go :rocket: ");
    cx.run_until_parked();
    assert_eq!(input.read_with(cx, |i, _| i.value().to_string()), "go 🚀 ");
    cx.simulate_input(":fir");
    cx.run_until_parked();
    let open = cx.update(|_, cx| {
        crate::suggest::SuggestStore::global(cx)
            .read(cx)
            .emoji
            .as_ref()
            .map(|o| o.query.clone())
    });
    assert_eq!(open.as_deref(), Some("fir"));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("emoji-suggest-row:0").is_some());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        input.read_with(cx, |i, _| i.value().to_string()),
        "go 🚀 🔥"
    );
    let open = cx.update(|_, cx| {
        crate::suggest::SuggestStore::global(cx)
            .read(cx)
            .emoji
            .is_some()
    });
    assert!(!open);
}

/// `textarea-caret.test.ts`: the caret rect the typeaheads anchor to (`textareaCaretRect`, here
/// the field's own `range_to_bounds`) is finite, inside the field, and moves right along a line.
#[gpui::test]
async fn the_caret_rect_sits_in_the_field_and_moves_right_along_a_line(cx: &mut TestAppContext) {
    init(cx);
    let (field, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| gpui_kit::base::input::InputState::new(window, cx));
        let attach = crate::suggest::attach_emoji(&input, None, window, cx);
        let emoji = cx
            .new(|cx| crate::suggest::SuggestOverlay::new(crate::suggest::SuggestKind::Emoji, cx));
        Field {
            input,
            emoji,
            _attach: attach,
        }
    });
    let input = field.read_with(cx, |f, _| f.input.clone());
    cx.update(|window, cx| {
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.simulate_input("hello :roc");
    cx.run_until_parked();
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let len = input.read_with(cx, |i, _| i.value().len());
    let rect = |at: usize, cx: &mut gpui::VisualTestContext| {
        input
            .read_with(cx, |i, _| i.range_to_bounds(&(at..at)))
            .expect("caret bounds")
    };
    let (start, end) = (rect(0, cx), rect(len, cx));
    assert!(f32::from(end.origin.x).is_finite() && f32::from(end.origin.y).is_finite());
    assert!(f32::from(end.size.height) > 0.);
    // The field starts at the view's 40px padding.
    assert!(f32::from(end.origin.x) >= 40. - 1.);
    assert!(end.origin.x > start.origin.x);
}

struct MentionField {
    input: Entity<gpui_kit::base::input::InputState>,
    overlay: Entity<crate::suggest::SuggestOverlay>,
    _attach: Entity<crate::suggest::SuggestAttach<gpui_kit::base::input::InputMode>>,
}

impl gpui_kit::Render for MentionField {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        gpui_kit::div()
            .size_full()
            .p(gpui_kit::px(40.))
            .child(gpui_kit::base::input::Input::new(&self.input))
            .child(self.overlay.clone())
    }
}

/// A field with `@login` typeahead over a selected GitHub PR with Avery on it.
fn mention_field(
    cx: &mut TestAppContext,
) -> (
    Entity<MentionField>,
    Arc<parking_lot::Mutex<String>>,
    &mut VisualTestContext,
) {
    init(cx);
    let bridge = MockBridge::fresh(MockOptions::default());
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services, LocalStorage::in_memory(), cx));
    state.update(cx, |s, _| {
        s.prs = vec![serde_json::from_value(json!({
            "id": "pr-1", "source": "gh", "participants": [{"login": "avery", "name": "Avery Rourke"}],
        }))
        .unwrap()];
        s.selected_pr_id = "pr-1".into();
    });
    let draft: Arc<parking_lot::Mutex<String>> = Default::default();
    let d2 = draft.clone();
    let (field, cx) = cx.add_window_view(move |window, cx| {
        let input = cx.new(|cx| gpui_kit::base::input::InputState::new(window, cx));
        let set: crate::suggest::SetFn = std::rc::Rc::new(
            move |v: &str, _: &mut gpui_kit::Window, _: &mut gpui_kit::App| {
                *d2.lock() = v.to_string()
            },
        );
        let attach =
            crate::suggest::attach_user_mentions(&input, state.clone(), Some(set), window, cx);
        let overlay = cx.new(|cx| {
            crate::suggest::SuggestOverlay::new(crate::suggest::SuggestKind::Mention, cx)
        });
        MentionField {
            input,
            overlay,
            _attach: attach,
        }
    });
    let input = field.read_with(cx, |f, _| f.input.clone());
    cx.update(|window, cx| {
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.run_until_parked();
    (field, draft, cx)
}

fn mention_open(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| {
        crate::suggest::SuggestStore::global(cx)
            .read(cx)
            .mention
            .as_ref()
            .map(|o| o.query.clone())
    })
}

#[gpui::test]
async fn picking_a_mention_replaces_the_query_and_parks_the_caret_after_the_space(
    cx: &mut TestAppContext,
) {
    let (field, draft, cx) = mention_field(cx);
    let input = field.read_with(cx, |f, _| f.input.clone());
    cx.simulate_input("hi @av");
    cx.run_until_parked();
    assert_eq!(mention_open(cx).as_deref(), Some("av"));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("user-mention-row:0").is_some());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*draft.lock(), "hi @avery ");
    assert_eq!(
        input.read_with(cx, |i, _| (i.value().to_string(), i.cursor())),
        ("hi @avery ".to_string(), 10)
    );
    assert!(mention_open(cx).is_none());
}

#[gpui::test]
async fn caret_movement_closes_the_picker_and_enter_does_not_rewrite_the_draft(
    cx: &mut TestAppContext,
) {
    let (field, draft, cx) = mention_field(cx);
    let input = field.read_with(cx, |f, _| f.input.clone());
    cx.simulate_input("Please review @av");
    cx.run_until_parked();
    assert_eq!(mention_open(cx).as_deref(), Some("av"));
    cx.update(|window, cx| {
        input.update(cx, |i, cx| {
            i.set_cursor_position(gpui_kit::base::input::Position::new(0, 0), window, cx)
        })
    });
    cx.run_until_parked();
    assert!(mention_open(cx).is_none());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*draft.lock(), "");
    assert!(input.read_with(cx, |i, _| i.value().starts_with("Please review @av")));
}

struct PromptsHost {
    prompts: Entity<crate::palettes::CommandPrompts>,
    events: std::rc::Rc<std::cell::RefCell<Vec<crate::palettes::CommandPromptsEvent>>>,
    _sub: gpui_kit::Subscription,
}

impl gpui_kit::Render for PromptsHost {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        recur_ui::style::body(cx)
            .size_full()
            .relative()
            .child(self.prompts.clone())
    }
}

#[gpui::test]
async fn command_prompts_resolve_with_their_values_and_escape_cancels(cx: &mut TestAppContext) {
    use recur_view_core::commands::CommandPrompt;
    init(cx);
    let (host, cx) = cx.add_window_view(|_, cx| {
        let prompts = cx.new(crate::palettes::CommandPrompts::new);
        let events: std::rc::Rc<std::cell::RefCell<Vec<_>>> = Default::default();
        let ev = events.clone();
        let _sub = cx.subscribe(
            &prompts,
            move |_, _, e: &crate::palettes::CommandPromptsEvent, _| {
                ev.borrow_mut().push(e.clone())
            },
        );
        PromptsHost {
            prompts,
            events,
            _sub,
        }
    });
    let prompts = host.read_with(cx, |h, _| h.prompts.clone());
    let field = |name: &str, default: &str| CommandPrompt {
        name: name.into(),
        label: String::new(),
        r#type: "text".into(),
        default: default.into(),
        choices: Vec::new(),
    };
    cx.update(|window, cx| {
        prompts.update(cx, |p, cx| {
            p.open(
                "Rename",
                vec![field("name", "old"), field("note", "")],
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    // The first field is focused at its default; typing appends.
    cx.simulate_input("er");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let got = host.read_with(cx, |h, _| h.events.borrow().clone());
    let want = std::collections::BTreeMap::from([
        ("name".to_string(), "older".to_string()),
        ("note".to_string(), String::new()),
    ]);
    assert_eq!(got, [crate::palettes::CommandPromptsEvent::Resolve(want)]);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let got = host.read_with(cx, |h, _| h.events.borrow().clone());
    assert_eq!(
        got.last(),
        Some(&crate::palettes::CommandPromptsEvent::Cancel)
    );
}
