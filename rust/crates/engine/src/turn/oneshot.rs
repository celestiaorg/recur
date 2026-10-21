//! One-shot title-model turns (Go `commit_message.go`): commit messages,
//! PR titles and descriptions, and the loop's engine commit subject. These
//! turns have no role, so they are neither mirrored nor broadcast; they run
//! with tools disabled (or the read-only trio for providers that cannot
//! disable tools) and one turn.

use std::time::Duration;

use recur_agent::{ProviderError, TurnRequest};
use recur_domain::{PERMISSION_MODE_DENY, parse_model_ref};
use tokio_util::sync::CancellationToken;

use super::AgentSpec;
use super::finalize::DEADLINE_EXCEEDED;
use crate::text::{hard_truncate_runes, truncate};
use crate::{Engine, EngineError};

const COMMIT_MESSAGE_TIMEOUT: Duration = Duration::from_secs(45);
/// Staged-diff bytes sent to the title model.
pub const COMMIT_MESSAGE_DIFF_CAP: usize = 12000;
const COMMIT_MESSAGE_MAX_LEN: usize = 500;

pub const COMMIT_MESSAGE_SYSTEM_PROMPT: &str = "You write git commit messages from a staged diff. Reply with ONLY the commit message: a short subject line (at most ~72 characters), optionally followed by a blank line and a brief body. Match the style of the recent commit subjects when provided. No quotes, no markdown fences, no explanation, no preamble. Do not call tools, read files, or explore the codebase — use the diff text alone.";

const PR_DESCRIPTION_TIMEOUT: Duration = Duration::from_secs(45);

pub const PR_DESCRIPTION_SYSTEM_PROMPT: &str = "You write pull request titles and descriptions from a branch diff. Describe ALL changes versus the base branch, not one commit or chat turn. Reply with ONLY a title line (at most 80 characters), a blank line, and a markdown body. Keep the body under five sentences unless the change is large. No quotes, no markdown fences around the whole reply, no greeting, no tools.";

/// GitHub's PR body limit (Go `githubPRBodyMax`).
pub const GITHUB_PR_BODY_MAX: usize = 60000;

/// Caps a PR body to the forge limit with a truncation note.
pub fn cap_github_pr_body(s: &str) -> String {
    if s.chars().count() <= GITHUB_PR_BODY_MAX {
        return s.to_owned();
    }
    format!(
        "{}\n\n…(truncated)…",
        hard_truncate_runes(s, GITHUB_PR_BODY_MAX)
    )
}

/// The engine commit subject cap.
pub const LOOP_COMMIT_SUMMARY_MAX: usize = 72;
const LOOP_COMMIT_FILE_HINT_MAX: usize = 3;

/// Why a one-shot failed: the provider's error, or a timeout.
fn deadline_error(partial: recur_agent::TurnResult) -> ProviderError {
    ProviderError::failed(DEADLINE_EXCEEDED).with_partial(partial)
}

