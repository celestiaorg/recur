//! Find in the transcript: the preview find bar over user and assistant message text.

use std::collections::HashMap;

use gpui_kit::{AppContext as _, Context, Entity, Subscription, Window, point, px};
use recur_content::preview::find::{
    FindOptions, PREVIEW_FIND_CAP, collect_preview_find_ranges, find_in_text, reveal_offset,
};
use recur_content::preview::find_bar::{FindBarEvent, PreviewFindBar};
use recur_engine::view::mentions::strip_attachment_markers;
use recur_ui::guard::Guarded;

use crate::blocks::Block;
use crate::chat::AgentChat;

/// One match: the message, its block key, and its ordinal among the message's matches.
#[derive(Clone)]
pub(crate) struct FindHit {
    msg: usize,
    key: String,
    nth: usize,
}

pub(crate) struct ChatFind {
    pub(crate) bar: Entity<PreviewFindBar>,
    pub(crate) open: bool,
    pub(crate) hits: Vec<FindHit>,
    /// The current match, 1-based (0: none).
    pub(crate) index: usize,
    /// Markdown keys carrying highlights.
    painted: Vec<String>,
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
        f.bar.update(cx, |b, cx| {
            b.reset_counts();
            cx.notify();
        });
        self.paint_find(cx);
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
                strip_attachment_markers(&m.body)
            } else {
                m.body.clone()
            };
            let Some(found) = find_in_text(&hay, &query, opts) else {
                valid = false;
                break;
            };
            hits.extend((0..found.len()).map(|nth| FindHit {
                msg: i,
                key: key.clone(),
                nth,
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
        self.paint_find(cx);
        self.reveal_find(window, cx);
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
    // rendered match at the same ordinal. User bubbles and streaming replies get no highlight.
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

    /// Centres the current match when its message has painted; otherwise scrolls its block in
    /// first (hydrating it) and then centres.
    fn reveal_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.current_hit() else {
            return;
        };
        let top = -f32::from(self.scroll.offset().y);
        let shown = self
            .block_ranges
            .borrow()
            .get(&hit.key)
            .is_some_and(|&(y, h)| y < top + self.viewport_h && y + h > top)
            && (!self.window_chat() || self.chat_hydrated.contains(&hit.key));
        if shown && (self.centre_find(window, cx) || self.find_block_fits(&hit.key)) {
            return;
        }
        let reveal = self.reveal_block(hit.key, cx);
        cx.spawn_in(window, async move |this, cx| {
            if reveal.await {
                let _ = this.update_in(cx, |c, window, cx| {
                    c.paint_find(cx);
                    c.centre_find(window, cx);
                });
            }
        })
        .detach();
    }

    fn find_block_fits(&self, key: &str) -> bool {
        let top = -f32::from(self.scroll.offset().y);
        self.block_ranges
            .borrow()
            .get(key)
            .is_some_and(|&(y, h)| y >= top && y + h <= top + self.viewport_h)
    }

    /// Scrolls the current match to the middle of the scroller; false when it has not painted.
    fn centre_find(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(hit) = self.current_hit() else {
            return false;
        };
        let ranges = self.rendered_ranges(hit.msg, cx);
        let Some((id, r)) = ranges
            .get(hit.nth.min(ranges.len().saturating_sub(1)))
            .cloned()
        else {
            return false;
        };
        let Some(b) = self
            .markdown
            .get(&self.md_key(hit.msg))
            .and_then(|v| v.read(cx).range_bounds(id, r, window))
        else {
            return false;
        };
        let view = self.scroll.bounds();
        let top = -f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y);
        let mid = |a: gpui_kit::Pixels, b: gpui_kit::Pixels| f32::from(a + b) / 2.;
        let target = reveal_offset(
            top,
            max,
            mid(b.top(), b.bottom()),
            mid(view.top(), view.bottom()),
        );
        if (target - top).abs() >= 1. {
            self.near_bottom = false;
            self.pending_scroll = None;
            self.scroll.set_offset(point(px(0.), px(-target)));
            self.last_scroll_top = target;
            self.notify_transcript(cx);
        }
        true
    }
}
