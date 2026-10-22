//! PR conversation and line-note thread rules (frontend
//! `lib/comment-thread.ts`): timeline roots, review children, replies,
//! diff-row anchoring, optimistic replies, edit / delete / react / resolve
//! permissions, resolved paint and the unmounted thread-editor drafts.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::LazyLock;

use recur_domain::{DiffLine, Pr, PrComment};
use regex::Regex;

use crate::prs::is_forge_source;

/// Optimistic line-note replies the UI inserts before ReplyPRComment returns.
pub const PENDING_REPLY_PREFIX: &str = "c-pending-";

/// Which split-diff column a row is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSide {
    Left,
    Right,
}

// simplify: byte order, not JS localeCompare; ids and RFC 3339 times are ASCII
// so only mixed-case ids could sort differently.
fn by_at_then_id(a: &&PrComment, b: &&PrComment) -> Ordering {
    a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id))
}

fn strip_inline_prefix(s: &str) -> &str {
    s.strip_prefix("gh-inline-").unwrap_or(s)
}

/// Top-level conversation rows: skip the description and nested review kids.
pub fn is_timeline_root(c: Option<&PrComment>) -> bool {
    let Some(c) = c else {
        return false;
    };
    if c.kind == "description" || c.kind == "hidden" || !c.in_reply_to_id.trim().is_empty() {
        return false;
    }
    c.parent_id.trim().is_empty()
}

/// Root line comments submitted with a review, oldest first.
pub fn review_children<'a>(items: &'a [PrComment], parent_id: &str) -> Vec<&'a PrComment> {
    if parent_id.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<_> = items
        .iter()
        .filter(|c| c.parent_id == parent_id && c.in_reply_to_id.trim().is_empty())
        .collect();
    out.sort_by(by_at_then_id);
    out
}

/// Replies on a line-note thread, oldest first.
pub fn thread_replies<'a>(items: &'a [PrComment], root_id: &str) -> Vec<&'a PrComment> {
    if root_id.is_empty() {
        return Vec::new();
    }
    let rest = strip_inline_prefix(root_id);
    let hit = |s: &str| s == root_id || (!rest.is_empty() && s == rest);
    let mut out: Vec<_> = items
        .iter()
        .filter(|c| {
            let to = c.in_reply_to_id.trim();
            hit(to) || hit(strip_inline_prefix(to))
        })
        .collect();
    out.sort_by(by_at_then_id);
    out
}

pub fn is_line_note(c: Option<&PrComment>) -> bool {
    c.is_some_and(PrComment::is_line_note)
}

pub fn same_comment_path(a: &str, b: &str) -> bool {
    let (x, y) = (a.trim(), b.trim());
    if x.is_empty() || y.is_empty() {
        return false;
    }
    x == y || x.ends_with(&format!("/{y}")) || y.ends_with(&format!("/{x}"))
}

/// Root of a posted line-note thread (not a reply).
pub fn is_line_note_root(c: &PrComment) -> bool {
    if c.outdated || !c.is_line_note() || !c.in_reply_to_id.trim().is_empty() {
        return false;
    }
    !c.path.is_empty() && c.line > 0
}

/// The note's line span: `(start, end)`, `(0, 0)` without a line.
pub fn line_note_range(c: &PrComment) -> (i64, i64) {
    let (start, end) = (c.start_line, c.line);
    if end <= 0 {
        (0, 0)
    } else if start > 0 && start < end {
        (start, end)
    } else {
        (end, end)
    }
}

fn resolve_note_path(path: &str, file_paths: &[String]) -> String {
    if path.is_empty() || file_paths.is_empty() {
        return path.to_owned();
    }
    file_paths
        .iter()
        .find(|p| *p == path)
        .or_else(|| file_paths.iter().find(|p| same_comment_path(p, path)))
        .cloned()
        .unwrap_or_else(|| path.to_owned())
}

