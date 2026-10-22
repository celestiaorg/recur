//! The PR conversation (Go `pr.go`): comments, pending line notes, reviews,
//! title/description edits, comment edits and deletes, replies, reactions,
//! thread resolution and draft state. A Local PR is written in place; a
//! forge PR goes through its forge first and the local row follows.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use recur_domain::{
    FINDING_ACTOR_ENGINE, FileDiff, FindingStatus, PendingLineComment, Pr, PrComment,
    PrParticipant, PrReaction, normalize_reaction_content, reaction_mine, toggle_reactions,
};
use recur_forge::model::comments_reviewers;
use recur_forge::{Forge, ForgeError};

use crate::Engine;
use crate::bus::{EngineEvent, WorkspaceRef};
use crate::loops::open_pr::upsert_description_comment;
use crate::prs::{comments, now_comment, touch_pr_updated};
use crate::text::{html_escape, unix_nanos};

/// Bounds one forge conversation mutation.
pub(crate) const FORGE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Bounds a draft flip.
const FORGE_DRAFT_TIMEOUT: Duration = Duration::from_secs(20);
/// Bounds a mention search.
const MENTION_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs a forge call under Go's context deadline.
pub(crate) async fn bounded<T>(
    d: Duration,
    fut: impl std::future::Future<Output = Result<T, ForgeError>>,
) -> Result<T, ForgeError> {
    tokio::time::timeout(d, fut)
        .await
        .unwrap_or_else(|_| Err(ForgeError::failed("context deadline exceeded")))
}

/// A negative line is the LEFT (old) side of a split diff.
pub fn split_pending_line(line: i64) -> (&'static str, i64) {
    if line < 0 { ("LEFT", -line) } else { ("RIGHT", line) }
}

/// LEFT, or RIGHT for anything else.
pub fn pending_side(side: &str) -> &'static str {
    if side.eq_ignore_ascii_case("LEFT") {
        "LEFT"
    } else {
        "RIGHT"
    }
}

/// A submitted pending note as a thread row.
pub fn pending_to_comment(pc: &PendingLineComment, files: &[FileDiff]) -> PrComment {
    let mut c = now_comment("AR", "You", &format!("<code>{}</code>", pending_loc(pc)));
    c.body_md = pc.body.trim().to_owned();
    c.path = pc.file.clone();
    c.line = pc.line;
    c.side = pending_side(&pc.side).to_owned();
    c.start_line = pc.start_line;
    c.start_side = pc.start_side.clone();
    c.diff_hunk = comment_hunk_from_files(files, &pc.file, pc.line, &c.side);
    c.thread_id = c.id.clone();
    c
}

/// `file:line` or `file:start-end`, HTML-escaped.
pub fn pending_loc(pc: &PendingLineComment) -> String {
    let loc = html_escape(&pc.file);
    if pc.line <= 0 {
        return loc;
    }
    if pc.start_line > 0 && pc.start_line < pc.line {
        return format!("{loc}:{}-{}", pc.start_line, pc.line);
    }
    format!("{loc}:{}", pc.line)
}

/// The diff hunk up to `line` on `side`, as the forge would record it
/// (empty when the line is not in the diff).
pub fn comment_hunk_from_files(files: &[FileDiff], path: &str, line: i64, side: &str) -> String {
    let Some(file) = files.iter().find(|f| f.path == path) else {
        return String::new();
    };
    if file.path.is_empty() || line <= 0 {
        return String::new();
    }
    let want_left = side.eq_ignore_ascii_case("LEFT");
    let mut buf: Vec<String> = Vec::new();
    let mut got = false;
    for ln in file.lines.iter().flatten() {
        if ln.r#type == "hunk" {
            if got {
                break;
            }
            buf.clear();
            let mut text = visible_diff_text(&ln.code_html);
            if text.is_empty() {
                text = "@@".into();
            }
            buf.push(text);
            continue;
        }
        if buf.is_empty() {
            continue;
        }
        let atoi = |s: &str| s.parse::<i64>().unwrap_or(0);
        let (prefix, n) = match ln.r#type.as_str() {
            "add" => ("+", atoi(&ln.new)),
            "del" => ("-", atoi(&ln.old)),
            _ if want_left => (" ", atoi(&ln.old)),
            _ => (" ", atoi(&ln.new)),
        };
        buf.push(format!("{prefix}{}", visible_diff_text(&ln.code_html)));
        let matched = if want_left {
            n == line && ln.r#type != "add"
        } else {
            n == line && ln.r#type != "del"
        };
        if matched {
            got = true;
            break;
        }
    }
    if !got {
        return String::new();
    }
    buf.join("\n")
}

