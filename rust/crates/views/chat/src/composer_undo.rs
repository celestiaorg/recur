//! Undo history for the composer (`lib/composerUndo.ts`): wire-text snapshots, with
//! single-character typing (or deleting) at the caret merging within a pause window.

use std::time::Duration;

/// Typing after this much idle time starts a new undo step.
pub const UNDO_PAUSE: Duration = Duration::from_millis(700);
const MAX_STEPS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoSnap<A> {
    pub text: String,
    pub atts: Vec<A>,
    /// A byte offset into `text`.
    pub caret: usize,
}

/// What an attachment is identified by.
pub trait HasId {
    fn id(&self) -> &str;
}

pub struct ComposerUndo<A> {
    stack: Vec<UndoSnap<A>>,
    i: usize,
    /// Milliseconds of the last recorded edit (0 = none).
    last_at: u64,
    /// +1 / -1 while the top step is single-char typing / deleting, else 0.
    kind: i64,
    pause_ms: u64,
}

fn same_ids<A: HasId>(a: &[A], b: &[A]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.id() == y.id())
}

fn utf16(text: &str) -> usize {
    text.encode_utf16().count()
}

/// A byte offset as a UTF-16 index (JS string offsets).
fn utf16_at(text: &str, byte: usize) -> usize {
    text.get(..byte.min(text.len())).map_or(0, utf16)
}

fn line_count(text: &str) -> usize {
    text.split('\n').count()
}

impl<A: HasId + Clone> ComposerUndo<A> {
    pub fn new(pause: Duration) -> Self {
        ComposerUndo {
            stack: Vec::new(),
            i: 0,
            last_at: 0,
            kind: 0,
            pause_ms: pause.as_millis() as u64,
        }
    }

    /// An external content change: drop history unless the content is unchanged (or `force`).
    pub fn reset(&mut self, s: UndoSnap<A>, force: bool) {
        if !force
            && let Some(top) = self.stack.get(self.i)
            && top.text == s.text
            && same_ids(&top.atts, &s.atts)
        {
            return;
        }
        self.stack = vec![s];
        self.i = 0;
        self.last_at = 0;
        self.kind = 0;
    }

    /// Records the state after an edit at `now_ms`.
    pub fn record(&mut self, s: UndoSnap<A>, now_ms: u64) {
        let Some(top) = self.stack.get(self.i) else {
            self.reset(s, false);
            return;
        };
        let same_atts = same_ids(&top.atts, &s.atts);
        if top.text == s.text && same_atts {
            self.stack[self.i] = s;
            return;
        }
        let delta = utf16(&s.text) as i64 - utf16(&top.text) as i64;
        let same_lines = line_count(&s.text) == line_count(&top.text);
        let kind = if delta.abs() == 1 && same_atts && same_lines {
            delta
        } else {
            0
        };
        let caret_jump =
            (utf16_at(&s.text, s.caret) as i64 - utf16_at(&top.text, top.caret) as i64).abs();
        let merge = kind != 0
            && kind == self.kind
            && now_ms.saturating_sub(self.last_at) < self.pause_ms
            && caret_jump <= 1;
        self.stack.truncate(self.i + 1);
        if merge {
            self.stack[self.i] = s;
        } else {
            self.stack.push(s);
            if self.stack.len() > MAX_STEPS {
                self.stack.remove(0);
            }
            self.i = self.stack.len() - 1;
        }
        self.kind = kind;
        self.last_at = now_ms;
    }

    pub fn undo(&mut self) -> Option<UndoSnap<A>> {
        if self.i == 0 {
            return None;
        }
        let from = self.stack[self.i].text.clone();
        self.i -= 1;
        Some(self.restore(&from))
    }

    pub fn redo(&mut self) -> Option<UndoSnap<A>> {
        if self.i + 1 >= self.stack.len() {
            return None;
        }
        let from = self.stack[self.i].text.clone();
        self.i += 1;
        Some(self.restore(&from))
    }

    fn restore(&mut self, from: &str) -> UndoSnap<A> {
        self.last_at = 0;
        self.kind = 0;
        let to = &self.stack[self.i];
        UndoSnap {
            text: to.text.clone(),
            atts: to.atts.clone(),
            caret: change_end(from, &to.text),
        }
    }
}

