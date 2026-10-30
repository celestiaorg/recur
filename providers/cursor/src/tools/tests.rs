//! Cursor tool vocabulary tests: the mapping and display cases. Fixtures
//! are VERBATIM args/results from a live bridge. The frame-fold cases live in
//! `turn/tests.rs`; the transcript and session-broadcast cases
//! (`flattenTranscript`, `sessionBroadcast`) belong to the engine's turn
//! pipeline.

use std::sync::Arc;

use recur_agent::Provider;
use recur_agent::display::{
    CHAT_DETAIL_LIMIT, chat_tool_input, chat_tool_result, create_plan_body, display_tool_result,
    is_create_plan_tool, summarize_tool_input, tool_description,
};
use recur_agent::testing::ScriptedProvider;
use serde_json::{Value, json};

use super::*;

pub(crate) const ARGS_SHELL: &str = r#"{"command":"echo hi","timeout":30000}"#;
pub(crate) const ARGS_READ: &str = r#"{"path":"/wt/hello.txt"}"#;
pub(crate) const ARGS_EDIT: &str = r#"{"path":"/wt/hello.txt"}"#;
pub(crate) const ARGS_GLOB: &str = r#"{"globPattern":"*.md","targetDirectory":"/wt"}"#;
pub(crate) const ARGS_GREP: &str = r#"{"offset":0,"path":"/wt","pattern":"bravo"}"#;
pub(crate) const ARGS_DELETE: &str = r#"{"path":"/wt/out.txt"}"#;
pub(crate) const ARGS_PLAN: &str = r#"{"plan":"Summary: touch work.txt.\n\nSteps:\n1. Write the marker\n\nDefinition of done: work.txt exists."}"#;
pub(crate) const ARGS_PLAN_EMPTY: &str = r#"{"plan":""}"#;
pub(crate) const RES_SHELL: &str = r#"{"status":"success","value":{"executionTime":467,"exitCode":0,"signal":"","stderr":"","stdout":"hi\n"}}"#;
pub(crate) const RES_PLAN: &str = r#"{"status":"success","value":{}}"#;
const RES_SHELL_E: &str =
    r#"{"status":"success","value":{"exitCode":2,"stderr":"boom","stdout":"partial\n"}}"#;
pub(crate) const RES_READ: &str = r#"{"status":"success","value":{"content":"alpha\nbravo\ncharlie\n","fileSize":20,"totalLines":4}}"#;
pub(crate) const RES_EDIT: &str = r#"{"status":"success","value":{"diffString":"--- a/hello.txt\n+++ b/hello.txt\n@@ -1,3 +1,3 @@\n alpha\n-bravo\n+BRAVO\n charlie","linesAdded":1,"linesRemoved":1}}"#;
pub(crate) const RES_GLOB: &str = r#"{"status":"success","value":{"clientTruncated":false,"files":["./notes.md","./hello.txt"],"totalFiles":2}}"#;
const RES_GREP: &str = r#"{"status":"success","value":{"workspaceResults":{"/wt":{"output":{"matches":[{"file":"./hello.txt"}],"totalMatches":0},"type":"content"}}}}"#;
pub(crate) const RES_MCP: &str = r#"{"status":"success","value":{"content":[{"text":{"text":"The magic number is 4271."}}],"isError":false}}"#;
const ARGS_TASK: &str = r#"{"description":"Explore provider error surfacing","prompt":"Find how provider errors are surfaced in the chat UI.","subagent_type":"explore"}"#;
const RES_TASK: &str =
    r#"{"status":"success","value":{"text":"Errors land in the tool card banner."}}"#;

fn obj(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("decode {s}: {e}"))
}

#[test]
fn canonical_names() {
    for (input, want) in [
        ("shell", "Bash"),
        ("read", "Read"),
        ("edit", "Edit"),
        ("write", "Write"),
        ("glob", "Glob"),
        ("grep", "Grep"),
        ("delete", "Delete"),
        ("updateTodos", "TodoWrite"),
        ("task", "Task"),
        ("createPlan", "CreatePlan"),
        // A tool Cursor adds later passes through.
        ("somethingNew", "somethingNew"),
    ] {
        assert_eq!(canonical_tool(input), want);
    }
}