/// Unresolved line-note roots grouped by review-file path, in first-seen
/// path order; each group sorted by line then id.
pub fn unresolved_line_notes_by_path<'a>(
    items: &'a [PrComment],
    file_paths: &[String],
) -> Vec<(String, Vec<&'a PrComment>)> {
    let mut out: Vec<(String, Vec<&PrComment>)> = Vec::new();
    for c in items {
        if !is_line_note_root(c) || c.resolved {
            continue;
        }
        let raw = c.path.trim();
        if raw.is_empty() {
            continue;
        }
        let p = resolve_note_path(raw, file_paths);
        match out.iter_mut().find(|(k, _)| *k == p) {
            Some((_, list)) => list.push(c),
            None => out.push((p, vec![c])),
        }
    }
    for (_, list) in &mut out {
        list.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.id.cmp(&b.id)));
    }
    out
}

pub fn comments_for_path<'a, T>(by_path: &'a [(String, Vec<T>)], path: &str) -> &'a [T] {
    if path.is_empty() {
        return &[];
    }
    if let Some((_, cs)) = by_path.iter().find(|(p, cs)| p == path && !cs.is_empty()) {
        return cs;
    }
    by_path
        .iter()
        .find(|(p, cs)| !cs.is_empty() && same_comment_path(p, path))
        .map_or(&[], |(_, cs)| cs)
}

pub fn comment_count_for_path<T>(by_path: &[(String, Vec<T>)], path: &str, is_dir: bool) -> usize {
    if path.is_empty() {
        return 0;
    }
    if !is_dir {
        return comments_for_path(by_path, path).len();
    }
    let prefix = format!("{path}/");
    by_path
        .iter()
        .filter(|(p, _)| p == path || p.starts_with(&prefix))
        .map(|(_, cs)| cs.len())
        .sum()
}

fn row_line(raw: &str) -> i64 {
    let digits: String = raw
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().unwrap_or(0)
}

fn note_side(c: &PrComment) -> String {
    if c.side.is_empty() {
        "RIGHT".into()
    } else {
        c.side.to_uppercase()
    }
}

/// Posted line-note roots anchored to this diff row (and optional split side).
pub fn line_notes_on_row<'a>(
    items: &'a [PrComment],
    path: &str,
    line: &DiffLine,
    side: Option<DiffSide>,
) -> Vec<&'a PrComment> {
    let (old_n, new_n) = (row_line(&line.old), row_line(&line.new));
    let mut out: Vec<_> = items
        .iter()
        .filter(|c| {
            if !is_line_note_root(c) || !same_comment_path(&c.path, path) {
                return false;
            }
            let want = note_side(c);
            let n = c.line;
            match (side, line.r#type.as_str()) {
                (Some(DiffSide::Left), _) | (None, "del") => want == "LEFT" && old_n == n,
                (Some(DiffSide::Right), _) | (None, "add") => want == "RIGHT" && new_n == n,
                _ if want == "LEFT" => old_n == n,
                _ => new_n == n,
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn note_hits_row(c: &PrComment, line: &DiffLine, side: Option<DiffSide>) -> bool {
    if !is_line_note_root(c) {
        return false;
    }
    let (start, end) = line_note_range(c);
    if end <= 0 {
        return false;
    }
    let want = note_side(c);
    let (old_n, new_n) = (row_line(&line.old), row_line(&line.new));
    let within = |n: i64| n >= start && n <= end;
    if side == Some(DiffSide::Left) || line.r#type == "del" {
        return want == "LEFT" && within(old_n);
    }
    if side == Some(DiffSide::Right) || line.r#type == "add" {
        return want == "RIGHT" && within(new_n);
    }
    if want == "LEFT" {
        within(old_n)
    } else {
        within(new_n)
    }
}

/// Line-note roots whose forge range includes this diff row.
pub fn line_notes_covering_row<'a>(
    items: &'a [PrComment],
    path: &str,
    line: &DiffLine,
    side: Option<DiffSide>,
) -> Vec<&'a PrComment> {
    let mut out: Vec<_> = items
        .iter()
        .filter(|c| same_comment_path(&c.path, path) && note_hits_row(c, line, side))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn is_pending_comment(c: Option<&PrComment>) -> bool {
    c.is_some_and(|c| c.id.starts_with(PENDING_REPLY_PREFIX))
}