impl Engine {
    /// A one-shot turn on the Settings workspace-title model with a deadline
    /// (Go `context.WithTimeout`): an expiry reads "context deadline
    /// exceeded", a caller cancel stays a cancel.
    async fn run_oneshot(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        worktree: &str,
        system_prompt: &str,
        prompt: String,
        timeout: Duration,
    ) -> Result<recur_agent::TurnResult, ProviderError> {
        let set = self.settings();
        let r = set.workspace_title_model();
        let model = r.to_string();
        let provider = self.providers.for_model(&model);
        let mut flags = set.workspace_title_flags();
        if flags.is_empty()
            && let Some(p) = &provider
        {
            flags = p.oneshot_flags(&parse_model_ref(&model).id);
        }
        let mut req = TurnRequest {
            // Attributes the turn; no role, so no mirror and no broadcast.
            workspace_id: workspace_id.to_owned(),
            model,
            flags,
            system_prompt: system_prompt.to_owned(),
            prompt,
            worktree: worktree.into(),
            // Exempt from the global permission mode: nothing to call, no
            // MCP endpoint, nobody to ask.
            permission_mode: PERMISSION_MODE_DENY.into(),
            tools: Some(String::new()),
            max_turns: 1,
            skip_thinking: true,
            ..TurnRequest::default()
        };
        if provider.is_some_and(|p| !p.caps().can_disable_tools) {
            req.allowed_tools = Some(vec!["Read".into(), "Glob".into(), "Grep".into()]);
        }
        let turn = cancel.child_token();
        let timer = {
            let t = turn.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                t.cancel();
                true
            })
        };
        let out = self.run_agent(&turn, AgentSpec::new(req)).await;
        timer.abort();
        let expired = turn.is_cancelled() && !cancel.is_cancelled();
        match out {
            Err(e) if expired && e.is_canceled() => Err(deadline_error(*e.partial)),
            other => other,
        }
    }

    /// A sanitized commit message for a staged diff, from a cheap one-shot
    /// turn. `recent_subjects` (newest first) are style hints.
    pub async fn generate_commit_message(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        worktree: &str,
        diff: &str,
        recent_subjects: &[String],
    ) -> Result<String, EngineError> {
        let diff = diff.trim();
        if diff.is_empty() {
            return Err(EngineError::new(
                "stage changes to generate a commit message",
            ));
        }
        let res = self
            .run_oneshot(
                cancel,
                workspace_id,
                worktree,
                COMMIT_MESSAGE_SYSTEM_PROMPT,
                build_commit_message_prompt(diff, recent_subjects),
                COMMIT_MESSAGE_TIMEOUT,
            )
            .await;
        self.publish_resource();
        let res = res.map_err(|e| EngineError(format!("generate commit message: {e}")))?;
        if res.is_error {
            return Err(EngineError::new("generate commit message failed"));
        }
        let msg = sanitize_commit_message(&res.text, COMMIT_MESSAGE_MAX_LEN);
        if msg.is_empty() {
            return Err(EngineError::new("empty commit message from model"));
        }
        Ok(msg)
    }

    /// The `/open-pr` one-shot: a workspace-wide title and body from the
    /// merge-base diff. An empty diff or reply is an error so callers fall
    /// back to a factual template.
    pub async fn generate_pr_description(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        worktree: &str,
        diff: &str,
        commits: &[String],
    ) -> Result<(String, String), EngineError> {
        let diff = diff.trim();
        if diff.is_empty() {
            return Err(EngineError::new("empty diff"));
        }
        let res = self
            .run_oneshot(
                cancel,
                workspace_id,
                worktree,
                PR_DESCRIPTION_SYSTEM_PROMPT,
                build_pr_description_prompt(diff, commits),
                PR_DESCRIPTION_TIMEOUT,
            )
            .await;
        self.publish_resource();
        let res = res.map_err(|e| EngineError(format!("generate PR description: {e}")))?;
        if res.is_error {
            return Err(EngineError::new("generate PR description failed"));
        }
        let (title, body) = parse_pr_description(&res.text);
        if title.is_empty() && body.is_empty() {
            return Err(EngineError::new("empty PR description from model"));
        }
        Ok((title, body))
    }
}

pub fn build_pr_description_prompt(diff: &str, commits: &[String]) -> String {
    let mut b = String::from(
        "Write a pull request title and description covering every change versus the base.\n\n",
    );
    if !commits.is_empty() {
        b.push_str("Commit subjects (newest first):\n");
        for s in commits {
            b.push_str("- ");
            b.push_str(s);
            b.push('\n');
        }
        b.push('\n');
    }
    b.push_str("Diff:\n\n");
    b.push_str(&cap_diff_covering_files(diff, COMMIT_MESSAGE_DIFF_CAP));
    b
}

