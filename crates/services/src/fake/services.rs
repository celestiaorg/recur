//! Every service method's default: seed reads, in-memory mutations that
//! return a shape-correct value so the UI's optimistic updates work, canned
//! answers, or an empty value. A record method with no default answer fails
//! with [`unanswered`], so specs that need one override it.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use recur_domain::{
    Agent, AgentSkill, Artifact, AttentionCounts, BranchChoice, BriefAction, BriefingCounts,
    BriefingSnapshot, ChatAttachment, ChatItem, Commit, CompatibleFlagDim, CompatibleProvider,
    CopyFilesPreview, DiffLine, DiffStat, Escalation, FeedItem, FileContent, FileDiff,
    FileDiffResult, FileSearchHit, FileSearchResult, Finding, Group, InboxItem, IssueRef,
    OpenInTarget, OpenPathResult, OutlineSymbol, PendingLineComment, PermissionRequest, Policy, Pr,
    PrComment, PrParticipant, PrReaction, Project, ProjectGroup, ReplaceResult, Resource,
    ScheduledPrompt, SearchHit, SearchOpts, SearchResult, SessionView, Settings, SymbolDef,
    SymbolRef, TreeEntry, UpdateWorkspaceResult, UserQuestion, Workspace, WorkspaceBrief,
    WorkspaceChatTab, WorkspaceLandState, WorkspaceState, WorktreeCommitResult,
};
use recur_engine::chat::tabs::apply_chat_tab_order;
use recur_engine::view::chat_surface::peel_chat_surface;
use recur_term::Info as TermInfo;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::attention::parse_chat_inbox_id;
use super::{Answer, Fake, Own, Seed, fake_services};
use crate::api::*;
use crate::{Error, Result};

/// The error a record method with no default answer gives.
fn unanswered<T>() -> Result<T> {
    Err(Error::new("not answered by the fake"))
}

/// A canned catalog written as JSON for brevity.
fn canned<T: DeserializeOwned>(v: serde_json::Value) -> T {
    serde_json::from_value(v).expect("canned fake data matches its type")
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Seed dumps omit `worktreePath`; the Observatory will not list a tree
/// without one.
fn with_mock_worktree(w: &Workspace) -> Workspace {
    let mut w = w.clone();
    if !w.archived && w.worktree_path.is_empty() && (w.phase != "failed" || w.last_error.is_empty())
    {
        let id = if w.id.is_empty() { "ws" } else { &w.id };
        w.worktree_path = format!("/tmp/recur-e2e/{id}");
    }
    w
}

fn or<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    if s.is_empty() { fallback } else { s }
}

static COMMENT_SEQ: AtomicU64 = AtomicU64::new(0);

/// A fresh "You commented" thread entry.
fn now_comment(body_html: &str) -> PrComment {
    let seq = COMMENT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    PrComment {
        id: format!("c-{}-{seq}", now_ms()),
        code: "AR".into(),
        author: "You".into(),
        verb: "commented".into(),
        avatar_css: "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)".into(),
        time: "now".into(),
        body_html: body_html.into(),
        ..Default::default()
    }
}

fn branch_choices(names: &[&str]) -> Vec<BranchChoice> {
    let mut uniq: Vec<&str> = Vec::new();
    for n in names {
        if !uniq.contains(n) {
            uniq.push(n);
        }
    }
    let choice = |n: &&str, group: &str| BranchChoice {
        name: (*n).to_owned(),
        group: group.into(),
    };
    let remote = uniq.iter().filter(|n| n.starts_with("origin/"));
    let local = uniq.iter().filter(|n| !n.starts_with("origin/"));
    remote
        .map(|n| choice(n, "origin"))
        .chain(local.map(|n| choice(n, "Local")))
        .collect()
}

fn reorder<T: Clone>(list: &[T], ids: &[String], id: impl Fn(&T) -> &str) -> Vec<T> {
    ids.iter()
        .filter_map(|i| list.iter().find(|x| id(x) == i).cloned())
        .collect()
}

fn local_branch(base: &str) -> String {
    let b = or(base, "main");
    or(b.strip_prefix("origin/").unwrap_or(b), "main").to_owned()
}

fn main_workspace(id: String, project_id: &str, branch: Option<String>) -> Workspace {
    with_mock_worktree(&Workspace {
        id,
        project_id: project_id.to_owned(),
        r#type: "main".into(),
        is_main: true,
        short: "Working copy".into(),
        goal: "Working copy".into(),
        branch: branch.unwrap_or_default(),
        status_label: "Idle".into(),
        ..Default::default()
    })
}

fn mock_briefing(s: &Seed) -> BriefingSnapshot {
    let mut workspaces = Vec::new();
    for l in &s.workspaces {
        if l.archived || l.is_main {
            continue;
        }
        let chat_note = s
            .inbox
            .iter()
            .find(|it| it.unread && it.kind == "chat" && it.workspace_id == l.id);
        let (group, state) = (l.group.as_str(), l.state.as_str());
        let (bucket, reason, next) =
            if group == "needs" || state == "escalated" || state == "failed" {
                let reason = if state == "failed" {
                    "failure"
                } else {
                    "question"
                };
                ("needs", reason, "Answer the prompt in this workspace")
            } else if chat_note.is_some() {
                ("needs", "unseen_chat", "Read the response")
            } else if group == "active" {
                ("working", "active_agent", "Let the agent finish this turn")
            } else if group == "ready" {
                (
                    "pickup",
                    "unfinished_plan",
                    "Pick this up where you left off",
                )
            } else {
                continue;
            };
        let steps = l.plan.as_ref().map(|p| p.steps.as_slice()).unwrap_or(&[]);
        let plan_done = steps
            .iter()
            .filter(|st| matches!(st.status.as_str(), "done" | "skipped"))
            .count();
        let target = match chat_note.map(|n| &n.target) {
            Some(t) if reason == "unseen_chat" => t.clone(),
            _ => format!("workspace:{}", l.id),
        };
        workspaces.push(WorkspaceBrief {
            workspace_id: l.id.clone(),
            project_id: l.project_id.clone(),
            project_name: l.project_id.clone(),
            title: or(&l.short, &l.goal).to_owned(),
            branch: l.branch.clone(),
            bucket: bucket.into(),
            reasons: Some(vec![reason.into()]),
            now: or(&l.last, or(&l.status_label, &l.phase)).to_owned(),
            next: next.into(),
            last_activity_at: now_ms(),
            last_summary: l.last.clone(),
            state: state.to_owned(),
            phase: l.phase.clone(),
            diff_add: l.diff.add,
            diff_del: l.diff.del,
            plan_done: plan_done as i64,
            plan_total: steps.len() as i64,
            primary: BriefAction {
                label: if reason == "unseen_chat" {
                    "Read"
                } else {
                    "Open"
                }
                .into(),
                target,
                kind: "open".into(),
            },
            secondary: if bucket == "pickup" {
                vec![BriefAction {
                    label: "Continue chat".into(),
                    target: format!("chat:{}:PA", l.id),
                    kind: "continue".into(),
                }]
            } else {
                Vec::new()
            },
            ..Default::default()
        });
    }
    let count = |b: &str| workspaces.iter().filter(|w| w.bucket == b).count() as i64;
    BriefingSnapshot {
        generated_at: now_ms(),
        counts: BriefingCounts {
            needs: count("needs"),
            working: count("working"),
            pickup: count("pickup"),
        },
        workspaces: Some(workspaces),
    }
}

fn tab(code: &str, title: &str, updated_at: i64) -> WorkspaceChatTab {
    WorkspaceChatTab {
        code: code.into(),
        title: title.into(),
        updated_at,
        ..Default::default()
    }
}

fn loop_tab(updated_at: i64) -> WorkspaceChatTab {
    WorkspaceChatTab {
        mode: "loop".into(),
        ..tab("MA", "Loop", updated_at)
    }
}