pub fn same_login(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && a.to_lowercase() == b.to_lowercase()
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new("<[^>]+>").expect("tag regex"));

/// The comment's markdown, else its HTML with tags removed.
pub fn comment_source(c: Option<&PrComment>) -> String {
    let Some(c) = c else {
        return String::new();
    };
    let md = c.body_md.trim();
    if !md.is_empty() {
        return md.to_owned();
    }
    TAG.replace_all(&c.body_html, "").trim().to_owned()
}

fn is_forge_pr(pr: Option<&Pr>) -> bool {
    pr.is_some_and(|p| is_forge_source(&p.source))
}

pub fn can_edit_pr_comment(pr: Option<&Pr>, c: Option<&PrComment>) -> bool {
    let Some(c) = c else {
        return false;
    };
    if c.kind == "action" || is_pending_comment(Some(c)) {
        return false;
    }
    match pr {
        Some(pr) if is_forge_source(&pr.source) => same_login(&pr.viewer_login, &c.author_login),
        _ => c.author == "You",
    }
}

/// A submitted review card (not its action row).
pub fn is_review_card(c: Option<&PrComment>) -> bool {
    c.is_some_and(|c| c.is_review_card() && !c.id.starts_with("gh-review-action-"))
}

pub fn can_delete_pr_comment(pr: Option<&Pr>, c: Option<&PrComment>) -> bool {
    let Some(c) = c else {
        return false;
    };
    if is_pending_comment(Some(c))
        || matches!(c.kind.as_str(), "action" | "description" | "hidden")
        || is_review_card(Some(c))
    {
        return false;
    }
    can_edit_pr_comment(pr, Some(c)) || (is_forge_pr(pr) && pr.is_some_and(|p| p.viewer_can_write))
}

/// Authenticated viewers can react on conversation items (not commits or
/// generic actions).
pub fn can_react_pr_comment(pr: Option<&Pr>, c: Option<&PrComment>) -> bool {
    let Some(c) = c else {
        return false;
    };
    if is_pending_comment(Some(c)) || c.kind == "hidden" || !c.commit_sha.trim().is_empty() {
        return false;
    }
    // The forge has no reaction picker on timeline events (force-push, merge,
    // empty approve / request-changes). Reviews with a written body are cards.
    if c.kind == "action" || (is_review_card(Some(c)) && comment_source(Some(c)).is_empty()) {
        return false;
    }
    match pr {
        Some(pr) if is_forge_source(&pr.source) => !pr.viewer_login.trim().is_empty(),
        _ => true,
    }
}

pub fn is_dropped_comment(c: &PrComment, id: &str) -> bool {
    c.id == id || c.in_reply_to_id == id
}

fn reply_anchor(c: &PrComment) -> &str {
    let raw = match c.in_reply_to_id.trim() {
        "" => c.thread_id.trim(),
        to => to,
    };
    strip_inline_prefix(raw)
}

/// Same thread and same body: pairs an optimistic reply with the forge row.
pub fn same_thread_reply(a: &PrComment, b: &PrComment) -> bool {
    let body = comment_source(Some(a));
    if body.is_empty() || body != comment_source(Some(b)) {
        return false;
    }
    let key = reply_anchor(a);
    !key.is_empty() && key == reply_anchor(b)
}

