//! Go `harness_test.go`: instructions, repo rules and history cases.

use std::path::PathBuf;

use recur_domain::ContentBlock;

use super::*;

fn msg(id: &str, role: &str, blocks: Vec<ContentBlock>) -> Message {
    Message {
        id: id.into(),
        role: role.into(),
        stream_status: "complete".into(),
        content: Some(blocks),
        ..Default::default()
    }
}

fn encoded(items: &[InputItem]) -> String {
    serde_json::to_string(items).unwrap()
}

#[test]
fn instructions_always_include_coding_prompt() {
    let got = harness_instructions(&TurnRequest::default());
    assert!(got.contains("You are Recur's coding agent"), "{got}");
    let got = harness_instructions(&TurnRequest {
        system_prompt: "role identity".into(),
        turn_append_prompt: "on the billing PR".into(),
        ..Default::default()
    });
    assert!(got.contains("You are Recur's coding agent") && got.contains("role identity"));
    assert!(got.contains("on the billing PR"), "{got}");
}

#[test]
fn instructions_include_env_and_repo_rules() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().into_owned();
    let req = TurnRequest {
        worktree: PathBuf::from(&wt),
        ..Default::default()
    };
    std::fs::write(dir.path().join("CLAUDE.md"), "claude rules").unwrap();
    let got = harness_instructions(&req);
    assert!(got.contains(&format!("Worktree (Bash cwd): {wt}")), "{got}");
    assert!(got.contains("claude rules"), "{got}");

    std::fs::write(dir.path().join("AGENTS.md"), "agents rules").unwrap();
    let got = harness_instructions(&req);
    assert!(
        got.contains("agents rules") && !got.contains("claude rules"),
        "{got}"
    );

    let other = tempfile::tempdir().unwrap();
    let secret = other.path().join("secret");
    std::fs::write(&secret, "hunter2").unwrap();
    std::fs::remove_file(dir.path().join("AGENTS.md")).unwrap();
    std::fs::remove_file(dir.path().join("CLAUDE.md")).unwrap();
    std::os::unix::fs::symlink(&secret, dir.path().join("AGENTS.md")).unwrap();
    assert!(!harness_instructions(&req).contains("hunter2"));
}

#[test]
fn history_skips_current_user_and_keeps_call_ids() {
    let msgs = vec![
        msg("u1", "user", vec![ContentBlock::text("hello")]),
        msg(
            "a1",
            "assistant",
            vec![ContentBlock::tool_use(
                "internal-1",
                "call_abc",
                "Read",
                r#"{"file_path":"a.go"}"#,
            )],
        ),
        msg(
            "r1",
            "user",
            vec![ContentBlock::tool_result("internal-1", "ok", false)],
        ),
        msg("u2", "user", vec![ContentBlock::text("again")]),
    ];
    let req = TurnRequest {
        prompt: "again".into(),
        user_message_id: "u2".into(),
        ..Default::default()
    };
    let s = encoded(&harness_history(&msgs, &req, None));
    assert_eq!(s.matches("\"again\"").count(), 1, "{s}");
    assert!(s.contains("\"call_abc\""), "{s}");
    assert!(!s.contains("internal-1"), "{s}");
}

#[test]
fn history_keeps_original_on_hidden_continue() {
    let orig = "plan this screenshot";
    let msgs = vec![
        msg("u1", "user", vec![ContentBlock::text(orig)]),
        msg("a1", "assistant", vec![ContentBlock::text("switching")]),
    ];
    let cont = "Your switch_mode call remounted this chat from ask to plan. The new tool policy is live on THIS turn.";
    let s = encoded(&harness_history(
        &msgs,
        &TurnRequest {
            prompt: cont.into(),
            display_text: "Switched to Plan: continuing".into(),
            user_message_id: "u1".into(),
            skip_record_user: true,
            ..Default::default()
        },
        None,
    ));
    assert!(s.contains(orig) && s.contains("from ask to plan"), "{s}");

    let s = encoded(&harness_history(
        &msgs,
        &TurnRequest {
            prompt:
                "You were interrupted because Recur restarted. Continue from where you left off."
                    .into(),
            display_text: CHAT_RESUME_DISPLAY.into(),
            user_message_id: "u1".into(),
            skip_record_user: true,
            ..Default::default()
        },
        None,
    ));
    assert!(s.contains(orig) && s.contains("Recur restarted"), "{s}");
}

#[test]
fn history_respects_fresh_session_cutoff() {
    let msgs = vec![
        msg(
            "old-u",
            "user",
            vec![ContentBlock::text("hello from last week")],
        ),
        msg("old-a", "assistant", vec![ContentBlock::text("ok")]),
        msg(
            "cut",
            "assistant",
            vec![ContentBlock::text("end of prior session")],
        ),
        msg("new-u", "user", vec![ContentBlock::text("WIRE")]),
    ];
    let s = encoded(&harness_history(
        &msgs,
        &TurnRequest {
            prompt: "WIRE".into(),
            user_message_id: "new-u".into(),
            history_after_id: "cut".into(),
            ..Default::default()
        },
        None,
    ));
    assert!(
        !s.contains("hello from last week") && !s.contains("end of prior session"),
        "{s}"
    );
    assert_eq!(s.matches("WIRE").count(), 1, "{s}");
}

#[test]
fn cap_history_keeps_call_output_pair() {
    let items = [
        function_call_item("c1", "Read", &"x".repeat(100)),
        function_output_item("c1", &"y".repeat(100)),
        user_text("latest"),
    ];
    assert_eq!(next_history_group(&items[..2]), 2);
    assert_eq!(next_history_group(&items[1..2]), 1);
}

#[test]
fn cap_history_keeps_parallel_call_batch() {
    let mark = "latest-user-keep";
    let mk = |n: usize| {
        let pad = "x".repeat(n);
        vec![
            function_call_item("c1", "Read", "{}"),
            function_call_item("c2", "Glob", "{}"),
            function_output_item("c1", &pad),
            function_output_item("c2", &pad),
            user_text(mark),
        ]
    };
    assert_eq!(next_history_group(&mk(8)[..4]), 4);
    let skel = encoded(&mk(0)).len();
    // Two outputs grow by n each: the full input is just over the cap while
    // dropping only call(c1) would still fit (the split this guards against).
    let n = ((MAX_HISTORY_BYTES - skel) / 2 + 1).max(1);
    let items = mk(n);
    assert!(encoded(&items).len() > MAX_HISTORY_BYTES);
    assert!(encoded(&items[1..]).len() <= MAX_HISTORY_BYTES);
    let got = cap_history(items);
    let calls = got.iter().filter(|i| i.r#type == "function_call").count();
    let outs = got
        .iter()
        .filter(|i| i.r#type == "function_call_output")
        .count();
    assert!(
        got.iter().any(|i| i.role == "user"),
        "dropped the latest user item"
    );
    assert!(
        calls == outs && (calls == 0 || calls == 2),
        "{calls}/{outs}"
    );
}

#[test]
fn stored_images_replay_from_the_data_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("w")).unwrap();
    std::fs::write(root.path().join("w/a.png"), [1u8, 2, 3]).unwrap();
    let p = ImagePayload {
        id: "i".into(),
        mime: String::new(),
        path: "w/a.png".into(),
        ..Default::default()
    };
    let img = user_image_from_stored(&p, Some(root.path())).unwrap();
    assert_eq!(img.data, "AQID");
    assert!(user_image_from_stored(&p, None).is_none());
    let missing = ImagePayload {
        path: "w/b.png".into(),
        ..p
    };
    assert!(user_image_from_stored(&missing, Some(root.path())).is_none());
}