/// Highlighted diff HTML back to its text.
pub fn visible_diff_text(code_html: &str) -> String {
    let mut b = String::new();
    let mut in_tag = false;
    for r in code_html.chars() {
        match r {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => b.push(r),
            _ => {}
        }
    }
    recur_forge::json::html_unescape(&b)
}

/// The review's label and the PR state words it implies ("" = unchanged).
pub fn review_label_state(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "approve" => ("Approved", "Approved"),
        "request" => ("Requested changes", "Changes requested"),
        _ => ("Commented", ""),
    }
}

/// The verb on a review card.
pub fn review_card_verb(kind: &str) -> &'static str {
    match kind {
        "approve" => "approved",
        "request" => "requested changes",
        _ => "commented",
    }
}

/// Nests a reply under its line-note thread root.
pub fn attach_line_reply(c: &mut PrComment, root: &PrComment) {
    c.in_reply_to_id = root.id.clone();
    c.parent_id = root.parent_id.clone();
    c.thread_id = if root.thread_id.is_empty() {
        root.id.clone()
    } else {
        root.thread_id.clone()
    };
    c.resolved = root.resolved;
    if c.path.is_empty() {
        c.path = root.path.clone();
        c.line = root.line;
        c.side = root.side.clone();
    }
}

/// The thread key a resolve targets: a row's thread id, else the root's.
pub fn resolve_thread_key(comments: &[PrComment], id: &str) -> String {
    if let Some(c) = comments.iter().find(|c| c.thread_id == id) {
        return c.thread_id.clone();
    }
    let root = find_line_thread_root(comments, id);
    if root.id.is_empty() {
        return String::new();
    }
    if !root.thread_id.is_empty() {
        return root.thread_id;
    }
    if root.is_line_note() {
        return root.id;
    }
    String::new()
}

/// Flags every row of a line-note thread resolved (or open).
pub fn mark_thread_resolved(comments: &mut [PrComment], thread_id: &str, resolved: bool) {
    let roots: Vec<PrComment> = comments
        .iter()
        .map(|c| {
            if c.thread_id == thread_id || c.id == thread_id || !c.is_line_note() {
                PrComment::default()
            } else {
                find_line_thread_root(comments, &c.id)
            }
        })
        .collect();
    for (c, root) in comments.iter_mut().zip(roots) {
        let matched = c.thread_id == thread_id
            || c.id == thread_id
            || (c.is_line_note() && (root.id == thread_id || root.thread_id == thread_id));
        if !matched {
            continue;
        }
        c.resolved = resolved;
        if c.thread_id.is_empty() {
            c.thread_id = thread_id.to_owned();
        }
    }
}

/// Follows `in_reply_to_id` to the thread's first row (empty when `id` is
/// unknown).
pub fn find_line_thread_root(comments: &[PrComment], id: &str) -> PrComment {
    let by_id: HashMap<&str, &PrComment> = comments.iter().map(|c| (c.id.as_str(), c)).collect();
    let Some(mut cur) = by_id.get(id).copied() else {
        return PrComment::default();
    };
    let mut seen = HashSet::new();
    while !cur.in_reply_to_id.is_empty() && seen.insert(cur.id.as_str()) {
        match by_id.get(cur.in_reply_to_id.as_str()) {
            Some(next) => cur = next,
            None => break,
        }
    }
    cur.clone()
}

/// `id` and its direct replies.
pub fn comment_ids_to_remove(comments: &[PrComment], id: &str) -> Vec<String> {
    let mut ids = vec![id.to_owned()];
    ids.extend(
        comments
            .iter()
            .filter(|c| c.in_reply_to_id == id)
            .map(|c| c.id.clone()),
    );
    ids
}

/// The rows with these ids, in thread order.
pub fn take_pr_comments(comments: &[PrComment], ids: &[String]) -> Vec<PrComment> {
    comments
        .iter()
        .filter(|c| ids.contains(&c.id))
        .cloned()
        .collect()
}

/// The thread without `id` and its direct replies.
pub fn remove_pr_comment(comments: &[PrComment], id: &str) -> Vec<PrComment> {
    let drop = comment_ids_to_remove(comments, id);
    comments
        .iter()
        .filter(|c| !drop.contains(&c.id))
        .cloned()
        .collect()
}

