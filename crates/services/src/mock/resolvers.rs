//! The base resolvers, keyed by method name, in the TypeScript's order. Getters
//! return seed data; mutations update the seed and return a shape-correct value
//! so the UI's optimistic updates work. Where the TypeScript object literal
//! names a method twice the later entry is the one the UI saw, and that is the
//! one ported here.

use std::sync::atomic::{AtomicU64, Ordering};

use regex::Regex;
use serde_json::{Map, Value, json};

use super::attention::parse_chat_inbox_id;
use super::catalog::{terminal_themes, tm_catalog};
use super::frontend::apply_chat_tab_order;
use super::js::{self, get, get_str, items, now_ms, nullish, or, str_arg, truthy};
use super::{Reply, Seed, Table};

/// An object literal; `None` entries are `undefined` (omitted).
fn obj(entries: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in entries {
        if let Some(v) = v {
            m.insert(k.to_owned(), v);
        }
    }
    Value::Object(m)
}

fn some(v: impl Into<Value>) -> Option<Value> {
    Some(v.into())
}

/// `x` itself, `undefined` when absent.
fn field(v: &Value, key: &str) -> Option<Value> {
    get(v, key).cloned()
}

/// `a || b` where `b` may be `undefined`.
fn or_field(a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    if truthy(a) { a.cloned() } else { b.cloned() }
}

/// `obj.key = value`, or `delete`-equivalent when `value` is `undefined`
/// (JSON.stringify drops undefined keys).
fn assign(v: &mut Value, key: &str, value: Option<Value>) {
    match value {
        Some(x) => js::set(v, key, x),
        None => js::delete(v, key),
    }
}

fn arr(v: Vec<Value>) -> Reply {
    Reply::Value(Value::Array(v))
}

fn strs(v: &[&str]) -> Value {
    Value::Array(v.iter().map(|s| Value::String((*s).to_owned())).collect())
}

/// `withMockWorktree`: seed dumps omit worktreePath; the Observatory will not
/// ListTree without one.
fn with_mock_worktree(ws: &Value) -> Value {
    if ws.is_null()
        || truthy(get(ws, "archived"))
        || truthy(get(ws, "worktreePath"))
        || (get_str(ws, "phase") == "failed" && truthy(get(ws, "lastError")))
    {
        return ws.clone();
    }
    let id = js::string(nullish(get(ws, "id"), &json!("ws")));
    js::spread(&[
        ws,
        &json!({ "worktreePath": format!("/tmp/recur-e2e/{id}") }),
    ])
}

static COMMENT_SEQ: AtomicU64 = AtomicU64::new(0);

/// `nowComment`: a fresh "You commented" thread entry.
fn now_comment(body_html: &str) -> Value {
    let seq = COMMENT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    json!({
        "id": format!("c-{}-{seq}", now_ms()),
        "code": "AR",
        "author": "You",
        "verb": "commented",
        "avatarCss": "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)",
        "time": "now",
        "bodyHtml": body_html,
    })
}

fn branch_choices(names: Vec<String>) -> Value {
    let mut uniq: Vec<String> = Vec::new();
    for n in names {
        if !uniq.contains(&n) {
            uniq.push(n);
        }
    }
    let remote = uniq
        .iter()
        .filter(|n| n.starts_with("origin/"))
        .map(|n| json!({ "name": n, "group": "origin" }));
    let local = uniq
        .iter()
        .filter(|n| !n.starts_with("origin/"))
        .map(|n| json!({ "name": n, "group": "Local" }));
    Value::Array(remote.chain(local).collect())
}

fn reorder(list: &[Value], ids: &[Value]) -> Vec<Value> {
    ids.iter()
        .filter_map(|id| list.iter().find(|x| get(x, "id") == Some(id)).cloned())
        .collect()
}

fn local_branch(base: Option<&Value>) -> String {
    let b = js::string(or(base, &json!("main")));
    let b = b.strip_prefix("origin/").unwrap_or(&b).to_owned();
    if b.is_empty() { "main".into() } else { b }
}

fn main_workspace(id: String, project_id: Value, branch: Option<String>) -> Value {
    // The main working copy's `baseBranch: ''` rides with an explicit branch.
    let base_branch = branch.as_ref().map(|_| Value::String(String::new()));
    obj(vec![
        ("id", some(id)),
        ("projectId", Some(project_id)),
        ("type", some("main")),
        ("isMain", some(true)),
        ("short", some("Working copy")),
        ("goal", some("Working copy")),
        ("branch", branch.map(Value::String)),
        ("baseBranch", base_branch),
        ("statusLabel", some("Idle")),
        ("diff", some(json!({ "add": 0, "del": 0 }))),
    ])
}

fn mock_briefing(s: &Seed) -> Value {
    let mut workspaces: Vec<Value> = Vec::new();
    for l in s.loops() {
        if truthy(get(l, "archived")) || truthy(get(l, "isMain")) {
            continue;
        }
        let id = get(l, "id");
        let chat_note = s.inbox.iter().find(|it| {
            truthy(get(it, "unread"))
                && get_str(it, "kind") == "chat"
                && get(it, "workspaceId") == id
        });
        let group = get_str(l, "group");
        let state = get_str(l, "state");
        let (bucket, reason, next) =
            if group == "needs" || state == "escalated" || state == "failed" {
                (
                    "needs",
                    if state == "failed" {
                        "failure"
                    } else {
                        "question"
                    },
                    "Answer the prompt in this workspace",
                )
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
        let steps = items(get(l, "plan").and_then(|p| get(p, "steps")));
        let plan_done = steps
            .iter()
            .filter(|st| matches!(get_str(st, "status"), "done" | "skipped"))
            .count();
        let id_s = js::string(nullish(id, &Value::Null));
        let id_s = if id.is_none() {
            "undefined".to_owned()
        } else {
            id_s
        };
        let diff = get(l, "diff");
        let target = match chat_note.and_then(|n| get(n, "target")) {
            Some(Value::String(t)) if reason == "unseen_chat" => t.clone(),
            _ => format!("workspace:{id_s}"),
        };
        let empty = json!("");
        workspaces.push(obj(vec![
            ("workspaceId", id.cloned()),
            ("projectId", field(l, "projectId")),
            ("projectName", field(l, "projectId")),
            ("title", or_field(get(l, "short"), get(l, "goal"))),
            ("branch", field(l, "branch")),
            ("bucket", some(bucket)),
            ("reasons", some(json!([reason]))),
            (
                "now",
                Some(
                    or(get(l, "last"), or(get(l, "statusLabel"), or(get(l, "phase"), &empty)))
                        .clone(),
                ),
            ),
            ("next", some(next)),
            ("lastActivityAt", some(now_ms())),
            ("lastSummary", Some(or(get(l, "last"), &empty).clone())),
            ("state", field(l, "state")),
            ("phase", field(l, "phase")),
            (
                "diffAdd",
                Some(nullish(diff.and_then(|d| get(d, "add")), &json!(0)).clone()),
            ),
            (
                "diffDel",
                Some(nullish(diff.and_then(|d| get(d, "del")), &json!(0)).clone()),
            ),
            ("planDone", some(plan_done)),
            ("planTotal", some(steps.len())),
            (
                "primary",
                some(json!({
                    "label": if reason == "unseen_chat" { "Read" } else { "Open" },
                    "target": target,
                    "kind": "open",
                })),
            ),
            (
                "secondary",
                (bucket == "pickup").then(|| {
                    json!([{ "label": "Continue chat", "target": format!("chat:{id_s}:PA"), "kind": "continue" }])
                }),
            ),
        ]));
    }
    let count = |b: &str| {
        workspaces
            .iter()
            .filter(|w| get_str(w, "bucket") == b)
            .count()
    };
    json!({
        "generatedAt": now_ms(),
        "counts": { "needs": count("needs"), "working": count("working"), "pickup": count("pickup") },
        "workspaces": workspaces,
    })
}

/// `listWorkspaceChats`: a workspace with an engaged run leads with the Loop
/// (MA) tab, mirroring `Engine.ListWorkspaceChats`.
fn list_workspace_chats(s: &Seed, ws_id: &str) -> Vec<Value> {
    let w = s.loop_by_id(ws_id);
    let has_run = w.is_some_and(|w| {
        truthy(get(w, "runStarted"))
            || js::string(nullish(get(w, "type"), &json!("loop"))) == "loop"
    });
    let extra = s.chats_by_loop.get(ws_id).cloned().unwrap_or_default();
    let rest: Vec<Value> = extra
        .iter()
        .filter(|t| !matches!(get_str(t, "code"), "PA" | "MA"))
        .cloned()
        .collect();
    let pa = extra.iter().find(|t| get_str(t, "code") == "PA");
    let mut tabs = vec![js::spread(&[
        &json!({ "code": "PA", "title": "New chat", "updatedAt": 0 }),
        pa.unwrap_or(&Value::Null),
    ])];
    tabs.extend(rest);
    let hidden = s.hidden_maestro.contains(ws_id);
    let unordered = if has_run && !hidden {
        let mut v = vec![json!({ "code": "MA", "title": "Loop", "mode": "loop", "updatedAt": 0 })];
        v.extend(tabs);
        v
    } else {
        tabs
    };
    let order = s.chat_order_by_loop.get(ws_id).cloned().unwrap_or_default();
    apply_chat_tab_order(unordered, &order)
        .into_iter()
        .map(|tab| {
            let key = format!(
                "{ws_id}\0{}",
                js::string(nullish(get(&tab, "code"), &Value::Null))
            );
            match s.chat_loop_by_key.get(&key) {
                Some(hit) => js::spread(&[&tab, hit]),
                None => tab,
            }
        })
        .collect()
}

fn created_workspace(a: &[Value], with: bool) -> Reply {
    let base = Some(or(a.get(2), &json!("main")).clone());
    if a.get(3).and_then(Value::as_str) == Some("loop") {
        return Reply::Value(obj(vec![
            ("id", some("LOOP-NEW")),
            ("projectId", a.first().cloned()),
            ("runStarted", some(true)),
            ("state", some("queued")),
            ("statusLabel", some("Creating")),
            ("phase", some("creating")),
            ("branch", some("recur/new")),
            ("baseBranch", base),
            (
                "loopId",
                Some(or(a.get(6), &json!("recur.loops.pr")).clone()),
            ),
            (
                "policyId",
                Some(or(a.get(7), &json!("recur.policies.balanced")).clone()),
            ),
        ]));
    }
    let mut entries = vec![
        ("id", some("WS-NEW")),
        ("projectId", a.first().cloned()),
        ("type", some("solo")),
        ("statusLabel", some("Creating")),
        ("phase", some("creating")),
        ("branch", some("recur/new")),
        ("baseBranch", base),
    ];
    if with {
        entries.push(("loopId", a.get(6).cloned()));
        entries.push(("policyId", a.get(7).cloned()));
    }
    Reply::Value(obj(entries))
}

fn land_state(s: &Seed, id: &str, other_base: &str) -> Reply {
    if s.loop_by_id(id).is_some_and(|w| truthy(get(w, "isMain"))) {
        return Reply::Value(json!({
            "behindBase": 0, "aheadBase": 0, "dirty": false, "unmerged": false,
            "base": "origin/main", "remote": "origin",
        }));
    }
    Reply::Value(json!({
        "behindBase": 0, "aheadBase": 0, "dirty": false, "unmerged": false, "base": other_base,
    }))
}

fn loop_or_empty(s: &Seed, a: &[Value]) -> Value {
    s.loop_by_id(&key_of(a.first()))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// A property key from a JS value (`obj[x]`).
fn key_of(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(v) => js::string(v),
    }
}

fn pr_linked_workspace(s: &Seed, ws_id: &Value) -> Value {
    let key = js::string(ws_id);
    s.loop_by_id(&key)
        .cloned()
        .or_else(|| s.loops().find(|l| get(l, "id") == Some(ws_id)).cloned())
        .unwrap_or_else(|| json!({}))
}

fn file_diff() -> Value {
    json!({
        "path": "src/main.go",
        "status": "m",
        "diff": { "add": 1, "del": 0 },
        "lines": [
            { "old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1,1 +1,2 @@" },
            { "old": "1", "new": "1", "type": "ctx", "codeHtml": "package main" },
            { "old": "", "new": "2", "type": "add", "codeHtml": "// edited" },
        ],
    })
}

fn skills() -> Value {
    json!([
        { "name": "design-audit", "description": "Audit UI polish", "scope": "workspace", "userInvocable": true },
        { "name": "create-rule", "description": "Create a Cursor rule", "scope": "user", "userInvocable": true },
    ])
}

fn commit_result() -> Value {
    json!({ "hash": "abc1234", "committed": true, "additions": 1, "deletions": 0 })
}

fn cursor_status(version: &str) -> Value {
    json!({
        "bridgeInstalled": true,
        "bridgePath": "/tmp/cursor-sdk-bridge",
        "bridgeVersion": version,
        "pinnedVersion": version,
        "updateAvailable": false,
        "authenticated": true,
        "account": "dev@example.com",
        "apiKeyName": "Recur laptop",
        "email": "dev@example.com",
        "name": "Dev User",
        "modelCount": 12,
        "error": "",
    })
}

fn empty_usage() -> Value {
    json!({ "fetchedAt": 0, "stale": false, "providers": [] })
}

fn model_catalog() -> Value {
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
    json!([{
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
    }])
}

fn prompt_actions() -> Value {
    fn p(
        id: &str,
        name: &str,
        desc: &str,
        slash: &str,
        mode: &str,
        timeout: i64,
        interp: &str,
    ) -> Value {
        json!({
            "id": id, "name": name, "description": desc, "slash": slash, "mode": mode,
            "timeoutMs": timeout, "interpreter": interp, "bundleName": "Chat", "sourceRoot": "default",
        })
    }
    let with = |mut v: Value, extra: Value| {
        if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
            m.extend(e);
        }
        v
    };
    let ops = json!({ "freshSession": true, "tabGroup": "ops" });
    json!([
        with(
            p(
                "recur.chat.fix-findings",
                "Fix Findings",
                "Fix or dispute open reviewer findings",
                "fix-findings",
                "agent",
                0,
                "typescript"
            ),
            ops.clone()
        ),
        with(
            p(
                "recur.chat.commit",
                "Commit",
                "Stage and commit all worktree changes",
                "commit",
                "agent",
                0,
                "typescript"
            ),
            ops.clone()
        ),
        with(
            p(
                "recur.chat.commit-push",
                "Commit Push",
                "Commit all changes and push to remote",
                "commit-push",
                "agent",
                0,
                "typescript"
            ),
            ops.clone()
        ),
        with(
            p(
                "recur.chat.open-pr",
                "Open PR",
                "Open a GitHub pull request with an agent-written description",
                "open-pr",
                "agent",
                15000,
                "shebang"
            ),
            ops.clone()
        ),
        with(
            p(
                "recur.chat.open-draft-pr",
                "Open draft PR",
                "Open a GitHub draft pull request with an agent-written description",
                "open-draft-pr",
                "agent",
                15000,
                "shebang"
            ),
            ops.clone()
        ),
        with(
            p(
                "recur.chat.fix-checks",
                "Fix checks",
                "Investigate and fix failing GitHub checks on this pull request",
                "fix-checks",
                "agent",
                15000,
                "shebang"
            ),
            json!({
                "takesArgs": true,
                "hasComplete": true,
                "freshSession": true,
                "tabGroup": "ops",
                "argPlaceholder": "all or a failing check",
                "args": [{ "name": "check", "placeholder": "all or a failing check", "optional": true }],
            })
        ),
        with(
            p(
                "recur.chat.review",
                "Review",
                "Review the current surface (workspace or pull request) and file findings",
                "review",
                "review",
                15000,
                "shebang"
            ),
            json!({ "freshSession": true, "tabGroup": "review" })
        ),
        with(
            p(
                "recur.chat.re-review",
                "Re-review",
                "Re-review the current surface after fixes",
                "re-review",
                "review",
                15000,
                "shebang"
            ),
            json!({ "freshSession": true })
        ),
        with(
            p(
                "recur.chat.simplify-review",
                "Simplify review",
                "Review the current surface for over-engineering and file findings",
                "simplify-review",
                "review",
                30000,
                "shebang"
            ),
            json!({ "freshSession": true, "tabGroup": "simplify-review" })
        ),
        with(
            p(
                "recur.chat.simplify-audit",
                "Simplify audit",
                "Audit the whole repository for over-engineering and file findings",
                "simplify-audit",
                "review",
                30000,
                "shebang"
            ),
            json!({ "freshSession": true, "tabGroup": "simplify-audit" })
        ),
        with(
            p(
                "recur.chat.review-and-fix",
                "Review & fix",
                "Review the current worktree and fix until the bar passes",
                "review-and-fix",
                "loop",
                0,
                "shebang"
            ),
            json!({ "loop": "recur.loops.converge" })
        ),
        with(
            p(
                "recur.chat.review-to-pr",
                "Review & fix to a PR",
                "Review and fix the current diff, then open a PR",
                "review-to-pr",
                "loop",
                0,
                "shebang"
            ),
            json!({ "loop": "recur.loops.converge-pr" })
        ),
        with(
            p(
                "recur.chat.to-a-pr",
                "To a PR",
                "Plan, implement, review, and open a pull request",
                "to-a-pr",
                "loop",
                0,
                "shebang"
            ),
            json!({ "loop": "recur.loops.pr" })
        ),
        with(
            p(
                "recur.chat.write-docs",
                "Write docs",
                "Plan, write, and edit documentation — no PR",
                "write-docs",
                "loop",
                0,
                "shebang"
            ),
            json!({ "loop": "recur.loops.docs" })
        ),
        with(
            p(
                "recur.chat.security-review",
                "Security review & fix",
                "Review and fix auth, secrets, and permissions until the bar passes",
                "security-review",
                "loop",
                0,
                "shebang"
            ),
            json!({ "loop": "recur.loops.security" })
        ),
    ])
}