/// Keep in-flight optimistic replies across a GetPR snapshot, unless the
/// forge echoed them.
pub fn keep_pending_replies(prev: &[PrComment], mut next: Vec<PrComment>) -> Vec<PrComment> {
    let mut arrived: Vec<&PrComment> = next
        .iter()
        .filter(|n| {
            !is_pending_comment(Some(n))
                && !n.id.is_empty()
                && !prev.iter().any(|p| !p.id.is_empty() && p.id == n.id)
        })
        .collect();
    let mut extras = Vec::new();
    for c in prev.iter().filter(|c| is_pending_comment(Some(c))) {
        if next.iter().any(|n| n.id == c.id) {
            continue;
        }
        if let Some(at) = arrived.iter().position(|n| same_thread_reply(n, c)) {
            arrived.remove(at);
            continue;
        }
        extras.push(c.clone());
    }
    next.extend(extras);
    next
}

/// Swap one optimistic reply for the persisted row (or drop the pending on
/// failure).
pub fn settle_posted_reply(
    mut comments: Vec<PrComment>,
    pending_id: &str,
    posted: Option<PrComment>,
) -> Vec<PrComment> {
    comments.retain(|c| c.id != pending_id);
    let Some(posted) = posted.filter(|p| !p.id.is_empty()) else {
        return comments;
    };
    match comments.iter_mut().find(|c| c.id == posted.id) {
        Some(c) => *c = posted,
        None => comments.push(posted),
    }
    comments
}

pub fn line_thread_key(c: Option<&PrComment>) -> String {
    let Some(c) = c else {
        return String::new();
    };
    let raw = if c.thread_id.is_empty() {
        &c.id
    } else {
        &c.thread_id
    };
    raw.trim().to_owned()
}

/// The PR author or a writer can resolve; a forge also needs its thread id.
pub fn can_resolve_line_note(pr: Option<&Pr>, c: Option<&PrComment>) -> bool {
    let Some(c) = c.filter(|c| c.is_line_note()) else {
        return false;
    };
    let Some(pr) = pr.filter(|p| is_forge_source(&p.source)) else {
        return true;
    };
    let login = pr.viewer_login.trim().to_lowercase();
    let author = pr.author_login.trim().to_lowercase();
    if !(pr.viewer_can_write || (!login.is_empty() && login == author)) {
        return false;
    }
    !c.thread_id.trim().is_empty()
}

pub fn comment_matches_thread(c: &PrComment, thread_id: &str) -> bool {
    let key = thread_id.trim();
    if key.is_empty() {
        return false;
    }
    c.thread_id.trim() == key || c.id.trim() == key || c.in_reply_to_id.trim() == key
}

/// Paint a line-note thread resolved or open without waiting for GetPR.
pub fn apply_thread_resolved(comments: &mut [PrComment], thread_id: &str, resolved: bool) {
    if thread_id.is_empty() {
        return;
    }
    for c in comments
        .iter_mut()
        .filter(|c| comment_matches_thread(c, thread_id))
    {
        c.resolved = resolved;
        if c.thread_id.is_empty() {
            c.thread_id = thread_id.to_owned();
        }
    }
}

/// Re-apply in-flight resolve toggles on top of a GetPR snapshot.
pub fn apply_thread_resolved_holds(comments: &mut [PrComment], holds: &HashMap<String, bool>) {
    for (id, resolved) in holds {
        apply_thread_resolved(comments, id, *resolved);
    }
}

/// Unsent reply or edit while a windowed line-note row is unmounted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadEditorDraft {
    pub reply_open: bool,
    pub reply: String,
    pub editing_id: String,
    pub edit: String,
}

/// The drafts by thread root id (one per app, as the TS module map).
#[derive(Debug, Default)]
pub struct ThreadEditorDrafts(HashMap<String, ThreadEditorDraft>);

impl ThreadEditorDrafts {
    pub fn get(&self, id: &str) -> Option<&ThreadEditorDraft> {
        self.0.get(id)
    }

    /// Stores the draft; an empty one is dropped.
    pub fn set(&mut self, id: &str, draft: ThreadEditorDraft) {
        if id.is_empty() {
            return;
        }
        if draft == ThreadEditorDraft::default() {
            self.0.remove(id);
        } else {
            self.0.insert(id.to_owned(), draft);
        }
    }
}

#[cfg(test)]
#[path = "comment_thread_tests.rs"]
mod tests;