#[test]
fn tool_input_is_canonical() {
    for (name, args, want) in [
        ("shell", ARGS_SHELL, json!({"command": "echo hi"})),
        ("read", ARGS_READ, json!({"file_path": "/wt/hello.txt"})),
        ("edit", ARGS_EDIT, json!({"file_path": "/wt/hello.txt"})),
        ("delete", ARGS_DELETE, json!({"file_path": "/wt/out.txt"})),
        ("glob", ARGS_GLOB, json!({"pattern": "*.md", "path": "/wt"})),
        (
            "grep",
            ARGS_GREP,
            json!({"pattern": "bravo", "path": "/wt"}),
        ),
    ] {
        // Cursor plumbing (timeout, offset) stays out of the card.
        assert_eq!(obj(&cursor_tool_input(name, args)), want, "{name}");
    }
}

#[test]
fn input_feeds_claude_summarizer() {
    for (tool, args, want) in [
        ("shell", ARGS_SHELL, "echo hi"),
        ("read", ARGS_READ, "/wt/hello.txt"),
        ("edit", ARGS_EDIT, "/wt/hello.txt"),
        ("delete", ARGS_DELETE, "/wt/out.txt"),
        ("glob", ARGS_GLOB, "*.md"),
    ] {
        let got = summarize_tool_input(canonical_tool(tool), &cursor_tool_input(tool, args));
        assert_eq!(got, want, "{tool}");
    }
}

