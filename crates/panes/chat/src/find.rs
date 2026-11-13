//! Find in the transcript: the preview find bar over user and assistant message text.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{AppContext as _, Context, Entity, Subscription, Task, Window, point, px};
use recur_content::preview::find::{
    FindOptions, PREVIEW_FIND_CAP, collect_preview_find_ranges, find_in_text, reveal_offset,
};
use recur_content::preview::find_bar::{FindBarEvent, PreviewFindBar};
use recur_engine::view::mentions::unique_artifact_id_for_title;
use recur_ui::guard::Guarded;

use crate::blocks::{Block, BubblePart, bubble_parts};
use crate::chat::AgentChat;
use crate::flow::ATOM;

/// A user bubble's text as it lays out: chips are one `ATOM` each.
fn bubble_text(parts: &[BubblePart]) -> String {
    let mut out = String::new();
    for p in parts {
        match p {
            BubblePart::Text(s) => out.push_str(s),
            _ => out.push(ATOM),
        }
    }
    out
}

/// One match: the message, its block key, and its ordinal among the message's matches.
#[derive(Clone)]
pub(crate) struct FindHit {
    msg: usize,
    key: String,
    nth: usize,
    /// Where the match starts in the message text, as a fraction of its length.
    at: f32,
}

pub(crate) struct ChatFind {
    pub(crate) bar: Entity<PreviewFindBar>,
    pub(crate) open: bool,
    pub(crate) hits: Vec<FindHit>,
    /// The current match, 1-based (0: none).
    pub(crate) index: usize,
    /// Markdown keys carrying highlights.
    painted: Vec<String>,
    /// The scroll to the current match; replacing it cancels the last one, which would
    /// otherwise keep pulling the scroller back to its own message. Holds the key it set as
    /// `reveal_key`, so dropping it only clears a `reveal_key` it still owns.
    reveal: Option<(String, Task<()>)>,
    /// The query and toggles the current match was picked for.
    searched: (String, FindOptions),
    _sub: Subscription,
}

impl AgentChat {
    /// ⌘F (`dir` 0) opens or refocuses the bar; ⌘G / ⇧⌘G (`dir` ±1) step while it is open.
    pub fn find(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        if dir != 0 && self.find.as_ref().is_some_and(|f| f.open) {
            return self.step_find(dir, window, cx);
        }
        if self.find.is_none() {
            let bar = cx.new(|cx| PreviewFindBar::new(window, cx));
            let sub =
                cx.guarded_subscribe_in(&bar, window, |c, _, ev: &FindBarEvent, w, cx| match ev {
                    FindBarEvent::Changed => c.refresh_find(w, cx),
                    FindBarEvent::Next => c.step_find(1, w, cx),
                    FindBarEvent::Previous => c.step_find(-1, w, cx),
                    FindBarEvent::Close => {
                        c.close_find(cx);
                        c.focus_composer(w, cx);
                    }
                });
            self.find = Some(ChatFind {
                bar,
                open: false,
                hits: Vec::new(),
                index: 0,
                painted: Vec::new(),
                reveal: None,
                searched: Default::default(),
                _sub: sub,
            });
        }
        let Some(f) = self.find.as_mut() else { return };
        f.open = true;
        f.bar.update(cx, |b, cx| b.focus_and_select(window, cx));
        cx.notify();
        self.refresh_find(window, cx);
    }

    pub fn close_find(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut().filter(|f| f.open) else {
            return;
        };
        f.open = false;
        f.hits.clear();
        f.index = 0;
        f.searched = Default::default();
        f.bar.update(cx, |b, cx| {
            b.reset_counts();
            cx.notify();
        });
        self.cancel_find_reveal();
        self.paint_find(cx);
        self.notify_transcript(cx);
        cx.notify();
    }