/// A workspace with an engaged run leads with the Loop (MA) tab, mirroring
/// `Engine.ListWorkspaceChats`.
fn list_workspace_chats(s: &Seed, ws_id: &str) -> Vec<WorkspaceChatTab> {
    let has_run = s
        .workspace(ws_id)
        .is_some_and(|w| w.run_started || or(&w.r#type, "loop") == "loop");
    let extra = s.chats.get(ws_id).map(Vec::as_slice).unwrap_or(&[]);
    let pa = extra
        .iter()
        .find(|t| t.code == "PA")
        .cloned()
        .unwrap_or_else(|| tab("PA", "New chat", 0));
    let mut tabs = Vec::new();
    if has_run && !s.hidden_loop_tabs.contains(ws_id) {
        tabs.push(loop_tab(0));
    }
    tabs.push(pa);
    tabs.extend(
        extra
            .iter()
            .filter(|t| !matches!(t.code.as_str(), "PA" | "MA"))
            .cloned(),
    );
    let order = s.chat_order.get(ws_id).map(Vec::as_slice).unwrap_or(&[]);
    apply_chat_tab_order(tabs, order)
        .into_iter()
        .map(|mut t| {
            if let Some((loop_id, policy_id)) =
                s.chat_loops.get(&(ws_id.to_owned(), t.code.clone()))
            {
                t.loop_id.clone_from(loop_id);
                t.policy_id.clone_from(policy_id);
            }
            t
        })
        .collect()
}

fn app_tabs(extra: &[WorkspaceChatTab]) -> Vec<WorkspaceChatTab> {
    let ask = |t: &WorkspaceChatTab| WorkspaceChatTab {
        mode: "ask".into(),
        ..t.clone()
    };
    std::iter::once(ask(&tab("APP", "Ask Recur", 0)))
        .chain(extra.iter().map(ask))
        .collect()
}

fn created_workspace(
    project_id: &str,
    base_branch: &str,
    mode: &str,
    with: Option<(&str, &str)>,
) -> Workspace {
    let base = or(base_branch, "main").to_owned();
    if mode == "loop" {
        let (loop_id, policy_id) = with.unwrap_or_default();
        return Workspace {
            id: "LOOP-NEW".into(),
            project_id: project_id.into(),
            run_started: true,
            state: WorkspaceState::Queued,
            status_label: "Creating".into(),
            phase: "creating".into(),
            branch: "recur/new".into(),
            base_branch: base,
            loop_id: or(loop_id, "recur.loops.pr").into(),
            policy_id: or(policy_id, "recur.policies.balanced").into(),
            ..Default::default()
        };
    }
    let (loop_id, policy_id) = with.unwrap_or_default();
    Workspace {
        id: "WS-NEW".into(),
        project_id: project_id.into(),
        r#type: "solo".into(),
        status_label: "Creating".into(),
        phase: "creating".into(),
        branch: "recur/new".into(),
        base_branch: base,
        loop_id: loop_id.into(),
        policy_id: policy_id.into(),
        ..Default::default()
    }
}

fn land_state(s: &Seed, id: &str, other_base: &str) -> WorkspaceLandState {
    if s.workspace(id).is_some_and(|w| w.is_main) {
        return WorkspaceLandState {
            base: "origin/main".into(),
            remote: "origin".into(),
            ..Default::default()
        };
    }
    WorkspaceLandState {
        base: other_base.into(),
        ..Default::default()
    }
}

fn workspace_or_empty(s: &Seed, id: &str) -> Workspace {
    s.workspace(id).cloned().unwrap_or_default()
}

fn updated(status: &str, workspace: Workspace, base: &str) -> UpdateWorkspaceResult {
    UpdateWorkspaceResult {
        status: status.into(),
        workspace,
        base: base.into(),
        ..Default::default()
    }
}

fn file_diff() -> FileDiff {
    let line = |old: &str, new: &str, ty: &str, code: &str| DiffLine {
        old: old.into(),
        new: new.into(),
        r#type: ty.into(),
        code_html: code.into(),
        ..Default::default()
    };
    FileDiff {
        path: "src/main.go".into(),
        status: "m".into(),
        diff: DiffStat { add: 1, del: 0 },
        lines: Some(vec![
            line("", "", "hunk", "@@ -1,1 +1,2 @@"),
            line("1", "1", "ctx", "package main"),
            line("", "2", "add", "// edited"),
        ]),
        ..Default::default()
    }
}

fn skills() -> Vec<AgentSkill> {
    let skill = |name: &str, description: &str, scope: &str| AgentSkill {
        name: name.into(),
        description: description.into(),
        scope: scope.into(),
        user_invocable: true,
        ..Default::default()
    };
    vec![
        skill("design-audit", "Audit UI polish", "workspace"),
        skill("create-rule", "Create a Cursor rule", "user"),
    ]
}

fn commit_result() -> WorktreeCommitResult {
    WorktreeCommitResult {
        hash: "abc1234".into(),
        committed: true,
        additions: 1,
        deletions: 0,
    }
}

fn cursor_status(version: &str) -> CursorStatus {
    CursorStatus {
        bridge_installed: true,
        bridge_path: "/tmp/cursor-sdk-bridge".into(),
        bridge_version: version.into(),
        pinned_version: version.into(),
        update_available: false,
        authenticated: true,
        account: "dev@example.com".into(),
        api_key_name: "Recur laptop".into(),
        email: "dev@example.com".into(),
        name: "Dev User".into(),
        model_count: 12,
        error: String::new(),
    }
}

fn empty_usage() -> Snapshot {
    Snapshot {
        providers: Some(Vec::new()),
        ..Default::default()
    }
}

fn model_catalog() -> Vec<ModelGroup> {
    let effort = json!({
        "id": "effort",
        "label": "Effort",
        "options": [
            { "value": "low", "label": "Low" },
            { "value": "medium", "label": "Medium" },
            { "value": "high", "label": "High", "default": true },
            { "value": "xhigh", "label": "Extra High" },
            { "value": "max", "label": "Max" },
        ],
    });
    let context = json!({
        "id": "context",
        "label": "Context",
        "options": [
            { "value": "200k", "label": "200K", "default": true },
            { "value": "1m", "label": "1M" },
        ],
    });
    let both = json!([effort, context]);
    let e = json!([effort]);
    canned(json!([{
        "provider": "claude-code",
        "label": "Claude Code",
        "models": [
            { "ref": "claude-code/claude-fable-5-1", "label": "Fable 5.1", "flags": e, "contextWindow": 1_000_000 },
            { "ref": "claude-code/claude-fable-5", "label": "Fable 5", "flags": e, "contextWindow": 1_000_000 },
            { "ref": "claude-code/claude-opus-5-5", "label": "Opus 5.5", "flags": e, "contextWindow": 1_000_000 },
            { "ref": "claude-code/claude-opus-5", "label": "Opus 5", "flags": e, "contextWindow": 1_000_000 },
            { "ref": "claude-code/claude-opus-4-6", "label": "Opus 4.6", "flags": both },
            { "ref": "claude-code/claude-sonnet-5-5", "label": "Sonnet 5.5", "flags": e, "contextWindow": 1_000_000 },
            { "ref": "claude-code/claude-sonnet-5", "label": "Sonnet 5", "flags": e, "contextWindow": 1_000_000 },
            { "ref": "claude-code/claude-sonnet-4-6", "label": "Sonnet 4.6", "flags": both },
            { "ref": "claude-code/claude-haiku-4-5-20251001", "label": "Haiku 4.5", "flags": e, "contextWindow": 200_000 },
        ],
    }]))
}

fn prompt_actions() -> Vec<PromptActionInfo> {
    let p = |id: &str,
             name: &str,
             desc: &str,
             slash: &str,
             mode: &str,
             timeout: i64,
             interp: &str,
             extra: serde_json::Value| {
        let mut v = json!({
            "id": id, "name": name, "description": desc, "slash": slash, "mode": mode,
            "timeoutMs": timeout, "interpreter": interp, "bundleName": "Chat", "sourceRoot": "default",
        });
        if let (Some(m), serde_json::Value::Object(e)) = (v.as_object_mut(), extra) {
            m.extend(e);
        }
        v
    };
    let ops = json!({ "freshSession": true, "tabGroup": "ops" });
    canned(json!([
        p(
            "recur.chat.fix-findings",
            "Fix Findings",
            "Fix or dispute open reviewer findings",
            "fix-findings",
            "agent",
            0,
            "typescript",
            ops.clone()
        ),
        p(
            "recur.chat.commit",
            "Commit",
            "Stage and commit all worktree changes",
            "commit",
            "agent",
            0,
            "typescript",
            ops.clone()
        ),
        p(
            "recur.chat.commit-push",
            "Commit Push",
            "Commit all changes and push to remote",
            "commit-push",
            "agent",
            0,
            "typescript",
            ops.clone()
        ),
        p(
            "recur.chat.open-pr",
            "Open PR",
            "Open a GitHub pull request with an agent-written description",
            "open-pr",
            "agent",
            15000,
            "shebang",
            ops.clone()
        ),
        p(
            "recur.chat.open-draft-pr",
            "Open draft PR",
            "Open a GitHub draft pull request with an agent-written description",
            "open-draft-pr",
            "agent",
            15000,
            "shebang",
            ops
        ),
        p(
            "recur.chat.fix-checks",
            "Fix checks",
            "Investigate and fix failing GitHub checks on this pull request",
            "fix-checks",
            "agent",
            15000,
            "shebang",
            json!({
                "takesArgs": true,
                "hasComplete": true,
                "freshSession": true,
                "tabGroup": "ops",
                "argPlaceholder": "all or a failing check",
                "args": [{ "name": "check", "placeholder": "all or a failing check", "optional": true }],
            })
        ),
        p(
            "recur.chat.review",
            "Review",
            "Review the current surface (workspace or pull request) and file findings",
            "review",
            "review",
            15000,
            "shebang",
            json!({ "freshSession": true, "tabGroup": "review" })
        ),
        p(
            "recur.chat.re-review",
            "Re-review",
            "Re-review the current surface after fixes",
            "re-review",
            "review",
            15000,
            "shebang",
            json!({ "freshSession": true })
        ),
        p(
            "recur.chat.simplify-review",
            "Simplify review",
            "Review the current surface for over-engineering and file findings",
            "simplify-review",
            "review",
            30000,
            "shebang",
            json!({ "freshSession": true, "tabGroup": "simplify-review" })
        ),
        p(
            "recur.chat.simplify-audit",
            "Simplify audit",
            "Audit the whole repository for over-engineering and file findings",
            "simplify-audit",
            "review",
            30000,
            "shebang",
            json!({ "freshSession": true, "tabGroup": "simplify-audit" })
        ),
        p(
            "recur.chat.review-and-fix",
            "Review & fix",
            "Review the current worktree and fix until the bar passes",
            "review-and-fix",
            "loop",
            0,
            "shebang",
            json!({ "loop": "recur.loops.converge" })
        ),
        p(
            "recur.chat.review-to-pr",
            "Review & fix to a PR",
            "Review and fix the current diff, then open a PR",
            "review-to-pr",
            "loop",
            0,
            "shebang",
            json!({ "loop": "recur.loops.converge-pr" })
        ),
        p(
            "recur.chat.to-a-pr",
            "To a PR",
            "Plan, implement, review, and open a pull request",
            "to-a-pr",
            "loop",
            0,
            "shebang",
            json!({ "loop": "recur.loops.pr" })
        ),
        p(
            "recur.chat.write-docs",
            "Write docs",
            "Plan, write, and edit documentation: no PR",
            "write-docs",
            "loop",
            0,
            "shebang",
            json!({ "loop": "recur.loops.docs" })
        ),
        p(
            "recur.chat.security-review",
            "Security review & fix",
            "Review and fix auth, secrets, and permissions until the bar passes",
            "security-review",
            "loop",
            0,
            "shebang",
            json!({ "loop": "recur.loops.security" })
        ),
    ]))
}

fn prompt_result(prompt: &str, display: &str) -> PromptResultDto {
    PromptResultDto {
        prompt: prompt.into(),
        display: display.into(),
        ..Default::default()
    }
}

fn run_prompt_action(id: &str, ctx: &PromptContextDto) -> PromptResultDto {
    match id {
        "recur.chat.review" => match ctx.surface.as_str() {
            "github_pr" => prompt_result(
                "Review this GitHub pull request and file findings. Use gh pr diff: do not treat git diff in cwd as the PR.",
                "Please review this GitHub pull request",
            ),
            "local_pr" => prompt_result(
                "Review this Local PR and file findings. Do not make code changes.",
                "Please review this Local PR",
            ),
            _ => prompt_result(
                "Review the changes in this worktree and file findings. Git context is already gathered: do NOT re-run git status/diff/log.",
                "Please review the changes in this worktree and file any real issues",
            ),
        },
        "recur.chat.re-review" => match ctx.surface.as_str() {
            "github_pr" => prompt_result(
                "Please re-review this GitHub pull request after the recent fixes. Do not make code changes.",
                "Please re-review this GitHub pull request",
            ),
            "local_pr" => prompt_result(
                "Please re-review this Local PR after the recent fixes. Do not make code changes.",
                "Please re-review this Local PR",
            ),
            _ => prompt_result(
                "Please re-review this worktree after the recent fixes. Do not make code changes.",
                "Please re-review this worktree after the recent fixes",
            ),
        },
        "recur.chat.open-pr" => prompt_result(
            "The user requested a pull request. Call mcp__recur__open_pr with a title and markdown body of ALL workspace changes versus the base. Do not run gh pr create yourself.",
            "Open a pull request",
        ),
        "recur.chat.open-draft-pr" => prompt_result(
            "The user requested a draft pull request. Call mcp__recur__open_pr with draft set to true, a title, and a markdown body of ALL workspace changes versus the base. Do not run gh pr create yourself.",
            "Open a draft pull request",
        ),
        "recur.chat.fix-checks" => {
            let raw = ctx.args.trim();
            if raw.is_empty() || raw.eq_ignore_ascii_case("all") {
                prompt_result(
                    "Investigate and fix the failing GitHub checks on this PR. Use gh run view --log-failed.",
                    "Fix failing GitHub checks",
                )
            } else {
                prompt_result(
                    &format!(
                        "Investigate and fix the failing GitHub check {raw}. Use gh run view --log-failed."
                    ),
                    &format!("Fix failing check: {raw}"),
                )
            }
        }
        "recur.chat.review-and-fix" => prompt_result(
            "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes. Do not reopen scope, do not plan a new feature, and do not open a PR.",
            "Review & fix",
        ),
        "recur.chat.review-to-pr" => prompt_result(
            "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR. Do not reopen scope and do not plan a new feature.",
            "Review & fix to a PR",
        ),
        "recur.chat.to-a-pr" => prompt_result(
            "Ship a correct, reviewable change to a pull request. Plan first when there is no approved plan, then implement and review until the policy bar passes, then open the PR. Do not reopen scope the human already closed.",
            "To a PR",
        ),
        "recur.chat.write-docs" => prompt_result(
            "Write the documentation the human asked for. Do not change product code unless the docs require it. Do not open a PR: the write-up lives in this worktree.",
            "Write docs",
        ),
        "recur.chat.security-review" => prompt_result(
            "Review the current worktree for auth, secrets, tokens, and permission checks. Fix real findings. Do not reopen product scope, and do not open a PR.",
            "Security review & fix",
        ),
        _ => prompt_result("hi", "Hi"),
    }
}

const COMPILED_FIX_FINDINGS: &str = r#"
exports.default = function () {
  return {
    prompt: "Open reviewer findings remain for this workspace. Call mcp__recur__list_findings to load them. Operator notes on a finding are instructions: follow them. For each open finding: verify it against the current code, then either fix it with minimal correct edits and mark it resolved via mcp__recur__update_finding, or mark it invalid with a clear reason if the claim is wrong or out of scope.",
    display: "Fix open reviewer findings"
  };
};
"#;

const COMPILED_COMMIT: &str = r#"
exports.default = function () {
  return {
    prompt: "Commit all changes. Write a clear message from the diff. Do not push or amend unless I ask. If there’s nothing to commit, say so.",
    display: "Commit all changes"
  };
};
"#;

const COMPILED_COMMIT_PUSH: &str = r#"
exports.default = function () {
  return {
    prompt: "Commit all changes, then push to the remote tracking branch (create/set upstream if needed). Write a clear message from the diff. Do not amend unless I ask. If there’s nothing to commit, still push any unpushed commits; if everything is already pushed, say so.",
    display: "Commit all changes and push"
  };
};
"#;

const COMPILED_PROMPT_DEFAULT: &str =
    r#"exports.default = function () { return { prompt: "hi", display: "Hi" }; };"#;

/// CommonJS shape matching esbuild FormatCommonJS + bundled `@recur/sdk`
/// (the Worker runs it via AsyncFunction), with a minimal Output/replaceEdited.
const COMPILED_SDK: &str = r#"
var Output = Object.freeze({
  replaceSelection: "replaceSelection",
  replaceDocument: "replaceDocument"
});
function replaceEdited(ctx, text) {
  return { text: text, output: ctx.selectedText ? Output.replaceSelection : Output.replaceDocument };
}
"#;

const COMPILED_SORT_LINES: &str = r#"
exports.default = function (ctx) {
  var ends = ctx.input.endsWith("\n");
  var lines = ctx.input.replace(/\n$/, "").split("\n");
  lines.sort(function (a, b) { return a.localeCompare(b); });
  var text = lines.join("\n");
  if (ends) text += "\n";
  return replaceEdited(ctx, text);
};
"#;

const COMPILED_IDENTITY: &str =
    r#"exports.default = function (ctx) { return { text: ctx.input }; };"#;

fn commands() -> Vec<CommandInfo> {
    let cmd = |id: &str,
               name: &str,
               cat: &str,
               scope: &str,
               glob: &str,
               input: &str,
               fallback: &str,
               output: &str,
               trigger: &str,
               key: &str,
               interp: &str,
               bundle: &str| {
        json!({
            "id": id, "name": name, "category": cat, "order": if cat == "Text" && name == "Uppercase" { 20 } else { 10 },
            "scope": scope, "fileGlob": glob, "input": input, "fallbackInput": fallback, "output": output,
            "triggers": [trigger], "prompts": [], "keyEquivalent": key, "language": "",
            "openInPreview": false, "interpreter": interp, "bundleName": bundle, "sourceRoot": "default",
        })
    };
    canned(json!([
        cmd(
            "recur.text.sort-lines-asc",
            "Sort Lines Ascending",
            "Text",
            "",
            "",
            "selection",
            "document",
            "replaceInput",
            "palette",
            "ctrl+f5",
            "typescript",
            "Text"
        ),
        cmd(
            "recur.text.uppercase",
            "Uppercase",
            "Text",
            "",
            "",
            "selection",
            "document",
            "replaceInput",
            "palette",
            "ctrl+u",
            "typescript",
            "Text"
        ),
        cmd(
            "recur.go.gofmt-on-save",
            "Gofmt on Save",
            "Go",
            "source.go",
            "*.go",
            "document",
            "",
            "replaceDocument",
            "willSave",
            "",
            "shebang",
            "Go"
        ),
    ]))
}

fn search_hit(query: &str) -> SearchHit {
    SearchHit {
        path: "src/main.go".into(),
        line: 3,
        col: 1,
        text: query.into(),
        preview: "func main() {}".into(),
    }
}

fn commits() -> Vec<Commit> {
    let commit = |hash: &str, fill: char, message: &str, add: i64, del: i64, when: &str| Commit {
        hash: hash.into(),
        sha: format!("{hash}{}", fill.to_string().repeat(31)),
        message: message.into(),
        author: "Ada".into(),
        author_role: "Author".into(),
        agent_code: "AD".into(),
        color: "var(--indigo)".into(),
        diff: DiffStat { add, del },
        when: when.into(),
        files: Some(Vec::new()),
        ..Default::default()
    };
    vec![
        commit("a3f91c2", 'a', "Add feature", 4, 1, "2m"),
        commit("b1c20d4", 'b', "Older work", 2, 0, "1d"),
    ]
}

/// The shipped catalog entries the fake serves: the Go grammar and three
/// themes, in this order.
const GRAMMAR_SCOPES: &[&str] = &["source.go"];
const THEME_IDS: &[&str] = &["recur", "paper", "cool-glow"];

fn catalog() -> &'static recur_bundles::Catalog {
    static CAT: OnceLock<recur_bundles::Catalog> = OnceLock::new();
    CAT.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles");
        recur_bundles::discover_catalog(&recur_bundles::DiscoverRoots {
            user_dir: String::new(),
            default_dir: root.to_string_lossy().into_owned(),
        })
        .unwrap_or_default()
    })
}