fn prompt_result(prompt: &str, display: &str) -> Reply {
    Reply::Value(json!({ "prompt": prompt, "display": display }))
}

fn run_prompt_action(a: &[Value]) -> Reply {
    let id = a.first().and_then(Value::as_str).unwrap_or("");
    let ctx = a.get(1).unwrap_or(&Value::Null);
    let surface = js::string(nullish(get(ctx, "surface"), &json!("")));
    match id {
        "recur.chat.review" => match surface.as_str() {
            "github_pr" => prompt_result(
                "Review this GitHub pull request and file findings. Use gh pr diff — do not treat git diff in cwd as the PR.",
                "Please review this GitHub pull request",
            ),
            "local_pr" => prompt_result(
                "Review this Local PR and file findings. Do not make code changes.",
                "Please review this Local PR",
            ),
            _ => prompt_result(
                "Review the changes in this worktree and file findings. Git context is already gathered — do NOT re-run git status/diff/log.",
                "Please review the changes in this worktree and file any real issues",
            ),
        },
        "recur.chat.re-review" => match surface.as_str() {
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
            let raw = js::string(nullish(get(ctx, "args"), &json!("")))
                .trim()
                .to_owned();
            let args = if raw.is_empty() || raw.to_lowercase() == "all" {
                String::new()
            } else {
                raw
            };
            if args.is_empty() {
                prompt_result(
                    "Investigate and fix the failing GitHub checks on this PR. Use gh run view --log-failed.",
                    "Fix failing GitHub checks",
                )
            } else {
                prompt_result(
                    &format!(
                        "Investigate and fix the failing GitHub check {args}. Use gh run view --log-failed."
                    ),
                    &format!("Fix failing check: {args}"),
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
            "Write the documentation the human asked for. Do not change product code unless the docs require it. Do not open a PR — the write-up lives in this worktree.",
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
    prompt: "Open reviewer findings remain for this workspace. Call mcp__recur__list_findings to load them. Operator notes on a finding are instructions — follow them. For each open finding: verify it against the current code, then either fix it with minimal correct edits and mark it resolved via mcp__recur__update_finding, or mark it invalid with a clear reason if the claim is wrong or out of scope.",
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

fn commands() -> Value {
    let cmd = |id: &str,
               name: &str,
               cat: &str,
               order: i64,
               scope: &str,
               glob: &str,
               input: &str,
               fallback: &str,
               output: &str,
               triggers: &[&str],
               key: &str,
               interp: &str,
               bundle: &str| {
        json!({
            "id": id, "name": name, "category": cat, "order": order, "scope": scope,
            "fileGlob": glob, "input": input, "fallbackInput": fallback, "output": output,
            "triggers": strs(triggers), "prompts": [], "keyEquivalent": key, "language": "",
            "openInPreview": false, "interpreter": interp, "bundleName": bundle, "sourceRoot": "default",
        })
    };
    json!([
        cmd(
            "recur.text.sort-lines-asc",
            "Sort Lines Ascending",
            "Text",
            10,
            "",
            "",
            "selection",
            "document",
            "replaceInput",
            &["palette"],
            "ctrl+f5",
            "typescript",
            "Text"
        ),
        cmd(
            "recur.text.uppercase",
            "Uppercase",
            "Text",
            20,
            "",
            "",
            "selection",
            "document",
            "replaceInput",
            &["palette"],
            "ctrl+u",
            "typescript",
            "Text"
        ),
        cmd(
            "recur.go.gofmt-on-save",
            "Gofmt on Save",
            "Go",
            10,
            "source.go",
            "*.go",
            "document",
            "",
            "replaceDocument",
            &["willSave"],
            "",
            "shebang",
            "Go"
        ),
    ])
}

fn search_hit(a: &[Value]) -> Value {
    obj(vec![
        ("path", some("src/main.go")),
        ("line", some(3)),
        ("col", some(1)),
        ("text", a.get(2).cloned()),
        ("preview", some("func main() {}")),
    ])
}

fn commits() -> Value {
    json!([{
        "hash": "a3f91c2",
        "sha": "a3f91c2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "message": "Add feature",
        "author": "Ada",
        "authorRole": "Author",
        "agentCode": "AD",
        "color": "var(--indigo)",
        "iterLabel": "",
        "diff": { "add": 4, "del": 1 },
        "when": "2m",
        "files": [],
    }, {
        "hash": "b1c20d4",
        "sha": "b1c20d4bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "message": "Older work",
        "author": "Ada",
        "authorRole": "Author",
        "agentCode": "AD",
        "color": "var(--indigo)",
        "iterLabel": "",
        "diff": { "add": 2, "del": 0 },
        "when": "1d",
        "files": [],
    }])
}

fn read_body(path: &std::path::Path, label: &str) -> Reply {
    match std::fs::read_to_string(path) {
        Ok(body) => Reply::value(body),
        Err(e) => Reply::error(format!("{label}: {e}")),
    }
}

fn update_editor_session(a: &[Value], s: &mut Seed) -> Reply {
    let key = key_of(a.first());
    let p = nullish(a.get(1), &json!({})).clone();
    let mut cur = s
        .editor_sessions
        .get(&key)
        .cloned()
        .unwrap_or_else(|| json!({ "openFiles": [], "selectedPath": "" }));
    if let Some(meta) = get(&p, "meta").filter(|m| truthy(Some(m))) {
        js::set(
            &mut cur,
            "openFiles",
            Value::Array(items(get(meta, "openFiles"))),
        );
        js::set(
            &mut cur,
            "selectedPath",
            nullish(get(meta, "selectedPath"), &json!("")).clone(),
        );
        assign(&mut cur, "splitPath", field(meta, "splitPath"));
        assign(&mut cur, "scratchSeq", field(meta, "scratchSeq"));
        assign(
            &mut cur,
            "fileViews",
            get(meta, "fileViews").filter(|v| truthy(Some(v))).cloned(),
        );
        for k in ["syntaxByPath", "syntaxByFileType"] {
            if let Some(m) = get(meta, k) {
                let keep = m.as_object().is_some_and(|o| !o.is_empty());
                assign(&mut cur, k, keep.then(|| m.clone()));
            }
        }
        assign(
            &mut cur,
            "mainSurface",
            (get_str(meta, "mainSurface") == "review").then(|| json!("review")),
        );
        for k in ["leftKind", "rightKind"] {
            assign(
                &mut cur,
                k,
                get(meta, k).filter(|v| truthy(Some(v))).cloned(),
            );
        }
        for k in ["leftOpen", "rightOpen"] {
            assign(
                &mut cur,
                k,
                get(meta, k).filter(|v| v.is_boolean()).cloned(),
            );
        }
    }
    for (bucket, drops) in [
        ("buffers", "dropBuffers"),
        ("scratchDocs", "dropScratchDocs"),
    ] {
        if let Some(Value::Object(add)) = get(&p, bucket) {
            if !matches!(get(&cur, bucket), Some(Value::Object(_))) {
                js::set(&mut cur, bucket, json!({}));
            }
            if let Some(Value::Object(m)) = cur.get_mut(bucket) {
                for (path, v) in add {
                    m.insert(path.clone(), v.clone());
                }
            }
        }
        for path in items(get(&p, drops)) {
            if let Some(Value::Object(m)) = cur.get_mut(bucket) {
                m.remove(&js::string(&path));
            }
        }
    }
    let len = |k: &str| get(&cur, k).and_then(Value::as_object).map_or(0, Map::len);
    let kind = |k: &str| get(&cur, k).filter(|v| truthy(Some(v))).map(js::string);
    let empty = get(&cur, "openFiles")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
        && len("buffers") == 0
        && len("scratchDocs") == 0
        && len("syntaxByPath") == 0
        && len("syntaxByFileType") == 0
        && get_str(&cur, "mainSurface") != "review"
        && kind("leftKind").is_none_or(|k| k == "maestro")
        && kind("rightKind").is_none_or(|k| k == "files")
        && get(&cur, "leftOpen") != Some(&Value::Bool(false))
        && get(&cur, "rightOpen") != Some(&Value::Bool(false));
    if empty {
        s.editor_sessions.remove(&key);
    } else {
        s.editor_sessions.insert(key, cur);
    }
    Reply::null()
}

fn add_pr_comment(a: &[Value], s: &mut Seed) -> Reply {
    let id = key_of(a.first());
    let file = a.get(1).cloned().unwrap_or(Value::Null);
    let line = a.get(2).cloned().unwrap_or(Value::Null);
    let body = a.get(3);
    let start_line = a.get(4);
    let body_s = js::string(nullish(body, &json!("")));
    let finding_id = js::string(nullish(a.get(5), &json!(""))).trim().to_owned();
    if s.pr_by_id(&id).is_some() && truthy(Some(&file)) && truthy(Some(&line)) {
        let line_n = js::number(Some(&line));
        let side = if line_n < 0.0 { "LEFT" } else { "RIGHT" };
        let n = (if line_n.is_nan() { 0.0 } else { line_n }).abs();
        let start = js::number(start_line);
        let start_n = (if start.is_nan() { 0.0 } else { start }).abs();
        let start_side = if start < 0.0 { "LEFT" } else { "RIGHT" };
        let text = body_s.trim().to_owned();
        let Some(pr) = s.pr_by_id_mut(&id) else {
            return Reply::null();
        };
        let pending = items(get(pr, "pendingComments"));
        if text.is_empty() {
            let kept: Vec<Value> = pending
                .into_iter()
                .filter(|c| {
                    finding_id.is_empty()
                        || (get_str(c, "id") != finding_id
                            && js::string(or(get(c, "findingId"), &json!(""))).trim() != finding_id)
                })
                .collect();
            js::set(pr, "pendingComments", Value::Array(kept));
            return Reply::Value(now_comment(""));
        }
        let mut pc = json!({
            "id": format!("pend-{}", now_ms()),
            "file": file,
            "line": js::num(n),
            "side": side,
            "body": text,
        });
        if !finding_id.is_empty() {
            js::set(&mut pc, "findingId", Value::String(finding_id));
        }
        if start_n > 0.0 && start_n < n {
            js::set(&mut pc, "startLine", js::num(start_n));
            js::set(&mut pc, "startSide", start_side.into());
        }
        let mut pending = pending;
        pending.push(pc);
        js::set(pr, "pendingComments", Value::Array(pending));
        return Reply::Value(now_comment(&text));
    }
    let loc = if truthy(Some(&file)) {
        let suffix = if js::number(Some(&line)) > 0.0 {
            format!(":{}", js::string(&line))
        } else {
            String::new()
        };
        format!("<code>{}{suffix}</code> — ", js::string(&file))
    } else {
        String::new()
    };
    let mut c = now_comment(&format!("{loc}{body_s}"));
    js::set(&mut c, "bodyMd", Value::String(body_s));
    js::set(&mut c, "author", "You".into());
    let login = s
        .pr_by_id(&id)
        .map(|pr| or(get(pr, "viewerLogin"), &json!("avery")).clone())
        .unwrap_or_else(|| json!("avery"));
    js::set(&mut c, "authorLogin", login);
    if let Some(pr) = s.pr_by_id_mut(&id) {
        let mut comments = items(get(pr, "comments"));
        comments.push(c.clone());
        js::set(pr, "comments", Value::Array(comments));
    }
    Reply::Value(c)
}

fn toggle_reaction(a: &[Value], s: &mut Seed) -> Reply {
    let key = key_of(a.first());
    let Some(pr) = s.pr_by_id_mut(&key) else {
        return Reply::null();
    };
    let id = str_arg(a, 1);
    let content = str_arg(a, 2).trim().to_owned();
    const ORDER: [&str; 8] = [
        "+1", "-1", "laugh", "hooray", "confused", "heart", "rocket", "eyes",
    ];
    let lower = content.to_lowercase();
    let n = if lower == "thumbs_up" {
        "+1".to_owned()
    } else if lower == "thumbs_down" {
        "-1".to_owned()
    } else if ORDER.contains(&content.as_str()) {
        content
    } else {
        lower
    };
    if id.is_empty() || !ORDER.contains(&n.as_str()) {
        return Reply::Value(pr.clone());
    }
    let comments: Vec<Value> = items(get(pr, "comments"))
        .into_iter()
        .map(|c| {
            if get(&c, "id").and_then(Value::as_str) != Some(id.as_str()) {
                return c;
            }
            let mut rs = items(get(&c, "reactions"));
            match rs
                .iter()
                .position(|r| get(r, "content").and_then(Value::as_str) == Some(n.as_str()))
            {
                Some(i) => {
                    let cur = rs[i].clone();
                    let count = js::number(get(&cur, "count"));
                    rs[i] = if truthy(get(&cur, "mine")) {
                        js::spread(&[
                            &cur,
                            &json!({ "count": js::num(count - 1.0), "mine": false }),
                        ])
                    } else {
                        js::spread(&[
                            &cur,
                            &json!({ "count": js::num(count + 1.0), "mine": true }),
                        ])
                    };
                }
                None => rs.push(json!({ "content": n, "count": 1, "mine": true })),
            }
            let rs: Vec<Value> = rs
                .into_iter()
                .filter(|r| js::number(Some(nullish(get(r, "count"), &json!(0)))) > 0.0)
                .collect();
            js::spread(&[&c, &json!({ "reactions": rs })])
        })
        .collect();
    js::set(pr, "comments", Value::Array(comments));
    Reply::Value(pr.clone())
}

fn set_pr_draft(a: &[Value], s: &mut Seed) -> Reply {
    let key = key_of(a.first());
    let Some(pr) = s.pr_by_id(&key).cloned() else {
        return Reply::Value(json!({}));
    };
    let draft = truthy(a.get(1));
    let status = if draft {
        json!("DRAFT")
    } else if get_str(&pr, "mergeStateStatus") == "DRAFT"
        && get(&pr, "mergeStateStatus").is_some_and(Value::is_string)
    {
        json!("CLEAN")
    } else {
        nullish(get(&pr, "mergeStateStatus"), &json!("CLEAN")).clone()
    };
    let next = js::spread(&[&pr, &json!({ "draft": draft, "mergeStateStatus": status })]);
    let pr_id = key_of(get(&pr, "id"));
    let obj_id = s.alloc(next.clone());
    s.set_pr_by_id(&pr_id, obj_id);
    let mut prs = s.pr_ids().to_vec();
    if let Some(i) = prs
        .iter()
        .position(|p| get(s.obj(*p), "id") == get(&pr, "id"))
    {
        prs[i] = obj_id;
    }
    s.set_prs(prs);
    Reply::Value(next)
}

fn search_files(a: &[Value], seed: &mut Seed, table: &Table) -> Reply {
    let query = str_arg(a, 2).trim().to_lowercase();
    let lim = js::number(a.get(3));
    let limit = if lim > 0.0 { lim as usize } else { 50 };
    let files_only = truthy(a.get(4));
    let files = match table.get("ListFiles") {
        Some(f) => match f(a, seed, table) {
            Reply::Value(Value::Array(v)) => v,
            Reply::Value(Value::Null) => {
                return Reply::error("Cannot read properties of null (reading 'filter')");
            }
            _ => return Reply::error("files.filter is not a function"),
        },
        None => vec![json!("src/main.go"), json!("README.md")],
    };
    let matches = |p: &str| query.is_empty() || p.to_lowercase().contains(&query);
    let file_hits: Vec<Value> = files
        .iter()
        .map(js::string)
        .filter(|p| matches(p))
        .map(|p| json!({ "path": p, "score": 1 }))
        .collect();
    if files_only {
        let hits: Vec<Value> = file_hits.into_iter().take(limit).collect();
        return Reply::Value(json!({ "hits": hits, "building": false, "ready": true }));
    }
    let mut seen = std::collections::HashSet::new();
    let mut dirs: Vec<String> = Vec::new();
    for file in &files {
        let mut p = js::string(file);
        while let Some(i) = p.rfind('/').filter(|i| *i > 0) {
            p.truncate(i);
            if !seen.insert(p.clone()) {
                break;
            }
            dirs.push(p.clone());
        }
    }
    let dir_hits = dirs
        .into_iter()
        .filter(|p| matches(p))
        .map(|p| json!({ "path": p, "score": 1, "dir": true }));
    let hits: Vec<Value> = file_hits.into_iter().chain(dir_hits).take(limit).collect();
    Reply::Value(json!({ "hits": hits, "building": false, "ready": true }))
}

#[allow(clippy::too_many_lines)]
pub(super) fn base() -> Table {
    let mut t = Table::default();

    // Projects
    t.insert("ListProjects", |_a, s, _t| arr(s.projects.clone()));
    t.insert("ListProjectGroups", |_a, s, _t| {
        arr(s.project_groups.clone())
    });
    t.insert("GetProject", |a, s, _t| {
        Reply::Value(
            s.projects
                .iter()
                .find(|p| get(p, "id") == a.first())
                .cloned()
                .unwrap_or(Value::Null),
        )
    });
    t.insert("AddProject", |a, s, _t| {
        let a0 = a.first().unwrap_or(&Value::Null);
        let name = or(get(a0, "name"), &json!("new-project")).clone();
        // The UI never sends `open` or `isGit` on create, so the TypeScript's
        // `?? default` always applied; a typed caller sends the zero value
        // for them, which reads the same.
        let is_git = get(a0, "isGit") == Some(&Value::Bool(true))
            || !js::string(or(get(a0, "repoPath"), &json!(""))).contains("no-git");
        let p = js::spread(&[
            a0,
            &json!({
                "id": or(get(a0, "id"), &name),
                "name": name,
                "open": true,
                "baseBranch": or(get(a0, "baseBranch"), &json!("origin/main")),
                "isGit": is_git,
            }),
        ]);
        s.projects.push(p.clone());
        let pid = get(&p, "id").cloned().unwrap_or(Value::Null);
        let has_main = s
            .loops()
            .any(|l| truthy(get(l, "isMain")) && get(l, "projectId") == Some(&pid));
        if is_git && !has_main {
            let local = local_branch(get(&p, "baseBranch"));
            let ws = with_mock_worktree(&main_workspace(
                format!("MAIN-{}", js::string(&pid)),
                pid,
                Some(local),
            ));
            s.insert_loop(ws);
        }
        Reply::Value(p)
    });
    t.insert("SuggestBaseBranch", |a, _s, _t| {
        let path = js::string(or(a.first(), &json!("")));
        if path.is_empty() || path.contains("no-git") {
            return Reply::value("main");
        }
        Reply::value("origin/main")
    });
    t.insert("ListBranchesForPath", |a, _s, _t| {
        let path = js::string(or(a.first(), &json!("")));
        if path.is_empty() || path.contains("no-git") {
            return arr(Vec::new());
        }
        let known: Vec<String> = ["origin/main", "develop", "feature/auth", "main"]
            .map(String::from)
            .to_vec();
        let preferred = js::string(or(a.get(1), &json!("")));
        let names = if !preferred.is_empty() && known.contains(&preferred) {
            std::iter::once(preferred).chain(known).collect()
        } else {
            known
        };
        Reply::Value(branch_choices(names))
    });
    t.insert("AddProjectGroup", |a, s, _t| {
        let a0 = a.first().unwrap_or(&Value::Null);
        let id = match get(a0, "id") {
            Some(v) if truthy(Some(v)) => v.clone(),
            _ => Value::String(new_uuid()),
        };
        let g = json!({
            "id": id,
            "name": or(get(a0, "name"), &json!("New group")),
            "open": nullish(get(a0, "open"), &json!(true)),
        });
        s.project_groups.push(g.clone());
        Reply::Value(g)
    });
    t.insert("UpdateProjectGroup", |a, s, _t| {
        let g = a.first().cloned().unwrap_or(Value::Null);
        let gid = get(&g, "id").cloned();
        for x in &mut s.project_groups {
            if get(x, "id").cloned() == gid {
                *x = js::spread(&[x, &g]);
            }
        }
        let found = s
            .project_groups
            .iter()
            .find(|x| get(x, "id").cloned() == gid)
            .cloned();
        Reply::Value(found.unwrap_or(g))
    });
    t.insert("DeleteProjectGroup", |a, s, _t| {
        let id = a.first().cloned();
        s.project_groups.retain(|x| get(x, "id").cloned() != id);
        for p in &mut s.projects {
            if get(p, "groupId").cloned() == id {
                *p = js::spread(&[p, &json!({ "groupId": "" })]);
            }
        }
        Reply::null()
    });
    t.insert("ReorderProjectGroups", |a, s, _t| {
        s.project_groups = reorder(&s.project_groups, &items(a.first()));
        Reply::null()
    });
    t.insert("SetProjectGroup", |a, s, _t| {
        let pid = a.first().cloned();
        let gid = or(a.get(1), &json!("")).clone();
        for p in &mut s.projects {
            if get(p, "id").cloned() == pid {
                *p = js::spread(&[p, &json!({ "groupId": gid })]);
            }
        }
        Reply::Value(
            s.projects
                .iter()
                .find(|p| get(p, "id").cloned() == pid)
                .cloned()
                .unwrap_or(Value::Null),
        )
    });
    t.insert("UpdateProjectRunConfig", |a, s, _t| {
        let p = a.first().cloned().unwrap_or(Value::Null);
        let pid = get(&p, "id").cloned();
        for x in &mut s.projects {
            if get(x, "id").cloned() == pid {
                // The argument is a whole Project in JSON, so a flag it omits is false.
                let cleared: serde_json::Map<String, Value> = x
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(k, v)| v.is_boolean() && get(&p, k).is_none())
                    .map(|(k, _)| (k.clone(), Value::Bool(false)))
                    .collect();
                *x = js::spread(&[x, &Value::Object(cleared), &p]);
            }
        }
        let found = s
            .projects
            .iter()
            .find(|x| get(x, "id").cloned() == pid)
            .cloned();
        Reply::Value(found.unwrap_or(p))
    });
    t.insert("PreviewCopyFiles", |a, _s, _t| {
        let patterns = str_arg(a, 1).trim().to_owned();
        let root = "/Users/josh/code/storefront";
        if patterns.is_empty() {
            return Reply::Value(json!({ "root": root, "files": [], "count": 0 }));
        }
        Reply::Value(json!({
            "root": root,
            "files": ["apps/web/.env", "apps/api/.env", "deploy/.env.mainnet"],
            "count": 3,
        }))
    });
    t.insert("ReorderProjects", |a, s, _t| {
        s.projects = reorder(&s.projects, &items(a.first()));
        Reply::null()
    });
    t.insert("DeleteProject", |_a, _s, _t| Reply::null());
    t.insert("ListBranches", |a, s, _t| {
        let p = s.projects.iter().find(|x| get(x, "id") == a.first());
        let Some(base) = p
            .and_then(|p| get(p, "baseBranch"))
            .filter(|b| truthy(Some(b)))
        else {
            return arr(Vec::new());
        };
        let names = std::iter::once(js::string(base))
            .chain(
                [
                    "origin/main",
                    "develop",
                    "feature/auth",
                    "feature/search",
                    "hotfix/login",
                ]
                .map(String::from),
            )
            .collect();
        Reply::Value(branch_choices(names))
    });

    // Workspaces
    t.insert("ListWorkspaces", |_a, s, _t| {
        let mut v: Vec<Value> = s.loops().map(with_mock_worktree).collect();
        v.reverse();
        arr(v)
    });
    t.insert("GetWorkspace", |a, s, _t| {
        Reply::Value(with_mock_worktree(
            s.loop_by_id(&key_of(a.first())).unwrap_or(&Value::Null),
        ))
    });
    t.insert("ListOpenInTargets", |_a, _s, _t| {
        Reply::Value(json!([
            { "id": "finder", "name": "Finder", "kind": "app" },
            { "id": "cursor", "name": "Cursor", "kind": "app" },
            { "id": "terminal", "name": "Terminal", "kind": "app" },
        ]))
    });
    t.insert("OpenWorkspaceIn", |_a, _s, _t| Reply::null());
    t.insert("GetWorkspaceFeed", |a, s, _t| {
        Reply::Value(
            s.feed_by_loop
                .get(&key_of(a.first()))
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
    });
    t.insert("LastAssistantText", |_a, _s, _t| Reply::value(""));
    t.insert("WatchWorkspace", |_a, _s, _t| Reply::null());
    t.insert("WorkspaceSessions", |_a, _s, _t| arr(Vec::new()));
    t.insert("WorkspaceChatItems", |_a, _s, _t| arr(Vec::new()));
    t.insert("WorkspaceChatModels", |_a, _s, _t| arr(Vec::new()));
    t.insert("WorkspaceChatSelection", |_a, _s, _t| {
        Reply::Value(json!({ "model": "", "flags": {}, "mode": "" }))
    });
    t.insert("RememberWorkspaceChatSelection", |_a, _s, _t| Reply::null());
    t.insert("RememberWorkspaceChatSimplify", |_a, _s, _t| Reply::null());
    t.insert("ApplyProvisionalChatTitles", |_a, _s, _t| Reply::null());
    t.insert("RememberWorkspaceChatPromptGroup", |a, s, _t| {
        let (ws_id, code, group) = (str_arg(a, 0), str_arg(a, 1), str_arg(a, 2));
        if ws_id.is_empty() || code.is_empty() || group.is_empty() {
            return Reply::null();
        }
        let list = s.chats_by_loop.entry(ws_id).or_default();
        match list.iter().position(|t| get_str(t, "code") == code) {
            Some(i) => list[i] = js::spread(&[&list[i], &json!({ "promptGroup": group })]),
            None => list.push(
                json!({ "code": code, "title": "New chat", "updatedAt": 0, "promptGroup": group }),
            ),
        }
        Reply::null()
    });
    t.insert("RememberChatLoopPolicy", |a, s, _t| {
        let (ws_id, code, loop_id, policy_id) =
            (str_arg(a, 0), str_arg(a, 1), str_arg(a, 2), str_arg(a, 3));
        let mem = json!({ "loopId": loop_id, "policyId": policy_id });
        s.chat_loop_by_key
            .insert(format!("{ws_id}\0{code}"), mem.clone());
        let extra = s.chats_by_loop.get(&ws_id).cloned().unwrap_or_default();
        match extra.iter().position(|t| get_str(t, "code") == code) {
            Some(i) => {
                if let Some(list) = s.chats_by_loop.get_mut(&ws_id) {
                    list[i] = js::spread(&[&list[i], &mem]);
                }
            }
            None if !code.is_empty() && code != "PA" && code != "MA" => {
                let mut list = extra;
                list.push(json!({
                    "code": code, "title": "New chat", "updatedAt": 0,
                    "loopId": loop_id, "policyId": policy_id,
                }));
                s.chats_by_loop.insert(ws_id, list);
            }
            None => {}
        }
        Reply::null()
    });
    t.insert("SetWorkspaceChatFocus", |_a, _s, _t| Reply::null());
    t.insert("SetChatUIOccluded", |_a, _s, _t| Reply::null());
    t.insert("MarkWorkspaceChatSeen", |_a, _s, _t| Reply::null());
    t.insert("AttentionCounts", |_a, _s, _t| {
        Reply::Value(json!({ "unseen": 0, "inboxUnread": 0, "dock": 0, "appUnseen": 0, "byWorkspace": {}, "byProject": {} }))
    });
    t.insert("GetBriefing", |_a, s, _t| Reply::Value(mock_briefing(s)));
    t.insert("MarkWorkspaceViewed", |_a, _s, _t| Reply::null());
    t.insert("AppChatItems", |_a, _s, _t| arr(Vec::new()));
    t.insert("AppChatSelection", |_a, _s, _t| {
        Reply::Value(json!({ "model": "", "flags": {} }))
    });
    t.insert("RememberAppChatSelection", |_a, _s, _t| Reply::null());
    t.insert("SendAppMessage", |_a, _s, _t| Reply::value(""));
    t.insert("CancelAppMessage", |_a, _s, _t| Reply::null());
    t.insert("SetAppChatFocus", |_a, _s, _t| Reply::null());
    t.insert("MarkAppChatSeen", |_a, _s, _t| Reply::null());
    fn app_tabs(extra: &[Value]) -> Reply {
        let mut v = vec![json!({ "code": "APP", "title": "Ask Recur", "mode": "ask" })];
        v.extend(
            extra
                .iter()
                .map(|t| js::spread(&[t, &json!({ "mode": "ask" })])),
        );
        arr(v)
    }
    t.insert("ListAppChats", |_a, s, _t| app_tabs(&s.app_chats));
    t.insert("NewAppChat", |_a, s, _t| {
        let tab = json!({ "code": format!("AP{}", s.app_chats.len() + 2), "title": "New chat" });
        s.app_chats.push(tab.clone());
        Reply::Value(js::spread(&[&tab, &json!({ "mode": "ask" })]))
    });
    t.insert("CloseAppChat", |a, s, _t| {
        let code = str_arg(a, 0);
        s.app_chats.retain(|t| {
            get_str(t, "code") != code || !get(t, "code").is_some_and(Value::is_string)
        });
        app_tabs(&s.app_chats)
    });
    t.insert("ListWorkspaceChats", |a, s, _t| {
        arr(list_workspace_chats(s, &key_of(a.first())))
    });
    t.insert("CreateWorkspaceChat", |a, s, _t| {
        let key = key_of(a.first());
        let list = s.chats_by_loop.entry(key).or_default();
        let tab =
            json!({ "code": format!("PA{}", list.len() + 2), "title": "New chat", "updatedAt": 0 });
        list.push(tab.clone());
        Reply::Value(tab)
    });
    t.insert("ForkWorkspaceChat", |a, s, _t| {
        let key = key_of(a.first());
        let list = s.chats_by_loop.entry(key).or_default();
        let tab = json!({ "code": format!("PA{}", list.len() + 2), "title": "Forked chat", "updatedAt": now_ms() });
        list.push(tab.clone());
        Reply::Value(tab)
    });
    t.insert("WorkspaceChatTurnDiff", |_a, _s, _t| {
        Reply::Value(json!({ "hash": "", "message": "", "files": [] }))
    });
    t.insert("ResetWorkspaceChat", |_a, _s, _t| Reply::null());
    t.insert("ForkWorkspaceChatToWorkspace", |a, s, _t| {
        let src = s.loop_by_id(&key_of(a.first())).cloned();
        let src = src.as_ref();
        let ws = obj(vec![
            ("id", some("WS-FORK")),
            ("projectId", src.and_then(|w| field(w, "projectId"))),
            ("type", some("solo")),
            ("goal", some("Forked chat")),
            ("short", some("Forked chat")),
            ("group", some("active")),
            ("statusLabel", some("Creating")),
            ("phase", some("creating")),
            ("branch", some("recur/fork")),
            (
                "baseBranch",
                Some(or(src.and_then(|w| get(w, "baseBranch")), &json!("main")).clone()),
            ),
        ]);
        s.insert_loop(ws.clone());
        Reply::Value(ws)
    });
    t.insert("CloseWorkspaceChat", |a, s, _t| {
        let ws_id = key_of(a.first());
        let code = a.get(1).cloned().unwrap_or(Value::Null);
        let extra = s.chats_by_loop.get(&ws_id).cloned().unwrap_or_default();
        let closed = s
            .closed_chats_by_loop
            .get(&ws_id)
            .cloned()
            .unwrap_or_default();
        if code == json!("MA") {
            s.hidden_maestro.insert(ws_id.clone());
            if let Some(w) = s.loop_by_id_mut(&ws_id) {
                js::set(w, "loopChatCode", json!(""));
            }
            let tab =
                json!({ "code": "MA", "title": "Loop", "mode": "loop", "updatedAt": now_ms() });
            let mut next = vec![tab];
            next.extend(closed.into_iter().filter(|t| get_str(t, "code") != "MA"));
            s.closed_chats_by_loop.insert(ws_id.clone(), next);
        } else if let Some(tab) = extra.iter().find(|t| get(t, "code") == Some(&code)) {
            let updated = or(get(tab, "updatedAt"), &json!(now_ms())).clone();
            let closed_tab = js::spread(&[tab, &json!({ "updatedAt": updated })]);
            let mut next = vec![closed_tab];
            next.extend(closed.into_iter().filter(|t| get(t, "code") != Some(&code)));
            s.closed_chats_by_loop.insert(ws_id.clone(), next);
            let rest = extra
                .iter()
                .filter(|t| get(t, "code") != Some(&code))
                .cloned()
                .collect();
            s.chats_by_loop.insert(ws_id.clone(), rest);
        }
        arr(list_workspace_chats(s, &ws_id))
    });
    t.insert("ListClosedWorkspaceChats", |a, s, _t| {
        arr(s
            .closed_chats_by_loop
            .get(&key_of(a.first()))
            .cloned()
            .unwrap_or_default())
    });
    t.insert("DeleteClosedWorkspaceChat", |a, s, _t| {
        let ws_id = key_of(a.first());
        let code = a.get(1).cloned();
        if list_workspace_chats(s, &ws_id)
            .iter()
            .any(|t| get(t, "code").cloned() == code)
        {
            return arr(s
                .closed_chats_by_loop
                .get(&ws_id)
                .cloned()
                .unwrap_or_default());
        }
        let list = s.closed_chats_by_loop.entry(ws_id).or_default();
        list.retain(|t| get(t, "code").cloned() != code);
        arr(list.clone())
    });
    t.insert("ReopenWorkspaceChat", |a, s, _t| {
        let ws_id = key_of(a.first());
        let code = a.get(1).cloned();
        let list = s
            .closed_chats_by_loop
            .get(&ws_id)
            .cloned()
            .unwrap_or_default();
        if let Some(tab) = list
            .iter()
            .find(|t| get(t, "code").cloned() == code)
            .cloned()
        {
            let rest = list
                .into_iter()
                .filter(|t| get(t, "code").cloned() != code)
                .collect();
            s.closed_chats_by_loop.insert(ws_id.clone(), rest);
            if code == Some(json!("MA")) {
                s.hidden_maestro.remove(&ws_id);
                if let Some(w) = s.loop_by_id_mut(&ws_id)
                    && !truthy(get(w, "loopChatCode"))
                {
                    js::set(w, "loopChatCode", json!("MA"));
                }
            } else {
                let tab_code = get(&tab, "code").cloned();
                let mut open: Vec<Value> = s
                    .chats_by_loop
                    .get(&ws_id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| get(t, "code").cloned() != tab_code)
                    .collect();
                open.push(tab);
                s.chats_by_loop.insert(ws_id.clone(), open);
            }
        }
        arr(list_workspace_chats(s, &ws_id))
    });
    t.insert("ReorderWorkspaceChats", |a, s, _t| {
        let ws_id = key_of(a.first());
        let codes = items(a.get(1));
        let current = list_workspace_chats(s, &ws_id);
        if codes.len() != current.len() {
            return Reply::error(format!(
                "reorder chats: got {} codes, have {} tabs",
                codes.len(),
                current.len()
            ));
        }
        let have: Vec<Option<&Value>> = current.iter().map(|t| get(t, "code")).collect();
        let mut seen: Vec<&Value> = Vec::new();
        for code in &codes {
            if !have.contains(&Some(code)) {
                return Reply::error(format!("reorder chats: unknown tab {}", js::string(code)));
            }
            if seen.contains(&code) {
                return Reply::error(format!("reorder chats: duplicate tab {}", js::string(code)));
            }
            seen.push(code);
        }
        s.chat_order_by_loop
            .insert(ws_id, codes.iter().map(js::string).collect());
        Reply::null()
    });
    t.insert("WorkspaceFacts", |_a, _s, _t| arr(Vec::new()));
    t.insert("SendWorkspaceMessage", |_a, _s, _t| {
        Reply::value("Working on it (mock reply).")
    });
    t.insert("CancelWorkspaceMessage", |_a, _s, _t| Reply::null());
    t.insert("StartWorkspace", |_a, _s, _t| Reply::value("LOOP-NEW"));
    t.insert("CreateWorkspace", |a, _s, _t| created_workspace(a, false));
    t.insert("CreateWorkspaceWith", |a, _s, _t| {
        created_workspace(a, true)
    });
    t.insert("ListPolicies", |_a, _s, _t| {
        Reply::Value(json!([
            { "id": "recur.policies.cautious", "name": "Cautious", "hint": "Close every finding; escalate after 2 rounds" },
            { "id": "recur.policies.balanced", "name": "Balanced", "hint": "Fix medium and up; escalate after 4 rounds" },
            { "id": "recur.policies.aggressive", "name": "Aggressive", "hint": "Ship-blockers only; proceed after 6 rounds" },
        ]))
    });
    t.insert("SetWorkspaceLoopPolicy", |a, s, _t| {
        let id = key_of(a.first());
        let Some(w) = s.loop_by_id(&id).cloned() else {
            return Reply::error(format!("workspace {id} not found"));
        };
        let mut updated = js::spread(&[&w]);
        assign(
            &mut updated,
            "loopId",
            or_field(a.get(1), get(&w, "loopId")),
        );
        assign(
            &mut updated,
            "policyId",
            or_field(a.get(2), get(&w, "policyId")),
        );
        let o = s.alloc(updated.clone());
        s.set_loop_by_id(&id, o);
        s.replace_loop(&id, o);
        Reply::Value(updated)
    });
    t.insert("EngageLoopWith", |_a, _s, _t| {
        Reply::value("Loop engaged — planning has started.")
    });
    t.insert("SetWorkspaceBaseBranch", |a, s, _t| {
        let id = key_of(a.first());
        let base = str_arg(a, 1).trim().to_owned();
        let Some(w) = s.loop_by_id(&id).cloned() else {
            return Reply::error(format!("workspace {id} not found"));
        };
        if truthy(get(&w, "isMain")) {
            return Reply::error("the main working copy has no merge base");
        }
        if base.is_empty() {
            return Reply::error("base branch is required");
        }
        if truthy(get(&w, "branch")) && get(&w, "branch") == Some(&Value::String(base.clone())) {
            return Reply::error("target branch cannot be the workspace's own branch");
        }
        let updated = with_mock_worktree(&js::spread(&[&w, &json!({ "baseBranch": base })]));
        let o = s.alloc(updated.clone());
        s.set_loop_by_id(&id, o);
        s.replace_loop(&id, o);
        let prs: Vec<_> = s.pr_ids().to_vec();
        let mut next = Vec::with_capacity(prs.len());
        for p in prs {
            let pv = s.obj(p);
            if get_str(pv, "workspaceId") == id
                && get(pv, "workspaceId").is_some_and(Value::is_string)
                && get_str(pv, "state") != "Merged"
                && get_str(pv, "group") != "merged"
            {
                let spread = js::spread(&[pv, &json!({ "base": base })]);
                next.push(s.alloc(spread));
            } else {
                next.push(p);
            }
        }
        s.set_prs(next);
        Reply::Value(updated)
    });
    t.insert("RemoveWorkspace", |a, s, _t| {
        let id = key_of(a.first());
        let cur = s.loop_by_id(&id).cloned().unwrap_or_else(|| json!({}));
        let updated = js::spread(&[&cur, &json!({ "archived": true, "worktreePath": "" })]);
        let o = s.alloc(updated.clone());
        s.set_loop_by_id(&id, o);
        s.replace_loop(&id, o);
        Reply::Value(updated)
    });
    t.insert("DeleteWorkspace", |_a, _s, _t| Reply::null());
    t.insert("WorkspaceDiscardRisk", |_a, _s, _t| Reply::value(""));
    t.insert("OpenWorkspacePR", |a, s, _t| {
        Reply::Value(
            s.loop_by_id(&key_of(a.first()))
                .cloned()
                .unwrap_or(Value::Null),
        )
    });
    t.insert("PullWorkspacePR", |a, s, _t| {
        Reply::Value(json!({
            "status": "updated",
            "workspace": with_mock_worktree(&loop_or_empty(s, a)),
            "base": "origin/main",
        }))
    });
    t.insert("MergeWorkspace", |a, s, _t| {
        let w = js::spread(&[
            &loop_or_empty(s, a),
            &json!({ "state": "merged", "statusLabel": "merged" }),
        ]);
        Reply::Value(with_mock_worktree(&w))
    });
    t.insert("UpdateWorkspaceWithBase", |a, s, _t| {
        Reply::Value(json!({
            "status": "updated",
            "workspace": with_mock_worktree(&loop_or_empty(s, a)),
            "base": "main",
        }))
    });
    t.insert("UpdateWorkingCopyFromRemote", |a, s, _t| {
        Reply::Value(
            json!({ "status": "updated", "workspace": loop_or_empty(s, a), "base": "origin/main" }),
        )
    });
    t.insert("FetchWorkingCopyUpstream", |a, s, _t| {
        land_state(s, &key_of(a.first()), "")
    });
    t.insert("PushWorkingCopy", |a, s, _t| {
        Reply::Value(loop_or_empty(s, a))
    });
    t.insert("WorkspaceLandState", |a, s, _t| {
        land_state(s, &key_of(a.first()), "main")
    });
    t.insert("OpenPath", |a, s, _t| {
        let path = str_arg(a, 0);
        let name = path
            .split('/')
            .rfind(|p| !p.is_empty())
            .unwrap_or("opened")
            .to_owned();
        let p = match s.projects.iter().find(|x| get_str(x, "repoPath") == path && get(x, "repoPath").is_some_and(Value::is_string)) {
            Some(p) => p.clone(),
            None => {
                let p = json!({ "id": format!("open-{name}"), "name": name, "repoPath": path, "isGit": true, "open": true });
                s.projects.push(p.clone());
                p
            }
        };
        let pid = get(&p, "id").cloned().unwrap_or(Value::Null);
        let existing = s
            .find_loop(|l| truthy(get(l, "isMain")) && get(l, "projectId") == Some(&pid))
            .map(|o| s.obj(o).clone());
        let created = existing.is_none();
        let ws = match existing {
            Some(ws) => ws,
            None => {
                let ws = with_mock_worktree(&main_workspace(format!("MAIN-{}", js::string(&pid)), pid.clone(), None));
                s.insert_loop(ws.clone());
                ws
            }
        };
        Reply::Value(json!({
            "path": path,
            "projectId": pid,
            "workspaceId": get(&ws, "id"),
            "created": created,
            "project": p,
            "workspace": ws,
        }))
    });
    t.insert("OpenMainWorkspace", |a, s, _t| {
        let project_id = a.first().cloned().unwrap_or(Value::Null);
        if let Some(o) =
            s.find_loop(|l| truthy(get(l, "isMain")) && get(l, "projectId") == Some(&project_id))
        {
            return Reply::Value(with_mock_worktree(s.obj(o)));
        }
        let base = s
            .projects
            .iter()
            .find(|x| get(x, "id") == Some(&project_id))
            .and_then(|p| get(p, "baseBranch"))
            .cloned();
        let ws = with_mock_worktree(&main_workspace(
            format!("MAIN-{}", key_of(Some(&project_id))),
            project_id,
            Some(local_branch(base.as_ref())),
        ));
        s.insert_loop(ws.clone());
        Reply::Value(ws)
    });
    t.insert("UpdatePlan", |_a, _s, _t| Reply::null());
    t.insert("RetryLoop", |_a, _s, _t| Reply::null());
    t.insert("RestartLoop", |_a, _s, _t| Reply::value("Restarting"));
    // `ListLoops` appears twice in the TypeScript object literal; this later
    // pre-rename alias (the workspace list) is what the UI's ListLoops got.
    t.insert("ListLoops", |_a, s, _t| arr(s.loops().cloned().collect()));
    t.insert("PauseLoop", |_a, _s, _t| Reply::null());
    t.insert("ResumeLoop", |_a, _s, _t| Reply::null());
    t.insert("StopLoop", |_a, _s, _t| Reply::null());

    // Maestro
    t.insert("SendMaestroMessage", |_a, _s, _t| {
        Reply::value("On it — keeping within your rails (mock reply).")
    });
    t.insert("CancelMaestroMessage", |_a, _s, _t| Reply::null());
    t.insert("GetMaestroChat", |_a, _s, _t| arr(Vec::new()));

    // Escalation
    t.insert("GetEscalation", |a, s, _t| {
        Reply::Value(
            s.esc_by_loop
                .get(&key_of(a.first()))
                .cloned()
                .unwrap_or_else(|| json!({})),
        )
    });
    t.insert("AnswerEscalation", |a, s, _t| {
        let key = key_of(a.first());
        let Some(existing) = s.esc_by_loop.get(&key).filter(|e| truthy(Some(e))).cloned() else {
            return Reply::Value(json!({}));
        };
        let answer = or(a.get(2), or(a.get(1), &json!(""))).clone();
        let updated = js::spread(&[
            &existing,
            &json!({ "status": "answered", "answer": answer }),
        ]);
        s.esc_by_loop.insert(key, updated.clone());
        Reply::Value(updated)
    });

    // Agents
    t.insert("ListLoopAgents", |a, s, _t| {
        Reply::Value(
            s.agents_by_loop
                .get(&key_of(a.first()))
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
    });
    t.insert("GetAgentTrace", |a, s, _t| {
        let agents = items(s.agents_by_loop.get(&key_of(a.first())));
        Reply::Value(
            agents
                .into_iter()
                .find(|g| get(g, "code") == a.get(1))
                .unwrap_or_else(|| json!({})),
        )
    });

    // PRs
    t.insert("ListPRs", |_a, s, _t| arr(s.prs().cloned().collect()));
    t.insert("SyncGitHubPRs", |_a, s, _t| arr(s.prs().cloned().collect()));
    t.insert("CreateWorkspaceFromPR", |a, s, _t| {
        let id = str_arg(a, 0);
        let Some(pr) = s.pr_by_id(&id).cloned() else { return Reply::Value(json!({})) };
        if let Some(ws_id) = get(&pr, "workspaceId").filter(|w| truthy(Some(w))) {
            return Reply::Value(pr_linked_workspace(s, ws_id));
        }
        let host = s.find_loop(|l| get(l, "hostPrId").and_then(Value::as_str) == Some(id.as_str()));
        let ws = match host {
            Some(h) => js::spread(&[
                s.obj(h),
                &json!({ "hostPrId": "", "type": "solo", "statusLabel": "Idle", "phase": "editing", "worktreePath": "/tmp/from-pr" }),
            ]),
            None => {
                let title = or(get(&pr, "title"), &json!("PR workspace")).clone();
                json!({
                    "id": "WS-FROM-PR",
                    "projectId": or(get(&pr, "projectId"), &json!("storefront")),
                    "type": "solo",
                    "goal": title,
                    "short": title,
                    "statusLabel": "Idle",
                    "phase": "editing",
                    "branch": or(get(&pr, "branch"), &json!("recur/from-pr")),
                    "baseBranch": or(get(&pr, "base"), &json!("main")),
                    "worktreePath": "/tmp/from-pr",
                })
            }
        };
        let ws_id = get(&ws, "id").cloned().unwrap_or(Value::Null);
        if let Some(p) = s.pr_by_id_mut(&id) {
            js::set(p, "workspaceId", ws_id.clone());
        }
        let prs: Vec<_> = s.pr_ids().to_vec();
        let mut next = Vec::with_capacity(prs.len());
        for p in prs {
            if get_str(s.obj(p), "id") == id && get(s.obj(p), "id").is_some_and(Value::is_string) {
                let spread = js::spread(&[s.obj(p), &json!({ "workspaceId": ws_id })]);
                next.push(s.alloc(spread));
            } else {
                next.push(p);
            }
        }
        s.set_prs(next);
        let ws_key = key_of(Some(&ws_id));
        let o = s.alloc(ws.clone());
        s.set_loop_by_id(&ws_key, o);
        if host.is_some() {
            s.replace_loop(&ws_key, o);
        } else {
            let mut loops = vec![o];
            loops.extend_from_slice(s.loop_ids());
            s.set_loops(loops);
        }
        Reply::Value(ws)
    });
    t.insert("EnsurePRChatHost", |a, s, _t| {
        let id = str_arg(a, 0);
        let Some(pr) = s.pr_by_id(&id).cloned() else {
            return Reply::Value(json!({}));
        };
        if let Some(ws_id) = get(&pr, "workspaceId").filter(|w| truthy(Some(w))) {
            return Reply::Value(pr_linked_workspace(s, ws_id));
        }
        if let Some(o) =
            s.find_loop(|l| get(l, "hostPrId").and_then(Value::as_str) == Some(id.as_str()))
        {
            return Reply::Value(s.obj(o).clone());
        }
        let title = or(get(&pr, "title"), &json!("PR chat")).clone();
        let ws = json!({
            "id": format!("WS-HOST-{id}"),
            "projectId": or(get(&pr, "projectId"), &json!("storefront")),
            "type": "solo",
            "hostPrId": id,
            "goal": title,
            "short": title,
            "statusLabel": "Review",
            "phase": "editing",
            "worktreePath": "/tmp/repo",
            "baseBranch": or(get(&pr, "base"), &json!("main")),
        });
        let o = s.alloc(ws.clone());
        s.set_loop_by_id(&format!("WS-HOST-{id}"), o);
        let mut loops = vec![o];
        loops.extend_from_slice(s.loop_ids());
        s.set_loops(loops);
        Reply::Value(ws)
    });
    t.insert("GetPR", |a, s, _t| {
        Reply::Value(
            s.pr_by_id(&key_of(a.first()))
                .cloned()
                .unwrap_or(Value::Null),
        )
    });
    t.insert("GetPRFiles", |a, s, _t| {
        Reply::Value(
            s.pr_by_id(&key_of(a.first()))
                .and_then(|p| get(p, "files"))
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
    });
    t.insert("GetPRCommits", |a, s, _t| {
        Reply::Value(
            s.pr_by_id(&key_of(a.first()))
                .and_then(|p| get(p, "commits"))
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
    });
    t.insert("GetCommitDiff", |a, s, _t| {
        let r = Value::String(str_arg(a, 1));
        let commits = items(
            s.pr_by_id(&key_of(a.first()))
                .and_then(|p| get(p, "commits")),
        );
        Reply::Value(
            commits
                .into_iter()
                .find(|c| get(c, "hash") == Some(&r) || get(c, "sha") == Some(&r))
                .unwrap_or_else(|| json!({})),
        )
    });
    t.insert("AddPRComment", |a, s, _t| add_pr_comment(a, s));
    t.insert("UpdatePRDetails", |a, s, _t| {
        let Some(pr) = s.pr_by_id_mut(&key_of(a.first())) else {
            return Reply::null();
        };
        let title = match a.get(1) {
            Some(v) if !v.is_null() => js::string(v),
            _ => get(pr, "title").map_or("undefined".into(), js::string),
        };
        js::set(pr, "title", Value::String(title));
        let body = str_arg(a, 2);
        let mut comments = items(get(pr, "comments"));
        match comments
            .iter_mut()
            .find(|c| get_str(c, "kind") == "description")
        {
            Some(desc) => js::set(desc, "bodyMd", Value::String(body)),
            None => {
                let author = or(get(pr, "authorLogin"), &json!("You")).clone();
                comments.insert(
                    0,
                    json!({
                        "id": "gh-description", "kind": "description", "author": author,
                        "bodyMd": body, "bodyHtml": "",
                    }),
                );
            }
        }
        js::set(pr, "comments", Value::Array(comments));
        Reply::Value(pr.clone())
    });
    t.insert("UpdatePRComment", |a, s, _t| {
        let Some(pr) = s.pr_by_id_mut(&key_of(a.first())) else {
            return Reply::null();
        };
        let id = Value::String(str_arg(a, 1));
        let body = str_arg(a, 2);
        let mut comments = items(get(pr, "comments"));
        let Some(c) = comments.iter_mut().find(|x| get(x, "id") == Some(&id)) else {
            return Reply::null();
        };
        js::set(c, "bodyMd", Value::String(body.clone()));
        if !truthy(get(c, "path")) {
            js::set(c, "bodyHtml", Value::String(body));
        }
        let out = c.clone();
        js::set(pr, "comments", Value::Array(comments));
        Reply::Value(out)
    });
    t.insert("DeletePRComment", |a, s, _t| {
        let Some(pr) = s.pr_by_id_mut(&key_of(a.first())) else {
            return Reply::null();
        };
        let id = Value::String(str_arg(a, 1));
        let comments: Vec<Value> = items(get(pr, "comments"))
            .into_iter()
            .filter(|c| get(c, "id") != Some(&id))
            .collect();
        js::set(pr, "comments", Value::Array(comments));
        Reply::Value(pr.clone())
    });
    t.insert("ReplyPRComment", |a, s, _t| {
        let Some(pr) = s.pr_by_id_mut(&key_of(a.first())) else {
            return Reply::null();
        };
        let id = Value::String(str_arg(a, 1));
        let body = str_arg(a, 2);
        let mut comments = items(get(pr, "comments"));
        let Some(root) = comments.iter().find(|c| get(c, "id") == Some(&id)).cloned() else {
            return Reply::null();
        };
        let reply = js::spread(&[
            &now_comment(&body),
            &obj(vec![
                ("bodyMd", some(body.clone())),
                ("inReplyToId", field(&root, "id")),
                ("parentId", field(&root, "parentId")),
                ("path", field(&root, "path")),
                ("line", field(&root, "line")),
                (
                    "threadId",
                    or_field(get(&root, "threadId"), get(&root, "id")),
                ),
                ("resolved", some(truthy(get(&root, "resolved")))),
            ]),
        ]);
        comments.push(reply.clone());
        js::set(pr, "comments", Value::Array(comments));
        Reply::Value(reply)
    });
    t.insert("TogglePRCommentReaction", |a, s, _t| toggle_reaction(a, s));
    t.insert("SetPRThreadResolved", |a, s, _t| {
        let Some(pr) = s.pr_by_id_mut(&key_of(a.first())) else {
            return Reply::null();
        };
        let key = Value::String(str_arg(a, 1));
        let resolved = truthy(a.get(2));
        let comments: Vec<Value> = items(get(pr, "comments"))
            .into_iter()
            .map(|c| {
                let hit = get(&c, "threadId") == Some(&key)
                    || get(&c, "id") == Some(&key)
                    || get(&c, "inReplyToId") == Some(&key);
                if !hit {
                    return c;
                }
                let thread = or(get(&c, "threadId"), &key).clone();
                js::spread(&[&c, &json!({ "resolved": resolved, "threadId": thread })])
            })
            .collect();
        js::set(pr, "comments", Value::Array(comments));
        Reply::Value(pr.clone())
    });
    t.insert("SubmitReview", |a, s, _t| {
        let Some(pr) = s.pr_by_id_mut(&key_of(a.first())) else {
            return Reply::null();
        };
        let pending = items(get(pr, "pendingComments"));
        js::set(pr, "pendingComments", json!([]));
        let body = str_arg(a, 2);
        let review = js::spread(&[&now_comment(&body), &json!({ "bodyMd": body })]);
        let review_id = get(&review, "id").cloned().unwrap_or(Value::Null);
        let kids: Vec<Value> = pending
            .iter()
            .map(|pc| {
                let file = js::string(get(pc, "file").unwrap_or(&json!("undefined")));
                let line = js::string(get(pc, "line").unwrap_or(&json!("undefined")));
                js::spread(&[
                    &now_comment(&format!("<code>{file}:{line}</code>")),
                    &obj(vec![
                        (
                            "bodyMd",
                            some(js::string(nullish(get(pc, "body"), &json!("")))),
                        ),
                        ("parentId", Some(review_id.clone())),
                        ("path", field(pc, "file")),
                        ("line", field(pc, "line")),
                        ("side", Some(or(get(pc, "side"), &json!("RIGHT")).clone())),
                    ]),
                ])
            })
            .collect();
        let mut comments = items(get(pr, "comments"));
        comments.push(review);
        comments.extend(kids);
        js::set(pr, "comments", Value::Array(comments));
        Reply::Value(pr.clone())
    });
    t.insert("UpdateWithBase", |a, s, _t| {
        let pr = s.pr_by_id(&key_of(a.first())).cloned();
        let ws = s
            .loop_by_id(&key_of(pr.as_ref().and_then(|p| get(p, "workspaceId"))))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let base = nullish(pr.as_ref().and_then(|p| get(p, "base")), &json!("main")).clone();
        Reply::Value(json!({ "status": "updated", "workspace": ws, "base": base }))
    });
    t.insert("MergePR", |a, s, _t| {
        let pr = s
            .pr_by_id(&key_of(a.first()))
            .cloned()
            .unwrap_or_else(|| json!({}));
        Reply::Value(js::spread(&[
            &pr,
            &json!({ "state": "Merged", "group": "merged" }),
        ]))
    });
    t.insert("SetPRDraft", |a, s, _t| set_pr_draft(a, s));
    t.insert("ReadyPR", |a, s, t| {
        let args = [
            a.first().cloned().unwrap_or(Value::Null),
            Value::Bool(false),
        ];
        t.call("SetPRDraft", &args, s)
    });
    t.insert("SearchPRMentionableUsers", |_a, _s, _t| arr(Vec::new()));

    // Policy
    t.insert("GetPolicy", |_a, s, _t| Reply::Value(s.policy.clone()));
    t.insert("SetPolicy", |a, _s, _t| {
        Reply::Value(a.first().cloned().unwrap_or(Value::Null))
    });

    // Inbox
    t.insert("GetInbox", |_a, s, _t| arr(s.inbox.clone()));
    t.insert("MarkInboxRead", |a, s, _t| {
        let id = str_arg(a, 0);
        let is_id =
            |it: &Value| get_str(it, "id") == id && get(it, "id").is_some_and(Value::is_string);
        if parse_chat_inbox_id(&id).is_some() {
            s.inbox.retain(|it| !is_id(it));
        } else {
            for it in &mut s.inbox {
                if is_id(it) {
                    *it = js::spread(&[it, &json!({ "unread": false })]);
                }
            }
        }
        Reply::value(
            s.inbox
                .iter()
                .filter(|it| truthy(get(it, "unread")))
                .count(),
        )
    });
    t.insert("MarkAllInboxRead", |_a, s, _t| {
        s.inbox = s
            .inbox
            .iter()
            .filter(|it| get_str(it, "kind") != "chat")
            .map(|it| js::spread(&[it, &json!({ "unread": false })]))
            .collect();
        Reply::value(0)
    });

    // Settings
    t.insert("DetectAgent", |_a, _s, _t| {
        Reply::value("Claude Code · dev@example.com (Acme)")
    });
    t.insert("GetClaudeStatus", |_a, _s, _t| {
        Reply::Value(json!({
            "authenticated": true,
            "path": "/usr/local/bin/claude",
            "version": "2.1.224",
            "provider": "Anthropic API",
            "loginMethod": "Claude Team account",
            "organization": "Acme",
            "email": "dev@example.com",
            "error": "",
        }))
    });
    t.insert("GetCodexStatus", |_a, _s, _t| {
        Reply::Value(json!({
            "binInstalled": true,
            "binPath": "/opt/homebrew/bin/codex",
            "authenticated": false,
            "loginMethod": "",
            "email": "",
            "plan": "",
            "error": "Not logged in. Run `codex login`.",
        }))
    });
    t.insert("GetChatGPTStatus", |_a, _s, _t| {
        Reply::Value(json!({
            "authenticated": false,
            "accountId": "",
            "email": "",
            "accounts": [],
            "modelCount": 0,
            "error": "",
        }))
    });
    t.insert("GetCursorStatus", |_a, _s, _t| {
        Reply::Value(cursor_status("1.0.31"))
    });
    t.insert("InstallCursorBridge", |_a, _s, _t| {
        Reply::Value(cursor_status("1.0.30"))
    });
    t.insert("GetSettings", |_a, s, _t| Reply::Value(s.settings.clone()));
    t.insert("GetInstanceLabel", |_a, _s, _t| Reply::value(""));
    t.insert("GitHubLogin", |_a, _s, _t| Reply::value("mycodecrafting"));
    t.insert("SetSettings", |a, s, _t| {
        s.settings = a.first().cloned().unwrap_or(Value::Null);
        Reply::Value(s.settings.clone())
    });
    t.insert("ListModelCatalog", |_a, _s, _t| {
        Reply::Value(model_catalog())
    });
    t.insert("GetResource", |_a, s, _t| Reply::Value(s.resource.clone()));
    t.insert("GetProviderUsage", |_a, _s, _t| Reply::Value(empty_usage()));
    t.insert("RefreshProviderUsage", |_a, _s, _t| {
        Reply::Value(empty_usage())
    });
    t.insert("ConnectCursorUsage", |_a, _s, _t| {
        Reply::Value(json!({ "loginUrl": "", "pending": false, "error": "not in e2e" }))
    });
    t.insert("CancelCursorUsageConnect", |_a, _s, _t| Reply::null());
    t.insert("DisconnectCursorUsage", |_a, _s, _t| Reply::null());
    t.insert("LogsPath", |_a, _s, _t| {
        Reply::value("/tmp/recur-test/logs/recur.log")
    });
    t.insert("RevealLogs", |_a, _s, _t| Reply::null());
    t.insert("ClearAllData", |_a, _s, _t| Reply::null());
    t.insert("ListMonospaceFonts", |_a, _s, _t| {
        Reply::Value(strs(&[
            "JetBrains Mono",
            "IBM Plex Mono",
            "Source Code Pro",
            "Inconsolata",
            "Menlo",
            "Monaco",
            "Fira Code",
        ]))
    });

    // Code observatory: one tree level at a time (CodeService depth=1 + lazy expand).
    t.insert("ListTree", |a, _s, _t| {
        let path = js::string(or(a.get(2), &json!("")));
        match path.as_str() {
            "" => Reply::Value(json!([{ "name": "src", "path": "src", "dir": true }])),
            "src" => {
                Reply::Value(json!([{ "name": "main.go", "path": "src/main.go", "dir": false }]))
            }
            _ => arr(Vec::new()),
        }
    });
    // Batched sibling of ListTree: delegates to whatever ListTree is in effect
    // so a test that overrides the tree shape only overrides one of them.
    t.insert("ListTrees", |a, s, t| {
        let mut out = Map::new();
        for dir in items(a.get(2)) {
            let args = [
                a.first().cloned().unwrap_or(Value::Null),
                a.get(1).cloned().unwrap_or(Value::Null),
                dir.clone(),
            ];
            out.insert(js::string(&dir), t.call("ListTree", &args, s).embedded());
        }
        Reply::Value(Value::Object(out))
    });
    t.insert("ReadFile", |a, _s, _t| {
        Reply::Value(json!({
            "path": or(a.get(2), &json!("src/main.go")),
            "language": "go",
            "content": "package main\n\nfunc main() {}\n",
            "size": 32,
            "writable": truthy(a.get(1)),
        }))
    });
    t.insert("WriteFile", |_a, _s, _t| Reply::null());
    // Editor session store: per-seed so a session survives a restart within
    // one test; mirrors the backend merge (meta replaces wholesale,
    // buffers/scratch upsert by path, explicit drops, empty deletes).
    t.insert("GetEditorSession", |a, s, _t| {
        Reply::Value(
            s.editor_sessions
                .get(&key_of(a.first()))
                .cloned()
                .unwrap_or(Value::Null),
        )
    });
    t.insert("UpdateEditorSession", |a, s, _t| {
        update_editor_session(a, s)
    });
    t.insert("ClearEditorSession", |a, s, _t| {
        s.editor_sessions.remove(&key_of(a.first()));
        Reply::null()
    });
    // Last-open workspace/view; survives a reload within a test.
    t.insert("GetShellState", |_a, s, _t| {
        Reply::Value(
            s.shell_state
                .clone()
                .unwrap_or_else(|| json!({ "view": "" })),
        )
    });
    t.insert("SetShellState", |a, s, _t| {
        s.shell_state = Some(nullish(a.first(), &json!({ "view": "" })).clone());
        Reply::null()
    });
    t.insert("GetExpandedSchedules", |_a, s, _t| {
        arr(s
            .expanded_schedules
            .iter()
            .cloned()
            .map(Value::String)
            .collect())
    });
    t.insert("SetExpandedSchedules", |a, s, _t| {
        s.expanded_schedules = items(a.first()).iter().map(js::string).collect();
        Reply::null()
    });

    // Recur Bundles
    t.insert("ListGrammars", |_a, _s, _t| {
        arr(tm_catalog()
            .grammars
            .iter()
            .map(|(g, _)| g.clone())
            .collect())
    });
    t.insert("ReadGrammar", |a, _s, _t| {
        let scope = str_arg(a, 0);
        match tm_catalog()
            .grammars
            .iter()
            .find(|(g, _)| get_str(g, "scopeName") == scope)
        {
            Some((_, abs)) => read_body(abs, &format!("grammar {scope}")),
            None => Reply::error(format!("grammar {scope} not found")),
        }
    });
    t.insert("ListThemes", |_a, _s, _t| {
        arr(tm_catalog().themes.iter().map(|(t, _)| t.clone()).collect())
    });
    t.insert("ReadTheme", |a, _s, _t| {
        let id = str_arg(a, 0);
        match tm_catalog()
            .themes
            .iter()
            .find(|(t, _)| get_str(t, "id") == id)
        {
            Some((_, abs)) => read_body(abs, &format!("theme {id}")),
            None => Reply::error(format!("theme {id} not found")),
        }
    });
    t.insert("ListTerminalThemes", |_a, _s, _t| {
        arr(terminal_themes().to_vec())
    });
    t.insert("ListSkippedBundles", |_a, _s, _t| arr(Vec::new()));
    t.insert("ExpandGoal", |a, _s, _t| {
        Reply::Value(json!({ "prompt": str_arg(a, 0), "display": "" }))
    });
    t.insert("ListPromptActions", |_a, _s, _t| {
        Reply::Value(prompt_actions())
    });
    t.insert("RunPromptAction", |a, _s, _t| run_prompt_action(a));
    t.insert("RunPromptComplete", |a, _s, _t| {
        let req = a.get(1).filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!({}));
        let term = js::string(nullish(get(&req, "args"), &json!(""))).trim().to_lowercase();
        if a.first().and_then(Value::as_str) == Some("recur.chat.fix-checks") {
            let mut hits = Vec::new();
            if term.is_empty() || "all".contains(&term) {
                hits.push(json!({ "insert": "all", "label": "All failing checks", "detail": "Every failing GitHub check", "badge": "All" }));
            }
            if term.is_empty() || "lint".contains(&term) || term == "lint" {
                hits.push(json!({ "insert": "lint", "label": "lint", "detail": "fail", "badge": "Check" }));
            }
            return Reply::Value(json!({ "hits": hits, "message": "" }));
        }
        Reply::Value(json!({ "hits": [], "message": "" }))
    });
    t.insert("CompiledPromptModule", |a, _s, _t| {
        Reply::value(match a.first().and_then(Value::as_str) {
            Some("recur.chat.fix-findings") => COMPILED_FIX_FINDINGS,
            Some("recur.chat.commit") => COMPILED_COMMIT,
            Some("recur.chat.commit-push") => COMPILED_COMMIT_PUSH,
            _ => COMPILED_PROMPT_DEFAULT,
        })
    });
    t.insert("ListCommands", |_a, _s, _t| Reply::Value(commands()));
    t.insert("ListSnippets", |_a, _s, _t| {
        Reply::Value(json!([
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
        ]))
    });
    t.insert("ListSymbols", |_a, _s, _t| {
        Reply::Value(json!([
            { "id": "recur.symbols.source.function", "scope": "entity.name.function", "show": true, "kind": "func", "bundleName": "Source", "sourceRoot": "default" },
            { "id": "recur.symbols.source.hide-calls", "scope": "meta.function-call entity.name.function", "show": false, "bundleName": "Source", "sourceRoot": "default" },
        ]))
    });
    t.insert("RunCommand", |a, _s, _t| {
        let rc = a.get(1).unwrap_or(&Value::Null);
        let input = js::string(nullish(get(rc, "input"), &json!("")));
        // Mock gofmt: collapse runs of blank lines and strip trailing blanks.
        static BLANKS: std::sync::OnceLock<(Regex, Regex)> = std::sync::OnceLock::new();
        let (blank, trailing) = BLANKS.get_or_init(|| {
            (
                Regex::new(r"\n{3,}").expect("blank-line regex"),
                Regex::new(r"(?m)[ \t]+$").expect("trailing-space regex"),
            )
        });
        let formatted = blank.replace_all(&input, "\n\n");
        let formatted = trailing.replace_all(&formatted, "").into_owned();
        let out = if formatted.ends_with('\n') {
            formatted
        } else {
            formatted + "\n"
        };
        Reply::Value(
            json!({ "stdout": out, "stderr": "", "exitCode": 0, "output": "replaceDocument" }),
        )
    });
    t.insert("CompiledModule", |a, _s, _t| {
        let body = if a.first().and_then(Value::as_str) == Some("recur.text.sort-lines-asc") {
            COMPILED_SORT_LINES
        } else {
            COMPILED_IDENTITY
        };
        Reply::value(format!("{COMPILED_SDK}{body}"))
    });
    t.insert("Search", |a, _s, _t| Reply::Value(json!([search_hit(a)])));
    t.insert("SearchAdvanced", |a, _s, _t| {
        Reply::Value(json!({ "hits": [search_hit(a)], "skippedLarge": 0, "truncated": false }))
    });
    t.insert("ReplaceInFiles", |_a, _s, _t| {
        Reply::Value(json!({ "filesChanged": 1, "replacements": 1 }))
    });
    t.insert("ListFiles", |_a, _s, _t| {
        Reply::Value(strs(&["src/main.go", "README.md"]))
    });
    t.insert("ListTerminals", |_a, _s, _t| arr(Vec::new()));
    t.insert("RestoreWorkspaceTerminals", |_a, _s, _t| arr(Vec::new()));
    t.insert("ListTerminalDockChrome", |_a, _s, _t| arr(Vec::new()));
    t.insert("SetTerminalDockChrome", |_a, _s, _t| Reply::null());
    t.insert("SetTerminalTitle", |_a, _s, _t| Reply::null());
    t.insert("SetActiveTerminal", |_a, _s, _t| Reply::null());
    t.insert("ReorderTerminals", |_a, _s, _t| Reply::null());
    // SearchFiles ranks in the backend in production; here it filters whatever
    // ListFiles is in effect, so a ListFiles override drives @-mention menus.
    t.insert("SearchFiles", search_files);
    t.insert("SetFileIndexExclude", |_a, _s, _t| Reply::null());
    t.insert("ListSkills", |_a, _s, _t| Reply::Value(skills()));
    t.insert("ListAllSkills", |_a, _s, _t| Reply::Value(skills()));
    t.insert("Outline", |_a, _s, _t| {
        Reply::Value(json!([{ "name": "main", "kind": "func", "line": 3 }]))
    });
    t.insert("FindDefs", |a, _s, _t| {
        Reply::Value(json!([obj(vec![
            ("name", a.get(2).cloned()),
            ("kind", some("func")),
            ("path", some("src/main.go")),
            ("line", some(3)),
        ])]))
    });
    t.insert("FindRefs", |_a, _s, _t| {
        Reply::Value(
            json!([{ "path": "src/main.go", "line": 3, "text": "func main() {}", "def": true }]),
        )
    });
    t.insert("OpenInEditor", |_a, _s, _t| Reply::null());
    t.insert("RevealInFinder", |_a, _s, _t| Reply::null());
    t.insert("AbsPath", |a, _s, _t| {
        Reply::value(format!("/mock/{}", js::string(or(a.get(2), &json!("")))))
    });
    for name in ["CreateFile", "Mkdir", "Rename", "Delete", "Copy"] {
        t.insert(name, |_a, _s, _t| Reply::null());
    }
    t.insert("WorkingStatus", |_a, _s, _t| {
        Reply::Value(json!({ "src/main.go": "m" }))
    });
    t.insert("StagedFiles", |_a, _s, _t| arr(Vec::new()));
    t.insert("Stage", |_a, _s, _t| Reply::null());
    t.insert("Unstage", |_a, _s, _t| Reply::null());
    t.insert("Commit", |_a, _s, _t| Reply::Value(commit_result()));
    t.insert("CommitStaged", |_a, _s, _t| Reply::Value(commit_result()));
    t.insert("DiscardFile", |_a, _s, _t| Reply::null());
    t.insert("DiscardAll", |_a, _s, _t| Reply::null());
    t.insert("ListFindings", |_a, _s, _t| arr(Vec::new()));
    t.insert("SetFindingStatus", |a, _s, _t| {
        Reply::Value(json!({
            "id": js::string(or(a.get(1), &json!(""))),
            "workspaceId": js::string(or(a.first(), &json!(""))),
            "status": js::string(or(a.get(2), &json!(""))),
        }))
    });
    t.insert("CreateFinding", |a, _s, _t| {
        Reply::Value(json!({
            "id": "f-user-1",
            "workspaceId": js::string(or(a.first(), &json!(""))),
            "file": js::string(or(a.get(1), &json!(""))),
            "line": js::num(js::number(Some(or(a.get(2), &json!(0))))),
            "message": js::string(or(a.get(3), &json!(""))),
            "endLine": js::num(js::number(Some(or(a.get(4), &json!(0))))),
            "status": "open",
            "severity": "info",
            "source": "user",
            "iteration": 1,
        }))
    });
    t.insert("ListArtifacts", |_a, _s, _t| arr(Vec::new()));
    t.insert("ListPRFindings", |_a, _s, _t| arr(Vec::new()));
    t.insert("ListChangedFiles", |_a, _s, _t| {
        Reply::Value(strs(&["src/main.go"]))
    });
    t.insert("GitStatus", |_a, _s, _t| {
        Reply::Value(json!({ "src/main.go": "m" }))
    });
    t.insert("FileAtBase", |_a, _s, _t| {
        Reply::value("package main\n\nfunc main() {}\n")
    });
    t.insert("FileDiff", |_a, _s, _t| Reply::Value(file_diff()));
    t.insert("DiffVsBase", |_a, s, t| {
        Reply::Value(json!({ "files": [t.call("FileDiff", &[], s).embedded()] }))
    });
    t.insert("DiffWorking", |_a, s, t| {
        Reply::Value(json!({ "files": [t.call("FileDiff", &[], s).embedded()] }))
    });
    t.insert("ListCommits", |_a, _s, _t| Reply::Value(commits()));
    t.insert("CommitDiff", |_a, s, t| {
        Reply::Value(json!([t.call("FileDiff", &[], s).embedded()]))
    });

    t
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