    /// Recounts over the model's message bodies, so messages not laid out count too.
    fn collect_find(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_ref().filter(|f| f.open) else {
            return;
        };
        let query = f.bar.read(cx).query(cx);
        let opts: FindOptions = f.bar.read(cx).options();
        let model = self.transcript_model();
        let arts = self.live_artifacts(cx);
        let art_id = |title: &str| {
            unique_artifact_id_for_title(
                title,
                arts.iter().map(|a| (a.id.as_str(), a.title.as_str())),
            )
        };
        let (blocks, keys) = &*model.blocks;
        let mut hits = Vec::new();
        let mut valid = true;
        for (b, key) in blocks.iter().zip(keys) {
            let Block::Msg(i) = *b else { continue };
            let m = &model.msgs[i];
            if m.log.is_some() || m.turn_end || m.note || m.stopped || m.error {
                continue;
            }
            let hay = if m.me {
                bubble_text(&bubble_parts(m, &art_id))
            } else {
                m.body.clone()
            };
            let Some(found) = find_in_text(&hay, &query, opts) else {
                valid = false;
                break;
            };
            hits.extend(found.iter().enumerate().map(|(nth, r)| FindHit {
                msg: i,
                key: key.clone(),
                nth,
                at: r.start as f32 / hay.len().max(1) as f32,
            }));
            if hits.len() >= PREVIEW_FIND_CAP {
                break;
            }
        }
        let capped = hits.len() >= PREVIEW_FIND_CAP;
        hits.truncate(PREVIEW_FIND_CAP);
        let Some(f) = self.find.as_mut() else { return };
        let n = hits.len();
        f.index = if n == 0 { 0 } else { f.index.clamp(1, n) };
        f.hits = hits;
        let index = f.index;
        f.bar
            .update(cx, |b, cx| b.set_results(index, n, capped, valid, cx));
    }

