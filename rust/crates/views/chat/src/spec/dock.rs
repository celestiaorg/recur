//! The chat halves of `maestro-dock.spec.ts` and `loading-states.spec.ts` (the dock's routing
//! and panel placement are the workspace view's, specced there), and Ask Recur
//! (`AppChat.svelte`).

use std::sync::Arc;

use gpui::{Entity, TestAppContext, VisualTestContext};
use gpui_kit::AppContext as _;
use parking_lot::Mutex;
use recur_engine::view::loop_cockpit::merge_loop_cockpit;
use recur_services::mock::{MockBridge, MockOptions, Release, Reply};
use recur_view_core::prefs::{CodePanelKey, PanelLayout, PanelSide};
use recur_view_core::{AppState, LocalStorage, ViewContext};
use serde_json::json;

use super::harness::*;
use crate::app_chat::{ACTIVE_KEY, APP, AppChat};
use crate::session_feed::SessionFeed;

#[test]
fn the_chat_panel_can_open_on_the_right_when_configured() {
    let mut storage = LocalStorage::in_memory();
    storage.set(
        "recur.codePanelSides",
        r#"{"maestro":"right","files":"left","outline":"right","search":"right","refs":"right"}"#,
    );
    let layout = PanelLayout::load(&storage);
    assert_eq!(layout.sides[&CodePanelKey::Maestro], PanelSide::Right);
    assert_eq!(layout.sides[&CodePanelKey::Files], PanelSide::Left);
    assert!(
        layout
            .panels_on_side(PanelSide::Right)
            .contains(&CodePanelKey::Maestro)
    );
}

#[gpui::test]
async fn the_loop_chat_renders_session_cards_once_the_delayed_fetch_resolves(
    cx: &mut TestAppContext,
) {
    let pending: Arc<Mutex<Vec<Release>>> = Default::default();
    let p = pending.clone();
    let opts = MockOptions::default()
        .resolver("WorkspaceSessions", move |_, _, _| {
            let (r, release) = Reply::deferred();
            p.lock().push(release);
            r
        })
        .resolver("WorkspaceFacts", |_, _, _| Reply::value(json!([])));
    let mut h = boot(cx, opts, ws_props(), ws_hooks());
    let services = h.bridge.services();
    let feed = h.cx.update(|_, cx| cx.new(|_| SessionFeed::new(services)));
    feed.update(h.cx, |f, cx| f.attach(WS, "MA", cx));
    h.settle();
    assert!(feed.read_with(h.cx, |f, _| f.sessions.is_empty()));
    h.wait(300);
    pending.lock().pop().expect("a sessions fetch").send(Reply::value(json!([{
        "id": "s1", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle", "createdAt": 1, "updatedAt": 2,
        "items": [{"role": "assistant", "kind": "text", "text": "implemented the change"}],
    }])));
    h.settle();
    let merged = feed.read_with(h.cx, |f, _| {
        let live: Vec<String> = f.live_codes.iter().cloned().collect();
        merge_loop_cockpit(&[], &f.sessions, &f.facts, &live, "")
    });
    let session = merged.iter().find_map(|m| match m.log.as_deref() {
        Some(recur_engine::view::chat::ChatLog::Session(s)) => Some(s.clone()),
        _ => None,
    });
    let session = session.expect("a session row");
    assert_eq!(session.code, "CO");
    assert_eq!(session.messages[0].body, "implemented the change");
    h.set_messages(merged);
    assert_eq!(h.read(|c| c.blocks(&c.transcript()).0.len()), 1);
}

// ---- Ask Recur ----

struct A<'a> {
    app: Entity<AppChat>,
    state: Entity<AppState>,
    sent: Calls,
    /// Sends stay in flight while the test runs.
    _held: Arc<Mutex<Vec<Release>>>,
    cx: &'a mut VisualTestContext,
}

fn ask(cx: &mut TestAppContext, opts: MockOptions, storage: LocalStorage) -> A<'_> {
    init(cx);
    let sent = calls();
    let held: Arc<Mutex<Vec<Release>>> = Default::default();
    let (s2, h2) = (sent.clone(), held.clone());
    let bridge = MockBridge::fresh(opts.resolver("SendAppMessage", move |a, _, _| {
        s2.lock().push(a.to_vec());
        let (r, release) = Reply::deferred();
        h2.lock().push(release);
        r
    }));
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), storage, cx));
    let vcx = ViewContext {
        state: state.clone(),
        services,
    };
    let (app, cx) = cx.add_window_view(move |window, cx| AppChat::new(&vcx, true, window, cx));
    cx.simulate_resize(gpui::size(gpui::px(420.), gpui::px(720.)));
    let mut a = A {
        app,
        state,
        sent,
        _held: held,
        cx,
    };
    a.settle();
    a
}

