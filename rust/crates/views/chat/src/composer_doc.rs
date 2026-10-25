//! The composer's editable document (`lib/composerDom.ts` as a native model): one string
//! where every chip is [`ATOM`] and lines are `\n`, the chips in order, a caret and a
//! selection anchor (byte offsets, never inside an atom). Wire text and chip hydration are
//! `recur_engine::view::composer`; this owns editing.

use std::ops::Range;

use recur_engine::view::composer::{
    ComposerAtt, HydrateOpts, Segment, hydrate_composer, serialize_composer,
};
use recur_engine::view::js;
use recur_engine::view::mentions::ATTACHMENT_MARKER_RE;

use crate::flow::{ATOM, ATOM_LEN};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComposerDoc {
    pub text: String,
    /// Non-text segments, one per [`ATOM`], in order.
    pub chips: Vec<Segment>,
    pub caret: usize,
    pub anchor: usize,
}

/// What kind of chip sits before the caret (`isCaretAfter*Chip`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipKind {
    File,
    Skill,
    Terminal,
    Artifact,
    Attachment,
    Prompt,
}

pub fn chip_kind(s: &Segment) -> Option<ChipKind> {
    Some(match s {
        Segment::Text(_) => return None,
        Segment::File { .. } => ChipKind::File,
        Segment::Skill(_) => ChipKind::Skill,
        Segment::Terminal { .. } => ChipKind::Terminal,
        Segment::Artifact { .. } => ChipKind::Artifact,
        Segment::Attachment(_) => ChipKind::Attachment,
        Segment::Prompt { .. } => ChipKind::Prompt,
    })
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl ComposerDoc {
    pub fn from_lines(lines: Vec<Vec<Segment>>) -> Self {
        let mut d = ComposerDoc::default();
        for (i, line) in lines.into_iter().enumerate() {
            if i > 0 {
                d.text.push('\n');
            }
            for s in line {
                match s {
                    Segment::Text(t) => d.text.push_str(&t.replace('\n', " ")),
                    chip => {
                        d.text.push(ATOM);
                        d.chips.push(chip);
                    }
                }
            }
        }
        d.caret = d.text.len();
        d.anchor = d.caret;
        d
    }

    /// `hydrateComposer`: chips from wire text.
    pub fn hydrate(wire: &str, attachments: &[ComposerAtt], opts: &HydrateOpts<'_>) -> Self {
        Self::from_lines(hydrate_composer(wire, attachments, opts))
    }

    pub fn lines(&self) -> Vec<Vec<Segment>> {
        let mut out = vec![Vec::new()];
        let mut chip = 0;
        let mut buf = String::new();
        let flush = |buf: &mut String, line: &mut Vec<Segment>| {
            if !buf.is_empty() {
                line.push(Segment::Text(std::mem::take(buf)));
            }
        };
        for ch in self.text.chars() {
            match ch {
                '\n' => {
                    let line = out.last_mut().expect("line");
                    flush(&mut buf, line);
                    out.push(Vec::new());
                }
                ATOM => {
                    let line = out.last_mut().expect("line");
                    flush(&mut buf, line);
                    if let Some(c) = self.chips.get(chip) {
                        line.push(c.clone());
                    }
                    chip += 1;
                }
                c => buf.push(c),
            }
        }
        flush(&mut buf, out.last_mut().expect("line"));
        out
    }

    /// `serializeComposer`.
    pub fn wire(&self) -> String {
        serialize_composer(&self.lines())
    }

    /// The chip index of the atom starting at `byte`.
    pub fn chip_at(&self, byte: usize) -> Option<usize> {
        if !self.text[byte..].starts_with(ATOM) {
            return None;
        }
        Some(self.text[..byte].matches(ATOM).count())
    }

    /// The doc byte where chip `i` sits.
    pub fn chip_byte(&self, i: usize) -> Option<usize> {
        self.text.match_indices(ATOM).nth(i).map(|(b, _)| b)
    }

    /// The wire offset of a doc byte (`composerCaretOffset`).
    pub fn wire_offset(&self, byte: usize) -> usize {
        let mut off = 0;
        let mut chip = 0;
        for (b, ch) in self.text.char_indices() {
            if b >= byte {
                break;
            }
            if ch == ATOM {
                off += self.chips.get(chip).map_or(0, |c| c.wire().len());
                chip += 1;
            } else {
                off += ch.len_utf8();
            }
        }
        off
    }

    /// The doc byte for a wire offset (`setComposerCaret`); inside a chip's wire lands after it.
    pub fn doc_offset(&self, wire: usize) -> usize {
        let mut off = 0;
        let mut chip = 0;
        for (b, ch) in self.text.char_indices() {
            if off >= wire {
                return b;
            }
            if ch == ATOM {
                off += self.chips.get(chip).map_or(0, |c| c.wire().len());
                chip += 1;
            } else {
                off += ch.len_utf8();
            }
        }
        self.text.len()
    }

    pub fn wire_caret(&self) -> usize {
        self.wire_offset(self.caret)
    }

    pub fn selection(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    fn set_caret(&mut self, b: usize, extend: bool) {
        self.caret = b.min(self.text.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn chips_in(&self, r: Range<usize>) -> Range<usize> {
        let before = self.text[..r.start].matches(ATOM).count();
        let inside = self.text[r].matches(ATOM).count();
        before..before + inside
    }

    /// Removes a doc range, its chips with it; returns the removed chips.
    pub fn remove_range(&mut self, r: Range<usize>) -> Vec<Segment> {
        let chips = self.chips_in(r.clone());
        let removed: Vec<Segment> = self.chips.drain(chips).collect();
        self.text.replace_range(r.clone(), "");
        self.caret = r.start;
        self.anchor = r.start;
        removed
    }

    /// Typing / paste: replaces the selection with plain text (CRLF folded).
    pub fn insert_text(&mut self, s: &str) -> Vec<Segment> {
        let s = s
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace(ATOM, "");
        let removed = self.remove_range(self.selection());
        self.text.insert_str(self.caret, &s);
        self.set_caret(self.caret + s.len(), false);
        removed
    }

    /// `insertComposerChip`: the chip at the caret (replacing the selection), then a trailing
    /// space unless told otherwise.
    pub fn insert_chip(&mut self, chip: Segment, trailing_space: bool) -> Vec<Segment> {
        let removed = self.remove_range(self.selection());
        let at = self.caret;
        let idx = self.text[..at].matches(ATOM).count();
        self.chips.insert(idx, chip);
        self.text.insert(at, ATOM);
        let mut end = at + ATOM_LEN;
        if trailing_space {
            self.text.insert(end, ' ');
            end += 1;
        }
        self.set_caret(end, false);
        removed
    }

    /// Shift+Enter (`insertComposerParagraphBreak`).
    pub fn break_line(&mut self) -> Vec<Segment> {
        self.insert_text("\n")
    }

    fn prev_boundary(&self, b: usize) -> usize {
        self.text[..b]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, b: usize) -> usize {
        self.text[b..]
            .chars()
            .next()
            .map_or(b, |c| b + c.len_utf8())
    }

    /// Backspace: the selection, else the char or whole chip before the caret.
    pub fn delete_backward(&mut self) -> Vec<Segment> {
        if self.has_selection() {
            return self.remove_range(self.selection());
        }
        if self.caret == 0 {
            return Vec::new();
        }
        let p = self.prev_boundary(self.caret);
        self.remove_range(p..self.caret)
    }

    pub fn delete_forward(&mut self) -> Vec<Segment> {
        if self.has_selection() {
            return self.remove_range(self.selection());
        }
        let n = self.next_boundary(self.caret);
        self.remove_range(self.caret..n)
    }

    fn word_left(&self, b: usize) -> usize {
        let head = &self.text[..b];
        let mut it = head.char_indices().rev().peekable();
        let mut at = b;
        while let Some(&(i, c)) = it.peek() {
            if c == ATOM {
                return if at == b { i } else { at };
            }
            if is_word(c) {
                break;
            }
            at = i;
            it.next();
        }
        while let Some(&(i, c)) = it.peek() {
            if !is_word(c) {
                break;
            }
            at = i;
            it.next();
        }
        at
    }

    fn word_right(&self, b: usize) -> usize {
        let mut at = b;
        let mut it = self.text[b..].char_indices().peekable();
        while let Some(&(i, c)) = it.peek() {
            if c == ATOM {
                return if at == b { b + i + ATOM_LEN } else { at };
            }
            if is_word(c) {
                break;
            }
            at = b + i + c.len_utf8();
            it.next();
        }
        while let Some(&(i, c)) = it.peek() {
            if !is_word(c) {
                break;
            }
            at = b + i + c.len_utf8();
            it.next();
        }
        at
    }

    pub fn delete_word_backward(&mut self) -> Vec<Segment> {
        if self.has_selection() {
            return self.remove_range(self.selection());
        }
        let w = self.word_left(self.caret);
        self.remove_range(w..self.caret)
    }

    pub fn delete_to_line_start(&mut self) -> Vec<Segment> {
        if self.has_selection() {
            return self.remove_range(self.selection());
        }
        let s = self.line_start(self.caret);
        let s = if s == self.caret {
            self.prev_boundary(s)
        } else {
            s
        };
        self.remove_range(s..self.caret)
    }

    pub fn line_start(&self, b: usize) -> usize {
        self.text[..b].rfind('\n').map_or(0, |i| i + 1)
    }

    pub fn line_end(&self, b: usize) -> usize {
        self.text[b..].find('\n').map_or(self.text.len(), |i| b + i)
    }

    pub fn move_left(&mut self, extend: bool, word: bool) {
        if !extend && self.has_selection() {
            let s = self.selection().start;
            return self.set_caret(s, false);
        }
        let b = if word {
            self.word_left(self.caret)
        } else {
            self.prev_boundary(self.caret)
        };
        self.set_caret(b, extend);
    }

    pub fn move_right(&mut self, extend: bool, word: bool) {
        if !extend && self.has_selection() {
            let e = self.selection().end;
            return self.set_caret(e, false);
        }
        let b = if word {
            self.word_right(self.caret)
        } else {
            self.next_boundary(self.caret)
        };
        self.set_caret(b, extend);
    }

    pub fn move_to(&mut self, b: usize, extend: bool) {
        self.set_caret(b, extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// The selection's wire text (copy / cut).
    pub fn selected_wire(&self) -> String {
        let r = self.selection();
        let wire = self.wire();
        wire[self.wire_offset(r.start)..self.wire_offset(r.end)].to_string()
    }

    /// The chip kind flush before the caret.
    pub fn kind_before_caret(&self) -> Option<ChipKind> {
        if !self.text[..self.caret].ends_with(ATOM) {
            return None;
        }
        let i = self.text[..self.caret].matches(ATOM).count() - 1;
        self.chips.get(i).and_then(chip_kind)
    }

    pub fn prompt_chip(&self) -> Option<usize> {
        self.chips
            .iter()
            .position(|c| matches!(c, Segment::Prompt { .. }))
    }

    /// Image draft ids still in the doc.
    pub fn attachment_ids(&self) -> Vec<String> {
        self.chips
            .iter()
            .filter_map(|c| match c {
                Segment::Attachment(a) => Some(a.id.clone()),
                _ => None,
            })
            .collect()
    }

    /// `chipifyComposerText`: typed `⟦attachment:id⟧` markers of known drafts become chips.
    pub fn chipify_attachments(&mut self, attachments: &[ComposerAtt]) {
        loop {
            let hit = ATTACHMENT_MARKER_RE
                .captures_iter(&self.text)
                .find_map(|c| {
                    let m = c.get(0)?;
                    let a = attachments
                        .iter()
                        .find(|a| a.id == c[1] && !a.data_url.is_empty())?;
                    Some((m.range(), a.clone()))
                });
            let Some((r, att)) = hit else { return };
            let caret = self.caret;
            let len = r.len();
            let idx = self.text[..r.start].matches(ATOM).count();
            self.text.replace_range(r.clone(), &ATOM.to_string());
            self.chips.insert(idx, Segment::Attachment(att));
            let shift = |b: usize| {
                if b >= r.end {
                    b - len + ATOM_LEN
                } else if b > r.start {
                    r.start + ATOM_LEN
                } else {
                    b
                }
            };
            self.caret = shift(caret);
            self.anchor = shift(self.anchor);
        }
    }

    /// `ensureChipBoundariesInDom`: a space before every file/skill/terminal/artifact chip
    /// (and after terminal/artifact chips) so their wire stays a mention.
    pub fn ensure_chip_boundaries(&mut self) {
        let mut i = 0;
        let mut chip = 0;
        // Jump from chip to chip: only a chip's neighbors can need a space.
        while let Some(off) = self.text[i..].find(ATOM) {
            i += off;
            let kind = self.chips.get(chip).and_then(chip_kind);
            chip += 1;
            let needs = matches!(
                kind,
                Some(ChipKind::File | ChipKind::Skill | ChipKind::Terminal | ChipKind::Artifact)
            );
            if needs {
                if let Some(prev) = self.text[..i].chars().next_back()
                    && prev != '\n'
                    && !js::is_space(prev)
                {
                    self.insert_at(i, ' ');
                    i += 1;
                }
                if matches!(kind, Some(ChipKind::Terminal | ChipKind::Artifact)) {
                    let after = i + ATOM_LEN;
                    if let Some(next) = self.text[after..].chars().next()
                        && next != '\n'
                        && !js::is_space(next)
                    {
                        self.insert_at(after, ' ');
                    }
                }
            }
            i += ATOM_LEN;
        }
    }

    fn insert_at(&mut self, at: usize, c: char) {
        self.text.insert(at, c);
        if self.caret >= at {
            self.caret += c.len_utf8();
        }
        if self.anchor >= at {
            self.anchor += c.len_utf8();
        }
    }

    /// Replaces chip `i` in place (prompt slot edits, refreshed titles).
    pub fn set_chip(&mut self, i: usize, chip: Segment) {
        if let Some(c) = self.chips.get_mut(i) {
            *c = chip;
        }
    }

    /// Removes chip `i` (its × button) and the caret lands where it was.
    pub fn remove_chip(&mut self, i: usize) -> Option<Segment> {
        let b = self.chip_byte(i)?;
        self.remove_range(b..b + ATOM_LEN).into_iter().next()
    }

    /// A UTF-16 offset (the input handler's unit) to a doc byte.
    pub fn byte_of_utf16(&self, u: usize) -> usize {
        let mut n = 0;
        for (b, c) in self.text.char_indices() {
            if n >= u {
                return b;
            }
            n += c.len_utf16();
        }
        self.text.len()
    }

    pub fn utf16_of_byte(&self, b: usize) -> usize {
        self.text[..b.min(self.text.len())].encode_utf16().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(p: &str) -> Segment {
        Segment::File {
            path: p.into(),
            dir: false,
        }
    }

    fn opts() -> HydrateOpts<'static> {
        HydrateOpts {
            chip_files: true,
            chip_skills: true,
            ..Default::default()
        }
    }

    #[test]
    fn hydrates_and_serializes_round_trip() {
        let wire = "see @src/a.ts and /review\nnext";
        let d = ComposerDoc::hydrate(wire, &[], &opts());
        assert_eq!(d.chips.len(), 2);
        assert_eq!(d.wire(), wire);
        assert_eq!(d.wire_offset(d.text.len()), wire.len());
        let at = d.chip_byte(0).unwrap();
        assert_eq!(d.wire_offset(at), 4);
        assert_eq!(d.doc_offset(6), at + ATOM_LEN);
    }

    #[test]
    fn backspace_removes_a_chip_whole() {
        let mut d = ComposerDoc::default();
        d.insert_text("look ");
        d.insert_chip(file("src/a.ts"), false);
        assert_eq!(d.kind_before_caret(), Some(ChipKind::File));
        let removed = d.delete_backward();
        assert_eq!(removed, vec![file("src/a.ts")]);
        assert_eq!(d.text, "look ");
        assert!(d.chips.is_empty());
    }

    #[test]
    fn trailing_space_after_inserted_chips() {
        let mut d = ComposerDoc::default();
        d.insert_chip(file("a.ts"), true);
        assert_eq!(d.wire(), "@a.ts ");
        d.insert_text("x");
        assert_eq!(d.wire(), "@a.ts x");
    }

    #[test]
    fn boundaries_restore_spaces_around_chips() {
        let mut d = ComposerDoc::default();
        d.insert_text("x");
        d.insert_chip(
            Segment::Terminal {
                id: "t1".into(),
                title: String::new(),
            },
            false,
        );
        d.insert_text("y");
        d.ensure_chip_boundaries();
        assert_eq!(d.wire(), "x @terminal:t1 y");
        assert_eq!(d.caret, d.text.len());
    }

    #[test]
    fn typed_markers_of_known_drafts_chipify() {
        let att = ComposerAtt {
            id: "a1".into(),
            data_url: "data:image/png;base64,AA".into(),
            name: "shot.png".into(),
        };
        let mut d = ComposerDoc::default();
        d.insert_text("hi ⟦attachment:a1⟧ ⟦attachment:zz⟧");
        d.chipify_attachments(std::slice::from_ref(&att));
        assert_eq!(d.attachment_ids(), vec!["a1".to_string()]);
        assert_eq!(d.wire(), "hi ⟦attachment:a1⟧ ⟦attachment:zz⟧");
    }

    #[test]
    fn word_moves_stop_at_chips_and_words() {
        let mut d = ComposerDoc::default();
        d.insert_text("one two");
        d.move_left(false, true);
        assert_eq!(d.caret, 4);
        d.move_left(false, true);
        assert_eq!(d.caret, 0);
        d.move_right(true, true);
        assert_eq!(d.selection(), 0..3);
        d.delete_backward();
        assert_eq!(d.text, " two");
    }

    #[test]
    fn line_breaks_and_line_start_delete() {
        let mut d = ComposerDoc::default();
        d.insert_text("ab");
        d.break_line();
        d.insert_text("cd");
        assert_eq!(d.lines().len(), 2);
        d.delete_to_line_start();
        assert_eq!(d.text, "ab\n");
        d.delete_to_line_start();
        assert_eq!(d.text, "ab");
    }
}