pub fn find_pr_comment(comments: &[PrComment], id: &str) -> Option<usize> {
    comments.iter().position(|c| c.id == id)
}

/// The forge's failure line for the workspace feed.
fn could_not(what: &str, err: &ForgeError) -> String {
    format!("{what}{}", html_escape(err.message.trim()))
}

impl Engine {
    /// Records a review note. A file+line note is pending until a review
    /// (or send-back) submits it; a conversation note posts now. A negative
    /// line is the LEFT side; `start_line` opens a multiline range with the
    /// same sign rule. An empty body removes the pending note whose id or
    /// finding id is `finding_id`.
    pub async fn add_pr_comment(
        &self,
        pr_id: &str,
        file: &str,
        line: i64,
        body: &str,
        start_line: i64,
        finding_id: &str,
    ) -> (PrComment, bool) {
        let Some(pr) = self.store.pr(pr_id) else {
            return (PrComment::default(), false);
        };
        let file = file.trim();
        let body = body.trim();
        if pr.conversation_locked() && !body.is_empty() {
            return (PrComment::default(), false);
        }
        let (side, line) = split_pending_line(line);
        let (mut start_side, mut start) = split_pending_line(start_line);
        if start <= 0 || start >= line {
            start = 0;
            start_side = "";
        } else if start_side.is_empty() {
            start_side = side;
        }
        if !file.is_empty() && line > 0 {
            return self.upsert_pending_line_comment(
                &pr, file, line, side, start, start_side, body, finding_id,
            );
        }
        if let Some(forge) = self.pr_forge(&pr) {
            return self.add_forge_pr_comment(forge.as_ref(), &pr, file, line, body).await;
        }
        let c = now_comment("AR", "You", &html_escape(body));
        let saved = self.store.update_pr(pr_id, |p| {
            p.comments.get_or_insert_with(Vec::new).push(c.clone());
            touch_pr_updated(p);
        });
        if saved.is_none() {
            return (PrComment::default(), false);
        }
        self.publish_pr_comment(pr_id, &c);
        self.publish_pr_updated(pr_id);
        (c, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_pending_line_comment(
        &self,
        pr: &Pr,
        file: &str,
        line: i64,
        side: &str,
        start: i64,
        start_side: &str,
        body: &str,
        finding_id: &str,
    ) -> (PrComment, bool) {
        if body.is_empty() {
            let id = finding_id.trim();
            if id.is_empty() {
                return (PrComment::default(), false);
            }
            let mut removed = false;
            let ok = self.store.update_pr(&pr.id, |p| {
                let before = p.pending_comments.len();
                p.pending_comments
                    .retain(|pc| pc.id != id && pc.finding_id.trim() != id);
                removed = p.pending_comments.len() != before;
            });
            if ok.is_none() || !removed {
                return (PrComment::default(), false);
            }
            self.publish_pr_pending_updated(&pr.id);
            return (
                PrComment {
                    id: "removed".into(),
                    ..PrComment::default()
                },
                true,
            );
        }
        let pc = PendingLineComment {
            id: format!("pend-{}", unix_nanos()),
            file: file.to_owned(),
            line,
            side: side.to_owned(),
            start_line: start,
            start_side: start_side.to_owned(),
            body: body.to_owned(),
            finding_id: finding_id.trim().to_owned(),
        };
        if self
            .store
            .update_pr(&pr.id, |p| p.pending_comments.push(pc.clone()))
            .is_none()
        {
            return (PrComment::default(), false);
        }
        self.publish_pr_pending_updated(&pr.id);
        (now_comment("AR", "You", &html_escape(body)), true)
    }

    async fn add_forge_pr_comment(
        &self,
        forge: &dyn Forge,
        pr: &Pr,
        file: &str,
        line: i64,
        body: &str,
    ) -> (PrComment, bool) {
        let repo = self.pr_repo_path(pr);
        let posted = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.post_comment(Path::new(&repo), pr.number, file, line, body),
        )
        .await;
        if let Err(e) = posted {
            let what = format!("Could not post the comment to {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (PrComment::default(), false);
        }
        self.refresh_pr_conversation(&pr.id).await;
        self.publish_pr_updated(&pr.id);
        let updated = self.store.pr(&pr.id).unwrap_or_default();
        if let Some(last) = comments(&updated).last() {
            return (last.clone(), true);
        }
        (now_comment("AR", "You", &html_escape(body.trim())), true)
    }

    /// Persists a Comment / Approve / Request-changes review with the
    /// pending line notes it submits. Approve and Request changes set the
    /// PR's state words.
    pub async fn submit_review(&self, pr_id: &str, kind: &str, body: &str) -> (Pr, bool) {
        let Some(pr) = self.store.pr(pr_id) else {
            return (Pr::default(), false);
        };
        if pr.conversation_locked() {
            return (pr, false);
        }
        if let Some(forge) = self.pr_forge(&pr) {
            if (kind == "approve" || kind == "request") && !pr.can_approve_review() {
                return (pr, false);
            }
            return self.submit_forge_review(forge.as_ref(), pr, kind, body).await;
        }
        let (_, state_words) = review_label_state(kind);
        let mut c = now_comment("AR", "You", &html_escape(body.trim()));
        c.verb = review_card_verb(kind).into();
        let mut flushed = Vec::new();
        let mut pending = Vec::new();
        let updated = self.store.update_pr(pr_id, |p| {
            pending = p.pending_comments.clone();
            let files = p.files.clone().unwrap_or_default();
            for pc in &p.pending_comments {
                let mut fc = pending_to_comment(pc, &files);
                fc.parent_id = c.id.clone();
                flushed.push(fc);
            }
            let cs = p.comments.get_or_insert_with(Vec::new);
            cs.push(c.clone());
            cs.extend(flushed.iter().cloned());
            p.pending_comments.clear();
            if !state_words.is_empty() {
                p.state = state_words.into();
            }
            p.reviewers = comments_reviewers(comments(p), &|l, a| self.local_pr_avatar(l, a));
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            return (Pr::default(), false);
        };
        self.resolve_pending_findings(&updated, &pending);
        for fc in &flushed {
            self.publish_pr_comment(pr_id, fc);
        }
        self.publish_pr_comment(pr_id, &c);
        self.publish_pr_updated(pr_id);
        (updated, true)
    }

    async fn submit_forge_review(
        &self,
        forge: &dyn Forge,
        pr: Pr,
        kind: &str,
        body: &str,
    ) -> (Pr, bool) {
        let repo = self.pr_repo_path(&pr);
        let pending = pr.pending_comments.clone();
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.submit_review(Path::new(&repo), pr.number, kind, body, &pending),
        )
        .await;
        if let Err(e) = res {
            let what = format!("Could not submit the review to {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (pr, false);
        }
        self.resolve_pending_findings(&pr, &pending);
        self.store
            .update_pr(&pr.id, |p| p.pending_comments.clear());
        self.refresh_pr_conversation(&pr.id).await;
        self.publish_pr_updated(&pr.id);
        match self.store.pr(&pr.id) {
            Some(updated) => (updated, true),
            None => (pr, true),
        }
    }

    /// Marks still-open findings resolved once their drafts were submitted.
    /// Runs on a preserved pending list so a failed conversation refresh
    /// cannot lose the conversion.
    pub(crate) fn resolve_pending_findings(&self, pr: &Pr, pending: &[PendingLineComment]) {
        let mut ids: Vec<&str> = Vec::new();
        for pc in pending {
            let id = pc.finding_id.trim();
            if !id.is_empty() && !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.is_empty() {
            return;
        }
        let ws_id = self.pr_findings_workspace_id(pr);
        if ws_id.is_empty() {
            return;
        }
        let resolved = FindingStatus::Resolved.as_str();
        let n = ids
            .iter()
            .filter(|id| {
                self.store
                    .update_finding_status(&ws_id, id, resolved, "", FINDING_ACTOR_ENGINE)
                    .is_ok_and(|f| f.status == resolved)
            })
            .count();
        if n == 0 {
            return;
        }
        self.bus
            .publish(EngineEvent::FindingsUpdated(WorkspaceRef { workspace_id: ws_id }));
    }

    /// The workspace this PR's findings live on: a live linked workspace,
    /// else the chat host that filed them before promote.
    pub fn pr_findings_workspace_id(&self, p: &Pr) -> String {
        let id = p.workspace_id.trim();
        if !id.is_empty() && self.store.workspace(id).is_some_and(|w| !w.archived) {
            return id.to_owned();
        }
        if let Some(host) = self.pr_chat_host(&p.id) {
            return host.id;
        }
        id.to_owned()
    }

    /// Changes the title and description. A forge PR is edited there first
    /// (author or write access).
    pub async fn update_pr_details(&self, pr_id: &str, title: &str, body: &str) -> (Pr, bool) {
        let title = title.trim();
        if title.is_empty() {
            return (Pr::default(), false);
        }
        let Some(pr) = self.store.pr(pr_id).filter(|p| p.can_edit_details()) else {
            return (Pr::default(), false);
        };
        if let Some(forge) = self.pr_forge(&pr) {
            return self
                .update_forge_pr_details(forge.as_ref(), pr, title, body)
                .await;
        }
        let updated = self.store.update_pr(pr_id, |p| {
            p.title = title.to_owned();
            upsert_description_comment(p, body);
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            return (Pr::default(), false);
        };
        self.publish_pr_updated(pr_id);
        (updated, true)
    }

    async fn update_forge_pr_details(
        &self,
        forge: &dyn Forge,
        pr: Pr,
        title: &str,
        body: &str,
    ) -> (Pr, bool) {
        let cur_body = comments(&pr)
            .iter()
            .find(|c| c.kind == "description")
            .map(|c| c.body_md.clone())
            .unwrap_or_default();
        let title_changed = title != pr.title.trim();
        let body_changed = body.trim() != cur_body.trim();
        if !title_changed && !body_changed {
            return (pr, true);
        }
        let repo = self.pr_repo_path(&pr);
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.edit_pr(
                Path::new(&repo),
                pr.number,
                title,
                body_changed.then_some(body),
            ),
        )
        .await;
        if let Err(e) = res {
            let what = format!("Could not update the pull request on {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (pr, false);
        }
        let updated = self.store.update_pr(&pr.id, |p| {
            p.title = title.to_owned();
            if body_changed {
                upsert_description_comment(p, body);
            }
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            return (pr, true);
        };
        self.publish_pr_updated(&pr.id);
        (updated, true)
    }

    /// Edits a conversation comment the viewer owns.
    pub async fn update_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> (PrComment, bool) {
        let (body, comment_id) = (body.trim(), comment_id.trim());
        if body.is_empty() || comment_id.is_empty() {
            return (PrComment::default(), false);
        }
        let Some(pr) = self.store.pr(pr_id) else {
            return (PrComment::default(), false);
        };
        match find_pr_comment(comments(&pr), comment_id) {
            Some(i) if pr.can_edit_comment(&comments(&pr)[i]) => {}
            _ => return (PrComment::default(), false),
        }
        if let Some(forge) = self.pr_forge(&pr) {
            return self
                .update_forge_pr_comment(forge.as_ref(), &pr, comment_id, body)
                .await;
        }
        let mut saved = PrComment::default();
        let ok = self.store.update_pr(pr_id, |p| {
            let cs = p.comments.get_or_insert_with(Vec::new);
            let Some(i) = find_pr_comment(cs, comment_id) else {
                return;
            };
            cs[i].body_md = body.to_owned();
            if cs[i].path.is_empty() {
                cs[i].body_html = html_escape(body);
            }
            saved = cs[i].clone();
            touch_pr_updated(p);
        });
        if ok.is_none() || saved.id.is_empty() {
            return (PrComment::default(), false);
        }
        self.publish_pr_thread_updated(pr_id);
        (saved, true)
    }

    async fn update_forge_pr_comment(
        &self,
        forge: &dyn Forge,
        pr: &Pr,
        comment_id: &str,
        body: &str,
    ) -> (PrComment, bool) {
        let repo = self.pr_repo_path(pr);
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.edit_comment(Path::new(&repo), pr.number, comment_id, body),
        )
        .await;
        if let Err(e) = res {
            let what = format!("Could not update the comment on {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (PrComment::default(), false);
        }
        let fallback = || PrComment {
            id: comment_id.to_owned(),
            body_md: body.to_owned(),
            ..PrComment::default()
        };
        let mut saved = PrComment::default();
        let ok = self.store.update_pr(&pr.id, |p| {
            let cs = p.comments.get_or_insert_with(Vec::new);
            let Some(i) = find_pr_comment(cs, comment_id) else {
                return;
            };
            cs[i].body_md = body.to_owned();
            saved = cs[i].clone();
            touch_pr_updated(p);
        });
        if ok.is_none() {
            return (fallback(), true);
        }
        self.publish_pr_thread_updated(&pr.id);
        if saved.id.is_empty() {
            return (fallback(), true);
        }
        (saved, true)
    }

    /// Deletes a line note or conversation comment (and its direct
    /// replies) the viewer may delete.
    pub async fn delete_pr_comment(&self, pr_id: &str, comment_id: &str) -> (Pr, bool) {
        let comment_id = comment_id.trim();
        if comment_id.is_empty() {
            return (Pr::default(), false);
        }
        let Some(pr) = self.store.pr(pr_id) else {
            return (Pr::default(), false);
        };
        match find_pr_comment(comments(&pr), comment_id) {
            Some(i) if pr.can_delete_comment(&comments(&pr)[i]) => {}
            _ => return (Pr::default(), false),
        }
        if let Some(forge) = self.pr_forge(&pr) {
            return self
                .delete_forge_pr_comment(forge.as_ref(), pr, comment_id)
                .await;
        }
        let updated = self.store.update_pr(pr_id, |p| {
            p.comments = Some(remove_pr_comment(comments(p), comment_id));
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            return (Pr::default(), false);
        };
        self.publish_pr_thread_updated(pr_id);
        (updated, true)
    }

    /// Removes the rows locally first (held against stale snapshots), then
    /// deletes on the forge; a forge failure restores them.
    async fn delete_forge_pr_comment(
        &self,
        forge: &dyn Forge,
        pr: Pr,
        comment_id: &str,
    ) -> (Pr, bool) {
        let ids = comment_ids_to_remove(comments(&pr), comment_id);
        let taken = take_pr_comments(comments(&pr), &ids);
        self.remember_dropped_comments(&pr.id, &ids);
        let updated = self.store.update_pr(&pr.id, |p| {
            p.comments = Some(remove_pr_comment(comments(p), comment_id));
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            self.forget_dropped_comments(&pr.id, &ids);
            return (Pr::default(), false);
        };
        self.publish_pr_thread_updated(&pr.id);
        let repo = self.pr_repo_path(&pr);
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.delete_comment(Path::new(&repo), pr.number, comment_id),
        )
        .await;
        if let Err(e) = res {
            self.forget_dropped_comments(&pr.id, &ids);
            let prev = pr.updated_at.clone();
            self.store.update_pr(&pr.id, |p| {
                p.comments.get_or_insert_with(Vec::new).extend(taken);
                p.updated_at = prev;
            });
            self.publish_pr_thread_updated(&pr.id);
            let what = format!("Could not delete the comment on {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (Pr::default(), false);
        }
        (updated, true)
    }

    /// Replies on a line-note thread (conversation comments take no
    /// in-thread replies).
    pub async fn reply_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> (PrComment, bool) {
        let (body, comment_id) = (body.trim(), comment_id.trim());
        if body.is_empty() || comment_id.is_empty() {
            return (PrComment::default(), false);
        }
        let Some(pr) = self.store.pr(pr_id) else {
            return (PrComment::default(), false);
        };
        if pr.conversation_locked() {
            return (PrComment::default(), false);
        }
        let root = find_line_thread_root(comments(&pr), comment_id);
        if root.id.is_empty() || !root.is_line_note() {
            return (PrComment::default(), false);
        }
        if let Some(forge) = self.pr_forge(&pr) {
            return self
                .reply_forge_pr_comment(forge.as_ref(), &pr, &root.id, body)
                .await;
        }
        let mut c = now_comment("AR", "You", &html_escape(body));
        c.body_md = body.to_owned();
        attach_line_reply(&mut c, &root);
        let ok = self.store.update_pr(pr_id, |p| {
            p.comments.get_or_insert_with(Vec::new).push(c.clone());
            touch_pr_updated(p);
        });
        if ok.is_none() {
            return (PrComment::default(), false);
        }
        self.publish_pr_comment(pr_id, &c);
        self.publish_pr_thread_updated(pr_id);
        (c, true)
    }

    async fn reply_forge_pr_comment(
        &self,
        forge: &dyn Forge,
        pr: &Pr,
        comment_id: &str,
        body: &str,
    ) -> (PrComment, bool) {
        let repo = self.pr_repo_path(pr);
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.reply(Path::new(&repo), pr.number, comment_id, body, &pr.url),
        )
        .await;
        let echoed = match res {
            Ok(c) => c,
            Err(e) => {
                let what = format!("Could not reply on {}: ", forge.label());
                self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
                return (PrComment::default(), false);
            }
        };
        let root = find_line_thread_root(comments(pr), comment_id);
        let mut c = echoed.unwrap_or_else(|| {
            let mut c = now_comment("AR", "You", &html_escape(body));
            c.body_md = body.to_owned();
            c.author_login = pr.viewer_login.clone();
            c
        });
        attach_line_reply(&mut c, &root);
        let ok = self.store.update_pr(&pr.id, |p| {
            p.comments.get_or_insert_with(Vec::new).push(c.clone());
            touch_pr_updated(p);
        });
        if ok.is_none() {
            return (PrComment::default(), false);
        }
        self.publish_pr_comment(&pr.id, &c);
        self.publish_pr_thread_updated(&pr.id);
        (c, true)
    }

    /// Adds or removes the viewer's emoji on a conversation item.
    pub async fn toggle_pr_comment_reaction(
        &self,
        pr_id: &str,
        comment_id: &str,
        content: &str,
    ) -> (Pr, bool) {
        let Some(content) = normalize_reaction_content(content) else {
            return (Pr::default(), false);
        };
        let comment_id = comment_id.trim();
        if comment_id.is_empty() {
            return (Pr::default(), false);
        }
        let Some(pr) = self.store.pr(pr_id) else {
            return (Pr::default(), false);
        };
        let Some(i) = find_pr_comment(comments(&pr), comment_id)
            .filter(|&i| pr.can_react_comment(&comments(&pr)[i]))
        else {
            return (Pr::default(), false);
        };
        let next = toggle_reactions(&comments(&pr)[i].reactions, content);
        let adding = reaction_mine(&next, content);
        if let Some(forge) = self.pr_forge(&pr) {
            return self
                .toggle_forge_reaction(forge.as_ref(), &pr, comment_id, content, adding, next)
                .await;
        }
        let updated = self.store.update_pr(pr_id, |p| {
            let cs = p.comments.get_or_insert_with(Vec::new);
            let Some(i) = find_pr_comment(cs, comment_id) else {
                return;
            };
            cs[i].reactions = next;
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            return (Pr::default(), false);
        };
        self.publish_pr_thread_updated(pr_id);
        (updated, true)
    }

    async fn toggle_forge_reaction(
        &self,
        forge: &dyn Forge,
        pr: &Pr,
        comment_id: &str,
        content: &str,
        adding: bool,
        next: Vec<PrReaction>,
    ) -> (Pr, bool) {
        let Some(i) = find_pr_comment(comments(pr), comment_id) else {
            return (Pr::default(), false);
        };
        let Some(subject) = forge
            .reaction_subject(&comments(pr)[i], pr)
            .filter(|s| !s.is_empty())
        else {
            return (Pr::default(), false);
        };
        let repo = self.pr_repo_path(pr);
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.set_reaction(Path::new(&repo), &subject, content, adding),
        )
        .await;
        if let Err(e) = res {
            let what = format!("Could not update the reaction on {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (Pr::default(), false);
        }
        self.remember_comment_reactions(&pr.id, comment_id, &next);
        let updated = self.store.update_pr(&pr.id, |p| {
            let cs = p.comments.get_or_insert_with(Vec::new);
            let Some(i) = find_pr_comment(cs, comment_id) else {
                return;
            };
            cs[i].reactions = next;
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            self.forget_comment_reactions(&pr.id, comment_id);
            return (Pr::default(), false);
        };
        self.publish_pr_thread_updated(&pr.id);
        (updated, true)
    }

    /// Resolves or reopens a line-note thread.
    pub async fn set_pr_thread_resolved(
        &self,
        pr_id: &str,
        thread_id: &str,
        resolved: bool,
    ) -> (Pr, bool) {
        let thread_id = thread_id.trim();
        if thread_id.is_empty() {
            return (Pr::default(), false);
        }
        let Some(pr) = self.store.pr(pr_id).filter(|p| p.can_resolve_thread()) else {
            return (Pr::default(), false);
        };
        let key = resolve_thread_key(comments(&pr), thread_id);
        if key.is_empty() {
            return (Pr::default(), false);
        }
        if let Some(forge) = self.pr_forge(&pr) {
            return self
                .set_forge_thread_resolved(forge.as_ref(), &pr, &key, resolved)
                .await;
        }
        let updated = self.store.update_pr(pr_id, |p| {
            mark_thread_resolved(p.comments.get_or_insert_with(Vec::new), &key, resolved);
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            return (Pr::default(), false);
        };
        self.publish_pr_thread_updated(pr_id);
        (updated, true)
    }

    /// Only a forge thread node resolves on the forge: a key that is a
    /// Recur-local id (`c-…`) or a row id (a note with no thread node) is
    /// refused.
    async fn set_forge_thread_resolved(
        &self,
        forge: &dyn Forge,
        pr: &Pr,
        thread_id: &str,
        resolved: bool,
    ) -> (Pr, bool) {
        if thread_id.starts_with("c-") || comments(pr).iter().any(|c| c.id == thread_id) {
            return (Pr::default(), false);
        }
        let repo = self.pr_repo_path(pr);
        let res = bounded(
            FORGE_WRITE_TIMEOUT,
            forge.set_thread_resolved(Path::new(&repo), thread_id, resolved),
        )
        .await;
        if let Err(e) = res {
            let what = format!("Could not update the conversation on {}: ", forge.label());
            self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(&what, &e));
            return (Pr::default(), false);
        }
        self.remember_thread_resolved(&pr.id, thread_id, resolved);
        let updated = self.store.update_pr(&pr.id, |p| {
            mark_thread_resolved(p.comments.get_or_insert_with(Vec::new), thread_id, resolved);
            touch_pr_updated(p);
        });
        let Some(updated) = updated else {
            self.forget_thread_resolved(&pr.id, thread_id);
            return (Pr::default(), false);
        };
        self.publish_pr_thread_updated(&pr.id);
        (updated, true)
    }

    /// Marks a forge draft PR ready for review.
    pub async fn ready_pr(&self, pr_id: &str) -> (Pr, bool) {
        self.set_pr_draft(pr_id, false).await
    }

    /// Converts a PR to a draft or marks it ready. A forge PR flips on the
    /// forge; a Local PR keeps the flag so a later forge open keeps the
    /// intent.
    pub async fn set_pr_draft(&self, pr_id: &str, draft: bool) -> (Pr, bool) {
        let Some(pr) = self.store.pr(pr_id) else {
            return (Pr::default(), false);
        };
        if pr.state == "Merged" || pr.state == "Closed" || pr.group == "merged" {
            self.emit_workspace_event(
                &pr.workspace_id,
                "MA",
                "Maestro",
                "Can't change draft state on a merged or closed PR.",
            );
            return (pr, false);
        }
        if pr.draft == draft {
            return (pr, true);
        }
        if let Some(forge) = self.pr_forge(&pr) {
            if pr.number <= 0 {
                self.emit_workspace_event(
                    &pr.workspace_id,
                    "MA",
                    "Maestro",
                    &format!(
                        "Only {} PRs can be converted to or from draft.",
                        forge.label()
                    ),
                );
                return (pr, false);
            }
            let repo = self.pr_repo_path(&pr);
            let res = bounded(
                FORGE_DRAFT_TIMEOUT,
                forge.set_draft(Path::new(&repo), pr.number, draft),
            )
            .await;
            if let Err(e) = res {
                let verb = if draft {
                    "Convert to draft failed: "
                } else {
                    "Mark ready failed: "
                };
                self.emit_workspace_event(&pr.workspace_id, "MA", "Maestro", &could_not(verb, &e));
                return (pr, false);
            }
        }
        self.store.update_pr(pr_id, |p| {
            p.draft = draft;
            touch_pr_updated(p);
        });
        // The conversation snapshot is not awaited: pr.updated already makes
        // the UI refetch.
        self.publish_pr_updated(pr_id);
        match self.store.pr(pr_id) {
            Some(updated) => (updated, true),
            None => (pr, true),
        }
    }

    /// People who can be @mentioned on a forge PR (empty for Local PRs).
    pub async fn search_pr_mentionable_users(&self, pr_id: &str, query: &str) -> Vec<PrParticipant> {
        let Some(pr) = self.store.pr(pr_id) else {
            return Vec::new();
        };
        let Some(forge) = self.pr_forge(&pr) else {
            return Vec::new();
        };
        let repo = self.pr_repo_path(&pr);
        tokio::time::timeout(
            MENTION_TIMEOUT,
            forge.mentionable_users(Path::new(&repo), &pr.url, query.trim()),
        )
        .await
        .unwrap_or_default()
    }
}