/// The title line (≤ 80 runes, quotes unwrapped) and the capped body.
pub fn parse_pr_description(raw: &str) -> (String, String) {
    let s = unwrap_pr_reply(raw);
    if s.is_empty() {
        return (String::new(), String::new());
    }
    let (title_line, rest) = match s.split_once('\n') {
        Some((t, r)) => (t, Some(r)),
        None => (s.as_str(), None),
    };
    let title = hard_truncate_runes(
        &recur_forge::model::single_line(&unwrap_line_quotes(title_line)),
        80,
    )
    .to_owned();
    let body = match rest {
        Some(r) => {
            let b = r.trim();
            if b.is_empty() {
                String::new()
            } else {
                cap_github_pr_body(b)
            }
        }
        None => String::new(),
    };
    (title, body)
}

/// Strips a wrapper around the whole reply (an outer fence, or matching
/// quotes); inner fences stay.
fn unwrap_pr_reply(raw: &str) -> String {
    let mut s = raw.trim().to_owned();
    if s.is_empty() {
        return s;
    }
    if s.starts_with("```") && s.ends_with("```") && s.len() > 6 {
        let mut inner = s[3..s.len() - 3].trim().to_owned();
        if let Some(i) = inner.find('\n') {
            let tag = inner[..i].trim();
            if !tag.is_empty() && !tag.contains([' ', '\t']) {
                inner = inner[i + 1..].trim().to_owned();
            }
        }
        s = inner;
    }
    unwrap_line_quotes(&s)
}

fn unwrap_line_quotes(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2 {
        let q = b[0];
        if (q == b'"' || q == b'\'')
            && b[b.len() - 1] == q
            && b.iter().filter(|c| **c == q).count() == 2
        {
            return s[1..s.len() - 1].trim().to_owned();
        }
    }
    s.to_owned()
}

