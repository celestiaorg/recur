//! Every app event reads the payload its emitter sent, and writes it back
//! unchanged.

use pretty_assertions::assert_eq;
use recur_services::events::{AppEvent, EventHub, WorkspaceRef};
use serde_json::{Value, json};

/// `(name, data)` as published.
fn wire() -> Vec<(&'static str, Value)> {
    let s = |extra: Value| {
        let mut base = json!({ "workspaceId": "W", "code": "PA", "ownerCode": "MA" });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        base
    };
    vec![
        (
            "agent.step",
            json!({ "workspaceId": "W", "agentId": "W-CO", "step": { "kind": "edit", "text": "a.go" } }),
        ),
        (
            "agent.updated",
            json!({ "workspaceId": "W", "agentId": "W-PA", "status": "idle" }),
        ),
        (
            "agent.updated",
            json!({ "workspaceId": "", "agentId": "app-APP", "status": "working", "surface": "app", "code": "APP" }),
        ),
        ("app.focused", json!({})),
        (
            "app.open",
            json!({ "path": "/r", "projectId": "p", "workspaceId": "MAIN-p", "created": true }),
        ),
        ("app.ready", Value::Null),
        (
            "artifact.created",
            json!({ "workspaceId": "W", "artifact": { "id": "a1", "title": "T", "kind": "markdown" } }),
        ),
        (
            "artifact.presented",
            json!({ "workspaceId": "W", "artifact": { "id": "a1", "title": "T", "kind": "markdown" } }),
        ),
        (
            "attention.updated",
            json!({ "unseen": 1, "inboxUnread": 2, "dock": 3, "byWorkspace": { "W": 1 }, "byProject": null, "appUnseen": 1, "workspaceId": "W", "code": "PA", "tabUnseen": true }),
        ),
        (
            "attention.updated",
            json!({ "unseen": 0, "inboxUnread": 0, "dock": 0, "byWorkspace": {}, "byProject": {}, "workspaceId": "", "code": "", "tabUnseen": false, "clearAllUnseen": true }),
        ),
        ("briefing.updated", json!({ "workspaceId": "W" })),
        ("briefing.updated", json!({})),
        (
            "chat.mode_changed",
            json!({ "workspaceId": "W", "agentCode": "PA", "mode": "plan", "from": "agent" }),
        ),
        (
            "cursor.install",
            json!({ "phase": "download", "message": "Downloading", "current": 5, "total": 10, "percent": 50 }),
        ),
        (
            "cursor.install",
            json!({ "phase": "error", "message": "boom", "percent": -1 }),
        ),
        (
            "escalation.raised",
            json!({ "workspaceId": "W", "escalation": { "id": "e", "workspaceId": "W", "question": "q?", "context": "", "options": null, "answer": "", "status": "open", "applied": false, "askedAgo": "" } }),
        ),
        (
            "escalation.resolved",
            json!({ "workspaceId": "W", "answer": "yes" }),
        ),
        (
            "escalation.resolved",
            json!({ "workspaceId": "W", "answer": "yes", "agentCode": "PA2", "open": false }),
        ),
        (
            "files.dropped",
            json!({ "files": ["/a.png"], "attributes": { "data-chat-drop": "W:PA" }, "id": "drop", "classList": ["x"] }),
        ),
        ("findings.updated", json!({ "workspaceId": "W" })),
        (
            "github.branch_rules",
            json!({ "projectId": "p", "repo": "o/r", "remote": "origin", "branch": "main", "requirePR": true }),
        ),
        ("inbox.updated", json!({ "unread": 3 })),
        ("lsp.exit", json!({ "id": "l1" })),
        (
            "lsp.message",
            json!({ "id": "l1", "message": "{\"jsonrpc\":\"2.0\"}" }),
        ),
        ("menu.action", json!("palette")),
        ("menu.state", json!({ "newTab": true, "save": false })),
        (
            "menu.themeState",
            json!({ "chromeDark": true, "appSelected": "dark", "editorSelected": "recur", "terminalSelected": "match-app" }),
        ),
        (
            "menu.themes",
            json!({ "app": [{ "key": "dark", "name": "Dark" }], "editor": [{ "key": "recur", "name": "Charcoal", "kind": "dark" }], "terminal": [{ "key": "match-app", "name": "Match App" }], "appSelected": "dark", "editorSelected": "recur", "terminalSelected": "match-app" }),
        ),
        ("notification.open", json!("workspace:W")),
        (
            "notify",
            json!({ "title": "A loop needs you", "body": "q?", "target": "workspace:W" }),
        ),
        (
            "notify",
            json!({ "title": "Git is too old", "body": "upgrade" }),
        ),
        (
            "permission.requested",
            json!({ "id": "p1", "workspaceId": "W", "tool": "Bash", "summary": "ls", "surface": "loop" }),
        ),
        (
            "permission.resolved",
            json!({ "id": "p1", "allow": true, "reason": "" }),
        ),
        (
            "pr.comment",
            json!({ "prId": "7", "comment": { "id": "c", "author": "You", "bodyHtml": "hi" } }),
        ),
        ("pr.removed", json!({ "prId": "7" })),
        (
            "pr.updated",
            json!({ "prId": "7", "state": "Open", "group": "review", "checksOk": true, "hasChecks": true, "fileCount": 2, "title": "T", "number": 7, "workspaceId": "W", "projectId": "p", "source": "gh", "branch": "b", "base": "main", "when": "now", "diff": { "add": 1, "del": 2 }, "behindBase": 0, "updatingBase": "", "pendingComments": null, "draft": false, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "reviewDecision": "", "reviewRequested": false, "attentionReasons": null, "participants": null, "reviewers": null, "allowedMergeMethods": ["merge"], "pendingOnly": true }),
        ),
        (
            "provider.usage",
            json!({ "fetchedAt": 1, "stale": false, "providers": [{ "id": "cursor", "name": "Cursor", "sourceId": "cursor_connect" }] }),
        ),
        ("prs.synced", json!({ "projectId": "p", "prs": null })),
        ("question.answered", json!({ "id": "q1", "answer": {} })),
        (
            "question.asked",
            json!({ "workspaceId": "W", "question": { "id": "q1", "workspaceId": "W", "question": "Which?" } }),
        ),
        (
            "resource.updated",
            json!({ "loopsRunning": 1, "needsYou": 0, "itersToday": 4, "gitWarning": "old git" }),
        ),
        (
            "session.compact",
            s(json!({ "status": "done", "trigger": "auto", "summary": "sum", "preTokens": 9 })),
        ),
        (
            "session.context_usage",
            s(
                json!({ "inputTokens": 1, "cacheCreationTokens": 2, "cacheReadTokens": 3, "contextWindow": 200000 }),
            ),
        ),
        (
            "session.delta",
            s(json!({ "text": "Hel", "seq": 1, "replace": true })),
        ),
        (
            "session.delta",
            s(json!({ "text": "Hello", "done": true, "uuid": "u1" })),
        ),
        (
            "session.delta",
            json!({ "surface": "app", "code": "APP", "text": "x", "seq": 2, "replace": true, "parentId": "toolu_1" }),
        ),
        (
            "session.fallback",
            s(
                json!({ "from": "a", "to": "b", "fromLabel": "A", "toLabel": "B", "retracted": ["u1"] }),
            ),
        ),
        ("session.notice", s(json!({ "text": "draining" }))),
        (
            "session.step",
            s(
                json!({ "id": "t1", "tool": "Read", "toolInput": "a.go", "description": "Read a.go", "input": "{}", "parentId": "", "uuid": "u2" }),
            ),
        ),
        ("session.thinking", s(json!({ "tokens": 40 }))),
        (
            "session.thinking",
            s(
                json!({ "text": "hmm", "seq": 3, "replace": true, "done": true, "parentId": "toolu_1" }),
            ),
        ),
        (
            "session.tool_input",
            json!({ "workspaceId": "W", "code": "PA", "id": "t1", "input": "{\"a\":1}", "toolInput": "a=1" }),
        ),
        (
            "session.tool_result",
            s(
                json!({ "id": "t1", "result": "ok", "isError": false, "input": "{}", "toolInput": "x" }),
            ),
        ),
        ("session.turn_end", s(json!({ "durationMs": 1200 }))),
        (
            "session.turn_end",
            s(
                json!({ "durationMs": 1200, "inputTokens": 1, "cacheCreationTokens": 2, "cacheReadTokens": 3, "contextWindow": 4 }),
            ),
        ),
        ("session.user", s(json!({ "text": "steer" }))),
        ("system.theme", json!(true)),
        ("term.busy", json!({ "id": "t", "workspaceId": "W" })),
        ("term.data", json!({ "id": "t", "data": "aGk=" })),
        (
            "term.exit",
            json!({ "id": "t", "workspaceId": "W", "exitCode": 0 }),
        ),
        ("term.idle", json!({ "id": "t", "workspaceId": "W" })),
        ("updater.check", Value::Null),
        (
            "updater:error",
            json!({ "stage": "check", "message": "offline" }),
        ),
        ("updater:no-update", Value::Null),
        (
            "updater:update-available",
            json!({ "version": "1.4.0", "publishedAt": "2026-10-01T12:00:00Z", "artifact": { "filename": "Recur.zip", "size": 10 } }),
        ),
        (
            "updater:update-ready",
            json!({ "version": "1.4.0", "publishedAt": "2026-10-01T12:00:00Z", "artifact": { "filename": "Recur.zip", "size": 10 } }),
        ),
        ("updater:user:restart", Value::Null),
        ("window.titlebarDblclick", Value::Null),
        (
            "workspace.chat_tab",
            json!({ "workspaceId": "W", "code": "PA2", "title": "Fix it" }),
        ),
        ("workspace.deleted", json!({ "workspaceId": "W" })),
        (
            "workspace.event",
            json!({ "workspaceId": "W", "event": { "id": "ev-1", "text": "edited" } }),
        ),
        (
            "workspace.file_index",
            json!({ "workspaceId": "W", "gen": 4, "ready": true, "building": false }),
        ),
        (
            "workspace.fs_changed",
            json!({ "workspaceId": "W", "rescan": false, "paths": ["a.go"] }),
        ),
        ("workspace.fs_changed", json!({ "workspaceId": "W" })),
        ("workspace.removed", json!({ "workspaceId": "W" })),
        (
            "workspace.updated",
            json!({ "workspaceId": "W", "state": "coding", "phase": "editing", "iteration": 2, "convergence": null, "archived": false, "group": "active", "statusLabel": "Working", "progress": 0.5, "goal": "g", "short": "s", "plan": null, "definitionOfDone": "", "branch": "b", "baseBranch": "main", "diff": { "add": 1, "del": 0 }, "behindBase": 0, "worktreePath": "/w", "port": 0, "lastError": "", "hostPrId": "", "loopChatCode": "MA", "scheduleId": "" }),
        ),
        (
            "workspace.updated",
            json!({ "workspaceId": "W", "fromWorkspaceId": "H", "codeRemap": { "PA": "PA2" }, "movedCodes": ["PA2"], "chatsChanged": true, "focusChatCode": "PA2" }),
        ),
    ]
}