/// `changeEnd`: the byte offset in `to` just past the region that differs from `from`.
pub fn change_end(from: &str, to: &str) -> usize {
    let f: Vec<char> = from.chars().collect();
    let t: Vec<char> = to.chars().collect();
    let n = f.len().min(t.len());
    let mut p = 0;
    while p < n && f[p] == t[p] {
        p += 1;
    }
    let mut s = 0;
    while s < n - p && f[f.len() - 1 - s] == t[t.len() - 1 - s] {
        s += 1;
    }
    t[..t.len() - s].iter().map(|c| c.len_utf8()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct A(&'static str);
    impl HasId for A {
        fn id(&self) -> &str {
            self.0
        }
    }

    fn snap(text: &str, atts: Vec<A>, caret: usize) -> UndoSnap<A> {
        UndoSnap {
            text: text.into(),
            atts,
            caret,
        }
    }

    fn type_text(u: &mut ComposerUndo<A>, start: &str, add: &str, t0: u64) -> u64 {
        let mut text = start.to_string();
        let mut t = t0;
        for ch in add.chars() {
            text.push(ch);
            u.record(snap(&text, vec![], text.len()), t);
            t += 50;
        }
        t
    }

    fn undo_text(u: &mut ComposerUndo<A>) -> Option<String> {
        u.undo().map(|s| s.text)
    }

    // composerUndo.test.ts
    #[test]
    fn merges_typing_and_splits_on_pause() {
        let mut u = ComposerUndo::new(UNDO_PAUSE);
        u.reset(snap("", vec![], 0), false);
        let t = type_text(&mut u, "", "hello", 0);
        type_text(&mut u, "hello", " world", t + 1000);
        assert_eq!(u.undo(), Some(snap("hello", vec![], 5)));
        assert_eq!(u.undo(), Some(snap("", vec![], 0)));
        assert_eq!(u.undo(), None);
        assert_eq!(u.redo().map(|s| s.text).as_deref(), Some("hello"));
        assert_eq!(u.redo().map(|s| s.text).as_deref(), Some("hello world"));
        assert_eq!(u.redo(), None);
    }

    #[test]
    fn splits_between_typing_and_deleting() {
        let mut u = ComposerUndo::new(UNDO_PAUSE);
        u.reset(snap("", vec![], 0), false);
        type_text(&mut u, "", "abc", 0);
        u.record(snap("ab", vec![], 2), 200);
        assert_eq!(undo_text(&mut u).as_deref(), Some("abc"));
        assert_eq!(undo_text(&mut u).as_deref(), Some(""));
    }

    #[test]
    fn pastes_chips_and_caret_jumps_are_steps() {
        let mut u = ComposerUndo::new(UNDO_PAUSE);
        u.reset(snap("", vec![], 0), false);
        type_text(&mut u, "", "ab", 0);
        u.record(snap("ab pasted", vec![], 9), 120);
        u.record(snap("ab pastedx", vec![], 10), 140);
        u.record(snap("yab pastedx", vec![], 1), 160);
        u.record(snap("yab pastedx", vec![A("img1")], 11), 180);
        assert_eq!(u.undo(), Some(snap("yab pastedx", vec![], 11)));
        assert_eq!(u.undo(), Some(snap("ab pastedx", vec![], 0)));
        assert_eq!(undo_text(&mut u).as_deref(), Some("ab pasted"));
        assert_eq!(undo_text(&mut u).as_deref(), Some("ab"));
    }

    #[test]
    fn splits_at_newline_and_survives_same_content_reset() {
        let mut u = ComposerUndo::new(UNDO_PAUSE);
        u.reset(snap("", vec![], 0), false);
        let t = type_text(&mut u, "", "ab", 0);
        u.reset(snap("ab", vec![], 2), false);
        type_text(&mut u, "ab", "\ncd", t);
        assert_eq!(undo_text(&mut u).as_deref(), Some("ab\n"));
        assert_eq!(undo_text(&mut u).as_deref(), Some("ab"));
        assert_eq!(undo_text(&mut u).as_deref(), Some(""));
        u.reset(snap("other", vec![], 5), false);
        assert_eq!(u.undo(), None);
    }

    #[test]
    fn forced_reset_and_redo_drop() {
        let mut u = ComposerUndo::new(UNDO_PAUSE);
        u.reset(snap("", vec![], 0), false);
        type_text(&mut u, "", "hi", 0);
        u.record(snap("", vec![], 0), 5000);
        u.reset(snap("", vec![], 0), true);
        assert_eq!(u.undo(), None);
        assert_eq!(u.redo(), None);
        let mut u = ComposerUndo::new(UNDO_PAUSE);
        u.reset(snap("", vec![], 0), false);
        type_text(&mut u, "", "ab", 0);
        u.undo();
        u.record(snap("z", vec![], 1), 5000);
        assert_eq!(u.redo(), None);
        assert_eq!(undo_text(&mut u).as_deref(), Some(""));
    }

    #[test]
    fn change_end_cases() {
        assert_eq!(change_end("hello world", "hello"), 5);
        assert_eq!(change_end("hello", "hello world"), 11);
        assert_eq!(change_end("ab", "aXb"), 2);
        assert_eq!(change_end("aXb", "ab"), 1);
    }
}
