//! Read access to the chat's internals for other crates' specs (feature `test-support`):
//! the workspace view's chat specs drive the real host and assert what the chat holds.

use std::collections::BTreeMap;

use gpui_kit::{Bounds, Context, Entity, Pixels, ScrollHandle, Window};
use recur_content::MarkdownView;
use recur_engine::view::chat::AgentMsg;

use crate::cards::session::{SessionCard, SessionCardProps};
use crate::chat::AgentChat;

impl SessionCard {
    pub fn test_props(&self) -> &SessionCardProps {
        &self.props
    }

    /// What a click on the row does.
    pub fn test_toggle(&mut self, cx: &mut Context<Self>) {
        self.toggle(cx);
    }

    pub fn test_expanded(&self) -> bool {
        self.expanded()
    }

    /// The read-only transcript, once the card has been opened.
    pub fn test_chat(&self) -> Option<Entity<AgentChat>> {
        self.chat.clone()
    }
}

impl AgentChat {
    /// The composer's lines (chips show as U+FFFC).
    pub fn test_lines(&self) -> Vec<String> {
        self.doc.text.split('\n').map(str::to_string).collect()
    }

    pub fn test_caret(&self) -> usize {
        self.doc.caret
    }

    pub fn test_chip_count(&self) -> usize {
        self.doc.chips.len()
    }

    pub fn test_has_file_chip(&self) -> bool {
        self.doc
            .chips
            .iter()
            .any(|s| matches!(s, recur_engine::view::composer::Segment::File { .. }))
    }

    pub fn test_mention_hits(&self) -> usize {
        self.mention_hits.len()
    }

    pub fn test_composer_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub fn test_file_drop_over(&self) -> bool {
        self.file_drop_over
    }

    /// An explorer drag dropped `paths` on the chat.
    pub fn test_drop_explorer(
        &mut self,
        paths: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_drop_over = true;
        self.drop_explorer_paths(paths, window, cx);
    }

    pub fn test_open_rows(&self) -> &BTreeMap<String, bool> {
        &self.open_rows
    }

    pub fn test_near_bottom(&self) -> bool {
        self.near_bottom
    }

    pub fn test_scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub fn test_live_block_count(&self) -> usize {
        self.live_blocks
    }

    pub fn test_window_chat(&self) -> bool {
        self.window_chat()
    }

    pub fn test_copied_bubble(&self) -> bool {
        self.copied_bubble.is_some()
    }

    pub fn test_turn_elapsed_sec(&self) -> i64 {
        self.turn_elapsed_sec
    }

    /// The findings the turn footer at `end_idx` lists.
    pub fn test_turn_digest(&self, msgs: &[AgentMsg], end_idx: usize) -> Vec<String> {
        self.turn_digest_visible(msgs, end_idx)
            .into_iter()
            .map(|f| f.message)
            .collect()
    }

    /// Bounds of `needle` in any rendered markdown block.
    pub fn test_text_bounds(
        &self,
        needle: &str,
        window: &Window,
        cx: &gpui_kit::App,
    ) -> Option<Bounds<Pixels>> {
        self.markdown.values().find_map(|v: &Entity<MarkdownView>| {
            let v = v.read(cx);
            v.text_blocks().into_iter().find_map(|(id, text)| {
                let start = text.find(needle)?;
                v.range_bounds(id, start..start + needle.len(), window)
            })
        })
    }
}