fn grammars() -> Vec<&'static recur_bundles::Grammar> {
    let cat = catalog();
    GRAMMAR_SCOPES
        .iter()
        .filter_map(|s| recur_bundles::find_grammar_by_scope(&cat.grammars, s))
        .collect()
}

fn themes() -> Vec<&'static recur_bundles::Theme> {
    let cat = catalog();
    THEME_IDS
        .iter()
        .filter_map(|id| recur_bundles::find_theme_by_id(&cat.themes, id))
        .collect()
}

fn read_body(path: &str, label: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::new(format!("{label}: {e}")))
}

fn finding(workspace_id: &str, id: &str, status: &str) -> Finding {
    Finding {
        id: id.into(),
        workspace_id: workspace_id.into(),
        status: status.into(),
        ..Default::default()
    }
}

fn add_pr_comment(
    s: &mut Seed,
    id: &str,
    file: &str,
    line: i64,
    body: &str,
    start_line: i64,
    finding_id: &str,
) -> Result<PrComment> {
    let finding_id = finding_id.trim();
    if s.pr(id).is_some() && !file.is_empty() && line != 0 {
        let side = |n: i64| if n < 0 { "LEFT" } else { "RIGHT" };
        let (n, start_n) = (line.abs(), start_line.abs());
        let text = body.trim();
        let Some(pr) = s.pr_mut(id) else {
            return unanswered();
        };
        if text.is_empty() {
            pr.pending_comments.retain(|c| {
                finding_id.is_empty() || (c.id != finding_id && c.finding_id.trim() != finding_id)
            });
            return Ok(now_comment(""));
        }
        let mut pc = PendingLineComment {
            id: format!("pend-{}", now_ms()),
            file: file.into(),
            line: n,
            side: side(line).into(),
            body: text.into(),
            finding_id: finding_id.into(),
            ..Default::default()
        };
        if start_n > 0 && start_n < n {
            pc.start_line = start_n;
            pc.start_side = side(start_line).into();
        }
        pr.pending_comments.push(pc);
        return Ok(now_comment(text));
    }
    let loc = if file.is_empty() {
        String::new()
    } else {
        let suffix = if line > 0 {
            format!(":{line}")
        } else {
            String::new()
        };
        format!("<code>{file}{suffix}</code>: ")
    };
    let mut c = now_comment(&format!("{loc}{body}"));
    c.body_md = body.into();
    c.author_login = s
        .pr(id)
        .map_or("avery", |pr| or(&pr.viewer_login, "avery"))
        .to_owned();
    if let Some(pr) = s.pr_mut(id) {
        pr.comments.get_or_insert_default().push(c.clone());
    }
    Ok(c)
}

fn toggle_reaction(pr: &mut Pr, comment_id: &str, content: &str) {
    const ORDER: [&str; 8] = [
        "+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes",
    ];
    let content = content.trim();
    let lower = content.to_lowercase();
    let n = match lower.as_str() {
        "thumbs_up" => "+1".to_owned(),
        "thumbs_down" => "-1".to_owned(),
        _ if ORDER.contains(&content) => content.to_owned(),
        _ => lower,
    };
    if comment_id.is_empty() || !ORDER.contains(&n.as_str()) {
        return;
    }
    for c in pr.comments.iter_mut().flatten() {
        if c.id != comment_id {
            continue;
        }
        match c.reactions.iter_mut().find(|r| r.content == n) {
            Some(r) => {
                r.count += if r.mine { -1 } else { 1 };
                r.mine = !r.mine;
            }
            None => c.reactions.push(PrReaction {
                content: n.clone(),
                count: 1,
                mine: true,
            }),
        }
        c.reactions.retain(|r| r.count > 0);
    }
}

fn search_files(files: Vec<String>, query: &str, limit: i64, files_only: bool) -> FileSearchResult {
    let query = query.trim().to_lowercase();
    let limit = if limit > 0 { limit as usize } else { 50 };
    let matches = |p: &str| query.is_empty() || p.to_lowercase().contains(&query);
    let hit = |path: &str, dir: bool| FileSearchHit {
        path: path.into(),
        score: 1,
        dir,
    };
    let file_hits = files.iter().filter(|p| matches(p)).map(|p| hit(p, false));
    let hits: Vec<FileSearchHit> = if files_only {
        file_hits.take(limit).collect()
    } else {
        let mut seen = std::collections::HashSet::new();
        let mut dirs: Vec<String> = Vec::new();
        for file in &files {
            let mut p = file.clone();
            while let Some(i) = p.rfind('/').filter(|i| *i > 0) {
                p.truncate(i);
                if !seen.insert(p.clone()) {
                    break;
                }
                dirs.push(p.clone());
            }
        }
        let dir_hits = dirs.iter().filter(|p| matches(p)).map(|p| hit(p, true));
        file_hits.chain(dir_hits).take(limit).collect()
    };
    FileSearchResult {
        hits: Some(hits),
        building: false,
        ready: true,
    }
}

fn tree_entry(name: &str, path: &str, dir: bool) -> TreeEntry {
    TreeEntry {
        name: name.into(),
        path: path.into(),
        dir,
        ..Default::default()
    }
}

fn changed_files() -> BTreeMap<String, String> {
    BTreeMap::from([("src/main.go".to_owned(), "m".to_owned())])
}

/// Swaps the workspace stored under `id` for `next`.
fn replace_workspace(s: &mut Seed, id: &str, next: &Workspace) {
    for w in s.workspaces.iter_mut().filter(|w| w.id == id) {
        *w = next.clone();
    }
}

