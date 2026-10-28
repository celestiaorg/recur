use super::*;

#[test]
fn truncate_is_rune_safe() {
    assert_eq!(truncate("hello", 5), "hello");
    assert_eq!(truncate("hello", 4), "hel…");
    assert_eq!(truncate("a—b", 3), "a…");
    assert_eq!(truncate("ab", 0), "…");
    assert_eq!(hard_truncate_runes("a—bc", 2), "a—");
    assert_eq!(hard_truncate_runes("ab", 5), "ab");
    assert_eq!(hard_truncate_runes("ab", 0), "");
}

#[test]
fn summaries_pull_the_salient_field() {
    for (tool, input, want) in [
        (
            "Edit",
            r#"{"file_path":"src/app.go","old_string":"a"}"#,
            "src/app.go",
        ),
        ("Bash", r#"{"command":"go test ./..."}"#, "go test ./..."),
        ("Glob", r#"{"glob_pattern":"*.rs"}"#, "*.rs"),
        (
            "Grep",
            r#"{"pattern":"PRDetail","path":"src"}"#,
            r#""PRDetail" in src"#,
        ),
        ("Grep", r#"{"pattern":"a\"b\tc"}"#, r#""a\"b\tc""#),
        ("WebSearch", r#"{"search_term":"q"}"#, "q"),
        ("Skill", r#"{"skill":"pdf","args":"x"}"#, "pdf x"),
        (
            "Task",
            r#"{"description":"d","subagent_type":"Explore","prompt":"p"}"#,
            "Explore",
        ),
        ("SendMessage", r#"{"to":"a1","prompt":"go on"}"#, "go on"),
        (
            "TaskUpdate",
            r##"{"taskId":"#3","status":"completed","subject":"S"}"##,
            "#3 → completed · S",
        ),
        ("TaskGet", r#"{"task_id":"4"}"#, "#4"),
        ("TaskList", "{}", ""),
        (
            "CreatePlan",
            r#"{"plan":"  First line\nsecond"}"#,
            "First line",
        ),
        (
            "TodoWrite",
            r#"{"todos":[{"content":"a","status":"completed"},{"content":"b","status":"in_progress"},{"content":"c","status":"pending"}]}"#,
            "1/3 · b",
        ),
        (
            "mcp__recur__done",
            r#"{"summary":"Created a game"}"#,
            "Created a game",
        ),
        (
            "mcp__recur__update_plan_step",
            r#"{"step_id":"S2","status":"done","note":"n"}"#,
            "S2 → done · n",
        ),
        (
            "mcp__recur__report_finding",
            r#"{"severity":"high","file":"a.go","line":3,"end_line":5,"message":"bug"}"#,
            "high · a.go:3-5 · bug",
        ),
        (
            "mcp__recur__report_finding",
            r#"{"severity":"info","message":"m"}"#,
            "m",
        ),
        ("mcp__recur__list_findings", "{}", "open"),
        (
            "mcp__recur__list_findings",
            r#"{"status":"resolved"}"#,
            "resolved",
        ),
        ("mcp__recur__read_terminal", "{}", "terminal"),
        ("mcp__recur__activate_skill", r#"{"name":"pdf"}"#, "/pdf"),
        ("mcp__recur__update_finding", r#"{"finding_id":"F1"}"#, "F1"),
        (
            "mcp__recur__open_pr",
            r#"{"title":"T","draft":true}"#,
            "draft · T",
        ),
        ("mcp__recur__attach_evidence", r#"{"summary":"s"}"#, "s"),
        ("mcp__linear__get_issue", r#"{"id":"ENG-1"}"#, "ENG-1"),
        ("mcp__MyLinearCo__x", r#"{"other":1}"#, ""),
        ("Unknown", r#"  {"a":1}  "#, r#"{"a":1}"#),
        ("Read", "not json", "not json"),
        ("Read", "null", "null"),
        ("Read", "", ""),
    ] {
        assert_eq!(summarize_tool_input(tool, input), want, "{tool} {input}");
    }
}

#[test]
fn chat_input_keeps_create_plan_parseable() {
    let body = "x".repeat(PLAN_MAX_PERSIST_CHARS + 10);
    let input = format!(r#"{{"plan":"{body}","other":1}}"#);
    let got = chat_tool_input("CreatePlan", &input);
    let v: Value = serde_json::from_str(&got).unwrap();
    assert_eq!(
        v["plan"].as_str().unwrap().len(),
        PLAN_MAX_PERSIST_CHARS - 1 + '…'.len_utf8()
    );
    assert_eq!(
        chat_tool_input("CreatePlan", r#"{"plan":"<b>"}"#),
        r#"{"plan":"<b>"}"#
    );
    assert_eq!(
        chat_tool_input("CreatePlan", r#"{"plan":""}"#),
        r#"{"plan":""}"#
    );
    assert_eq!(chat_tool_input("Read", ""), "");
}

#[test]
fn chat_input_keeps_task_envelope() {
    let prompt = "p".repeat(CHAT_DETAIL_LIMIT);
    let input =
        format!(r#"{{"subagent_type":"Explore","prompt":"{prompt}","n":2.0,"description":"d"}}"#);
    let got = chat_tool_input("Agent", &input);
    assert_eq!(
        got,
        format!(r#"{{"description":"d","n":2,"prompt":"{prompt}","subagent_type":"Explore"}}"#)
    );
    let small = r#"{"prompt": "hi"}"#;
    assert_eq!(chat_tool_input("Task", small), small);
    let long = "y".repeat(CHAT_DETAIL_LIMIT + 5);
    assert_eq!(
        chat_tool_input("Read", &long),
        truncate(&long, CHAT_DETAIL_LIMIT)
    );
}

#[test]
fn chat_result_hides_launch_ack_and_compacts_findings() {
    assert_eq!(
        chat_tool_result("Task", "  Async agent launched successfully.\nagentId: x"),
        ""
    );
    assert_eq!(
        chat_tool_result("Read", "Async agent launched successfully."),
        "Async agent launched successfully."
    );
    let small = r#"{"status":"open","count":0,"findings":[]}"#;
    assert_eq!(chat_tool_result("mcp__recur__list_findings", small), small);

    let row = |i: usize| {
        format!(
            r#"{{"id":"F{i}","file":"a.go","line":{i},"severity":"low","message":"{}","status":"open","notes":[{{"body":"{}"}}]}}"#,
            "m".repeat(1000),
            "n".repeat(1000)
        )
    };
    let compact = |n: usize| {
        let rows: Vec<String> = (1..=n).map(row).collect();
        let big = format!(r#"{{"status":"open","findings":[{}]}}"#, rows.join(","));
        let got = chat_tool_result("mcp__recur__list_findings", &big);
        assert!(got.len() <= CHAT_DETAIL_LIMIT);
        serde_json::from_str::<Value>(&got).unwrap()
    };
    let v = compact(40);
    assert_eq!(v["count"], 40);
    let findings = v["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 40);
    assert!(findings[0].get("notes").is_none());
    assert_eq!(
        findings[0]["message"].as_str().unwrap().len(),
        119 + '…'.len_utf8()
    );
    let v = compact(200);
    assert_eq!(v["count"], 200);
    let n = v["findings"].as_array().unwrap().len();
    assert!(n > 0 && n < 200, "{n}");

    let three = format!(
        r#"{{"status":"open","count":3,"findings":[{},{}]}}"#,
        row(1),
        row(2)
    );
    let v: Value = serde_json::from_str(&chat_tool_result("list_findings", &three)).unwrap();
    assert_eq!(v["count"], 3);
    assert_eq!(
        v["findings"][0]["notes"][0]["body"].as_str().unwrap().len(),
        1000
    );

    let not_envelope = format!(r#"{{"error":"{}"}}"#, "e".repeat(CHAT_DETAIL_LIMIT));
    assert_eq!(
        chat_tool_result("list_findings", &not_envelope),
        truncate(&not_envelope, CHAT_DETAIL_LIMIT)
    );
}

#[test]
fn descriptions_skip_task_bodies() {
    assert_eq!(
        tool_description("Bash", r#"{"description":" clean "}"#),
        "clean"
    );
    assert_eq!(
        tool_description("TaskCreate", r#"{"description":"body"}"#),
        ""
    );
    assert_eq!(tool_description("Bash", r#"{"description":3}"#), "");
    assert_eq!(tool_description("Bash", "null"), "");
}

#[test]
fn tool_predicates() {
    assert!(
        is_list_findings_tool("mcp__recur__list_findings")
            && is_list_findings_tool("list_findings")
    );
    assert!(!is_list_findings_tool("xlist_findings"));
    assert_eq!(canonical_claude_tool("Agent"), "Task");
    assert_eq!(canonical_claude_tool("Bash"), "Bash");
    assert_eq!(create_plan_body(r#"{"plan":"  x  "}"#), "x");
    assert_eq!(create_plan_body(r#"{"plan":1}"#), "");
}