impl A<'_> {
    fn settle(&mut self) {
        for _ in 0..3 {
            self.cx.run_until_parked();
            self.cx.update(|window, _| window.refresh());
        }
        self.cx.run_until_parked();
    }

    fn has(&mut self, sel: &str) -> bool {
        self.settle();
        self.cx
            .debug_bounds(Box::leak(sel.to_string().into_boxed_str()))
            .is_some()
    }

    fn click(&mut self, sel: &str) {
        self.settle();
        let p = self
            .cx
            .debug_bounds(Box::leak(sel.to_string().into_boxed_str()))
            .unwrap_or_else(|| panic!("no {sel}"))
            .center();
        self.cx
            .simulate_mouse_move(p, None, gpui::Modifiers::none());
        self.cx.simulate_click(p, gpui::Modifiers::none());
        self.settle();
    }
}

#[gpui::test]
async fn ask_chips_send_with_an_optimistic_bubble_and_stamp_the_tab_title(cx: &mut TestAppContext) {
    let mut a = ask(cx, MockOptions::default(), LocalStorage::in_memory());
    assert!(a.has("ask-chip:What needs me?"));
    a.click("ask-chip:What needs me?");
    let sent: Vec<String> = a
        .sent
        .lock()
        .iter()
        .map(|c| c[1].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(sent, ["What needs me?"]);
    assert!(a.has("me:What needs me?"));
    assert!(a.has("chat-stop"));
    let title = a.app.read_with(a.cx, |app, _| {
        app.tabs()
            .iter()
            .find(|t| t.code == APP)
            .map(|t| t.title.clone())
    });
    assert!(title.is_some());
}

#[gpui::test]
async fn ask_live_events_stream_into_the_active_tab_and_turn_end_settles(cx: &mut TestAppContext) {
    let mut a = ask(cx, MockOptions::default(), LocalStorage::in_memory());
    a.click("ask-chip:Where did I leave off?");
    a.app.update(a.cx, |app, cx| {
        app.on_session_for_test(
            "",
            APP,
            crate::session_events::SessionSignal::Delta(crate::session_events::StreamEvt {
                text: "You were fixing the dock.".into(),
                seq: 1,
                replace: true,
                ..Default::default()
            }),
            cx,
        )
    });
    assert!(a.has("streaming:You were fixing the dock."));
    // Another workspace's events do not land here.
    a.app.update(a.cx, |app, cx| {
        app.on_session_for_test(
            "LOOP-2231",
            APP,
            crate::session_events::SessionSignal::Delta(crate::session_events::StreamEvt {
                text: "not mine".into(),
                seq: 2,
                replace: true,
                ..Default::default()
            }),
            cx,
        )
    });
    assert!(!a.has("streaming:not mine"));
    a.app.update(a.cx, |app, cx| {
        app.on_session_for_test(
            "",
            APP,
            crate::session_events::SessionSignal::TurnEnd(crate::session_events::TurnEndEvt {
                duration_ms: 1500,
                usage: None,
            }),
            cx,
        )
    });
    assert!(a.has("turn-end"));
    assert!(!a.has("chat-turn-elapsed"));
}

#[gpui::test]
async fn ask_tabs_open_select_and_persist_the_active_one(cx: &mut TestAppContext) {
    let mut a = ask(cx, MockOptions::default(), LocalStorage::in_memory());
    assert_eq!(
        a.app.read_with(a.cx, |app, _| app.active().to_string()),
        APP
    );
    a.app.update(a.cx, |app, cx| app.new_tab(cx));
    a.settle();
    let active = a.app.read_with(a.cx, |app, _| app.active().to_string());
    assert_ne!(active, APP);
    let stored = a
        .state
        .read_with(a.cx, |s, _| s.storage().get(ACTIVE_KEY).map(str::to_string));
    assert_eq!(stored.as_deref(), Some(active.as_str()));
    let key = a.app.read_with(a.cx, |app, cx| {
        app.chat().read(cx).props().reset_key.clone()
    });
    assert_eq!(key, format!("app:{active}"));
    a.app.update(a.cx, |app, cx| app.select_tab(APP, cx));
    a.settle();
    assert_eq!(
        a.app.read_with(a.cx, |app, _| app.active().to_string()),
        APP
    );
}

#[gpui::test]
async fn ask_falls_back_from_a_saved_tab_the_backend_no_longer_lists(cx: &mut TestAppContext) {
    let mut storage = LocalStorage::in_memory();
    storage.set(ACTIVE_KEY, "AP9");
    let a = ask(cx, MockOptions::default(), storage);
    // A saved tab the backend no longer lists falls back to the primary chat.
    assert_eq!(
        a.app.read_with(a.cx, |app, _| app.active().to_string()),
        APP
    );
}