#[test]
fn tool_input_passes_unknown_through() {
    assert!(cursor_tool_input("brandNew", r#"{"foo":"bar"}"#).contains(r#""foo""#));
}

#[test]
fn tool_result_text() {
    for (tool, result, want) in [
        ("shell", RES_SHELL, "hi\n"),
        ("read", RES_READ, "alpha\nbravo\ncharlie\n"),
        ("glob", RES_GLOB, "./notes.md\n./hello.txt"),
        ("grep", RES_GREP, "./hello.txt"),
        (
            crate::events::TOOL_NAME_MCP,
            RES_MCP,
            "The magic number is 4271.",
        ),
    ] {
        assert_eq!(cursor_tool_result_text(tool, result), want, "{tool}");
    }
}

#[test]
fn shell_result_includes_stderr_and_exit() {
    let got = cursor_tool_result_text("shell", RES_SHELL_E);
    for want in ["partial", "boom", "exit 2"] {
        assert!(got.contains(want), "{got:?} missing {want}");
    }
}

#[test]
fn edit_result_carries_diff() {
    let got = cursor_tool_result_text("edit", RES_EDIT);
    assert!(got.contains("-bravo") && got.contains("+BRAVO"), "{got:?}");
}

#[test]
fn result_empty_inputs() {
    assert_eq!(cursor_tool_result_text("shell", ""), "");
    assert_eq!(cursor_tool_result_text("shell", "not json"), "");
}

#[test]
fn display_renders_common_vocabulary() {
    let d = display_tool("shell", ARGS_SHELL);
    assert_eq!(d.name, "Bash");
    assert!(d.input.contains(r#""command":"echo hi""#), "{}", d.input);
    assert!(!d.input.contains("timeout"));
    assert_eq!(cursor_tool_result_text("shell", RES_SHELL), "hi\n");
}

#[test]
fn display_passes_claude_through() {
    let claude = ScriptedProvider::new(vec![]);
    let input = r#"{"command":"ls","description":"list"}"#;
    let d = claude.display_tool("Bash", input);
    assert_eq!((d.name.as_str(), d.input.as_str()), ("Bash", input));
    assert_eq!(
        display_tool_result(&claude, "Bash", "raw output"),
        "raw output"
    );
}

#[test]
fn mcp_display() {
    let args = r#"{"providerIdentifier":"recur","toolName":"done","args":{"summary":"all done"}}"#;
    let d = display_tool("mcp", args);
    assert_eq!(d.name, "mcp__recur__done");
    assert!(
        d.input.contains(r#""summary""#) && !d.input.contains("providerIdentifier"),
        "{}",
        d.input
    );
    assert_eq!(
        cursor_tool_result_text("mcp", RES_MCP),
        "The magic number is 4271."
    );
    // Unparseable routing keeps the raw call.
    let raw = display_tool("mcp", "not json");
    assert_eq!((raw.name.as_str(), raw.input.as_str()), ("mcp", "not json"));
}

#[test]
fn todo_statuses_normalized() {
    let args = r#"{"todos":[
        {"content":"Define CLI requirements","status":"completed"},
        {"content":"Set up scaffolding","status":"inProgress"},
        {"content":"Implement handlers","status":"pending"}
    ]}"#;
    let got = obj(&cursor_tool_input("updateTodos", args));
    let statuses: Vec<&str> = got["todos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["completed", "in_progress", "pending"]);
    assert_eq!(got["todos"][0]["content"], "Define CLI requirements");
}

#[test]
fn todo_unknown_status_passes_through() {
    assert!(
        cursor_tool_input(
            "updateTodos",
            r#"{"todos":[{"content":"x","status":"blocked"}]}"#
        )
        .contains(r#""blocked""#)
    );
}

#[test]
fn todo_malformed_is_safe() {
    assert!(cursor_tool_input("updateTodos", r#"{"todos":"nope"}"#).contains(r#""nope""#));
}

#[test]
fn summarize_handles_todo_write() {
    let raw = cursor_tool_input(
        "updateTodos",
        r#"{"todos":[{"content":"first","status":"completed"},{"content":"second","status":"inProgress"}]}"#,
    );
    let got = summarize_tool_input(canonical_tool("updateTodos"), &raw);
    assert!(got.starts_with("1/2") && got.contains("second"), "{got}");
    let readable = summarize_tool_input(
        "TodoWrite",
        r#"{"todos":[{"content":"Define requirements","status":"completed"},{"content":"Set up scaffolding","status":"in_progress"},{"content":"Implement handlers","status":"pending"}]}"#,
    );
    assert!(
        readable.starts_with("1/3") && readable.contains("Set up scaffolding"),
        "{readable}"
    );
    assert!(
        !readable.contains('{') && !readable.contains('"'),
        "{readable}"
    );
    assert_eq!(
        summarize_tool_input(
            "TodoWrite",
            r#"{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"pending"}]}"#
        ),
        "1/2"
    );
}

#[test]
fn task_maps_to_canonical() {
    assert_eq!(canonical_tool("task"), "Task");
    let got = obj(&cursor_tool_input("task", ARGS_TASK));
    assert_eq!(got["description"], "Explore provider error surfacing");
    assert_ne!(got["prompt"], "");
    assert_eq!(got["subagent_type"], "explore");
}

#[test]
fn task_result_text() {
    assert_eq!(
        cursor_tool_result_text("task", RES_TASK),
        "Errors land in the tool card banner."
    );
}

#[test]
fn summarize_handles_task() {
    let got = summarize_tool_input(
        canonical_tool("task"),
        &cursor_tool_input("task", ARGS_TASK),
    );
    assert_eq!(got, "explore");
    let raw2 = cursor_tool_input(
        "task",
        r#"{"description":"Check diffs","prompt":"Review the open PR diff for regressions in tool cards."}"#,
    );
    let got2 = summarize_tool_input("Task", &raw2);
    assert!(
        !got2.is_empty() && !got2.contains('{') && !got2.contains("Check diffs"),
        "{got2}"
    );
}

#[test]
fn summarize_handles_task_create_and_update() {
    let raw = r#"{"subject":"Guard theme re-apply (registry + host)","description":"setActiveTheme memoize; applyColorMap no-op on identical maps.","activeForm":"Guarding theme re-apply"}"#;
    assert_eq!(
        summarize_tool_input("TaskCreate", raw),
        "Guard theme re-apply (registry + host)"
    );
    assert_eq!(tool_description("TaskCreate", raw), "");
    assert_eq!(
        summarize_tool_input("TaskUpdate", r#"{"taskId":"1","status":"in_progress"}"#),
        "#1 → in_progress"
    );
}

#[test]
fn create_plan_body_and_mapping() {
    assert!(create_plan_body(ARGS_PLAN).contains("touch work.txt"));
    assert_eq!(create_plan_body(ARGS_PLAN_EMPTY), "");
    assert!(is_create_plan_tool("createPlan") && is_create_plan_tool("CreatePlan"));
    assert!(!is_create_plan_tool("updateTodos"));
    let raw = cursor_tool_input("createPlan", ARGS_PLAN);
    assert!(
        obj(&raw)["plan"].as_str().is_some_and(|p| !p.is_empty()),
        "{raw}"
    );
    assert_eq!(
        cursor_tool_result_text("createPlan", RES_PLAN),
        "",
        "createPlan result is an empty ack"
    );
    let sum = summarize_tool_input(canonical_tool("createPlan"), &raw);
    assert!(sum.contains("Summary:") && !sum.contains('\n'), "{sum}");
}

#[test]
fn summarize_recur_and_web_tools() {
    for (tool, input, want) in [
        (
            "mcp__recur__submit_plan",
            r#"{"summary":"Polish plan tool cards","steps":["Add helpers","Render checklist"]}"#,
            "Polish plan tool cards",
        ),
        (
            "mcp__recur__update_plan_step",
            r#"{"step_id":"S2","status":"done","note":"wired"}"#,
            "S2 → done · wired",
        ),
        (
            "mcp__recur__update_plan_step",
            r#"{"step_id":"S1","status":"doing"}"#,
            "S1 → doing",
        ),
        (
            "mcp__recur__edit_plan",
            r#"{"plan":"Add teal paddles.\nKeep scores white."}"#,
            "Add teal paddles.",
        ),
        (
            "mcp__recur__ask_user",
            r#"{"question":"Which color?","context":"follow-up","options":[{"text":"Blue"}]}"#,
            "Which color?",
        ),
        ("mcp__recur__switch_mode", r#"{"mode":"plan"}"#, "plan"),
        (
            "mcp__recur__open_pr",
            r###"{"title":"Align screening allow cache TTL to 24 hours","body":"## Summary\nSet ALLOW_TTL.","draft":true}"###,
            "draft · Align screening allow cache TTL to 24 hours",
        ),
        (
            "WebFetch",
            r#"{"url":"https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token","prompt":"List the request body fields. Is clientIp required?"}"#,
            "https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token",
        ),
        (
            "WebSearch",
            r#"{"query":"onramp session token clientIp"}"#,
            "onramp session token clientIp",
        ),
        (
            "WebSearch",
            r#"{"search_term":"partnerUserRef"}"#,
            "partnerUserRef",
        ),
        (
            "mcp__linear__list_projects",
            r#"{"query":"Apollo"}"#,
            "Apollo",
        ),
        (
            "mcp__linear__list_issue_labels",
            r#"{"team":"ACME"}"#,
            "ACME",
        ),
        ("mcp__plugin_linear_linear__list_teams", "{}", ""),
    ] {
        assert_eq!(summarize_tool_input(tool, input), want, "{tool} {input}");
    }
}

#[test]
fn chat_input_keeps_create_plan_and_task_parseable() {
    let body = "plan step: with detail. ".repeat(900);
    let raw = serde_json::to_string(&json!({ "plan": body })).unwrap();
    assert!(raw.len() > CHAT_DETAIL_LIMIT);
    let got = chat_tool_input("CreatePlan", &raw);
    let v: Value = serde_json::from_str(&got).expect("parseable CreatePlan input");
    assert!(v["plan"].as_str().unwrap().contains("plan step"));
    assert_eq!(chat_tool_input("Bash", ARGS_SHELL), ARGS_SHELL);

    let prompt = "Search the Svelte frontend for PR detail. ".repeat(600);
    let raw = serde_json::to_string(&json!({
        "description": "Map PR detail UI structure", "subagent_type": "Explore", "prompt": prompt
    }))
    .unwrap();
    assert!(raw.len() > CHAT_DETAIL_LIMIT);
    let v: Value =
        serde_json::from_str(&chat_tool_input("Task", &raw)).expect("parseable Task input");
    assert_eq!(v["description"], "Map PR detail UI structure");
    assert_eq!(v["subagent_type"], "Explore");
    assert!(v["prompt"].as_str().unwrap().contains("Svelte frontend"));
}

#[test]
fn display_result_hides_subagent_launch_meta() {
    let claude: Arc<dyn Provider> = Arc::new(ScriptedProvider::new(vec![]));
    let ack = "Async agent launched successfully.\n(This tool result is internal metadata – never quote or paste any part of it, including the agentId below, into a user-facing reply.)\n\nagentId: abc\n";
    assert_eq!(display_tool_result(claude.as_ref(), "Task", ack), "");
    assert_eq!(
        display_tool_result(claude.as_ref(), "Task", "tabs live in PRDetail.svelte"),
        "tabs live in PRDetail.svelte"
    );
    let mentioned = "isSubagentLaunchMeta reports Async agent launched successfully plus agentId and internal metadata";
    assert_eq!(
        display_tool_result(claude.as_ref(), "Read", mentioned),
        mentioned
    );
    assert_eq!(chat_tool_result("Read", mentioned), mentioned);
}

fn findings_json(n: usize, extra: Value) -> String {
    let rows: Vec<Value> = (0..n)
        .map(|i| {
            let mut r = json!({
                "id": format!("F-{i:02}"), "file": "docs/bundles.md", "line": 294 + i,
                "severity": "high", "message": "Deadlock between qa, verify, and rail #1. ".repeat(40),
                "status": "open",
            });
            if let Value::Object(e) = &extra {
                for (k, v) in e {
                    r[k] = v.clone();
                }
            }
            r
        })
        .collect();
    serde_json::to_string(&json!({"status": "all", "count": n, "findings": rows})).unwrap()
}

#[test]
fn chat_result_keeps_list_findings_parseable() {
    let raw = findings_json(15, Value::Null);
    assert!(raw.len() > CHAT_DETAIL_LIMIT);
    let got = chat_tool_result("mcp__recur__list_findings", &raw);
    assert!(got.len() <= CHAT_DETAIL_LIMIT);
    let v: Value = serde_json::from_str(&got).expect("parseable findings");
    assert_eq!(
        (v["status"].as_str(), v["count"].as_i64()),
        (Some("all"), Some(15))
    );
    let first = &v["findings"][0];
    assert!(
        first["id"].as_str().is_some_and(|s| !s.is_empty())
            && first["file"].as_str().is_some_and(|s| !s.is_empty())
    );
    assert_eq!(chat_tool_result("Bash", "ok"), "ok");
}

#[test]
fn chat_result_compaction_rejects_non_envelopes() {
    // Oversized valid JSON that is not the tool's shape keeps the byte bound
    // instead of becoming an empty envelope.
    let blob = format!(r#"{{"error":"{}"}}"#, "x".repeat(CHAT_DETAIL_LIMIT));
    for tool in [
        "mcp__recur__list_findings",
        "mcp__recur__list_artifacts",
        "mcp__recur__list_workspace_briefs",
    ] {
        let got = chat_tool_result(tool, &blob);
        assert!(
            !got.contains(r#""findings""#)
                && !got.contains(r#""artifacts""#)
                && !got.contains(r#""workspaces""#),
            "{tool}"
        );
        assert!(got.len() <= CHAT_DETAIL_LIMIT + "…".len(), "{tool}");
    }
    assert!(
        chat_tool_result(
            "mcp__recur__list_findings",
            r#"{"status":"open","count":0,"findings":[]}"#
        )
        .contains(r#""findings""#)
    );
    assert!(
        chat_tool_result(
            "mcp__recur__list_artifacts",
            r#"{"count":0,"artifacts":[]}"#
        )
        .contains(r#""artifacts""#)
    );
    assert!(
        chat_tool_result(
            "mcp__recur__list_workspace_briefs",
            r#"{"counts":{"needs":0,"working":0,"pickup":0},"workspaces":[]}"#
        )
        .contains(r#""workspaces""#)
    );
}

#[test]
fn chat_result_keeps_list_artifacts_parseable() {
    let rows: Vec<Value> = (0..80)
        .map(|i| {
            let id = format!("aaaaaaaa-bbbb-4ccc-8ddd-{i:012}");
            json!({
                "id": id, "workspaceId": "recur.bubbly-parfait",
                "title": format!("Modes as bundle items {i:02}: long agent title"), "kind": "html",
                "path": format!("workspaces/recur.bubbly-parfait/artifacts/{id}.html"),
                "createdAt": "2026-08-20T04:32:46Z", "createdBy": "PA",
            })
        })
        .collect();
    let raw = serde_json::to_string(&json!({"count": 80, "artifacts": rows})).unwrap();
    assert!(raw.len() > CHAT_DETAIL_LIMIT);
    let got = chat_tool_result("mcp__recur__list_artifacts", &raw);
    assert!(got.len() <= CHAT_DETAIL_LIMIT);
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["count"], 80);
    assert!(
        v["artifacts"][0]["id"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        v["artifacts"][0]["path"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
}

#[test]
fn compact_list_artifacts_keeps_row_when_title_is_huge() {
    let title = "Modes as bundle items: ".repeat(900);
    let raw = serde_json::to_string(&json!({"count": 1, "artifacts": [{
        "id": "aaaaaaaa-bbbb-4ccc-8ddd-000000000001", "title": title, "kind": "html",
        "path": "workspaces/recur.bubbly-parfait/artifacts/aaaaaaaa-bbbb-4ccc-8ddd-000000000001.html",
    }]}))
    .unwrap();
    assert!(raw.len() > CHAT_DETAIL_LIMIT);
    let got = chat_tool_result("mcp__recur__list_artifacts", &raw);
    assert!(got.len() <= CHAT_DETAIL_LIMIT);
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["count"], 1);
    let rows = v["artifacts"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let t = rows[0]["title"].as_str().unwrap();
    assert!(!t.is_empty() && t.len() < title.len());
}

#[test]
fn chat_result_keeps_briefs_and_context_parseable() {
    let briefs: Vec<Value> = (0..40)
        .map(|i| {
            json!({
                "workspaceId": format!("recur.workspace-{i:02}"), "projectName": "recur",
                "title": format!("Account linking architecture {i:02}"),
                "branch": "mycodecrafting/multi-account-linking", "bucket": "pickup",
                "reasons": ["unfinished_plan", "recent_chat"], "now": "Plan is ready but not approved",
                "next": "Review and approve the plan",
                "planSummary": "Identity per provider, not per account. ".repeat(80),
                "primary": {"label": "Review plan", "target": format!("workspace:recur.workspace-{i:02}")},
            })
        })
        .collect();
    let raw = serde_json::to_string(
        &json!({"counts": {"needs": 3, "working": 0, "pickup": 40}, "workspaces": briefs}),
    )
    .unwrap();
    assert!(raw.len() > CHAT_DETAIL_LIMIT);
    let got = chat_tool_result("mcp__recur__list_workspace_briefs", &raw);
    assert!(got.len() <= CHAT_DETAIL_LIMIT);
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["counts"]["pickup"], 40);
    assert!(
        v["workspaces"][0]["workspaceId"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        chat_tool_result(
            "mcp__recur__list_workspace_briefs",
            r#"{"counts":{"needs":0,"working":0,"pickup":0},"workspaces":[]}"#
        )
        .contains(r#""workspaces""#)
    );

    let ctx = serde_json::to_string(&json!({
        "brief": {
            "workspaceId": "recur.zesty-quiche", "title": "Account linking architecture", "bucket": "needs",
            "now": "Plan is ready but not approved", "next": "Review and approve the plan",
            "planSummary": "Identity per provider. ".repeat(900),
            "primary": {"label": "Review plan", "target": "workspace:recur.zesty-quiche"},
        },
        "conversation": [{"role": "assistant", "text": "plan step with detail. ".repeat(400)}],
    }))
    .unwrap();
    assert!(ctx.len() > CHAT_DETAIL_LIMIT);
    let got = chat_tool_result("mcp__recur__get_workspace_context", &ctx);
    assert!(got.len() <= CHAT_DETAIL_LIMIT);
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["brief"]["workspaceId"], "recur.zesty-quiche");
}

#[test]
fn compact_list_findings_shrink_is_monotonic_and_keeps_notes() {
    let raw = findings_json(40, json!({"summary": "summary word ".repeat(200)}));
    let got = chat_tool_result("mcp__recur__list_findings", &raw);
    assert!(got.len() <= CHAT_DETAIL_LIMIT);
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(v["count"], 40);
    let rows = v["findings"].as_array().unwrap();
    assert!(rows.len() >= 20, "kept {} rows", rows.len());
    for f in rows {
        assert!(f["message"].as_str().unwrap_or("").len() <= 800 + "…".len());
        assert!(f["summary"].as_str().unwrap_or("").len() <= 800 + "…".len());
    }

    let raw = findings_json(
        15,
        json!({"notes": [{"id": "n1", "author": "human", "body": "please fix the rail"}]}),
    );
    let v: Value =
        serde_json::from_str(&chat_tool_result("mcp__recur__list_findings", &raw)).unwrap();
    assert_eq!(v["findings"][0]["notes"][0]["body"], "please fix the rail");
}