fake_services! {
    AgentsService {
        async fn get_agent_trace(&self, workspace_id: &str, code: &str) -> Result<Agent> {
            Ok(self.with(|s| {
                s.agents
                    .get(workspace_id)
                    .and_then(|a| a.iter().find(|g| g.code == code).cloned())
                    .unwrap_or_default()
            }))
        }

        async fn list_loop_agents(&self, workspace_id: &str) -> Result<Vec<Agent>> {
            Ok(self.with(|s| s.agents.get(workspace_id).cloned().unwrap_or_default()))
        }
    }

    ArtifactsService {
        async fn delete_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
            Ok(())
        }

        async fn list_artifacts(&self, workspace_id: &str) -> Result<Vec<Artifact>> {
            Ok(Vec::new())
        }

        async fn present_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
            Ok(())
        }

        async fn rename_artifact(&self, workspace_id: &str, id: &str, title: &str) -> Result<Artifact> {
            unanswered()
        }

        async fn reveal_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
            Ok(())
        }
    }

    AssetsService {
        async fn fetch_asset(&self, url: &str) -> std::result::Result<Asset, AssetError> {
            Err(AssetError::not_found())
        }
    }

    BriefingService {
        async fn app_chat_items(&self, code: &str) -> Result<Vec<ChatItem>> {
            Ok(Vec::new())
        }

        async fn app_chat_selection(&self, code: &str) -> Result<AppChatSelection> {
            Ok(AppChatSelection::default())
        }

        async fn cancel_app_message(&self, code: &str) -> Result<()> {
            Ok(())
        }

        async fn close_app_chat(&self, code: &str) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| {
                s.app_chats.retain(|t| t.code != code);
                app_tabs(&s.app_chats)
            }))
        }

        async fn get_briefing(&self) -> Result<BriefingSnapshot> {
            Ok(self.with(|s| mock_briefing(s)))
        }

        async fn list_app_chats(&self) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| app_tabs(&s.app_chats)))
        }

        async fn mark_app_chat_seen(&self, code: &str) -> Result<()> {
            Ok(())
        }

        async fn mark_workspace_viewed(&self, workspace_id: &str, seen_through_activity_at: i64) -> Result<()> {
            Ok(())
        }

        async fn new_app_chat(&self) -> Result<WorkspaceChatTab> {
            Ok(self.with(|s| {
                let t = tab(&format!("AP{}", s.app_chats.len() + 2), "New chat", 0);
                s.app_chats.push(t.clone());
                WorkspaceChatTab { mode: "ask".into(), ..t }
            }))
        }

        async fn remember_app_chat_selection(&self, code: &str, model: &str, flags: &BTreeMap<String, String>) -> Result<()> {
            Ok(())
        }

        async fn send_app_message(&self, code: &str, text: &str, model: &str, flags: &BTreeMap<String, String>) -> Result<String> {
            Ok(String::new())
        }

        async fn set_app_chat_focus(&self, visible: bool, code: &str) -> Result<()> {
            Ok(())
        }
    }

    BundleService {
        async fn compiled_complete_module(&self, id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn compiled_module(&self, id: &str) -> Result<String> {
            let body = if id == "recur.text.sort-lines-asc" {
                COMPILED_SORT_LINES
            } else {
                COMPILED_IDENTITY
            };
            Ok(format!("{COMPILED_SDK}{body}"))
        }

        async fn compiled_prompt_module(&self, id: &str) -> Result<String> {
            Ok(match id {
                "recur.chat.fix-findings" => COMPILED_FIX_FINDINGS,
                "recur.chat.commit" => COMPILED_COMMIT,
                "recur.chat.commit-push" => COMPILED_COMMIT_PUSH,
                _ => COMPILED_PROMPT_DEFAULT,
            }
            .to_owned())
        }

        async fn expand_goal(&self, text: &str) -> Result<PromptResultDto> {
            Ok(prompt_result(text, ""))
        }

        async fn invalidate_catalog(&self) -> Result<()> {
            Ok(())
        }

        async fn list_bundle_settings(&self) -> Result<Vec<BundleSettingInfo>> {
            Ok(Vec::new())
        }

        async fn list_commands(&self, scope: &str, path: &str) -> Result<Vec<CommandInfo>> {
            Ok(commands())
        }

        async fn list_grammars(&self) -> Result<Vec<GrammarInfo>> {
            let lang = &catalog().language_data;
            Ok(grammars()
                .into_iter()
                .map(|g| crate::app::bundle::to_grammar_info(g, lang))
                .collect())
        }

        // The workspaces stand in for loops, as the e2e seed's `ListLoops` alias.
        async fn list_loops(&self) -> Result<Vec<LoopInfo>> {
            Ok(self.with(|s| {
                s.workspaces
                    .iter()
                    .map(|w| LoopInfo { id: w.id.clone(), ..Default::default() })
                    .collect()
            }))
        }

        async fn list_policies(&self) -> Result<Vec<PolicyInfo>> {
            let p = |id: &str, name: &str, hint: &str| PolicyInfo {
                id: id.into(),
                name: name.into(),
                hint: hint.into(),
                ..Default::default()
            };
            Ok(vec![
                p("recur.policies.cautious", "Cautious", "Close every finding; escalate after 2 rounds"),
                p("recur.policies.balanced", "Balanced", "Fix medium and up; escalate after 4 rounds"),
                p("recur.policies.aggressive", "Aggressive", "Ship-blockers only; proceed after 6 rounds"),
            ])
        }

        async fn list_prompt_actions(&self) -> Result<Vec<PromptActionInfo>> {
            Ok(prompt_actions())
        }

        async fn list_roles(&self) -> Result<Vec<RoleInfo>> {
            Ok(Vec::new())
        }

        async fn list_skipped_bundles(&self) -> Result<Vec<BundleSkipInfo>> {
            Ok(Vec::new())
        }

        async fn list_snippets(&self, scope: &str, path: &str) -> Result<Vec<SnippetInfo>> {
            Ok(canned(json!([
                {
                    "id": "recur.go.ife",
                    "name": "If Err",
                    "tabTrigger": "ife",
                    "scope": "source.go",
                    "fileGlob": "*.go",
                    "body": "if ${1:err} != nil {\n\t${2:log.Fatal(${1:err})}\n}",
                    "keyEquivalent": "",
                    "order": 10,
                    "bundleName": "Go",
                    "sourceRoot": "default",
                },
                {
                    "id": "recur.html.wrap",
                    "name": "Wrap Selection in Tag",
                    "tabTrigger": "",
                    "scope": "text.html.basic",
                    "fileGlob": "*.{html,htm}",
                    "body": "<${1:p}>${2:$TM_SELECTED_TEXT}</$1>",
                    "keyEquivalent": "ctrl+w",
                    "order": 10,
                    "bundleName": "HTML",
                    "sourceRoot": "default",
                },
            ])))
        }

        async fn list_symbols(&self) -> Result<Vec<SymbolInfo>> {
            Ok(canned(json!([
                { "id": "recur.symbols.source.function", "scope": "entity.name.function", "show": true, "kind": "func", "bundleName": "Source", "sourceRoot": "default" },
                { "id": "recur.symbols.source.hide-calls", "scope": "meta.function-call entity.name.function", "show": false, "bundleName": "Source", "sourceRoot": "default" },
            ])))
        }

        async fn list_terminal_themes(&self) -> Result<Vec<TerminalThemeInfo>> {
            Ok(catalog()
                .terminal_themes
                .iter()
                .map(crate::app::bundle::to_terminal_theme_info)
                .collect())
        }

        async fn list_themes(&self) -> Result<Vec<ThemeInfo>> {
            Ok(themes().into_iter().map(crate::app::bundle::to_theme_info).collect())
        }

        async fn read_grammar(&self, scope_name: &str) -> Result<String> {
            match grammars().into_iter().find(|g| g.scope_name == scope_name) {
                Some(g) => read_body(&g.abs_path, &format!("grammar {scope_name}")),
                None => Err(Error::new(format!("grammar {scope_name} not found"))),
            }
        }

        async fn read_theme(&self, id: &str) -> Result<String> {
            match themes().into_iter().find(|t| t.id == id) {
                Some(t) => read_body(&t.abs_path, &format!("theme {id}")),
                None => Err(Error::new(format!("theme {id} not found"))),
            }
        }

        // Mock gofmt: collapse runs of blank lines and strip trailing blanks.
        async fn run_command(&self, id: &str, rc: &RunContextDto) -> Result<RunResultDto> {
            static BLANKS: OnceLock<(Regex, Regex)> = OnceLock::new();
            let (blank, trailing) = BLANKS.get_or_init(|| {
                (
                    Regex::new(r"\n{3,}").expect("blank-line regex"),
                    Regex::new(r"(?m)[ \t]+$").expect("trailing-space regex"),
                )
            });
            let formatted = blank.replace_all(&rc.input, "\n\n");
            let mut out = trailing.replace_all(&formatted, "").into_owned();
            if !out.ends_with('\n') {
                out.push('\n');
            }
            Ok(RunResultDto {
                stdout: out,
                output: "replaceDocument".into(),
                ..Default::default()
            })
        }

        async fn run_prompt_action(&self, id: &str, rc: &PromptContextDto) -> Result<PromptResultDto> {
            Ok(run_prompt_action(id, rc))
        }

        async fn run_prompt_complete(&self, id: &str, req: &PromptCompleteRequestDto) -> Result<PromptCompleteResultDto> {
            let term = req.args.trim().to_lowercase();
            let mut hits = Vec::new();
            if id == "recur.chat.fix-checks" {
                let hit = |insert: &str, label: &str, detail: &str, badge: &str| PromptCompleteHitDto {
                    insert: insert.into(),
                    label: label.into(),
                    detail: detail.into(),
                    badge: badge.into(),
                    ..Default::default()
                };
                if "all".contains(&term) {
                    hits.push(hit("all", "All failing checks", "Every failing GitHub check", "All"));
                }
                if "lint".contains(&term) {
                    hits.push(hit("lint", "lint", "fail", "Check"));
                }
            }
            Ok(PromptCompleteResultDto { hits: Some(hits), message: String::new() })
        }

        async fn search_palette(&self, query: &str) -> Result<PaletteSearchDto> {
            unanswered()
        }
    }

    CodeService {
        async fn abs_path(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<String> {
            Ok(format!("/mock/{path}"))
        }

        async fn bridge_bench_done(&self, stats_json: &str) -> Result<()> {
            Ok(())
        }

        async fn bridge_bench_enabled(&self) -> Result<bool> {
            Ok(false)
        }

        async fn bridge_echo(&self, payload: &str) -> Result<i64> {
            Ok(0)
        }

        async fn bridge_ping(&self, n: i64) -> Result<i64> {
            Ok(0)
        }

        async fn commit(&self, project_id: &str, workspace_id: &str, message: &str) -> Result<WorktreeCommitResult> {
            Ok(commit_result())
        }

        async fn commit_diff(&self, project_id: &str, workspace_id: &str, hash: &str) -> Result<Vec<FileDiff>> {
            Ok(vec![CodeService::file_diff(self, project_id, workspace_id, "").await?])
        }

        async fn commit_staged(&self, project_id: &str, workspace_id: &str, message: &str) -> Result<WorktreeCommitResult> {
            Ok(commit_result())
        }

        async fn copy(&self, project_id: &str, workspace_id: &str, from: &str, to: &str) -> Result<()> {
            Ok(())
        }

        async fn create_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
            Ok(())
        }

        async fn delete(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
            Ok(())
        }

        async fn diff_vs_base(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult> {
            let files = vec![CodeService::file_diff(self, project_id, workspace_id, "").await?];
            Ok(FileDiffResult { files: Some(files), truncated: false })
        }

        async fn diff_working(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult> {
            let files = vec![CodeService::file_diff(self, project_id, workspace_id, "").await?];
            Ok(FileDiffResult { files: Some(files), truncated: false })
        }

        async fn discard_all(&self, project_id: &str, workspace_id: &str) -> Result<()> {
            Ok(())
        }

        async fn discard_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
            Ok(())
        }

        async fn file_at_base(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<String> {
            Ok("package main\n\nfunc main() {}\n".into())
        }

        async fn file_diff(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<FileDiff> {
            Ok(file_diff())
        }

        async fn find_defs(&self, project_id: &str, workspace_id: &str, name: &str) -> Result<Vec<SymbolDef>> {
            Ok(vec![SymbolDef {
                name: name.into(),
                kind: "func".into(),
                path: "src/main.go".into(),
                line: 3,
            }])
        }

        async fn find_refs(&self, project_id: &str, workspace_id: &str, name: &str) -> Result<Vec<SymbolRef>> {
            Ok(vec![SymbolRef {
                path: "src/main.go".into(),
                line: 3,
                text: "func main() {}".into(),
                def: true,
            }])
        }

        async fn generate_commit_message(&self, project_id: &str, workspace_id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn git_status(&self, project_id: &str, workspace_id: &str) -> Result<BTreeMap<String, String>> {
            Ok(changed_files())
        }

        async fn list_all_skills(&self, project_id: &str, workspace_id: &str) -> Result<Vec<AgentSkill>> {
            Ok(skills())
        }

        async fn list_changed_files(&self, workspace_id: &str) -> Result<Vec<String>> {
            Ok(vec!["src/main.go".into()])
        }

        async fn list_commits(&self, project_id: &str, workspace_id: &str) -> Result<Vec<Commit>> {
            Ok(commits())
        }

        async fn list_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
            Ok(vec!["src/main.go".into(), "README.md".into()])
        }

        async fn list_skills(&self, project_id: &str, workspace_id: &str) -> Result<Vec<AgentSkill>> {
            Ok(skills())
        }

        // One tree level at a time (depth 1 plus lazy expand).
        async fn list_tree(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<Vec<TreeEntry>> {
            Ok(match path {
                "" => vec![tree_entry("src", "src", true)],
                "src" => vec![tree_entry("main.go", "src/main.go", false)],
                _ => Vec::new(),
            })
        }

        // Delegates to `list_tree` so a spec that overrides the tree shape
        // overrides one method.
        async fn list_trees(&self, project_id: &str, workspace_id: &str, dirs: &[String]) -> Result<BTreeMap<String, Vec<TreeEntry>>> {
            let mut out = BTreeMap::new();
            for dir in dirs {
                out.insert(dir.clone(), CodeService::list_tree(self, project_id, workspace_id, dir).await?);
            }
            Ok(out)
        }

        async fn mkdir(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
            Ok(())
        }

        async fn open_in_editor(&self, project_id: &str, workspace_id: &str, path: &str, line: i64) -> Result<()> {
            Ok(())
        }

        async fn outline(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<Vec<OutlineSymbol>> {
            Ok(vec![OutlineSymbol {
                name: "main".into(),
                kind: "func".into(),
                line: 3,
                ..Default::default()
            }])
        }

        async fn read_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<FileContent> {
            Ok(FileContent {
                path: or(path, "src/main.go").into(),
                language: "go".into(),
                content: "package main\n\nfunc main() {}\n".into(),
                size: 32,
                writable: !workspace_id.is_empty(),
                ..Default::default()
            })
        }

        async fn refresh_file_index(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn rename(&self, project_id: &str, workspace_id: &str, from: &str, to: &str) -> Result<()> {
            Ok(())
        }

        async fn replace_in_files(&self, project_id: &str, workspace_id: &str, query: &str, replacement: &str, opts: &SearchOpts) -> Result<ReplaceResult> {
            Ok(ReplaceResult { files_changed: 1, replacements: 1 })
        }

        async fn reveal_in_finder(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
            Ok(())
        }

        async fn search(&self, project_id: &str, workspace_id: &str, query: &str) -> Result<Vec<SearchHit>> {
            Ok(vec![search_hit(query)])
        }

        async fn search_advanced(&self, project_id: &str, workspace_id: &str, query: &str, opts: &SearchOpts) -> Result<SearchResult> {
            Ok(SearchResult {
                hits: Some(vec![search_hit(query)]),
                skipped_large: 0,
                truncated: false,
            })
        }

        // Ranks in the backend in production; here it filters whatever
        // `list_files` answers, so a `list_files` override drives @-mentions.
        async fn search_files(&self, project_id: &str, workspace_id: &str, query: &str, limit: i64, files_only: bool) -> Result<FileSearchResult> {
            let files = CodeService::list_files(self, project_id, workspace_id).await?;
            Ok(search_files(files, query, limit, files_only))
        }

        async fn set_file_index_exclude(&self, patterns: &[String]) -> Result<()> {
            Ok(())
        }

        async fn stage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()> {
            Ok(())
        }

        async fn staged_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn unstage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()> {
            Ok(())
        }

        async fn working_status(&self, project_id: &str, workspace_id: &str) -> Result<BTreeMap<String, String>> {
            Ok(changed_files())
        }

        async fn write_file(&self, project_id: &str, workspace_id: &str, path: &str, content: &str) -> Result<()> {
            Ok(())
        }
    }

    EditorSessionService {
        async fn clear_editor_session(&self, workspace_id: &str) -> Result<()> {
            self.with(|s| s.editor_sessions.remove(workspace_id));
            Ok(())
        }

        async fn get_editor_session(&self, workspace_id: &str) -> Result<Option<EditorSession>> {
            Ok(self.with(|s| s.editor_sessions.get(workspace_id).cloned()))
        }

        async fn update_editor_session(&self, workspace_id: &str, patch: &EditorSessionPatch) -> Result<()> {
            self.with(|s| {
                let cur = s.editor_sessions.remove(workspace_id);
                if let Some(next) = crate::app::merge_editor_session(cur, patch) {
                    s.editor_sessions.insert(workspace_id.to_owned(), next);
                }
            });
            Ok(())
        }
    }

    EscalationService {
        async fn answer_escalation(&self, workspace_id: &str, choice: &str, free_text: &str, agent_code: &str) -> Result<Escalation> {
            Ok(self.with(|s| {
                let Some(e) = s.escalations.get_mut(workspace_id) else {
                    return Escalation::default();
                };
                e.status = "answered".into();
                e.answer = or(free_text, choice).to_owned();
                e.clone()
            }))
        }

        async fn get_escalation(&self, workspace_id: &str) -> Result<Escalation> {
            Ok(self.with(|s| s.escalations.get(workspace_id).cloned().unwrap_or_default()))
        }
    }

    InboxService {
        async fn get_inbox(&self) -> Result<Vec<InboxItem>> {
            Ok(self.with(|s| s.inbox.clone()))
        }

        async fn mark_all_inbox_read(&self) -> Result<i64>, before { self.read_all_chat_inbox(); } {
            self.with(|s| {
                s.inbox.retain(|it| it.kind != "chat");
                for it in &mut s.inbox {
                    it.unread = false;
                }
            });
            Ok(0)
        }

        async fn mark_inbox_read(&self, id: &str) -> Result<i64>, before { self.read_chat_inbox(id); } {
            Ok(self.with(|s| {
                if parse_chat_inbox_id(id).is_some() {
                    s.inbox.retain(|it| it.id != id);
                } else {
                    for it in s.inbox.iter_mut().filter(|it| it.id == id) {
                        it.unread = false;
                    }
                }
                s.inbox.iter().filter(|it| it.unread).count() as i64
            }))
        }
    }

    LspService {
        async fn close_lsp(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn close_workspace_lsp(&self, workspace_id: &str) -> Result<()> {
            Ok(())
        }

        async fn ensure_lsp(&self, project_id: &str, workspace_id: &str, language_id: &str, rel_path: &str) -> Result<EnsureResult> {
            unanswered()
        }

        async fn list_language_servers(&self) -> Result<Vec<LanguageServerInfo>> {
            Ok(Vec::new())
        }

        async fn write_lsp(&self, id: &str, message: &str) -> Result<()> {
            Ok(())
        }
    }

    MaestroService {
        async fn cancel_maestro_message(&self, workspace_id: &str) -> Result<()> {
            Ok(())
        }

        async fn get_maestro_chat(&self, workspace_id: &str) -> Result<Vec<ChatItem>> {
            Ok(Vec::new())
        }

        async fn send_maestro_message(&self, workspace_id: &str, text: &str, attachments: &[ChatAttachment]) -> Result<String> {
            Ok("On it: keeping within your rails (mock reply).".into())
        }
    }

    ObservatoryService {
        async fn add_finding_note(&self, workspace_id: &str, finding_id: &str, body: &str) -> Result<Finding> {
            unanswered()
        }

        async fn create_finding(&self, workspace_id: &str, file: &str, line: i64, message: &str, end_line: i64) -> Result<Finding> {
            Ok(Finding {
                file: file.into(),
                line,
                message: message.into(),
                end_line,
                severity: "info".into(),
                source: "user".into(),
                iteration: 1,
                ..finding(workspace_id, "f-user-1", "open")
            })
        }

        async fn delete_finding_note(&self, workspace_id: &str, finding_id: &str, note_id: &str) -> Result<Finding> {
            unanswered()
        }

        async fn list_findings(&self, workspace_id: &str) -> Result<Vec<Finding>> {
            Ok(Vec::new())
        }

        async fn set_finding_status(&self, workspace_id: &str, finding_id: &str, status: &str) -> Result<Finding> {
            Ok(finding(workspace_id, finding_id, status))
        }
    }

    PermissionsService {
        async fn answer_permission(&self, id: &str, allow: bool, reason: &str, remember: bool) -> Result<()> {
            Ok(())
        }

        async fn list_pending_permissions(&self) -> Result<Vec<PermissionRequest>> {
            Ok(Vec::new())
        }
    }

    PolicyService {
        async fn get_policy(&self) -> Result<Policy> {
            Ok(self.with(|s| s.policy.clone()))
        }

        async fn set_policy(&self, p: &Policy) -> Result<Policy> {
            Ok(p.clone())
        }
    }

    ProjectsService {
        async fn add_project(&self, p: &Project) -> Result<Project> {
            let name = or(&p.name, "new-project").to_owned();
            let is_git = p.is_git || !p.repo_path.contains("no-git");
            let p = Project {
                id: or(&p.id, &name).to_owned(),
                name,
                open: true,
                base_branch: or(&p.base_branch, "origin/main").to_owned(),
                is_git,
                ..p.clone()
            };
            self.with(|s| {
                s.projects.push(p.clone());
                let has_main = s.workspaces.iter().any(|l| l.is_main && l.project_id == p.id);
                if is_git && !has_main {
                    let ws = main_workspace(format!("MAIN-{}", p.id), &p.id, Some(local_branch(&p.base_branch)));
                    s.workspaces.push(ws);
                }
            });
            Ok(p)
        }

        async fn add_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup> {
            let g = ProjectGroup {
                id: if g.id.is_empty() { uuid::Uuid::new_v4().to_string() } else { g.id.clone() },
                name: or(&g.name, "New group").to_owned(),
                open: g.open,
                ..Default::default()
            };
            self.with(|s| s.project_groups.push(g.clone()));
            Ok(g)
        }

        async fn delete_project(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn delete_project_group(&self, id: &str) -> Result<()> {
            self.with(|s| {
                s.project_groups.retain(|g| g.id != id);
                for p in s.projects.iter_mut().filter(|p| p.group_id == id) {
                    p.group_id.clear();
                }
            });
            Ok(())
        }

        async fn delete_scheduled_prompt(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn get_project(&self, id: &str) -> Result<Project> {
            self.with(|s| s.projects.iter().find(|p| p.id == id).cloned())
                .map_or_else(unanswered, Ok)
        }

        async fn list_branches(&self, project_id: &str) -> Result<Vec<BranchChoice>> {
            Ok(self.with(|s| {
                let base = s
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.base_branch.clone())
                    .unwrap_or_default();
                if base.is_empty() {
                    return Vec::new();
                }
                branch_choices(&[&base, "origin/main", "develop", "feature/auth", "feature/search", "hotfix/login"])
            }))
        }

        async fn list_branches_for_path(&self, repo_path: &str, preferred: &str) -> Result<Vec<BranchChoice>> {
            if repo_path.is_empty() || repo_path.contains("no-git") {
                return Ok(Vec::new());
            }
            let mut names = vec!["origin/main", "develop", "feature/auth", "main"];
            if names.contains(&preferred) {
                names.insert(0, preferred);
            }
            Ok(branch_choices(&names))
        }

        async fn list_project_groups(&self) -> Result<Vec<ProjectGroup>> {
            Ok(self.with(|s| s.project_groups.clone()))
        }

        async fn list_projects(&self) -> Result<Vec<Project>> {
            Ok(self.with(|s| s.projects.clone()))
        }

        async fn list_scheduled_prompts(&self, project_id: &str) -> Result<Vec<ScheduledPrompt>> {
            Ok(Vec::new())
        }

        async fn pause_scheduled_prompt(&self, id: &str, paused: bool) -> Result<()> {
            Ok(())
        }

        async fn preview_copy_files(&self, project_id: &str, patterns: &str) -> Result<CopyFilesPreview> {
            let files: Vec<String> = if patterns.trim().is_empty() {
                Vec::new()
            } else {
                ["apps/web/.env", "apps/api/.env", "deploy/.env.mainnet"].map(String::from).to_vec()
            };
            Ok(CopyFilesPreview {
                root: "/Users/josh/code/storefront".into(),
                count: files.len() as i64,
                files: Some(files),
                ..Default::default()
            })
        }

        async fn preview_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<Vec<i64>> {
            Ok(Vec::new())
        }

        async fn reorder_project_groups(&self, ids: &[String]) -> Result<()> {
            self.with(|s| s.project_groups = reorder(&s.project_groups, ids, |g| &g.id));
            Ok(())
        }

        async fn reorder_projects(&self, ids: &[String]) -> Result<()> {
            self.with(|s| s.projects = reorder(&s.projects, ids, |p| &p.id));
            Ok(())
        }

        async fn run_scheduled_prompt_now(&self, id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn save_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<ScheduledPrompt> {
            unanswered()
        }

        async fn set_project_group(&self, project_id: &str, group_id: &str) -> Result<Project> {
            self.with(|s| {
                let p = s.projects.iter_mut().find(|p| p.id == project_id)?;
                p.group_id = group_id.to_owned();
                Some(p.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn suggest_base_branch(&self, repo_path: &str) -> Result<String> {
            Ok(if repo_path.is_empty() || repo_path.contains("no-git") { "main" } else { "origin/main" }.into())
        }

        async fn update_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup> {
            Ok(self.with(|s| {
                let Some(x) = s.project_groups.iter_mut().find(|x| x.id == g.id) else {
                    return g.clone();
                };
                x.name.clone_from(&g.name);
                x.open = g.open;
                x.clone()
            }))
        }

        async fn update_project_run_config(&self, p: &Project) -> Result<Project> {
            self.with(|s| {
                for x in s.projects.iter_mut().filter(|x| x.id == p.id) {
                    *x = p.clone();
                }
            });
            Ok(p.clone())
        }
    }

    PrsService {
        async fn add_pr_comment(&self, pr_id: &str, file: &str, line: i64, body: &str, start_line: i64, finding_id: &str) -> Result<PrComment> {
            self.with(|s| add_pr_comment(s, pr_id, file, line, body, start_line, finding_id))
        }

        async fn create_workspace_from_pr(&self, pr_id: &str) -> Result<Workspace> {
            Ok(self.with(|s| {
                let Some(pr) = s.pr(pr_id).cloned() else {
                    return Workspace::default();
                };
                if !pr.workspace_id.is_empty() {
                    return workspace_or_empty(s, &pr.workspace_id);
                }
                let host = s.workspaces.iter().position(|w| w.host_pr_id == pr_id);
                let ws = match host {
                    Some(i) => Workspace {
                        host_pr_id: String::new(),
                        r#type: "solo".into(),
                        status_label: "Idle".into(),
                        phase: "editing".into(),
                        worktree_path: "/tmp/from-pr".into(),
                        ..s.workspaces[i].clone()
                    },
                    None => {
                        let title = or(&pr.title, "PR workspace");
                        Workspace {
                            id: "WS-FROM-PR".into(),
                            project_id: or(&pr.project_id, "storefront").into(),
                            r#type: "solo".into(),
                            goal: title.into(),
                            short: title.into(),
                            status_label: "Idle".into(),
                            phase: "editing".into(),
                            branch: or(&pr.branch, "recur/from-pr").into(),
                            base_branch: or(&pr.base, "main").into(),
                            worktree_path: "/tmp/from-pr".into(),
                            ..Default::default()
                        }
                    }
                };
                if let Some(p) = s.pr_mut(pr_id) {
                    p.workspace_id.clone_from(&ws.id);
                }
                match host {
                    Some(i) => s.workspaces[i] = ws.clone(),
                    None => s.workspaces.insert(0, ws.clone()),
                }
                ws
            }))
        }

        async fn delete_pr_comment(&self, pr_id: &str, comment_id: &str) -> Result<Pr> {
            self.with(|s| {
                let pr = s.pr_mut(pr_id)?;
                pr.comments.get_or_insert_default().retain(|c| c.id != comment_id);
                Some(pr.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn ensure_pr_chat_host(&self, pr_id: &str) -> Result<Workspace> {
            Ok(self.with(|s| {
                let Some(pr) = s.pr(pr_id).cloned() else {
                    return Workspace::default();
                };
                if !pr.workspace_id.is_empty() {
                    return workspace_or_empty(s, &pr.workspace_id);
                }
                if let Some(w) = s.workspaces.iter().find(|w| w.host_pr_id == pr_id) {
                    return w.clone();
                }
                let title = or(&pr.title, "PR chat");
                let ws = Workspace {
                    id: format!("WS-HOST-{pr_id}"),
                    project_id: or(&pr.project_id, "storefront").into(),
                    r#type: "solo".into(),
                    host_pr_id: pr_id.into(),
                    goal: title.into(),
                    short: title.into(),
                    status_label: "Review".into(),
                    phase: "editing".into(),
                    worktree_path: "/tmp/repo".into(),
                    base_branch: or(&pr.base, "main").into(),
                    ..Default::default()
                };
                s.workspaces.insert(0, ws.clone());
                ws
            }))
        }

        async fn get_commit_diff(&self, id: &str, hash: &str) -> Result<Commit> {
            Ok(self.with(|s| {
                s.pr(id)
                    .and_then(|p| p.commits.as_ref())
                    .and_then(|cs| cs.iter().find(|c| c.hash == hash || c.sha == hash).cloned())
                    .unwrap_or_default()
            }))
        }

        async fn get_pr(&self, id: &str) -> Result<Pr> {
            self.with(|s| s.pr(id).cloned()).map_or_else(unanswered, Ok)
        }

        async fn get_pr_commits(&self, id: &str) -> Result<Vec<Commit>> {
            Ok(self.with(|s| s.pr(id).and_then(|p| p.commits.clone()).unwrap_or_default()))
        }

        async fn get_pr_files(&self, id: &str) -> Result<Vec<FileDiff>> {
            Ok(self.with(|s| s.pr(id).and_then(|p| p.files.clone()).unwrap_or_default()))
        }

        async fn list_pr_findings(&self, pr_id: &str) -> Result<Vec<Finding>> {
            Ok(Vec::new())
        }

        async fn list_prs(&self) -> Result<Vec<Pr>> {
            Ok(self.with(|s| {
                s.prs
                    .iter()
                    .map(|p| Pr { files: None, commits: None, ..p.clone() })
                    .collect()
            }))
        }

        async fn merge_pr(&self, pr_id: &str, method: &str) -> Result<Pr> {
            let pr = self.with(|s| s.pr(pr_id).cloned()).unwrap_or_default();
            Ok(Pr { state: "Merged".into(), group: "merged".into(), ..pr })
        }

        async fn ready_pr(&self, pr_id: &str) -> Result<Pr> {
            PrsService::set_pr_draft(self, pr_id, false).await
        }

        async fn reply_pr_comment(&self, pr_id: &str, comment_id: &str, body: &str) -> Result<PrComment> {
            self.with(|s| {
                let pr = s.pr_mut(pr_id)?;
                let comments = pr.comments.get_or_insert_default();
                let root = comments.iter().find(|c| c.id == comment_id)?.clone();
                let reply = PrComment {
                    body_md: body.into(),
                    in_reply_to_id: root.id.clone(),
                    parent_id: root.parent_id.clone(),
                    path: root.path.clone(),
                    line: root.line,
                    thread_id: or(&root.thread_id, &root.id).into(),
                    resolved: root.resolved,
                    ..now_comment(body)
                };
                comments.push(reply.clone());
                Some(reply)
            })
            .map_or_else(unanswered, Ok)
        }

        async fn search_pr_mentionable_users(&self, pr_id: &str, query: &str) -> Result<Vec<PrParticipant>> {
            Ok(Vec::new())
        }

        async fn send_back_to_loop(&self, pr_id: &str, comments: &[String]) -> Result<Pr> {
            unanswered()
        }

        async fn set_pr_draft(&self, pr_id: &str, draft: bool) -> Result<Pr> {
            Ok(self.with(|s| {
                let Some(pr) = s.pr_mut(pr_id) else {
                    return Pr::default();
                };
                pr.draft = draft;
                pr.merge_state_status = if draft {
                    "DRAFT".into()
                } else if pr.merge_state_status == "DRAFT" {
                    "CLEAN".into()
                } else {
                    or(&pr.merge_state_status, "CLEAN").into()
                };
                pr.clone()
            }))
        }

        async fn set_pr_thread_resolved(&self, pr_id: &str, thread_id: &str, resolved: bool) -> Result<Pr> {
            self.with(|s| {
                let pr = s.pr_mut(pr_id)?;
                for c in pr.comments.iter_mut().flatten() {
                    if c.thread_id == thread_id || c.id == thread_id || c.in_reply_to_id == thread_id {
                        c.resolved = resolved;
                        if c.thread_id.is_empty() {
                            c.thread_id = thread_id.into();
                        }
                    }
                }
                Some(pr.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn submit_review(&self, pr_id: &str, kind: &str, body: &str) -> Result<Pr> {
            self.with(|s| {
                let pr = s.pr_mut(pr_id)?;
                let pending = std::mem::take(&mut pr.pending_comments);
                let review = PrComment { body_md: body.into(), ..now_comment(body) };
                let kids: Vec<PrComment> = pending
                    .iter()
                    .map(|pc| PrComment {
                        body_md: pc.body.clone(),
                        parent_id: review.id.clone(),
                        path: pc.file.clone(),
                        line: pc.line,
                        side: or(&pc.side, "RIGHT").into(),
                        ..now_comment(&format!("<code>{}:{}</code>", pc.file, pc.line))
                    })
                    .collect();
                let comments = pr.comments.get_or_insert_default();
                comments.push(review);
                comments.extend(kids);
                Some(pr.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn sync_github_prs(&self) -> Result<Vec<Pr>> {
            PrsService::list_prs(self).await
        }

        async fn toggle_pr_comment_reaction(&self, pr_id: &str, comment_id: &str, content: &str) -> Result<Pr> {
            self.with(|s| {
                let pr = s.pr_mut(pr_id)?;
                toggle_reaction(pr, comment_id, content);
                Some(pr.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn update_pr_comment(&self, pr_id: &str, comment_id: &str, body: &str) -> Result<PrComment> {
            self.with(|s| {
                let c = s.pr_mut(pr_id)?.comments.iter_mut().flatten().find(|c| c.id == comment_id)?;
                c.body_md = body.into();
                if c.path.is_empty() {
                    c.body_html = body.into();
                }
                Some(c.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn update_pr_details(&self, pr_id: &str, title: &str, body: &str) -> Result<Pr> {
            self.with(|s| {
                let pr = s.pr_mut(pr_id)?;
                pr.title = title.into();
                let author = or(&pr.author_login, "You").to_owned();
                let comments = pr.comments.get_or_insert_default();
                match comments.iter_mut().find(|c| c.kind == "description") {
                    Some(desc) => desc.body_md = body.into(),
                    None => comments.insert(0, PrComment {
                        id: "gh-description".into(),
                        kind: "description".into(),
                        author,
                        body_md: body.into(),
                        ..Default::default()
                    }),
                }
                Some(pr.clone())
            })
            .map_or_else(unanswered, Ok)
        }

        async fn update_with_base(&self, pr_id: &str, method: &str) -> Result<UpdateWorkspaceResult> {
            Ok(self.with(|s| {
                let pr = s.pr(pr_id).cloned().unwrap_or_default();
                let base = if s.pr(pr_id).is_some() { pr.base.clone() } else { "main".into() };
                updated("updated", workspace_or_empty(s, &pr.workspace_id), &base)
            }))
        }
    }

    QuestionsService {
        async fn answer_question(&self, id: &str, choice: &str, free_text: &str, attachments: &[ChatAttachment]) -> Result<()> {
            Ok(())
        }

        async fn list_pending_questions(&self) -> Result<Vec<UserQuestion>> {
            Ok(Vec::new())
        }
    }

    SettingsService {
        async fn cancel_chat_gpt_login(&self, login_id: &str) -> Result<()> {
            Ok(())
        }

        async fn cancel_codex_login(&self, login_id: &str) -> Result<()> {
            Ok(())
        }

        async fn cancel_cursor_usage_connect(&self) -> Result<()> {
            Ok(())
        }

        async fn clear_all_data(&self) -> Result<()> {
            Ok(())
        }

        async fn compatible_flag_preset(&self, name: &str) -> Result<Vec<CompatibleFlagDim>> {
            Ok(Vec::new())
        }

        async fn connect_cursor_usage(&self) -> Result<CursorUsageConnect> {
            Ok(CursorUsageConnect {
                error: "not in e2e".into(),
                ..Default::default()
            })
        }

        async fn detect_agent(&self) -> Result<String> {
            Ok("Claude Code · dev@example.com (Acme)".into())
        }

        async fn disconnect_chat_gpt(&self) -> Result<ChatGPTStatus> {
            unanswered()
        }

        async fn disconnect_cursor_usage(&self) -> Result<()> {
            Ok(())
        }

        async fn get_chat_gpt_status(&self) -> Result<ChatGPTStatus> {
            Ok(ChatGPTStatus::default())
        }

        async fn get_claude_status(&self) -> Result<ClaudeStatus> {
            Ok(ClaudeStatus {
                authenticated: true,
                path: "/usr/local/bin/claude".into(),
                version: "2.1.224".into(),
                provider: "Anthropic API".into(),
                login_method: "Claude Team account".into(),
                organization: "Acme".into(),
                email: "dev@example.com".into(),
                error: String::new(),
            })
        }

        async fn get_codex_status(&self) -> Result<CodexStatus> {
            Ok(CodexStatus {
                bin_installed: true,
                bin_path: "/opt/homebrew/bin/codex".into(),
                error: "Not logged in. Run `codex login`.".into(),
                ..Default::default()
            })
        }

        async fn get_cursor_status(&self) -> Result<CursorStatus> {
            Ok(cursor_status("1.0.31"))
        }

        async fn get_instance_label(&self) -> Result<String> {
            Ok(String::new())
        }

        async fn get_provider_usage(&self) -> Result<Snapshot> {
            Ok(empty_usage())
        }

        async fn get_resource(&self) -> Result<Resource> {
            Ok(self.with(|s| s.resource.clone()))
        }

        async fn get_settings(&self) -> Result<Settings> {
            Ok(self.with(|s| s.settings.clone()))
        }

        async fn github_login(&self) -> Result<String> {
            Ok("mycodecrafting".into())
        }

        async fn install_cursor_bridge(&self) -> Result<CursorStatus> {
            Ok(cursor_status("1.0.30"))
        }

        async fn list_assigned_issues(&self) -> Result<Vec<AssignedIssue>> {
            Ok(Vec::new())
        }

        async fn list_model_catalog(&self) -> Result<Vec<ModelGroup>> {
            Ok(model_catalog())
        }

        async fn list_monospace_fonts(&self) -> Result<Vec<String>> {
            Ok(["JetBrains Mono", "IBM Plex Mono", "Source Code Pro", "Inconsolata", "Menlo", "Monaco", "Fira Code"]
                .map(String::from)
                .to_vec())
        }

        async fn logs_path(&self) -> Result<String> {
            Ok("/tmp/recur-test/logs/recur.log".into())
        }

        async fn open_linear_in_app(&self, raw: &str) -> Result<bool> {
            Ok(false)
        }

        async fn refresh_chat_gpt_models(&self) -> Result<ChatGPTStatus> {
            unanswered()
        }

        async fn refresh_provider_usage(&self) -> Result<Snapshot> {
            Ok(empty_usage())
        }

        async fn reveal_logs(&self) -> Result<()> {
            Ok(())
        }

        async fn set_settings(&self, set: &Settings) -> Result<Settings> {
            self.with(|s| s.settings = set.clone());
            Ok(set.clone())
        }

        async fn start_chat_gpt_login(&self, account_id: &str, method: &str) -> Result<ChatGPTLogin> {
            unanswered()
        }

        async fn start_codex_login(&self) -> Result<CodexLogin> {
            unanswered()
        }

        async fn test_codex_key(&self, api_key: &str, chat_gpt_auth: bool) -> Result<CodexStatus> {
            unanswered()
        }

        async fn test_compatible_provider(&self, p: &CompatibleProvider) -> Result<CompatibleTestResult> {
            unanswered()
        }

        async fn test_cursor_key(&self, api_key: &str) -> Result<CursorStatus> {
            unanswered()
        }

        async fn wait_chat_gpt_login(&self, login_id: &str) -> Result<ChatGPTStatus> {
            unanswered()
        }

        async fn wait_codex_login(&self, login_id: &str) -> Result<CodexStatus> {
            unanswered()
        }
    }

    ShellService {
        async fn flush(&self) -> Result<()> {
            Ok(())
        }

        async fn get_expanded_schedules(&self) -> Result<Vec<String>> {
            Ok(self.with(|s| s.expanded_schedules.clone()))
        }

        async fn get_issues_view(&self) -> Result<IssuesView> {
            unanswered()
        }

        // Last-open workspace and view; survives a reload within a spec.
        async fn get_shell_state(&self) -> Result<ShellState> {
            Ok(self.with(|s| s.shell_state.clone().unwrap_or_default()))
        }

        async fn get_ui_zoom(&self) -> Result<f32> {
            Ok(1.)
        }

        async fn set_expanded_schedules(&self, ids: &[String]) -> Result<()> {
            self.with(|s| s.expanded_schedules = ids.to_vec());
            Ok(())
        }

        async fn set_issues_view(&self, v: &IssuesView) -> Result<()> {
            Ok(())
        }

        async fn set_shell_state(&self, st: &ShellState) -> Result<()> {
            self.with(|s| s.shell_state = Some(st.clone()));
            Ok(())
        }

        async fn set_ui_zoom(&self, zoom: f32) -> Result<()> {
            Ok(())
        }
    }

    TerminalService {
        async fn ack_terminal(&self, id: &str, n: i64) -> Result<()> {
            Ok(())
        }

        async fn attach_terminal(&self, id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn clear_terminal(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn close_terminal(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn detach_terminal(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn interrupt_terminal(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn list_terminal_dock_chrome(&self) -> Result<Vec<TerminalDockChrome>> {
            Ok(Vec::new())
        }

        async fn list_terminals(&self, workspace_id: &str) -> Result<Vec<TermInfo>> {
            Ok(Vec::new())
        }

        async fn open_terminal(&self, workspace_id: &str, cols: i64, rows: i64) -> Result<TermInfo> {
            unanswered()
        }

        async fn reorder_terminals(&self, workspace_id: &str, ids: &[String]) -> Result<()> {
            Ok(())
        }

        async fn resize_terminal(&self, id: &str, cols: i64, rows: i64) -> Result<()> {
            Ok(())
        }

        async fn restore_workspace_terminals(&self, workspace_id: &str, cols: i64, rows: i64) -> Result<Vec<TermInfo>> {
            Ok(Vec::new())
        }

        async fn set_active_terminal(&self, workspace_id: &str, id: &str) -> Result<()> {
            Ok(())
        }

        async fn set_terminal_dock_chrome(&self, workspace_id: &str, chrome: &TerminalDockChrome) -> Result<()> {
            Ok(())
        }

        async fn set_terminal_title(&self, id: &str, title: &str) -> Result<()> {
            Ok(())
        }

        async fn write_terminal(&self, id: &str, data: &str) -> Result<()> {
            Ok(())
        }
    }

    WorkspacesService {
        async fn apply_provisional_chat_titles(&self, workspace_id: &str, code: &str, text: &str) -> Result<()> {
            Ok(())
        }

        async fn approve_plan(&self, workspace_id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn attention_counts(&self) -> Result<AttentionCounts> {
            Ok(self.live_attention())
        }

        async fn cancel_workspace_message(&self, workspace_id: &str, code: &str) -> Result<()>, before { self.cancel_chat(workspace_id, code); } {
            Ok(())
        }

        async fn clear_workspace_issue(&self, workspace_id: &str) -> Result<Workspace> {
            unanswered()
        }

        async fn close_workspace_chat(&self, workspace_id: &str, code: &str) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| {
                let ws_id = workspace_id.to_owned();
                if code == "MA" {
                    s.hidden_loop_tabs.insert(ws_id.clone());
                    if let Some(w) = s.workspace_mut(workspace_id) {
                        w.loop_chat_code.clear();
                    }
                    let closed = s.closed_chats.entry(ws_id).or_default();
                    closed.retain(|t| t.code != "MA");
                    closed.insert(0, loop_tab(now_ms()));
                } else if let Some(t) = s.chats.get(workspace_id).and_then(|l| l.iter().find(|t| t.code == code)).cloned() {
                    let closed_tab = WorkspaceChatTab {
                        updated_at: if t.updated_at == 0 { now_ms() } else { t.updated_at },
                        ..t
                    };
                    let closed = s.closed_chats.entry(ws_id.clone()).or_default();
                    closed.retain(|t| t.code != code);
                    closed.insert(0, closed_tab);
                    s.chats.entry(ws_id).or_default().retain(|t| t.code != code);
                }
                list_workspace_chats(s, workspace_id)
            }))
        }

        async fn create_workspace(&self, project_id: &str, name: &str, base_branch: &str, mode: &str, prompt: &str, autonomy: &str, simplify: Option<bool>) -> Result<Workspace> {
            Ok(created_workspace(project_id, base_branch, mode, None))
        }

        async fn create_workspace_chat(&self, workspace_id: &str) -> Result<WorkspaceChatTab> {
            Ok(self.with(|s| {
                let list = s.chats.entry(workspace_id.to_owned()).or_default();
                let t = tab(&format!("PA{}", list.len() + 2), "New chat", 0);
                list.push(t.clone());
                t
            }))
        }

        async fn create_workspace_with(&self, project_id: &str, name: &str, base_branch: &str, mode: &str, prompt: &str, autonomy: &str, loop_id: &str, policy_id: &str, simplify: Option<bool>, attachments: &[ChatAttachment]) -> Result<Workspace> {
            Ok(created_workspace(project_id, base_branch, mode, Some((loop_id, policy_id))))
        }

        async fn delete_closed_workspace_chat(&self, workspace_id: &str, code: &str) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| {
                let open = list_workspace_chats(s, workspace_id).iter().any(|t| t.code == code);
                let list = s.closed_chats.entry(workspace_id.to_owned()).or_default();
                if !open {
                    list.retain(|t| t.code != code);
                }
                list.clone()
            }))
        }

        async fn delete_workspace(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn engage_loop(&self, workspace_id: &str, goal: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn engage_loop_with(&self, workspace_id: &str, goal: &str, loop_id: &str, policy_id: &str) -> Result<String> {
            Ok("Loop engaged: planning has started.".into())
        }

        async fn fetch_working_copy_upstream(&self, id: &str) -> Result<WorkspaceLandState> {
            Ok(self.with(|s| land_state(s, id, "")))
        }

        async fn fork_workspace_chat(&self, workspace_id: &str, code: &str, message_id: &str) -> Result<WorkspaceChatTab> {
            Ok(self.with(|s| {
                let list = s.chats.entry(workspace_id.to_owned()).or_default();
                let t = tab(&format!("PA{}", list.len() + 2), "Forked chat", now_ms());
                list.push(t.clone());
                t
            }))
        }

        async fn fork_workspace_chat_to_workspace(&self, workspace_id: &str, code: &str, message_id: &str) -> Result<Workspace> {
            Ok(self.with(|s| {
                let src = workspace_or_empty(s, workspace_id);
                let ws = Workspace {
                    id: "WS-FORK".into(),
                    project_id: src.project_id,
                    r#type: "solo".into(),
                    goal: "Forked chat".into(),
                    short: "Forked chat".into(),
                    group: Group::Active,
                    status_label: "Creating".into(),
                    phase: "creating".into(),
                    branch: "recur/fork".into(),
                    base_branch: or(&src.base_branch, "main").into(),
                    ..Default::default()
                };
                s.workspaces.push(ws.clone());
                ws
            }))
        }

        async fn get_workspace(&self, id: &str) -> Result<Workspace> {
            self.with(|s| s.workspace(id).map(with_mock_worktree)).map_or_else(unanswered, Ok)
        }

        async fn get_workspace_feed(&self, workspace_id: &str) -> Result<Vec<FeedItem>> {
            Ok(self.with(|s| s.feeds.get(workspace_id).cloned().unwrap_or_default()))
        }

        async fn last_assistant_text(&self, workspace_id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn list_closed_workspace_chats(&self, workspace_id: &str) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| s.closed_chats.get(workspace_id).cloned().unwrap_or_default()))
        }

        async fn list_open_in_targets(&self) -> Result<Vec<OpenInTarget>> {
            let t = |id: &str, name: &str| OpenInTarget {
                id: id.into(),
                name: name.into(),
                kind: "app".into(),
                ..Default::default()
            };
            Ok(vec![t("finder", "Finder"), t("cursor", "Cursor"), t("terminal", "Terminal")])
        }

        async fn list_workspace_chats(&self, workspace_id: &str) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| list_workspace_chats(s, workspace_id)))
        }

        async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
            Ok(self.with(|s| s.workspaces.iter().rev().map(with_mock_worktree).collect()))
        }

        async fn mark_workspace_chat_seen(&self, workspace_id: &str, code: &str) -> Result<()> {
            Ok(())
        }

        async fn merge_workspace(&self, workspace_id: &str, method: &str) -> Result<Workspace> {
            let w = self.with(|s| workspace_or_empty(s, workspace_id));
            Ok(with_mock_worktree(&Workspace {
                state: WorkspaceState::Merged,
                status_label: "merged".into(),
                ..w
            }))
        }

        async fn open_main_workspace(&self, project_id: &str) -> Result<Workspace> {
            Ok(self.with(|s| {
                if let Some(w) = s.workspaces.iter().find(|l| l.is_main && l.project_id == project_id) {
                    return with_mock_worktree(w);
                }
                let base = s
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .map(|p| p.base_branch.clone())
                    .unwrap_or_default();
                let ws = main_workspace(format!("MAIN-{project_id}"), project_id, Some(local_branch(&base)));
                s.workspaces.push(ws.clone());
                ws
            }))
        }

        async fn open_path(&self, path: &str) -> Result<OpenPathResult> {
            Ok(self.with(|s| {
                let name = path.split('/').rfind(|p| !p.is_empty()).unwrap_or("opened");
                let p = match s.projects.iter().find(|x| x.repo_path == path) {
                    Some(p) => p.clone(),
                    None => {
                        let p = Project {
                            id: format!("open-{name}"),
                            name: name.into(),
                            repo_path: path.into(),
                            is_git: true,
                            open: true,
                            ..Default::default()
                        };
                        s.projects.push(p.clone());
                        p
                    }
                };
                let existing = s.workspaces.iter().find(|l| l.is_main && l.project_id == p.id).cloned();
                let created = existing.is_none();
                let ws = existing.unwrap_or_else(|| {
                    let ws = main_workspace(format!("MAIN-{}", p.id), &p.id, None);
                    s.workspaces.push(ws.clone());
                    ws
                });
                OpenPathResult {
                    path: path.into(),
                    project_id: p.id.clone(),
                    workspace_id: ws.id.clone(),
                    created,
                    project: Some(Box::new(p)),
                    workspace: Some(Box::new(ws)),
                    ..Default::default()
                }
            }))
        }

        async fn open_workspace_in(&self, workspace_id: &str, target_id: &str) -> Result<()> {
            Ok(())
        }

        async fn open_workspace_pr(&self, workspace_id: &str, force: bool) -> Result<String> {
            Ok(self.with(|s| s.workspace(workspace_id).map(|w| w.id.clone()).unwrap_or_default()))
        }

        async fn pause_loop(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn pull_workspace_pr(&self, workspace_id: &str) -> Result<UpdateWorkspaceResult> {
            let w = self.with(|s| workspace_or_empty(s, workspace_id));
            Ok(updated("updated", with_mock_worktree(&w), "origin/main"))
        }

        async fn push_working_copy(&self, workspace_id: &str) -> Result<Workspace> {
            Ok(self.with(|s| workspace_or_empty(s, workspace_id)))
        }

        async fn remember_chat_loop_policy(&self, workspace_id: &str, code: &str, loop_id: &str, policy_id: &str) -> Result<()> {
            self.with(|s| {
                s.chat_loops.insert(
                    (workspace_id.to_owned(), code.to_owned()),
                    (loop_id.to_owned(), policy_id.to_owned()),
                );
                let list = s.chats.entry(workspace_id.to_owned()).or_default();
                match list.iter_mut().find(|t| t.code == code) {
                    Some(t) => {
                        t.loop_id = loop_id.into();
                        t.policy_id = policy_id.into();
                    }
                    None if !matches!(code, "" | "PA" | "MA") => list.push(WorkspaceChatTab {
                        loop_id: loop_id.into(),
                        policy_id: policy_id.into(),
                        ..tab(code, "New chat", 0)
                    }),
                    None => {}
                }
            });
            Ok(())
        }

        async fn remember_workspace_chat_prompt_group(&self, workspace_id: &str, code: &str, group: &str) -> Result<()> {
            if workspace_id.is_empty() || code.is_empty() || group.is_empty() {
                return Ok(());
            }
            self.with(|s| {
                let list = s.chats.entry(workspace_id.to_owned()).or_default();
                match list.iter_mut().find(|t| t.code == code) {
                    Some(t) => t.prompt_group = group.into(),
                    None => list.push(WorkspaceChatTab {
                        prompt_group: group.into(),
                        ..tab(code, "New chat", 0)
                    }),
                }
            });
            Ok(())
        }

        async fn remember_workspace_chat_selection(&self, workspace_id: &str, code: &str, model: &str, flags: &BTreeMap<String, String>, mode: &str, simplify: Option<bool>) -> Result<()> {
            Ok(())
        }

        async fn remember_workspace_chat_simplify(&self, workspace_id: &str, code: &str, on: bool) -> Result<()> {
            Ok(())
        }

        async fn remove_workspace(&self, id: &str) -> Result<Workspace> {
            Ok(self.with(|s| {
                let w = Workspace {
                    archived: true,
                    worktree_path: String::new(),
                    ..workspace_or_empty(s, id)
                };
                replace_workspace(s, id, &w);
                w
            }))
        }

        async fn reopen_workspace_chat(&self, workspace_id: &str, code: &str) -> Result<Vec<WorkspaceChatTab>> {
            Ok(self.with(|s| {
                let closed = s.closed_chats.entry(workspace_id.to_owned()).or_default();
                if let Some(i) = closed.iter().position(|t| t.code == code) {
                    let t = closed.remove(i);
                    if code == "MA" {
                        s.hidden_loop_tabs.remove(workspace_id);
                        if let Some(w) = s.workspace_mut(workspace_id).filter(|w| w.loop_chat_code.is_empty()) {
                            w.loop_chat_code = "MA".into();
                        }
                    } else {
                        let open = s.chats.entry(workspace_id.to_owned()).or_default();
                        open.retain(|o| o.code != t.code);
                        open.push(t);
                    }
                }
                list_workspace_chats(s, workspace_id)
            }))
        }

        async fn reorder_workspace_chats(&self, workspace_id: &str, codes: &[String]) -> Result<()> {
            self.with(|s| {
                let current = list_workspace_chats(s, workspace_id);
                if codes.len() != current.len() {
                    return Err(Error::new(format!(
                        "reorder chats: got {} codes, have {} tabs",
                        codes.len(),
                        current.len()
                    )));
                }
                let mut seen = Vec::new();
                for code in codes {
                    if !current.iter().any(|t| &t.code == code) {
                        return Err(Error::new(format!("reorder chats: unknown tab {code}")));
                    }
                    if seen.contains(&code) {
                        return Err(Error::new(format!("reorder chats: duplicate tab {code}")));
                    }
                    seen.push(code);
                }
                s.chat_order.insert(workspace_id.to_owned(), codes.to_vec());
                Ok(())
            })
        }

        async fn reset_workspace_chat(&self, workspace_id: &str, code: &str, message_id: &str) -> Result<()> {
            Ok(())
        }

        async fn restart_loop(&self, id: &str) -> Result<String> {
            Ok("Restarting".into())
        }

        async fn resume_loop(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn retry_loop(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn retry_workspace_setup(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn send_workspace_message(&self, workspace_id: &str, code: &str, text: &str, model: &str, flags: &BTreeMap<String, String>, attachments: &[ChatAttachment], mode: &str, display_text: &str, fresh_session: bool) -> Result<String>, before { let text: &str = &peel_chat_surface(text); } {
            Ok("Working on it (mock reply).".into())
        }

        async fn set_chat_ui_occluded(&self, occluded: bool) -> Result<()>, before { self.occlude_chat(occluded); } {
            Ok(())
        }

        async fn set_workspace_base_branch(&self, workspace_id: &str, base_branch: &str) -> Result<Workspace> {
            let base = base_branch.trim();
            self.with(|s| {
                let Some(w) = s.workspace(workspace_id).cloned() else {
                    return Err(Error::new(format!("workspace {workspace_id} not found")));
                };
                if w.is_main {
                    return Err(Error::new("the main working copy has no merge base"));
                }
                if base.is_empty() {
                    return Err(Error::new("base branch is required"));
                }
                if !w.branch.is_empty() && w.branch == base {
                    return Err(Error::new("target branch cannot be the workspace's own branch"));
                }
                let w = with_mock_worktree(&Workspace { base_branch: base.into(), ..w });
                replace_workspace(s, workspace_id, &w);
                for p in s.prs.iter_mut() {
                    if p.workspace_id == workspace_id && p.state != "Merged" && p.group != "merged" {
                        p.base = base.into();
                    }
                }
                Ok(w)
            })
        }

        async fn set_workspace_chat_focus(&self, workspace_id: &str, code: &str) -> Result<()>, before { self.focus_chat(workspace_id, code); } {
            Ok(())
        }

        async fn set_workspace_issue(&self, workspace_id: &str, issue: &IssueRef) -> Result<Workspace> {
            unanswered()
        }

        async fn set_workspace_loop_policy(&self, workspace_id: &str, loop_id: &str, policy_id: &str) -> Result<Workspace> {
            self.with(|s| {
                let Some(w) = s.workspace(workspace_id).cloned() else {
                    return Err(Error::new(format!("workspace {workspace_id} not found")));
                };
                let w = Workspace {
                    loop_id: or(loop_id, &w.loop_id).into(),
                    policy_id: or(policy_id, &w.policy_id).into(),
                    ..w
                };
                replace_workspace(s, workspace_id, &w);
                Ok(w)
            })
        }

        async fn start_workspace(&self, project_id: &str, task: &str, base_branch: &str, autonomy: &str) -> Result<String> {
            Ok("LOOP-NEW".into())
        }

        async fn stop_loop(&self, id: &str) -> Result<()> {
            Ok(())
        }

        async fn submit_plan(&self, workspace_id: &str, summary: &str, body: &str, steps: &[String], step_ids: &[String]) -> Result<String> {
            Ok(String::new())
        }

        async fn update_plan(&self, workspace_id: &str, plan: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn update_plan_step(&self, workspace_id: &str, step_id: &str, status: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn update_working_copy_from_remote(&self, workspace_id: &str, method: &str) -> Result<UpdateWorkspaceResult> {
            let w = self.with(|s| workspace_or_empty(s, workspace_id));
            Ok(updated("updated", w, "origin/main"))
        }

        async fn update_workspace_with_base(&self, workspace_id: &str, method: &str) -> Result<UpdateWorkspaceResult> {
            let w = self.with(|s| workspace_or_empty(s, workspace_id));
            Ok(updated("updated", with_mock_worktree(&w), "main"))
        }

        async fn watch_workspace(&self, workspace_id: &str) -> Result<()> {
            Ok(())
        }

        async fn unwatch_workspace(&self, workspace_id: &str) -> Result<()> {
            Ok(())
        }

        async fn workspace_chat_items(&self, workspace_id: &str, code: &str) -> Result<Vec<ChatItem>> {
            Ok(Vec::new())
        }

        async fn workspace_chat_models(&self) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn workspace_chat_selection(&self, workspace_id: &str, code: &str) -> Result<WorkspaceChatSelection> {
            Ok(WorkspaceChatSelection::default())
        }

        async fn workspace_chat_turn_diff(&self, workspace_id: &str, code: &str, message_id: &str) -> Result<Commit> {
            Ok(Commit { files: Some(Vec::new()), ..Default::default() })
        }

        async fn workspace_discard_risk(&self, id: &str) -> Result<String> {
            Ok(String::new())
        }

        async fn workspace_facts(&self, workspace_id: &str) -> Result<Vec<FeedItem>> {
            Ok(Vec::new())
        }

        async fn workspace_land_state(&self, id: &str) -> Result<WorkspaceLandState> {
            Ok(self.with(|s| land_state(s, id, "main")))
        }

        async fn workspace_sessions(&self, workspace_id: &str, owner_code: &str, agent_code: &str) -> Result<Vec<SessionView>> {
            Ok(Vec::new())
        }
    }
}