    fn refresh_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.collect_find(cx);
        self.start_find_in_view(cx);
        self.paint_find(cx);
        self.notify_transcript(cx);
        self.reveal_find(window, cx);
    }

    /// A new query starts at the first match at or below the scroller's top (else the last one
    /// above it), so typing does not send the view to the transcript's start on each keystroke.
    fn start_find_in_view(&mut self, cx: &mut Context<Self>) {
        let top = -f32::from(self.scroll.offset().y);
        let Some(f) = self.find.as_ref().filter(|f| !f.hits.is_empty()) else {
            return;
        };
        let bar = f.bar.read(cx);
        let searched = (bar.query(cx), bar.options());
        if searched == f.searched {
            return;
        }
        let n = f.hits.len();
        let below = f.hits.iter().position(|h| {
            self.find_block_range(&h.key)
                .is_some_and(|(y, h)| y + h > top)
        });
        let Some(f) = self.find.as_mut() else { return };
        f.searched = searched;
        f.index = below.unwrap_or(n - 1) + 1;
        let index = f.index;
        f.bar.update(cx, |b, cx| {
            b.set_results(index, n, n >= PREVIEW_FIND_CAP, true, cx)
        });
    }

    fn step_find(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        self.collect_find(cx);
        let Some(f) = self.find.as_mut().filter(|f| !f.hits.is_empty()) else {
            return;
        };
        let n = f.hits.len() as i64;
        f.index = ((f.index as i64 - 1 + dir as i64).rem_euclid(n) + 1) as usize;
        let (index, bar) = (f.index, f.bar.clone());
        let n = n as usize;
        bar.update(cx, |b, cx| {
            b.set_results(index, n, n >= PREVIEW_FIND_CAP, true, cx)
        });
        self.paint_find(cx);
        self.notify_transcript(cx);
        self.reveal_find(window, cx);
    }

    fn current_hit(&self) -> Option<FindHit> {
        let f = self.find.as_ref()?;
        f.hits.get(f.index.checked_sub(1)?).cloned()
    }

    fn md_key(&self, msg: usize) -> String {
        format!("{}:m{msg}-md", self.props.reset_key)
    }

    /// The rendered match ranges in a settled reply's markdown, by text block.
    fn rendered_ranges(
        &self,
        msg: usize,
        cx: &Context<Self>,
    ) -> Vec<(usize, std::ops::Range<usize>)> {
        let (Some(f), Some(v)) = (self.find.as_ref(), self.markdown.get(&self.md_key(msg))) else {
            return Vec::new();
        };
        let bar = f.bar.read(cx);
        collect_preview_find_ranges(&v.read(cx).text_blocks(), &bar.query(cx), bar.options()).ranges
    }

    /// Highlights the matches in every settled reply that has rendered.
    // simplify: counts come from the raw message text and highlights from the rendered text, so
    // markup inside a match (`**a**b`) can make the two disagree; the current highlight is the
    // rendered match at the same ordinal. Streaming replies get no highlight; user bubbles mark
    // theirs as they render (`bubble_find_marks`).
    fn paint_find(&mut self, cx: &mut Context<Self>) {
        let current = self.current_hit();
        let mut msgs: Vec<usize> = self
            .find
            .as_ref()
            .filter(|f| f.open)
            .map(|f| f.hits.iter().map(|h| h.msg).collect())
            .unwrap_or_default();
        msgs.dedup();
        let mut painted = Vec::new();
        for msg in msgs {
            let ranges = self.rendered_ranges(msg, cx);
            let Some(v) = self.markdown.get(&self.md_key(msg)).cloned() else {
                continue;
            };
            let cur = current
                .as_ref()
                .filter(|h| h.msg == msg)
                .map(|h| h.nth.min(ranges.len().saturating_sub(1)));
            let mut map: HashMap<usize, Vec<(std::ops::Range<usize>, bool)>> = HashMap::new();
            for (i, (id, r)) in ranges.into_iter().enumerate() {
                map.entry(id).or_default().push((r, Some(i) == cur));
            }
            v.update(cx, |v, cx| v.set_highlights(map, cx));
            painted.push(self.md_key(msg));
        }
        let Some(f) = self.find.as_mut() else { return };
        for key in std::mem::replace(&mut f.painted, painted.clone()) {
            if !painted.contains(&key)
                && let Some(v) = self.markdown.get(&key)
            {
                v.update(cx, |v, cx| v.set_highlights(HashMap::new(), cx));
            }
        }
    }

    /// Repaints the highlights once a reply with matches that just got a fresh markdown view
    /// (a windowed row hydrating again) has built its text blocks, after this frame.
    pub(crate) fn paint_find_into(&mut self, key: &str, cx: &mut Context<Self>) {
        let has_hits = self
            .find
            .as_ref()
            .filter(|f| f.open)
            .is_some_and(|f| f.hits.iter().any(|h| self.md_key(h.msg) == key));
        if has_hits {
            let this = cx.entity();
            cx.defer(move |cx| this.update(cx, |c, cx| c.paint_find(cx)));
        }
    }

    /// Centres the current match. A message the transcript has stubbed out is hydrated first
    /// (`reveal_key`), then each painted frame moves the scroller by how far the match sits from
    /// the middle until it stops moving. The moves are relative to where the match painted, so
    /// they compose with `hold_anchor` instead of fighting it.
    fn cancel_find_reveal(&mut self) {
        if let Some((key, _)) = self.find.as_mut().and_then(|f| f.reveal.take())
            && self.reveal_key == key
        {
            self.reveal_key.clear();
        }
    }

    fn reveal_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_find_reveal();
        let Some(hit) = self.current_hit() else {
            return;
        };
        let key = hit.key;
        if self.find_step(&key, window, cx) {
            return;
        }
        self.reveal_key = key.clone();
        self.notify_transcript(cx);
        let owned = key.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            for _ in 0..120 {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let done = this.update_in(cx, |c, window, cx| {
                    c.reveal_key != key || c.find_step(&key, window, cx)
                });
                if done.unwrap_or(true) {
                    break;
                }
            }
            let _ = this.update(cx, |c, _| {
                if c.reveal_key == key {
                    c.reveal_key.clear();
                }
            });
        });
        if let Some(f) = self.find.as_mut() {
            f.reveal = Some((owned, task));
        }
    }

    /// One reveal step; true once the match (or its message, when it has no painted match)
    /// sits centred.
    fn find_step(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let top = -f32::from(self.scroll.offset().y);
        let painted = (self.seen_scroll_top - top).abs() < 0.5;
        let live = !self.window_chat() || self.chat_hydrated.contains(key);
        if !painted || !live || !self.block_ranges.borrow().contains_key(key) {
            self.notify_transcript(cx);
            return false;
        }
        self.paint_find(cx);
        let moved = match self.centre_find(window, cx) {
            Some(moved) => moved,
            None => self.centre_block(key, cx),
        };
        !moved
    }

    /// Centres a message whose match has not painted (a user bubble, a streaming reply) unless
    /// it is already in view.
    // simplify: the match's line is estimated from its offset in the text, not measured, so a
    // tall bubble can land a few lines off. Measuring needs bounds from the bubble's flow.
    fn centre_block(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let (Some((y, h)), Some(hit)) = (self.find_block_range(key), self.current_hit()) else {
            return false;
        };
        let line = y + hit.at * h;
        let top = -f32::from(self.scroll.offset().y);
        let margin = (self.viewport_h / 4.).min(80.);
        if line >= top + margin && line <= top + self.viewport_h - margin {
            return false;
        }
        self.scroll_find_to(line - self.viewport_h / 2., cx)
    }

    /// A block's top in the scroll content and its height, from the last frame (`block_ranges`
    /// holds window coordinates).
    fn find_block_range(&self, key: &str) -> Option<(f32, f32)> {
        let &(y, h) = self.block_ranges.borrow().get(key)?;
        Some((y - self.column_y, h))
    }

    fn scroll_find_to(&mut self, target: f32, cx: &mut Context<Self>) -> bool {
        let top = -f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y).max(0.);
        let target = target.clamp(0., max);
        if (target - top).abs() < 1. {
            return false;
        }
        self.near_bottom = false;
        self.pending_scroll = None;
        self.scroll.set_offset(point(px(0.), px(-target)));
        self.last_scroll_top = target;
        self.notify_transcript(cx);
        true
    }

    /// Moves the current match to the middle of the scroller unless it is already in view:
    /// `None` when it has not painted, else whether the scroller moved.
    fn centre_find(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<bool> {
        let hit = self.current_hit()?;
        let ranges = self.rendered_ranges(hit.msg, cx);
        let (id, r) = ranges
            .get(hit.nth.min(ranges.len().saturating_sub(1)))
            .cloned()?;
        let b = self
            .markdown
            .get(&self.md_key(hit.msg))
            .and_then(|v| v.read(cx).range_bounds(id, r, window))?;
        let view = self.scroll.bounds();
        if b.top() >= view.top() && b.bottom() <= view.bottom() {
            return Some(false);
        }
        let top = -f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y);
        let mid = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| f32::from(a + b) / 2.;
        let target = reveal_offset(
            top,
            max,
            mid(b.top(), b.bottom()),
            mid(view.top(), view.bottom()),
        );
        Some(self.scroll_find_to(target, cx))
    }

    /// Find marks for a user bubble's laid-out `text` in message `msg`, whose earlier text
    /// already held `before` matches; also returns how many matches `text` holds.
    pub(crate) fn bubble_find_marks(
        &self,
        text: &str,
        msg: usize,
        before: usize,
        cx: &gpui_kit::App,
    ) -> (Vec<(std::ops::Range<usize>, gpui_kit::Hsla)>, usize) {
        let Some(f) = self
            .find
            .as_ref()
            .filter(|f| f.open && f.hits.iter().any(|h| h.msg == msg))
        else {
            return (Vec::new(), 0);
        };
        let bar = f.bar.read(cx);
        let found = find_in_text(text, &bar.query(cx), bar.options()).unwrap_or_default();
        let current = self.current_hit().filter(|h| h.msg == msg).map(|h| h.nth);
        let amber = recur_ui::Theme::of(cx).tokens.amber;
        let n = found.len();
        let marks = found
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let on = current == Some(before + i);
                (
                    r,
                    crate::style::hsla(amber.fade(if on { 0.75 } else { 0.35 })),
                )
            })
            .collect();
        (marks, n)
    }
}