/// Bounds a unified diff so every file still appears.
pub fn cap_diff_covering_files(diff: &str, cap: usize) -> String {
    if cap == 0 || diff.len() <= cap {
        return diff.to_owned();
    }
    let parts = split_unified_diff_files(diff);
    if parts.len() <= 1 {
        return truncate_diff(diff, cap);
    }
    let per = (cap / parts.len()).max(64);
    parts
        .iter()
        .map(|p| cap_one_diff_file(p, per))
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_unified_diff_files(diff: &str) -> Vec<&str> {
    if diff.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0;
    while let Some(rel) = diff[start..].find("\ndiff --git ") {
        parts.push(&diff[start..start + rel]);
        start += rel + 1;
    }
    parts.push(&diff[start..]);
    parts
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn cap_one_diff_file(p: &str, per: usize) -> String {
    let p = p.trim_end_matches('\n');
    if p.len() <= per {
        return p.to_owned();
    }
    let mut cut = per;
    if let Some(i) = p.find('\n')
        && cut < i + 1
    {
        cut = i + 1;
    }
    if cut >= p.len() {
        return p.to_owned();
    }
    let cut = floor_char_boundary(p, cut);
    format!("{}\n[truncated]", p[..cut].trim_end_matches('\n'))
}

fn truncate_diff(diff: &str, cap: usize) -> String {
    if diff.len() <= cap {
        return diff.to_owned();
    }
    let cut = floor_char_boundary(diff, cap);
    format!("{}\n\n[diff truncated]", diff[..cut].trim_end_matches('\n'))
}

pub fn build_commit_message_prompt(diff: &str, recent_subjects: &[String]) -> String {
    let mut b = String::new();
    if !recent_subjects.is_empty() {
        b.push_str("Recent commit subjects (newest first):\n");
        for s in recent_subjects {
            b.push_str("- ");
            b.push_str(s);
            b.push('\n');
        }
        b.push('\n');
    }
    b.push_str("Staged diff:\n\n");
    if diff.len() > COMMIT_MESSAGE_DIFF_CAP {
        // Go sliced bytes here; keep it on a rune start so the prompt
        // stays valid text.
        let cut = floor_char_boundary(diff, COMMIT_MESSAGE_DIFF_CAP);
        b.push_str(&diff[..cut]);
        b.push_str("\n\n[diff truncated]");
    } else {
        b.push_str(diff);
    }
    b
}

/// A model reply as a usable commit message: fences, quotes and backticks
/// stripped, multi-line bodies kept, capped.
pub fn sanitize_commit_message(raw: &str, max_len: usize) -> String {
    let mut s = raw.trim().to_owned();
    if s.is_empty() {
        return s;
    }
    if let Some(rest) = s.strip_prefix("```") {
        let mut rest = rest.to_owned();
        if let Some(i) = rest.find('\n') {
            rest = rest[i + 1..].to_owned();
        }
        if let Some(j) = rest.rfind("```") {
            rest.truncate(j);
        }
        s = rest.trim().to_owned();
    }
    let s = s.trim_matches(['"', '\'', '`']).trim();
    if s.is_empty() {
        return String::new();
    }
    if max_len > 0 && s.len() > max_len {
        return truncate(s, max_len);
    }
    s.to_owned()
}

/// The engine commit subject for one coding iteration: the Coder's done
/// summary when it has one, else a short goal headline (issue `Title:` /
/// first line) plus this iteration's files.
pub fn loop_commit_message(done_summary: &str, goal: &str, files: &[String]) -> String {
    let mut s = sanitize_loop_commit_summary(done_summary);
    let from_done = !s.is_empty();
    if !from_done {
        s = commit_summary_from_goal(goal);
    }
    if s.is_empty() {
        s = "changes".into();
    }
    if !from_done {
        s = append_file_hint(&s, files);
    }
    s
}

fn append_file_hint(summary: &str, files: &[String]) -> String {
    let hint = file_hint(files);
    if hint.is_empty() {
        return summary.to_owned();
    }
    let overhead = " ()".chars().count() + hint.chars().count();
    let room = LOOP_COMMIT_SUMMARY_MAX as i64 - overhead as i64;
    if room < 40 && summary.chars().count() as i64 > room {
        return summary.to_owned();
    }
    let summary = hard_truncate_runes(summary, room.max(0) as usize);
    format!("{summary} ({hint})")
}

fn sanitize_loop_commit_summary(s: &str) -> String {
    let mut s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    if let Some(i) = s.find(['\n', '\r']) {
        s = s[..i].trim();
    }
    let s = s.trim_start_matches('#').trim();
    let s = s.trim_matches(['`', '"', '\'']).trim();
    if s.is_empty() {
        return String::new();
    }
    hard_truncate_runes(s, LOOP_COMMIT_SUMMARY_MAX).to_owned()
}

fn commit_summary_from_goal(goal: &str) -> String {
    let (ident, title) = recur_issues::goal_meta(goal);
    if !title.is_empty() {
        if !ident.is_empty() {
            return sanitize_loop_commit_summary(&format!("{ident}: {title}"));
        }
        return sanitize_loop_commit_summary(&title);
    }
    sanitize_loop_commit_summary(&recur_agent::error::first_line(goal))
}

/// A worktree's changed paths, sorted.
pub async fn status_paths(worktree: &str) -> Vec<String> {
    let Some(st) = recur_git::status_map(worktree).await else {
        return Vec::new();
    };
    let mut paths: Vec<String> = st
        .keys()
        .filter(|p| !p.trim().is_empty())
        .cloned()
        .collect();
    paths.sort();
    paths
}

/// Up to three sorted unique basenames, plus `+N` for the rest.
pub fn file_hint(paths: &[String]) -> String {
    let mut bases = unique_basenames(paths);
    if bases.is_empty() {
        return String::new();
    }
    let extra = bases.len().saturating_sub(LOOP_COMMIT_FILE_HINT_MAX);
    bases.truncate(LOOP_COMMIT_FILE_HINT_MAX);
    let mut s = bases.join(", ");
    if extra > 0 {
        s.push_str(&format!(" +{extra}"));
    }
    s
}

fn unique_basenames(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let b = recur_bundles::go::base(&p.trim().replace('\\', "/"));
        if b.is_empty() || b == "." || b == "/" || out.contains(&b) {
            continue;
        }
        out.push(b);
    }
    out.sort();
    out
}

#[cfg(test)]
#[path = "oneshot_tests.rs"]
mod tests;
