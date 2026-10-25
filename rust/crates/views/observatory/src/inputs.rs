//! The Observatory's text fields: the Search panel's query, replacement and glob, and the
//! Source Control commit message (with `:emoji:` and `@mention` typeahead, `autosizeTextarea`
//! 56–120px, ⌘↵ to commit).

use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Subscription, Window};
use recur_ui::guard::Guarded;
use recur_view_chat::suggest::{SuggestAttach, attach_emoji, attach_user_mentions};
use recur_view_core::AppState;

use crate::obs::Observatory;

pub struct Inputs {
    pub query: Entity<InputState>,
    pub replace: Entity<InputState>,
    pub glob: Entity<InputState>,
    pub commit: Entity<TextareaState>,
    pub _attach: (
        Entity<SuggestAttach<gpui_kit::base::input::TextareaMode>>,
        Entity<SuggestAttach<gpui_kit::base::input::TextareaMode>>,
    ),
    pub _subs: Vec<Subscription>,
}

pub const SEARCH_PLACEHOLDER: &str = "Search";
pub const REPLACE_PLACEHOLDER: &str = "Replace";
pub const GLOB_PLACEHOLDER: &str = "Glob (e.g. *.go)";
pub const COMMIT_PLACEHOLDER: &str = "Commit message";

impl Inputs {
    pub fn new(
        state: Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Observatory>,
    ) -> Self {
        let field = |ph: &'static str, window: &mut Window, cx: &mut Context<Observatory>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(SharedString::from(ph)))
        };
        let query = field(SEARCH_PLACEHOLDER, window, cx);
        let replace = field(REPLACE_PLACEHOLDER, window, cx);
        let glob = field(GLOB_PLACEHOLDER, window, cx);
        let commit = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 5)
                .placeholder(SharedString::from(COMMIT_PLACEHOLDER))
        });
        let emoji = attach_emoji(&commit, None, window, cx);
        let mentions = attach_user_mentions(&commit, state, None, window, cx);
        let subs =
            vec![
                cx.guarded_subscribe_in(
                    &query,
                    window,
                    |o, _, ev: &InputEvent, window, cx| match ev {
                        InputEvent::PressEnter { .. } => o.run_search(true, window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    },
                ),
                cx.guarded_subscribe(&replace, |_, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                    }
                }),
                cx.guarded_subscribe(&glob, |_, _, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                    }
                }),
                cx.guarded_subscribe_in(&commit, window, |o, _, ev: &InputEvent, window, cx| {
                    match ev {
                        InputEvent::PressEnter { secondary, .. } if *secondary => {
                            o.human_commit(window, cx);
                        }
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    }
                }),
            ];
        Inputs {
            query,
            replace,
            glob,
            commit,
            _attach: (emoji, mentions),
            _subs: subs,
        }
    }

    pub fn query(&self, cx: &gpui_kit::App) -> String {
        self.query.read(cx).value().to_string()
    }

    pub fn replace(&self, cx: &gpui_kit::App) -> String {
        self.replace.read(cx).value().to_string()
    }

    pub fn glob(&self, cx: &gpui_kit::App) -> String {
        self.glob.read(cx).value().to_string()
    }

    pub fn commit_msg(&self, cx: &gpui_kit::App) -> String {
        self.commit.read(cx).value().to_string()
    }

    /// Focus the commit message box (the SCM panel shows it).
    pub fn focus_commit_msg(&self, window: &mut Window, cx: &mut gpui_kit::App) -> bool {
        self.commit.update(cx, |s, cx| s.focus(window, cx));
        gpui_kit::Focusable::focus_handle(self.commit.read(cx), cx).is_focused(window)
    }

    pub fn set_commit_msg(&self, text: &str, window: &mut Window, cx: &mut gpui_kit::App) {
        let text = text.to_string();
        self.commit
            .update(cx, |s, cx| s.set_value(text, window, cx));
    }
}
