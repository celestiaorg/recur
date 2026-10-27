//! Text-field typeaheads: `lib/emoji.ts` (`:name` shortcodes, expanded as you type) with
//! `EmojiSuggest.svelte`, and `lib/user-mention.ts` (GitHub `@login`) with
//! `UserMentionSuggest.svelte`.
//!
//! A host attaches a typeahead to its gpui-base input or textarea with [`attach_emoji`] /
//! [`attach_user_mentions`] and keeps the returned entity alive. The open menu lives in the
//! app-wide [`SuggestStore`]; the shell paints it through the [`SuggestOverlay`]s registered
//! in the `EmojiSuggest` / `UserMentionSuggest` overlay slots. Carets are byte offsets.

use recur_ui::guard::Guarded;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::Duration;

use gpui_kit::base::input::{InputBaseState, InputEvent, InputModeKind, Position};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EntityId, Focusable, Global, InteractiveElement,
    IntoElement, KeystrokeEvent, MouseMoveEvent, ObjectFit, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, StyledImage, Subscription,
    Task, Window, div, img, px,
};
use recur_content::emoji::{expand_emoji_shortcodes_at, shortcode_entries};
use recur_domain::{PrParticipant, PrSource};
use recur_engine::view::mentions::mention_query_at;
use recur_forge::Forge;
use recur_forge::github::GitHub;
use recur_forge::view::{
    PrPerson, mention_login, mention_people, merge_mention_hits, participant_avatar_url,
    participant_initials, participant_label, suggest_user_mentions,
};
use recur_ui::fonts::Family;
use recur_ui::logic::fuzzy::{FuzzyItem, fuzzy_filter_prepared, prepare_fuzzy};
use recur_ui::logic::menu_space::{MENU_EDGE_PX, clamp_menu_left};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, Theme, Tokens, hex, rgba};
use recur_view_core::AppState;

use recur_ui::logic::hover_gate::HoverGate;
use recur_ui::set::hsla;

const MENU_WIDTH: f32 = 240.;
const MENU_GAP: f32 = 4.;
const LIMIT: usize = 8;

/// GitHub's default `:` list (minus `1234`), plus rocket and fire.
pub const DEFAULT_EMOJI_NAMES: [&str; 6] = ["+1", "-1", "100", "1st_place_medal", "rocket", "fire"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiHit {
    pub name: String,
    pub emoji: String,
}

/// `emojiQueryAt`: the in-progress `:name` ending at `caret` (its `:` offset and the name so
/// far). An empty query is a lone `:` at a word boundary.
pub fn emoji_query_at(text: &str, caret: usize) -> Option<(usize, String)> {
    if caret < 1 || caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let name = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'+' || b == b'-';
    let bytes = text.as_bytes();
    let mut i = caret;
    while i > 0 && name(bytes[i - 1]) {
        i -= 1;
    }
    if i == 0 || bytes[i - 1] != b':' {
        return None;
    }
    let colon = i - 1;
    if let Some(prev) = text[..colon].chars().next_back()
        && (prev.is_ascii_alphanumeric() || prev == '_' || prev == ':')
    {
        return None;
    }
    Some((colon, text[i..caret].to_string()))
}

fn prepared_shortcodes() -> &'static [FuzzyItem<EmojiHit>] {
    static PREPARED: LazyLock<Vec<FuzzyItem<EmojiHit>>> = LazyLock::new(|| {
        let hits: Vec<EmojiHit> = shortcode_entries()
            .iter()
            .map(|(name, emoji)| EmojiHit {
                name: name.clone(),
                emoji: emoji.clone(),
            })
            .collect();
        prepare_fuzzy(&hits, |h| h.name.clone())
    });
    &PREPARED
}

/// `suggestEmoji`.
pub fn suggest_emoji(query: &str, limit: usize) -> Vec<EmojiHit> {
    let q = query.trim();
    if q.is_empty() {
        return DEFAULT_EMOJI_NAMES
            .iter()
            .filter_map(|n| {
                recur_content::emoji::emoji_for_shortcode(n).map(|e| EmojiHit {
                    name: n.to_string(),
                    emoji: e.to_string(),
                })
            })
            .collect();
    }
    fuzzy_filter_prepared(prepared_shortcodes(), q, limit)
        .into_iter()
        .map(|h| h.item)
        .collect()
}