/// A `null` field reads as absent / empty in the typed payload, so compare
/// with those dropped.
fn without_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(without_nulls).collect()),
        v => v,
    }
}

#[test]
fn every_event_round_trips() {
    for (name, data) in wire() {
        let ev = AppEvent::from_wire(name, data.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(ev.name(), name);
        let back = ev.payload();
        let expect = without_nulls(data.clone());
        let got = without_nulls(back.clone());
        // Typed payloads fill fields always sent (domain types write their
        // non-omitempty zero values); every field sent must come back.
        if let (Value::Object(want), Value::Object(have)) = (&expect, &got) {
            for (k, v) in want {
                let h = have.get(k).unwrap_or_else(|| panic!("{name}: lost {k}"));
                if v.is_object() || v.is_array() {
                    assert_contains(name, k, v, h);
                } else {
                    assert_eq!(h, v, "{name}.{k}");
                }
            }
        } else {
            assert_eq!(got, expect, "{name}");
        }
        assert_eq!(
            AppEvent::from_wire(name, back).unwrap(),
            ev,
            "{name}: payload re-reads"
        );
    }
}

fn assert_contains(name: &str, key: &str, want: &Value, have: &Value) {
    match (want, have) {
        (Value::Object(w), Value::Object(h)) => {
            for (k, v) in w {
                let hv = h.get(k).unwrap_or_else(|| panic!("{name}.{key}: lost {k}"));
                assert_contains(name, k, v, hv);
            }
        }
        (Value::Array(w), Value::Array(h)) => {
            assert_eq!(w.len(), h.len(), "{name}.{key}");
            for (w, h) in w.iter().zip(h) {
                assert_contains(name, key, w, h);
            }
        }
        _ => assert_eq!(have, want, "{name}.{key}"),
    }
}

#[test]
fn partial_and_present_fields() {
    let AppEvent::PrUpdated(p) = AppEvent::from_wire(
        "pr.updated",
        json!({ "prId": "7", "pendingComments": null }),
    )
    .unwrap() else {
        unreachable!()
    };
    assert_eq!(
        p.pending_comments,
        Some(vec![]),
        "present null clears the drafts"
    );
    assert_eq!(p.draft, None, "absent leaves the draft flag alone");
    assert_eq!(p.state, None);

    let AppEvent::WorkspaceUpdated(u) = AppEvent::from_wire(
        "workspace.updated",
        json!({ "workspaceId": "W", "worktreePath": "" }),
    )
    .unwrap() else {
        unreachable!()
    };
    assert_eq!(
        u.worktree_path.as_deref(),
        Some(""),
        "present empty is applied"
    );
    assert_eq!(u.phase, None);
    assert_eq!(
        AppEvent::from_wire("menu.action", json!({ "action": "settings" })).unwrap(),
        AppEvent::MenuAction("settings".into())
    );
    assert_eq!(
        AppEvent::from_wire("inbox.updated", Value::Null)
            .unwrap()
            .payload(),
        json!({})
    );
    assert!(AppEvent::from_wire("loop.event", Value::Null).is_err());
    assert!(AppEvent::from_wire("updater:bogus", Value::Null).is_err());
}

#[test]
fn hub_fans_out_in_order_and_prunes_closed_receivers() {
    let hub = EventHub::new();
    let mut a = hub.subscribe();
    let b = hub.subscribe();
    drop(b);
    for i in 0..3 {
        hub.publish(AppEvent::WorkspaceDeleted(WorkspaceRef {
            workspace_id: i.to_string(),
        }));
    }
    let got: Vec<String> = std::iter::from_fn(|| a.try_recv().ok())
        .map(|e| match e {
            AppEvent::WorkspaceDeleted(r) => r.workspace_id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(got, ["0", "1", "2"]);
    let mut late = hub.subscribe();
    assert!(
        late.try_recv().is_err(),
        "a new subscriber sees only later events"
    );
}