/// `suggestUserMentions` with the UI's fuzzy ranker.
pub fn rank_user_mentions(people: &[PrPerson], query: &str, limit: usize) -> Vec<PrPerson> {
    suggest_user_mentions(people, query, limit, |labels, q, limit| {
        let idx: Vec<usize> = (0..labels.len()).collect();
        fuzzy_filter_prepared(&prepare_fuzzy(&idx, |i| labels[*i].clone()), q, limit)
            .into_iter()
            .map(|h| h.item)
            .collect()
    })
}

/// `ghPRMentionPeople`: the selected GitHub PR's people with a login.
pub fn gh_pr_mention_people(state: &AppState) -> Vec<PrPerson> {
    match state.pr(&state.selected_pr_id) {
        Some(pr) if pr.source == PrSource::GitHub => mention_people(pr),
        _ => Vec::new(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuggestItem {
    Emoji(EmojiHit),
    Person(PrPerson),
}

impl SuggestItem {
    fn login(&self) -> String {
        match self {
            SuggestItem::Person(p) => mention_login(p).to_lowercase(),
            SuggestItem::Emoji(h) => h.name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SuggestKind {
    Emoji,
    Mention,
}

type BoundsFn = Rc<dyn Fn(&App) -> Option<Bounds<Pixels>>>;
type ApplyFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// One open typeahead (`$emojiSuggest` / `$userMentionSuggest`).
#[derive(Clone)]
pub struct Suggest {
    pub owner: EntityId,
    pub start: usize,
    pub query: String,
    pub items: Vec<SuggestItem>,
    pub sel: usize,
    caret: BoundsFn,
    apply: ApplyFn,
}

/// The open typeaheads, one per kind.
#[derive(Default)]
pub struct SuggestStore {
    pub emoji: Option<Suggest>,
    pub mention: Option<Suggest>,
}

struct GlobalSuggest(Entity<SuggestStore>);

impl Global for GlobalSuggest {}

impl SuggestStore {
    pub fn global(cx: &mut App) -> Entity<SuggestStore> {
        if !cx.has_global::<GlobalSuggest>() {
            let store = cx.new(|_| SuggestStore::default());
            cx.set_global(GlobalSuggest(store));
        }
        cx.global::<GlobalSuggest>().0.clone()
    }

    pub fn slot(&self, kind: SuggestKind) -> Option<&Suggest> {
        match kind {
            SuggestKind::Emoji => self.emoji.as_ref(),
            SuggestKind::Mention => self.mention.as_ref(),
        }
    }

    fn slot_mut(&mut self, kind: SuggestKind) -> &mut Option<Suggest> {
        match kind {
            SuggestKind::Emoji => &mut self.emoji,
            SuggestKind::Mention => &mut self.mention,
        }
    }

    fn close_for(&mut self, kind: SuggestKind, owner: EntityId, cx: &mut Context<Self>) {
        let slot = self.slot_mut(kind);
        if slot.as_ref().is_some_and(|s| s.owner == owner) {
            *slot = None;
            cx.notify();
        }
    }

    fn set_sel(&mut self, kind: SuggestKind, sel: usize, cx: &mut Context<Self>) {
        if let Some(s) = self.slot_mut(kind)
            && s.sel != sel
            && sel < s.items.len()
        {
            s.sel = sel;
            cx.notify();
        }
    }

    /// Runs the open menu's pick for row `i`.
    pub fn pick(
        store: &Entity<SuggestStore>,
        kind: SuggestKind,
        i: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(apply) = store.read(cx).slot(kind).map(|s| s.apply.clone()) else {
            return;
        };
        apply(i, window, cx);
    }
}

fn position_of(text: &str, byte: usize) -> Position {
    let before = &text[..byte.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() as u32;
    Position::new(line, col)
}

/// Replace `value[start..end]` with `insert`, put the caret after it and focus the field.
fn splice<M: InputModeKind>(
    input: &Entity<InputBaseState<M>>,
    start: usize,
    end: usize,
    insert: &str,
    on_set: &Option<SetFn>,
    window: &mut Window,
    cx: &mut App,
) {
    let value = input.read(cx).value().to_string();
    let (start, end) = (start.min(value.len()), end.min(value.len()).max(start));
    let next = format!("{}{insert}{}", &value[..start], &value[end..]);
    let pos = position_of(&next, start + insert.len());
    input.update(cx, |s, cx| {
        s.set_value(next.clone(), window, cx);
        s.set_cursor_position(pos, window, cx);
    });
    if let Some(f) = on_set {
        f(&next, window, cx);
    }
}

pub type SetFn = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// The per-field typeahead state (`escaped`, the remote search sequence).
pub struct SuggestAttach<M: InputModeKind> {
    kind: SuggestKind,
    input: Entity<InputBaseState<M>>,
    on_set: Option<SetFn>,
    store: Entity<SuggestStore>,
    state: Option<Entity<AppState>>,
    escaped: Option<(usize, String)>,
    synced: Option<(String, usize)>,
    seq: u64,
    search: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

/// `emojiShortcodes(set)`: expand `:name:` as you type and offer a typeahead for `:partial`.
/// `on_set` hears the new value whenever the typeahead rewrites the field.
pub fn attach_emoji<M: InputModeKind>(
    input: &Entity<InputBaseState<M>>,
    on_set: Option<SetFn>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SuggestAttach<M>> {
    SuggestAttach::new(SuggestKind::Emoji, input, on_set, None, window, cx)
}

/// `userMentions(set)`: the GitHub `@login` typeahead over the selected PR's people, merged
/// with `SearchPRMentionableUsers` after 150ms.
pub fn attach_user_mentions<M: InputModeKind>(
    input: &Entity<InputBaseState<M>>,
    state: Entity<AppState>,
    on_set: Option<SetFn>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SuggestAttach<M>> {
    SuggestAttach::new(SuggestKind::Mention, input, on_set, Some(state), window, cx)
}

impl<M: InputModeKind> SuggestAttach<M> {
    fn new(
        kind: SuggestKind,
        input: &Entity<InputBaseState<M>>,
        on_set: Option<SetFn>,
        state: Option<Entity<AppState>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let store = SuggestStore::global(cx);
        cx.new(|cx| {
            let mut subs = vec![cx.guarded_subscribe_in(
                input,
                window,
                |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => this.on_input(window, cx),
                    InputEvent::Blur => {
                        let input = this.input.clone();
                        let owner = input.entity_id();
                        let store = this.store.clone();
                        let kind = this.kind;
                        window.defer(cx, move |window, cx| {
                            if input.read(cx).focus_handle(cx).is_focused(window) {
                                return;
                            }
                            store.update(cx, |s, cx| s.close_for(kind, owner, cx));
                        });
                        this.seq += 1;
                        this.search = None;
                    }
                    _ => {}
                },
            )];
            if kind == SuggestKind::Mention {
                // `selectionchange` while open: follow the caret.
                subs.push(
                    cx.guarded_observe_in(input, window, |this, input, window, cx| {
                        let owner = input.entity_id();
                        if this
                            .store
                            .read(cx)
                            .slot(this.kind)
                            .is_none_or(|s| s.owner != owner)
                        {
                            return;
                        }
                        this.sync(window, cx);
                    }),
                );
            }
            let me = cx.entity().downgrade();
            subs.push(
                cx.intercept_keystrokes(move |ev: &KeystrokeEvent, window, cx| {
                    let Some(me) = me.upgrade() else { return };
                    if me.update(cx, |a, cx| a.on_key(ev, window, cx)) {
                        cx.stop_propagation();
                    }
                }),
            );
            SuggestAttach {
                kind,
                input: input.clone(),
                on_set,
                store,
                state,
                escaped: None,
                synced: None,
                seq: 0,
                search: None,
                _subs: subs,
            }
        })
    }

    fn owner(&self) -> EntityId {
        self.input.entity_id()
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let (kind, owner) = (self.kind, self.owner());
        self.store.update(cx, |s, cx| s.close_for(kind, owner, cx));
    }

    fn on_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.kind == SuggestKind::Emoji {
            let (value, caret) = {
                let s = self.input.read(cx);
                (s.value().to_string(), s.cursor())
            };
            if let Some((text, caret)) = expand_emoji_shortcodes_at(&value, caret) {
                let pos = position_of(&text, caret);
                self.input.update(cx, |s, cx| {
                    s.set_value(text.clone(), window, cx);
                    s.set_cursor_position(pos, window, cx);
                });
                if let Some(f) = &self.on_set {
                    f(&text, window, cx);
                }
            }
        }
        self.synced = None;
        self.sync(window, cx);
    }

    /// `syncSuggest(value, caret)`.
    fn sync(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (value, caret) = {
            let s = self.input.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let key = (value.clone(), caret);
        if self.synced.as_ref() == Some(&key) {
            return;
        }
        self.synced = Some(key);
        let q = match self.kind {
            SuggestKind::Emoji => emoji_query_at(&value, caret),
            SuggestKind::Mention => mention_query_at(&value, caret).map(|m| (m.start, m.query)),
        };
        let Some((start, query)) = q.filter(|q| self.escaped.as_ref() != Some(q)) else {
            self.seq += 1;
            self.search = None;
            self.close(cx);
            return;
        };
        self.escaped = None;
        match self.kind {
            SuggestKind::Emoji => {
                let hits = suggest_emoji(&query, LIMIT)
                    .into_iter()
                    .map(SuggestItem::Emoji)
                    .collect();
                self.show(start, &query, hits, cx);
            }
            SuggestKind::Mention => {
                let Some(state) = self.state.clone() else {
                    return;
                };
                let people = gh_pr_mention_people(state.read(cx));
                let local = rank_user_mentions(&people, &query, LIMIT);
                self.show(
                    start,
                    &query,
                    local.into_iter().map(SuggestItem::Person).collect(),
                    cx,
                );
                self.seq += 1;
                let seq = self.seq;
                let pr_id = state.read(cx).selected_pr_id.clone();
                if pr_id.is_empty() {
                    self.search = None;
                    return;
                }
                let services = state.read(cx).services().clone();
                self.search = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let Ok(remote) = services
                        .prs
                        .search_pr_mentionable_users(&pr_id, &query)
                        .await
                    else {
                        return;
                    };
                    let _ = this.update(cx, |a, cx| {
                        if a.seq != seq {
                            return;
                        }
                        let (value, caret) = {
                            let s = a.input.read(cx);
                            (s.value().to_string(), s.cursor())
                        };
                        match mention_query_at(&value, caret) {
                            Some(now) if now.start == start && now.query == query => {}
                            _ => return,
                        }
                        let remote: Vec<PrPerson> =
                            remote.iter().map(|p: &PrParticipant| p.into()).collect();
                        let Some(state) = a.state.clone() else { return };
                        let local = rank_user_mentions(
                            &gh_pr_mention_people(state.read(cx)),
                            &query,
                            LIMIT,
                        );
                        let hits = merge_mention_hits(&local, &remote, LIMIT);
                        a.show(
                            start,
                            &query,
                            hits.into_iter().map(SuggestItem::Person).collect(),
                            cx,
                        );
                    });
                }));
            }
        }
    }

    /// `showHits`: open or refresh the menu, keeping the selection on the same row.
    fn show(&mut self, start: usize, query: &str, items: Vec<SuggestItem>, cx: &mut Context<Self>) {
        if items.is_empty() {
            self.close(cx);
            return;
        }
        let owner = self.owner();
        let kind = self.kind;
        let cur = self
            .store
            .read(cx)
            .slot(kind)
            .filter(|s| s.owner == owner && s.query == query && s.start == start);
        let sel = match (cur, kind) {
            (None, _) => 0,
            (Some(c), SuggestKind::Emoji) => c.sel.min(items.len() - 1),
            (Some(c), SuggestKind::Mention) => {
                let login = c
                    .items
                    .get(c.sel)
                    .map(SuggestItem::login)
                    .unwrap_or_default();
                items
                    .iter()
                    .position(|h| !login.is_empty() && h.login() == login)
                    .unwrap_or(0)
            }
        };
        let weak = self.input.downgrade();
        let caret: BoundsFn = Rc::new(move |cx: &App| {
            weak.upgrade()
                .and_then(|i| i.read(cx).range_to_bounds(&(start..start)))
        });
        let me = cx.entity().downgrade();
        let apply: ApplyFn = Rc::new(move |i, window, cx| {
            let _ = me.update(cx, |a, cx| a.apply(i, window, cx));
        });
        let next = Suggest {
            owner,
            start,
            query: query.to_string(),
            items,
            sel,
            caret,
            apply,
        };
        self.store.update(cx, |s, cx| {
            *s.slot_mut(kind) = Some(next);
            cx.notify();
        });
    }

    /// `applyEmojiSuggestion` / `applyUserMention`.
    fn apply(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self
            .store
            .read(cx)
            .slot(self.kind)
            .filter(|s| s.owner == self.owner())
            .cloned()
        else {
            return;
        };
        let Some(hit) = open.items.get(i).cloned() else {
            return;
        };
        let (value, caret) = {
            let s = self.input.read(cx);
            (s.value().to_string(), s.cursor())
        };
        match hit {
            SuggestItem::Emoji(h) => {
                self.close(cx);
                splice(
                    &self.input,
                    open.start,
                    caret,
                    &h.emoji,
                    &self.on_set,
                    window,
                    cx,
                );
            }
            SuggestItem::Person(p) => {
                let login = mention_login(&p).to_string();
                if login.is_empty() {
                    return;
                }
                self.close(cx);
                let Some(now) = mention_query_at(&value, caret)
                    .filter(|n| n.start == open.start && n.query == open.query)
                else {
                    return;
                };
                splice(
                    &self.input,
                    now.start,
                    now.end,
                    &format!("@{login} "),
                    &self.on_set,
                    window,
                    cx,
                );
            }
        }
        self.synced = None;
    }

    /// The capture-phase `keydown` while this field's menu is open.
    fn on_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let owner = self.owner();
        let Some(open) = self
            .store
            .read(cx)
            .slot(self.kind)
            .filter(|s| s.owner == owner)
            .cloned()
        else {
            return false;
        };
        if !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        let k = &ev.keystroke;
        let m = k.modifiers;
        let kind = self.kind;
        match k.key.as_str() {
            "down" => {
                let sel = (open.sel + 1).min(open.items.len() - 1);
                self.store.update(cx, |s, cx| s.set_sel(kind, sel, cx));
                true
            }
            "up" => {
                let sel = open.sel.saturating_sub(1);
                self.store.update(cx, |s, cx| s.set_sel(kind, sel, cx));
                true
            }
            "enter" | "tab" if !m.platform && !m.control => {
                if open.sel >= open.items.len() {
                    return false;
                }
                self.apply(open.sel, window, cx);
                true
            }
            "escape" => {
                self.seq += 1;
                self.search = None;
                self.escaped = Some((open.start, open.query.clone()));
                self.close(cx);
                true
            }
            _ => false,
        }
    }
}

/// `menuViewportSpace` with the typeahead's 4px gap.
fn space(top: f32, bottom: f32, up: bool, vh: f32) -> f32 {
    let s = if up {
        top - MENU_GAP - MENU_EDGE_PX
    } else {
        vh - bottom - MENU_GAP - MENU_EDGE_PX
    };
    s.floor().max(0.)
}

/// `.pr-face`'s inline `avatarCss`: a hex or two-stop `linear-gradient` background and a hex or
/// token text color.
// simplify: only the forms the backend emits (`background:` hex / linear-gradient, `color:` hex
// or var(--token)); a general CSS parser would be needed for anything else.
fn face_style(css: &str, t: &Tokens) -> (Option<(f32, Color, Color)>, Option<Color>) {
    let color_of = |v: &str| -> Option<Color> {
        let v = v.trim();
        if let Some(h) = v.strip_prefix('#') {
            let n = u32::from_str_radix(h, 16).ok()?;
            return Some(match h.len() {
                3 => hex(((((n >> 8) & 0xf) * 0x11) << 16)
                    | ((((n >> 4) & 0xf) * 0x11) << 8)
                    | ((n & 0xf) * 0x11)),
                6 => hex(n),
                _ => return None,
            });
        }
        match v.strip_prefix("var(--")?.strip_suffix(')')? {
            "muted" => Some(t.muted),
            "text" => Some(t.text),
            "faint" => Some(t.faint),
            "amber" => Some(t.amber),
            "cyan" => Some(t.cyan),
            _ => None,
        }
    };
    let (mut bg, mut fg) = (None, None);
    for decl in css.split(';') {
        let Some((k, v)) = decl.split_once(':') else {
            continue;
        };
        match k.trim() {
            "background" | "background-image" | "background-color" => {
                let v = v.trim();
                if let Some(args) = v
                    .strip_prefix("linear-gradient(")
                    .and_then(|a| a.strip_suffix(')'))
                {
                    let parts: Vec<&str> = args.split(',').collect();
                    if let [a, c1, c2] = parts.as_slice()
                        && let Some(deg) = a
                            .trim()
                            .strip_suffix("deg")
                            .and_then(|d| d.parse::<f32>().ok())
                        && let (Some(c1), Some(c2)) = (color_of(c1), color_of(c2))
                    {
                        bg = Some((deg, c1, c2));
                    }
                } else if let Some(c) = color_of(v) {
                    bg = Some((180., c, c));
                }
            }
            "color" => fg = color_of(v),
            _ => {}
        }
    }
    (bg, fg)
}

static GITHUB: LazyLock<GitHub> = LazyLock::new(GitHub::default);

/// `.pr-face` at 20px: initials under the photo.
fn pr_face(p: &PrPerson, t: &Tokens) -> impl IntoElement {
    let url = participant_avatar_url(p, &*GITHUB as &dyn Forge);
    let (bg, fg) = face_style(&p.avatar_css, t);
    div()
        .relative()
        .flex_none()
        .size(px(20.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .map(|d| match bg {
            Some((deg, a, b)) => d.child(recur_ui::style::css_gradient(
                deg,
                (a, 0.),
                (b, 1.),
                10.,
                0.,
                recur_ui::color::TRANSPARENT,
            )),
            None => d.bg(hsla(t.pane_2)),
        })
        .child(
            text(participant_initials(p))
                .family(Family::Mono)
                .size(8.)
                .weight(700.)
                .line_height(LineHeight::Px(8.))
                .color(fg.unwrap_or(t.muted)),
        )
        .when(!url.is_empty(), |d| {
            d.child(
                img(SharedString::from(url))
                    .absolute()
                    .inset_0()
                    .size_full()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
}

/// `EmojiSuggest.svelte` / `UserMentionSuggest.svelte`: the open typeahead, fixed under (or
/// over) the caret.
pub struct SuggestOverlay {
    kind: SuggestKind,
    store: Entity<SuggestStore>,
    hover: HoverGate,
    scroll: ScrollHandle,
    shown: Option<(EntityId, usize, usize)>,
    _sub: Subscription,
}

impl SuggestOverlay {
    pub fn new(kind: SuggestKind, cx: &mut Context<Self>) -> Self {
        let store = SuggestStore::global(cx);
        let _sub = cx.guarded_observe(&store, |_, _, cx| cx.notify());
        SuggestOverlay {
            kind,
            store,
            hover: HoverGate::default(),
            scroll: ScrollHandle::new(),
            shown: None,
            _sub,
        }
    }
}

impl SuggestOverlay {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let Some(open) = self.store.read(cx).slot(self.kind).cloned() else {
            self.shown = None;
            return div().into_any_element();
        };
        let Some(caret) = (open.caret)(cx) else {
            return div().into_any_element();
        };
        let key = (open.owner, open.start, open.sel);
        if self
            .shown
            .is_none_or(|(o, s, _)| (o, s) != (open.owner, open.start))
        {
            self.hover.reset();
        }
        if self.shown != Some(key) {
            self.scroll.scroll_to_item(open.sel);
        }
        self.shown = Some(key);
        let t = Theme::of(cx).tokens;
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let (top, bottom, left) = (
            f32::from(caret.top()),
            f32::from(caret.bottom()),
            f32::from(caret.left()),
        );
        let up = space(top, bottom, true, vh) > space(top, bottom, false, vh);
        let room = space(top, bottom, up, vh);
        let l = clamp_menu_left(left, MENU_WIDTH, vw);
        let kind = self.kind;
        let rows = open.items.iter().enumerate().map(|(i, h)| {
            let on = i == open.sel;
            let mut row = div()
                .id(("suggest-row", i))
                .debug_selector(|| {
                    format!(
                        "{}-row:{i}",
                        if kind == SuggestKind::Emoji {
                            "emoji-suggest"
                        } else {
                            "user-mention"
                        }
                    )
                })
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .px(px(8.))
                .py(px(7.))
                .rounded(px(7.))
                .text_size(px(12.))
                .text_color(hsla(t.text))
                .cursor_pointer()
                .when(on, |d| d.bg(hsla(t.pane_2)));
            match h {
                SuggestItem::Emoji(e) => {
                    row = row
                        .child(
                            div().flex_none().w(px(22.)).flex().justify_center().child(
                                text(e.emoji.clone())
                                    .size(16.)
                                    .line_height(LineHeight::Px(16.)),
                            ),
                        )
                        .child(
                            text(e.name.clone())
                                .weight(500.)
                                .line_height(LineHeight::Normal)
                                .nowrap(),
                        );
                }
                SuggestItem::Person(p) => {
                    let login = mention_login(p).to_string();
                    let name = p.name.trim().to_string();
                    row = row
                        .child(pr_face(p, &t))
                        .child(
                            text(participant_label(p))
                                .weight(500.)
                                .line_height(LineHeight::Normal)
                                .nowrap(),
                        )
                        .when(
                            !name.is_empty() && name.to_lowercase() != login.to_lowercase(),
                            |d| {
                                d.child(
                                    div()
                                        .ml_auto()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .pl(px(8.))
                                        .child(
                                            text(name)
                                                .size(11.)
                                                .color(t.muted)
                                                .line_height(LineHeight::Normal)
                                                .truncate(),
                                        ),
                                )
                            },
                        );
                }
            }
            let store = self.store.clone();
            row.on_mouse_move(cx.guarded_listener(move |o, ev: &MouseMoveEvent, _, cx| {
                if o.hover
                    .allows(f32::from(ev.position.x), f32::from(ev.position.y))
                {
                    o.store.update(cx, |s, cx| s.set_sel(kind, i, cx));
                }
            }))
            .on_click(move |_, window, cx| SuggestStore::pick(&store, kind, i, window, cx))
        });
        let list = div()
            .id(match kind {
                SuggestKind::Emoji => "emoji-suggest",
                SuggestKind::Mention => "user-mention",
            })
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .w(px(MENU_WIDTH))
            .max_h(px(220f32.min(room)))
            .p(px(4.))
            .flex()
            .flex_col()
            .children(rows);
        let card = recur_ui::style::card(t.pane, t.line_bright, 10.).child(list);
        let card = with_shadow(
            card,
            &[css_box_shadow(0., 14., 36., -12., rgba(0, 0, 0, 0.7))],
            10.,
        )
        .occlude();
        let placed = div().absolute().left(px(l)).child(card);
        let placed = if up {
            placed.bottom(px(vh - top + MENU_GAP))
        } else {
            placed.top(px(bottom + MENU_GAP))
        };
        gpui_kit::deferred(div().absolute().inset_0().child(placed))
            .with_priority(60)
            .into_any_element()
    }
}

impl Render for SuggestOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use recur_content::emoji::{emoji_for_shortcode, expand_emoji_shortcodes};

    // emoji.test.ts
    #[test]
    fn resolves_github_aliases() {
        assert_eq!(emoji_for_shortcode("rocket"), Some("🚀"));
        assert_eq!(emoji_for_shortcode("+1"), Some("👍"));
        assert_eq!(emoji_for_shortcode("thumbsup"), Some("👍"));
        assert_eq!(emoji_for_shortcode("smile"), Some("😄"));
    }

    #[test]
    fn expands_completed_tokens_and_leaves_the_rest() {
        assert_eq!(
            expand_emoji_shortcodes("LGTM :rocket: :+1:", false),
            "LGTM 🚀 👍"
        );
        assert_eq!(
            expand_emoji_shortcodes("see :notanemoji: and :rock", false),
            "see :notanemoji: and :rock"
        );
        let src = "ok :rocket: and `:rocket:`\n```\n:smile:\n```";
        assert_eq!(
            expand_emoji_shortcodes(src, true),
            "ok 🚀 and `:rocket:`\n```\n:smile:\n```"
        );
    }

    #[test]
    fn expansion_keeps_the_caret_after_the_same_point() {
        assert_eq!(
            expand_emoji_shortcodes_at("hi :rocket:", 11),
            Some(("hi 🚀".into(), "hi 🚀".len()))
        );
        let src = ":rocket: later";
        assert_eq!(
            expand_emoji_shortcodes_at(src, src.len()),
            Some(("🚀 later".into(), "🚀 later".len()))
        );
    }

    #[test]
    fn detects_an_in_progress_query_at_the_caret() {
        assert_eq!(emoji_query_at("LGTM :roc", 9), Some((5, "roc".into())));
        assert_eq!(emoji_query_at("LGTM :roc", 5), None);
        assert_eq!(emoji_query_at("see:roc", 7), None);
        assert_eq!(emoji_query_at("http://x", 8), None);
        assert_eq!(emoji_query_at(":", 1), Some((0, String::new())));
        assert_eq!(emoji_query_at("LGTM :", 6), Some((5, String::new())));
        assert_eq!(emoji_query_at("(:", 2), Some((1, String::new())));
        assert_eq!(emoji_query_at("see:", 4), None);
        assert_eq!(emoji_query_at("::ro", 4), None);
    }

    #[test]
    fn ranks_shortcodes_and_offers_defaults() {
        let hits = suggest_emoji("rock", LIMIT);
        assert_eq!(hits[0].name, "rock");
        assert!(hits.iter().any(|h| h.name == "rocket" && h.emoji == "🚀"));
        assert_eq!(
            suggest_emoji("rocket", LIMIT)[0],
            EmojiHit {
                name: "rocket".into(),
                emoji: "🚀".into()
            }
        );
        let d = suggest_emoji("", LIMIT);
        assert_eq!(
            d.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
            DEFAULT_EMOJI_NAMES
        );
        assert_eq!(
            suggest_emoji(" ", LIMIT)[0],
            EmojiHit {
                name: "+1".into(),
                emoji: "👍".into()
            }
        );
    }

    // user-mention.test.ts
    #[test]
    fn suggests_people_with_logins() {
        let p = |login: &str, name: &str| PrPerson {
            login: login.into(),
            name: name.into(),
            ..Default::default()
        };
        let people = vec![
            p("avery", "Avery Rourke"),
            p("loopbot", "loopbot"),
            p("sam", "sam"),
            PrPerson {
                name: "Coder".into(),
                code: "CO".into(),
                ..Default::default()
            },
        ];
        let logins = |v: Vec<PrPerson>| v.into_iter().map(|p| p.login).collect::<Vec<_>>();
        assert_eq!(
            logins(rank_user_mentions(&people, "", LIMIT)),
            ["avery", "loopbot", "sam"]
        );
        let many: Vec<PrPerson> = (0..12).map(|i| p(&format!("u{i}"), "")).collect();
        assert_eq!(rank_user_mentions(&many, "", LIMIT).len(), 8);
        assert_eq!(logins(rank_user_mentions(&people, "av", LIMIT)), ["avery"]);
        assert_eq!(
            logins(rank_user_mentions(&people, "rourke", LIMIT)),
            ["avery"]
        );
        assert!(rank_user_mentions(&people, "Coder", LIMIT).is_empty());
        let local = vec![p("avery", "Avery"), p("loopbot", "")];
        let remote = vec![
            p("Avery", "Avery Rourke"),
            p("octocat", "The Octocat"),
            p("hubot", ""),
        ];
        assert_eq!(
            logins(merge_mention_hits(&local, &remote, 3)),
            ["avery", "loopbot", "octocat"]
        );
        assert_eq!(
            logins(merge_mention_hits(&[], &remote, LIMIT)),
            ["Avery", "octocat", "hubot"]
        );
    }

    #[test]
    fn mention_ranking_and_face_css() {
        let people = vec![
            PrPerson {
                login: "avery".into(),
                name: "Avery Rourke".into(),
                ..Default::default()
            },
            PrPerson {
                login: "".into(),
                name: "No Login".into(),
                ..Default::default()
            },
            PrPerson {
                login: "@bo".into(),
                ..Default::default()
            },
        ];
        let all = rank_user_mentions(&people, "", LIMIT);
        assert_eq!(all.len(), 2);
        assert_eq!(rank_user_mentions(&people, "rour", LIMIT)[0].login, "avery");
        let t = recur_ui::ThemeId::default().tokens();
        let (bg, fg) = face_style(
            "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)",
            &t,
        );
        assert_eq!(bg, Some((135., hex(0x39434f), hex(0x1d242c))));
        assert_eq!(fg, Some(t.muted));
    }
}
