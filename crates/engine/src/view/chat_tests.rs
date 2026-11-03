//! Transcript mapping, live merges and reducers.

use recur_domain::ChatItem;

use super::*;
use crate::view::mentions::attachment_marker;

/// An `AgentMsg` literal: `m!{me: true, body: "x"}`.
macro_rules! m {
    ($($f:ident : $v:expr),* $(,)?) => {
        AgentMsg { $($f: $v.into(),)* ..AgentMsg::default() }
    };
}

const NOW: i64 = 1_800_000_000_000;

fn history() -> Vec<AgentMsg> {
    vec![
        m! {me: true, body: "first question"},
        m! {body: "first answer"},
        m! {me: true, body: "second question"},
        m! {body: "second answer"},
    ]
}

fn cat(parts: &[&[AgentMsg]]) -> Vec<AgentMsg> {
    parts.concat()
}

fn merge(p: &[AgentMsg], l: &[AgentMsg]) -> Vec<AgentMsg> {
    merge_persisted_with_live(p, l)
}

#[test]
fn merge_keeps_one_side_when_the_other_is_empty() {
    assert_eq!(merge(&history(), &[]), history());
    let live = vec![m! {body: "streaming…", streaming: true}];
    assert_eq!(merge(&[], &live), live);
}

#[test]
fn merge_grafts_stream_tails_onto_history() {
    let live = vec![m! {body: "mid-stream…", streaming: true}];
    assert_eq!(merge(&history(), &live), cat(&[&history(), &live]));
    let live = vec![
        m! {tool: "Read", tool_id: "t1", result_pending: true},
        m! {body: "after the read…", streaming: true},
    ];
    assert_eq!(merge(&history(), &live), cat(&[&history(), &live]));
}

#[test]
fn merge_keeps_live_with_the_full_conversation() {
    let live = cat(&[
        &history(),
        &[
            m! {me: true, body: "third question"},
            m! {body: "working…", streaming: true},
        ],
    ]);
    assert_eq!(merge(&history(), &live), live);
}

#[test]
fn merge_preserves_optimistic_and_resent_users() {
    let live = vec![m! {me: true, body: "just sent", optimistic: true}];
    assert_eq!(merge(&history(), &live), cat(&[&history(), &live]));
    let live = vec![m! {me: true, body: "second question", optimistic: true}];
    assert_eq!(merge(&history(), &live), cat(&[&history(), &live]));
}

#[test]
fn merge_does_not_duplicate_the_trailing_user() {
    let live = vec![
        m! {me: true, body: "second question"},
        m! {body: "mid-stream…", streaming: true},
    ];
    assert_eq!(
        merge(&history(), &live),
        cat(&[&history(), &[m! {body: "mid-stream…", streaming: true}]])
    );
}

#[test]
fn merge_does_not_duplicate_a_pending_tool() {
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
        ],
    ]);
    let live = vec![
        m! {tool: "Read", tool_input: "app.go", result_pending: true, tool_id: "t1"},
        m! {body: "mid-stream…", streaming: true},
    ];
    assert_eq!(
        merge(&persisted, &live),
        cat(&[&history(), &[m! {me: true, body: "third"}], &live])
    );
}

#[test]
fn merge_keeps_settled_live_rows_newer_than_the_snapshot() {
    let persisted = cat(&[&history(), &[m! {me: true, body: "third"}]]);
    let live = vec![
        m! {tool: "Read", tool_input: "app.go", result: "ok"},
        m! {body: "mid-stream…", streaming: true},
    ];
    assert_eq!(merge(&persisted, &live), cat(&[&persisted, &live]));
}

#[test]
fn merge_overlays_a_live_result_onto_a_pending_snapshot() {
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
        ],
    ]);
    let live = vec![
        m! {tool: "Read", tool_input: "app.go", result: "ok"},
        m! {body: "mid-stream…", streaming: true},
    ];
    assert_eq!(
        merge(&persisted, &live),
        cat(&[&history(), &[m! {me: true, body: "third"}], &live])
    );
}

#[test]
fn merge_does_not_duplicate_history_when_live_is_behind() {
    let live = cat(&[
        &history(),
        &[m! {me: true, body: "third", optimistic: true}],
    ]);
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
        ],
    ]);
    assert_eq!(merge(&persisted, &live), persisted);
}

#[test]
fn merge_aligns_an_optimistic_send_with_history() {
    let prior = vec![
        m! {me: true, body: "first"},
        m! {body: "answer"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let marked = format!("{}review this", attachment_marker("img_a"));
    let persisted = cat(&[
        &prior,
        &[
            m! {me: true, body: marked.as_str()},
            m! {body: "I will fix it"},
            m! {tool: "Read", result_pending: true},
        ],
    ]);
    let live = cat(&[
        &prior,
        &[m! {me: true, body: marked.as_str(), optimistic: true}],
    ]);
    assert_eq!(merge(&persisted, &live), persisted);
    let ended = cat(&[&persisted, &[m! {turn_end: true, duration_ms: 1}]]);
    assert_eq!(merge(&ended, &live), ended);
}

#[test]
fn merge_keeps_an_optimistic_repeated_steer() {
    let persisted = vec![
        m! {me: true, body: "please stop"},
        m! {tool: "Read", result: "ok"},
    ];
    let live = cat(&[
        &persisted,
        &[m! {me: true, body: "please stop", optimistic: true, pending_id: "new-steer"}],
    ]);
    assert_eq!(merge(&persisted, &live), live);
}

#[test]
fn merge_appends_live_streaming_after_a_shared_prefix() {
    let live = cat(&[
        &history(),
        &[
            m! {me: true, body: "third", optimistic: true},
            m! {body: "mid-stream…", streaming: true},
        ],
    ]);
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
            m! {tool: "Grep", tool_input: "TODO", result_pending: true},
        ],
    ]);
    assert_eq!(
        merge(&persisted, &live),
        cat(&[&persisted, &[m! {body: "mid-stream…", streaming: true}]])
    );
}

#[test]
fn merge_does_not_duplicate_a_settled_current_turn_row() {
    let think = m! {thinking: true, body: "weighing options"};
    let live = cat(&[
        &history(),
        &[
            m! {me: true, body: "third", optimistic: true},
            think.clone(),
        ],
    ]);
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            think,
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
        ],
    ]);
    assert_eq!(merge(&persisted, &live), persisted);
}

#[test]
fn merge_drops_queued_follow_ups() {
    let live = vec![
        m! {body: "mid-stream…", streaming: true},
        m! {me: true, body: "follow-up", queued: true, pending_id: "p1"},
    ];
    assert_eq!(
        merge(&history(), &live),
        cat(&[&history(), &[m! {body: "mid-stream…", streaming: true}]])
    );
}

#[test]
fn merge_does_not_overlay_a_new_dispatch_onto_an_earlier_turn() {
    let prompt = "Please review the changes in this worktree and file any real issues";
    let persisted = vec![
        m! {me: true, body: prompt},
        m! {body: "I'll review the committed Tetris changes"},
        m! {turn_end: true, duration_ms: 1},
        m! {me: true, body: prompt},
    ];
    let live = vec![
        m! {me: true, body: prompt},
        m! {thinking: true, body: "Reviewing the diff", streaming: true},
    ];
    assert_eq!(
        merge(&persisted, &live),
        cat(&[
            &persisted,
            &[m! {thinking: true, body: "Reviewing the diff", streaming: true}]
        ])
    );
}

#[test]
fn merge_heap_remount_does_not_duplicate_streaming_text() {
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {thinking: true, body: "plan"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
            m! {body: "hello world"},
            m! {tool: "Grep", tool_input: "TODO", result_pending: true},
        ],
    ]);
    let live = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {thinking: true, body: "plan"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true, tool_id: "t1"},
            m! {body: "hello world", streaming: true},
        ],
    ]);
    assert_eq!(
        merge(&persisted, &live),
        cat(&[
            &history(),
            &[
                m! {me: true, body: "third"},
                m! {thinking: true, body: "plan"},
                m! {tool: "Read", tool_input: "app.go", result_pending: true, tool_id: "t1"},
                m! {body: "hello world", streaming: true},
                m! {tool: "Grep", tool_input: "TODO", result_pending: true},
            ],
        ])
    );
}

#[test]
fn merge_queued_flush_users_with_markers() {
    let marked = format!("{}pr settings alignment", attachment_marker("img_a"));
    let persisted = vec![
        m! {me: true, body: marked.as_str()},
        m! {tool: "Read", tool_input: "app.go", result_pending: true},
    ];
    let live = vec![m! {me: true, body: "pr settings alignment", optimistic: true}];
    assert_eq!(merge(&persisted, &live), persisted);

    let persisted = vec![
        m! {me: true, body: marked.as_str()},
        m! {tool: "Read", tool_input: "app.go", result: "ok"},
        m! {tool: "Grep", tool_input: "TODO", result: "none"},
        m! {body: "fixed"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let live = vec![
        m! {me: true, body: "pr settings alignment", optimistic: true},
        m! {me: true, body: marked.as_str()},
        m! {tool: "Read", tool_input: "app.go", result: "ok"},
        m! {body: "fixed"},
    ];
    let got = merge(&persisted, &live);
    let users: Vec<&AgentMsg> = got.iter().filter(|m| m.me).collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].body, marked);
    assert!(got.iter().any(|m| m.turn_end));
}

#[test]
fn merge_keeps_an_optimistic_resend_after_output() {
    let persisted = vec![
        m! {me: true, body: "first"},
        m! {body: "answer 1"},
        m! {turn_end: true, duration_ms: 1},
        m! {me: true, body: "again"},
        m! {body: "answer 2"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let live = vec![
        m! {me: true, body: "again", optimistic: true},
        m! {body: "new response", streaming: true},
    ];
    assert_eq!(merge(&persisted, &live), cat(&[&persisted, &live]));
}

#[test]
fn merge_overlays_results_with_an_optimistic_tail() {
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
        ],
    ]);
    let live = vec![
        m! {me: true, body: "third", optimistic: true},
        m! {tool: "Read", tool_input: "app.go", tool_id: "t1", result: "ok"},
        m! {body: "mid-stream…", streaming: true},
    ];
    assert_eq!(
        merge(&persisted, &live),
        cat(&[
            &history(),
            &[
                m! {me: true, body: "third"},
                m! {tool: "Read", tool_input: "app.go", tool_id: "t1", result: "ok"},
                m! {body: "mid-stream…", streaming: true},
            ],
        ])
    );
    let persisted = cat(&[
        &history(),
        &[
            m! {me: true, body: "third"},
            m! {body: "I will read the file"},
            m! {tool: "Read", tool_input: "app.go", result_pending: true},
        ],
    ]);
    let live = vec![
        m! {me: true, body: "third", optimistic: true},
        m! {body: "I will read the file"},
        m! {tool: "Read", tool_input: "app.go", tool_id: "t1", result: "ok"},
    ];
    assert_eq!(
        merge(&persisted, &live),
        cat(&[
            &history(),
            &[
                m! {me: true, body: "third"},
                m! {body: "I will read the file"},
                m! {tool: "Read", tool_input: "app.go", tool_id: "t1", result: "ok"},
            ],
        ])
    );
}

#[test]
fn merge_drops_a_lone_settled_replay() {
    let persisted = vec![
        m! {me: true, body: "first"},
        m! {body: "answer 1"},
        m! {turn_end: true, duration_ms: 1},
        m! {me: true, body: "second question"},
        m! {body: "answer 2"},
        m! {turn_end: true, duration_ms: 1},
    ];
    assert_eq!(
        merge(&persisted, &[m! {me: true, body: "second question"}]),
        persisted
    );
}

#[test]
fn merge_does_not_overlay_a_resend_onto_a_completed_turn() {
    let persisted = vec![
        m! {me: true, body: "first"},
        m! {body: "answer"},
        m! {turn_end: true, duration_ms: 1},
        m! {me: true, body: "review again"},
        m! {tool: "Read", tool_input: "app.go", tool_id: "old", result: "old contents"},
        m! {body: "done"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let live = vec![
        m! {me: true, body: "review again", optimistic: true},
        m! {tool: "Read", tool_input: "app.go", tool_id: "new", result: "new contents"},
        m! {body: "new response", streaming: true},
    ];
    assert_eq!(merge(&persisted, &live), cat(&[&persisted, &live]));
}

fn img(id: &str) -> ChatAttachmentView {
    ChatAttachmentView {
        id: id.into(),
        name: "shot.png".into(),
        mime: "image/png".into(),
        data_url: "data:image/png;base64,xx".into(),
        ..ChatAttachmentView::default()
    }
}

#[test]
fn retain_optimistic_users_cases() {
    let live = vec![m! {me: true, body: "review the auth flow", optimistic: true}];
    assert_eq!(retain_optimistic_users(&[], &live), live);
    let mapped = vec![m! {me: true, body: "review the auth flow"}];
    assert_eq!(retain_optimistic_users(&mapped, &live), mapped);
    let marked = format!("{}review the auth flow", attachment_marker("img_a"));
    let mapped = vec![m! {me: true, body: marked.as_str()}];
    assert_eq!(retain_optimistic_users(&mapped, &live), mapped);
    let mapped = vec![
        m! {me: true, body: marked.as_str()},
        m! {body: "done"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let idle = vec![
        m! {me: true, body: "review the auth flow", optimistic: true},
        m! {me: true, body: marked.as_str()},
        m! {body: "done"},
    ];
    assert_eq!(retain_optimistic_users(&mapped, &idle), mapped);
    let streaming = vec![
        m! {me: true, body: "review the auth flow", optimistic: true},
        m! {body: "working…", streaming: true},
    ];
    assert_eq!(
        retain_optimistic_users(&[], &streaming),
        [m! {me: true, body: "review the auth flow", optimistic: true}]
    );
}

#[test]
fn retain_keeps_repeated_and_image_sends() {
    let mapped = vec![
        m! {me: true, body: "please stop"},
        m! {tool: "Read", result: "ok"},
        m! {me: true, body: "please stop"},
        m! {turn_end: true, duration_ms: 1},
    ];
    assert_eq!(retain_optimistic_users(&mapped, &[]), mapped);

    let marked = format!("{}review this", attachment_marker("new_image"));
    let mapped = vec![
        m! {me: true, body: "review this"},
        m! {body: "done"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let live = vec![
        m! {me: true, body: marked.as_str(), optimistic: true, attachments: vec![img("new_image")]},
    ];
    assert_eq!(
        retain_optimistic_users(&mapped, &live),
        cat(&[&mapped, &live])
    );

    let marked = format!("{}review this", attachment_marker("img_a"));
    let mapped = vec![
        m! {me: true, body: "review this"},
        m! {tool: "Read", result: "ok"},
        m! {me: true, body: marked.as_str()},
        m! {turn_end: true, duration_ms: 1},
    ];
    assert_eq!(retain_optimistic_users(&mapped, &[]), mapped);

    let mapped = vec![
        m! {me: true, body: marked.as_str()},
        m! {body: "done"},
        m! {turn_end: true, duration_ms: 1},
    ];
    let live = vec![m! {me: true, body: "review this", optimistic: true}];
    assert_eq!(
        retain_optimistic_users(&mapped, &live),
        cat(&[&mapped, &live])
    );
    let live = cat(&[
        &mapped,
        &[m! {me: true, body: "review this", optimistic: true}],
    ]);
    assert_eq!(
        retain_optimistic_users(&mapped, &live),
        cat(&[&mapped, &live[live.len() - 1..]])
    );

    let mapped = vec![
        m! {me: true, body: marked.as_str()},
        m! {tool: "Read", result: "ok"},
    ];
    let live = cat(&[
        &mapped,
        &[m! {me: true, body: "review this", optimistic: true, pending_id: "new-steer"}],
    ]);
    assert_eq!(retain_optimistic_users(&mapped, &live), live);
    assert_eq!(merge(&mapped, &live), live);
}

#[test]
fn option_choice_text_strips_only_real_tags() {
    assert_eq!(
        option_choice_text(r#"Reject with <code style="font-family:var(--mono)">429</code>"#),
        "Reject with 429"
    );
    assert_eq!(
        option_choice_text("count < 5 or when rate > 2/s"),
        "count < 5 or when rate > 2/s"
    );
    assert_eq!(
        option_choice_text("Cap at < 100 req/min"),
        "Cap at < 100 req/min"
    );
    assert_eq!(
        option_choice_text("Token bucket with refill"),
        "Token bucket with refill"
    );
    assert_eq!(
        option_choice_text("<a_b> x"),
        "<a_b> x",
        "\\b after the tag name"
    );
}

#[test]
fn ask_user_display_text() {
    assert_eq!(format_ask_user_display_text("Blue", ""), "Blue");
    assert_eq!(
        format_ask_user_display_text("", "navy preferred"),
        "navy preferred"
    );
    assert_eq!(
        format_ask_user_display_text("Blue", "navy preferred"),
        "Blue: navy preferred"
    );
    assert_eq!(format_ask_user_display_text("  ", "  "), "(empty)");
}

#[test]
fn pending_send_bubble_bodies() {
    let marked = format!("{}pr settings alignment", attachment_marker("img_a"));
    assert_eq!(pending_send_bubble_body("", &marked), marked);
    assert_eq!(
        pending_send_bubble_body("Fix open reviewer findings", "WIRE do not show"),
        "Fix open reviewer findings"
    );
}

#[test]
fn display_user_chat_text_collapses_resume_and_dispatch_wire() {
    let wire = [
        "The user answered your earlier ask_user question.",
        "",
        "Question: Which color?",
        "Context: pick one",
        "Answer: Blue",
        "",
        "Continue with this answer. Do not re-ask the same question unless you still need clarification.",
    ]
    .join("\n");
    assert_eq!(display_user_chat_text(&wire), "Blue");
    let wire = [
        "The user answered your earlier ask_user question.",
        "",
        "Question: Which color?",
        "Answer: Blue",
        "Details: navy preferred",
        "",
        "Continue with this answer. Do not re-ask the same question unless you still need clarification.",
    ]
    .join("\n");
    assert_eq!(display_user_chat_text(&wire), "Blue: navy preferred");
    let fix = [
        "You are the Coder agent. A reviewer found issues with the current diff. ",
        "Fix every finding below with minimal, correct edits.",
        "",
        "Original task: create a cli game of pong",
    ]
    .join("\n");
    assert_eq!(display_user_chat_text(&fix), "Fix open reviewer findings");
    assert_eq!(
        display_user_chat_text("You were interrupted mid-task. Pick up."),
        "Resume coding"
    );
    assert_eq!(
        display_user_chat_text("create a cli game of pong"),
        "create a cli game of pong"
    );
}

#[test]
fn display_user_chat_text_matches_the_engine_resume_prompt() {
    let q = recur_domain::UserQuestion {
        question: "Which color?".into(),
        ..Default::default()
    };
    let ans = recur_domain::UserQuestionAnswer {
        choice: "Blue".into(),
        free_text: "navy preferred".into(),
    };
    let wire = crate::ask_user::format_ask_user_resume_prompt(&q, &ans);
    assert_eq!(display_user_chat_text(&wire), "Blue: navy preferred");
    let multi = recur_domain::UserQuestionAnswer {
        free_text: "navy\npreferred".into(),
        ..ans
    };
    let wire = crate::ask_user::format_ask_user_resume_prompt(&q, &multi);
    assert_eq!(
        display_user_chat_text(&wire),
        wire,
        "`.` stops at a newline, as in JS"
    );
}

const SPEND: &str = "You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit";

fn prefixed() -> String {
    format!("{AGENT_ERROR_PREFIX} {SPEND}")
}

#[test]
fn agent_error_titles_and_details() {
    assert_eq!(agent_error_title(&prefixed()), "Usage limit");
    assert_eq!(
        agent_error_title("Agent error: rate limit exceeded"),
        "Rate limit"
    );
    assert_eq!(
        agent_error_title("Agent error: cursor run: boom"),
        "Agent error"
    );
    assert_eq!(
        agent_error_title("Agent error: HTTP 429 from upstream"),
        "Rate limit"
    );
    assert_eq!(
        agent_error_title("Agent error: quota exhausted"),
        "Usage limit"
    );
    assert_eq!(agent_error_title("Agent error: quotas"), "Agent error");
    assert_eq!(agent_error_detail(&prefixed()), SPEND);
    let ansi = "\x1b[2m2026-09-10T15:43:08.630243Z\x1b[0m \x1b[31mERROR\x1b[0m \x1b[2mcodex_login::auth::manager\x1b[0m \x1b[2m-\x1b[0m Failed to refresh token: 401 Unauthorized {";
    let body = format!("{AGENT_ERROR_PREFIX} {ansi}");
    assert_eq!(agent_error_detail(&body), CODEX_LOGIN_EXPIRED);
    assert_eq!(agent_error_title(&body), "Codex login");
    assert_eq!(
        agent_error_detail(
            "Agent error: ERROR workspace::tools - workspace tool failed to start {"
        ),
        "workspace tool failed to start"
    );
    let msg = r#"API Error: 400 {"error":{"message":"model does not support images"}}"#;
    assert_eq!(agent_error_detail(&format!("Agent error: {msg}")), msg);
    assert_eq!(
        agent_error_detail(&format!(
            "{AGENT_ERROR_PREFIX} ERROR codex_login::auth::manager - 401 Unauthorized"
        )),
        CODEX_LOGIN_EXPIRED
    );
}

#[test]
fn agent_error_reply_detection() {
    assert!(is_agent_error_reply(&prefixed()));
    assert!(is_agent_error_reply("The agent hit an error: try again."));
    assert!(is_agent_error_reply(
        "I hit an error trying to respond just now. Try again."
    ));
    assert!(!is_agent_error_reply("Done."));
    assert!(is_chat_stopped_reply(" Stopped by user "));
    assert!(is_chat_stopped_reply("Stopped."));
    assert!(is_chat_injected_reply(CHAT_INJECTED_REPLY));
    assert!(!is_chat_injected_reply(""));
}

#[test]
fn finish_turn_marks_errors_and_stamps_duration() {
    let out = finish_turn(
        &[m! {me: true, body: "do the thing"}],
        &prefixed(),
        false,
        62000,
        NOW,
    );
    assert!(out.iter().any(|m| m.error && m.body == prefixed()));
    assert!(out.iter().any(|m| m.turn_end && m.duration_ms == 62000));
    let out = finish_turn(
        &[
            m! {me: true, body: "do the thing"},
            m! {body: SPEND},
            m! {turn_end: true, duration_ms: 62000},
        ],
        &prefixed(),
        true,
        0,
        NOW,
    );
    let errors: Vec<&AgentMsg> = out.iter().filter(|m| m.error).collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].body, prefixed());
    assert!(!out.iter().any(|m| m.body == SPEND && !m.error));
    assert!(out.iter().any(|m| m.turn_end));
}

#[test]
fn finish_turn_reply_and_stop_paths() {
    let out = finish_turn(
        &[
            m! {me: true, body: "go"},
            m! {body: "partial", streaming: true},
        ],
        "Done.",
        true,
        0,
        NOW,
    );
    assert_eq!(out.len(), 2, "streamed text hides an ordinary fallback");
    assert!(!out[1].streaming);
    let out = finish_turn(&[m! {me: true, body: "go"}], "Done.", false, 0, NOW);
    assert_eq!(out[1], m! {body: "Done."});
    let out = finish_turn(
        &[m! {me: true, body: "go"}],
        CHAT_STOPPED_REPLY,
        true,
        500,
        NOW,
    );
    assert_eq!(out[1], m! {body: CHAT_STOPPED_REPLY, stopped: true});
    assert_eq!(out[2], m! {turn_end: true, duration_ms: 500, ts: NOW});
    assert_eq!(
        finish_turn(&[m! {me: true, body: "go"}], " ", false, 0, NOW).len(),
        1
    );
}

#[test]
fn end_turn_settles_in_flight_rows() {
    let out = end_turn(
        &[
            m! {me: true, body: "go"},
            m! {body: "partial", streaming: true},
            m! {tool: "Read", result_pending: true},
        ],
        103000,
        NOW,
    );
    assert!(!out.iter().any(|m| m.streaming || m.result_pending));
    let last = out.last().unwrap();
    assert!(last.turn_end && last.duration_ms == 103000 && last.ts == Some(NOW));
    assert_eq!(end_turn(&out, 103000, NOW + 1), out);
    assert_eq!(end_turn(&[], 0, NOW), []);
}

const FOOTER_AT: i64 = 1_700_000_000_000;

fn done() -> Vec<AgentMsg> {
    vec![
        m! {me: true, body: "go"},
        m! {body: "ok"},
        m! {turn_end: true, duration_ms: 103000, ts: FOOTER_AT},
    ]
}

#[test]
fn current_turn_has_ended_cases() {
    assert!(!current_turn_has_ended(&[], 0));
    assert!(!current_turn_has_ended(&[m! {me: true, body: "go"}], 0));
    assert!(!current_turn_has_ended(
        &[m! {me: true, body: "go"}, m! {body: "ok"}],
        0
    ));
    assert!(current_turn_has_ended(&done(), 0));
    assert!(current_turn_has_ended(&done(), FOOTER_AT - 50_000));
    let log = ChatLog::Session(ChatLogSession {
        code: "CO".into(),
        name: "Coder".into(),
        live: true,
        ..ChatLogSession::default()
    });
    let mut with_log = done();
    with_log.push(AgentMsg {
        log: Some(Box::new(log)),
        ..AgentMsg::default()
    });
    assert!(current_turn_has_ended(&with_log, 0));
    assert!(current_turn_has_ended(
        &cat(&[&done(), &[m! {me: true, body: "queued next", queued: true}]]),
        0
    ));
    assert!(!current_turn_has_ended(&done(), FOOTER_AT));
    assert!(!current_turn_has_ended(&done(), FOOTER_AT + 10));
    for tail in [
        m! {body: "retried reply", streaming: true},
        m! {tool: "Read", result_pending: true},
        m! {note: true, body: "Waiting for previous run run-1 to finish."},
        m! {me: true, body: "next turn", optimistic: true},
    ] {
        assert!(!current_turn_has_ended(&cat(&[&done(), &[tail]]), 0));
    }
    let no_ts = vec![
        m! {me: true, body: "go"},
        m! {turn_end: true, duration_ms: 1},
    ];
    assert!(current_turn_has_ended(&no_ts, 0));
    assert!(!current_turn_has_ended(&no_ts, 10));
}

#[test]
fn drop_live_after_footer_gates() {
    let apply = |list: &[AgentMsg], started: i64, busy: bool| {
        if drop_live_after_footer(list, started, busy) {
            list.to_vec()
        } else {
            apply_stream(
                list,
                StreamKind::Text,
                "late leftover chunk",
                &StreamOpts::default(),
            )
        }
    };
    assert_eq!(apply(&done(), FOOTER_AT - 50_000, true), done());
    assert_eq!(apply(&done(), 0, false), done());
    let next = apply(&done(), 0, true);
    assert_ne!(next, done());
    let last = next.last().unwrap();
    assert!(last.body == "late leftover chunk" && last.streaming);
    let retried = apply(&done(), FOOTER_AT + 10, true);
    assert_eq!(retried.last().unwrap().body, "late leftover chunk");
    let no_ts = vec![
        m! {me: true, body: "go"},
        m! {turn_end: true, duration_ms: 1},
    ];
    assert_ne!(apply(&no_ts, 10, false), no_ts);
}

#[test]
fn turn_stamp_and_idle_gates() {
    assert!(should_refresh_turn_stamp(&done(), 0));
    assert!(should_refresh_turn_stamp(&done(), FOOTER_AT - 50_000));
    assert!(!should_refresh_turn_stamp(&done(), FOOTER_AT + 10));
    assert!(!should_refresh_turn_stamp(&[m! {me: true, body: "go"}], 1));

    let short = vec![
        m! {me: true, body: "go"},
        m! {turn_end: true, duration_ms: 1, ts: FOOTER_AT},
    ];
    assert!(!should_ignore_stale_idle(&short, 0));
    assert!(!should_ignore_stale_idle(&short, FOOTER_AT - 1));
    assert!(should_ignore_stale_idle(&short, FOOTER_AT + 10));
    assert!(should_ignore_stale_idle(&short, FOOTER_AT));
    assert!(!should_ignore_stale_idle(
        &[m! {me: true, body: "go"}],
        FOOTER_AT + 10
    ));
    assert!(!should_ignore_stale_idle(
        &[
            m! {me: true, body: "go"},
            m! {turn_end: true, duration_ms: 1}
        ],
        10
    ));

    assert!(!should_skip_idle_transcript_reset(
        &short,
        FOOTER_AT - 1,
        true
    ));
    assert!(should_skip_idle_transcript_reset(
        &short,
        FOOTER_AT + 10,
        true
    ));
    assert!(!should_skip_idle_transcript_reset(
        &short,
        FOOTER_AT + 10,
        false
    ));
    let live = cat(&[&short, &[m! {body: "second turn", streaming: true}]]);
    assert!(should_skip_idle_transcript_reset(
        &live,
        FOOTER_AT + 10,
        true
    ));
    assert!(!should_skip_idle_transcript_reset(
        &live,
        FOOTER_AT + 10,
        false
    ));
    assert!(!should_skip_idle_transcript_reset(
        &[m! {me: true, body: "go"}, m! {body: "reply"}],
        10,
        false
    ));
    assert!(!should_skip_idle_transcript_reset(&[], 10, true));
}

#[test]
fn last_assistant_text_before_the_footer() {
    let msgs = vec![
        m! {me: true, body: "do it"},
        m! {body: "first"},
        m! {tool: "Read", input: "{}"},
        m! {body: "final reply"},
        m! {turn_end: true, duration_ms: 12},
    ];
    assert_eq!(
        last_assistant_text_in_turn(&msgs, 4).map(|m| m.body.as_str()),
        Some("final reply")
    );
    assert!(last_assistant_text_in_turn(&msgs, 0).is_none());
    assert!(last_assistant_text_in_turn(&[m! {turn_end: true}], 0).is_none());
}

fn item(role: &str, kind: &str) -> ChatItem {
    ChatItem {
        role: role.into(),
        kind: kind.into(),
        ..ChatItem::default()
    }
}

fn text_item(role: &str, text: &str, ts: i64) -> ChatItem {
    ChatItem {
        text: text.into(),
        ts,
        ..item(role, "text")
    }
}

#[test]
fn map_chat_items_footers() {
    let mapped = map_chat_items(&[
        text_item("user", "review this", 0),
        ChatItem {
            tool: "mcp__recur__report_finding".into(),
            input: "{}".into(),
            ..item("assistant", "tool")
        },
        text_item("assistant", "Filed 1 finding.", 0),
        ChatItem {
            duration_ms: 8400,
            ..item("assistant", "turn")
        },
    ]);
    assert_eq!(
        mapped.iter().find(|m| m.turn_end).map(|m| m.duration_ms),
        Some(8400)
    );
    let mapped = map_chat_items(&[
        text_item("user", "review this", 0),
        text_item("assistant", "Filed 1 finding.", 0),
    ]);
    assert!(!mapped.iter().any(|m| m.turn_end));
    assert!(
        mapped[1].widen_stream,
        "the trailing assistant row can widen"
    );
}

#[test]
fn map_chat_items_errors_sentinels_and_dupes() {
    let mapped = map_chat_items(&[
        text_item("user", "do the thing", 0),
        text_item("assistant", SPEND, 0),
        ChatItem {
            duration_ms: 62000,
            message_id: "msg-turn-1".into(),
            ..item("assistant", "turn")
        },
        text_item("assistant", &prefixed(), 0),
        text_item("user", CHAT_RETRY_DISPLAY, 0),
        text_item("user", "Switched to Plan: continuing", 0),
    ]);
    assert_eq!(
        mapped
            .iter()
            .find(|m| m.turn_end)
            .map(|m| m.message_id.as_str()),
        Some("msg-turn-1")
    );
    assert_eq!(mapped.iter().filter(|m| m.me).count(), 1);
    let errs: Vec<&AgentMsg> = mapped.iter().filter(|m| m.error).collect();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].body, prefixed());
}

#[test]
fn map_chat_items_keeps_ts_on_repeated_errors() {
    let err = "Agent error: [resource_exhausted] Error. Cursor is rate-limited.";
    let mapped = map_chat_items(&[
        text_item("user", "go", 1),
        text_item("assistant", err, 10),
        ChatItem {
            text: "retrying".into(),
            ts: 20,
            ..item("assistant", "thinking")
        },
        text_item("assistant", err, 30),
    ]);
    let ts: Vec<Option<i64>> = mapped.iter().filter(|m| m.error).map(|m| m.ts).collect();
    assert_eq!(ts, [Some(10), Some(30)]);
}

#[test]
fn map_chat_items_rows() {
    let wire = "You are the Coder agent. A reviewer found issues with the current diff.\nFix every finding.";
    let mapped = map_chat_items(&[text_item("user", wire, 0)]);
    assert!(mapped[0].me && mapped[0].body == "Fix open reviewer findings");
    let mapped = map_chat_items(&[
        ChatItem {
            text: "older turns folded".into(),
            tokens: 167219,
            ..item("assistant", "compact")
        },
        text_item("assistant", "continuing", 0),
        ChatItem {
            text: "Opus 4.8".into(),
            description: "Fable 5".into(),
            ..item("assistant", "fallback")
        },
    ]);
    assert!(
        mapped[0].compact && mapped[0].body == "older turns folded" && mapped[0].tokens == 167219
    );
    assert!(
        mapped[2].fallback && mapped[2].body == "Opus 4.8" && mapped[2].description == "Fable 5"
    );
    let mapped = map_chat_items(&[
        text_item("user", DISPLAY_HIDDEN_SENTINEL, 0),
        text_item(
            "user",
            &format!("{DISPLAY_HIDDEN_SENTINEL} Explore finished"),
            0,
        ),
        text_item("assistant", "Stopped.", 0),
        ChatItem {
            attachments: vec![recur_domain::ChatImageRef {
                id: "img_a".into(),
                ..Default::default()
            }],
            ..text_item("user", "see", 0)
        },
    ]);
    assert_eq!(mapped.len(), 3);
    assert!(
        mapped[0].note
            && mapped[0].note_label == "Subagent"
            && mapped[0].body == "Explore finished"
    );
    assert!(mapped[1].stopped && mapped[1].body == CHAT_STOPPED_REPLY);
    assert_eq!(mapped[2].attachments[0].name, "img_a");
}

#[test]
fn map_chat_items_nests_children() {
    let mapped = map_chat_items(&[ChatItem {
        tool: "Task".into(),
        tool_id: "toolu_task_1".into(),
        description: "Map UI".into(),
        children: vec![
            ChatItem {
                tool: "Grep".into(),
                tool_id: "t_nested".into(),
                tool_input: "PRDetail".into(),
                result: "src/PR.svelte".into(),
                ..item("assistant", "tool")
            },
            text_item("assistant", "Found foo.go", 0),
        ],
        ..item("assistant", "tool")
    }]);
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].tool_id, "toolu_task_1");
    assert_eq!(mapped[0].children.len(), 2);
    let c = &mapped[0].children;
    assert!(c[0].tool == "Grep" && c[0].tool_id == "t_nested" && c[0].result == "src/PR.svelte");
    assert_eq!(c[1].body, "Found foo.go");
}

fn compact(status: &str, summary: &str, pre_tokens: i64) -> CompactEvt {
    CompactEvt {
        status: status.into(),
        summary: summary.into(),
        pre_tokens,
        ..CompactEvt::default()
    }
}

fn done_opts() -> StreamOpts {
    StreamOpts {
        done: true,
        ..StreamOpts::default()
    }
}

#[test]
fn compaction_rows() {
    let live = apply_compact(&[], &compact("started", "", 180000));
    assert!(live[0].compact && live[0].compacting && live[0].tokens == 180000);
    let done = apply_compact(&live, &compact("done", "folded", 0));
    assert_eq!(done.len(), 1);
    assert!(done[0].compact && !done[0].compacting && done[0].body == "folded");
    let c = apply_compact(&[], &compact("done", "older turns folded", 0));
    let next = apply_stream(&c, StreamKind::Text, "continuing", &done_opts());
    assert_eq!(next.len(), 2);
    assert_eq!(next[0].body, "older turns folded");
    assert!(next[1].body == "continuing" && !next[1].streaming);
    let first = apply_compact(&[], &compact("done", "first fold", 0));
    let second = apply_compact(&first, &compact("started", "", 90000));
    assert_eq!(second.len(), 2);
    assert!(!second[0].compacting && second[0].body == "first fold");
    assert!(second[1].compacting && second[1].tokens == 90000 && second[1].body.is_empty());
    let live = apply_compact(&[m! {body: "hello"}], &compact("started", "", 180000));
    assert_eq!(drop_open_compact(&live), [m! {body: "hello"}]);
}

#[test]
fn notices_are_not_assistant_text() {
    let live = apply_notice(&[], "Waiting for previous run run-1 to finish.");
    assert!(live[0].note && live[0].body == "Waiting for previous run run-1 to finish.");
    let next = apply_stream(&live, StreamKind::Text, "delivered", &done_opts());
    assert_eq!(next.len(), 2);
    assert!(next[0].note);
    assert!(next[1].body == "delivered" && !next[1].streaming);
    let replaced = apply_notice(
        &live,
        "Previous run did not finish; cancelling and retrying.",
    );
    assert_eq!(replaced.len(), 1);
    assert_eq!(
        replaced[0].body,
        "Previous run did not finish; cancelling and retrying."
    );
    assert_eq!(apply_notice(&live, "  "), live);
}

fn fb(to_label: &str, from_label: &str, retracted: &[&str]) -> FallbackEvt {
    FallbackEvt {
        to_label: to_label.into(),
        from_label: from_label.into(),
        retracted: retracted.iter().map(|s| (*s).to_owned()).collect(),
        ..FallbackEvt::default()
    }
}

#[test]
fn fallback_rows() {
    let live = apply_fallback(
        &[],
        &FallbackEvt {
            from: "claude-fable-5".into(),
            to: "claude-opus-4-8".into(),
            from_label: "Fable 5".into(),
            to_label: "Opus 4.8".into(),
            retracted: vec![],
        },
    );
    assert!(live[0].fallback && live[0].body == "Opus 4.8" && live[0].description == "Fable 5");
    assert_eq!(
        apply_fallback(&live, &fb("Opus 4.8", "Fable 5", &[])).len(),
        1
    );
    let incomplete = apply_fallback(&[], &fb("Opus 4.8", "", &[]));
    assert!(incomplete[0].fallback && incomplete[0].description.is_empty());
    let enriched = apply_fallback(&incomplete, &fb("Opus 4.8", "Fable 5", &[]));
    assert_eq!(enriched.len(), 1);
    assert_eq!(enriched[0].description, "Fable 5");
    let retracted = apply_fallback(
        &[
            m! {body: "refused", provider_uuid: "fable-frame"},
            m! {body: "opus", provider_uuid: "opus-frame"},
        ],
        &fb("Opus 4.8", "Fable 5", &["fable-frame"]),
    );
    assert_eq!(retracted.len(), 2);
    assert!(retracted[0].fallback && retracted[0].body == "Opus 4.8");
    assert!(retracted[1].body == "opus" && retracted[1].provider_uuid == "opus-frame");
    let fallback = apply_fallback(&[], &fb("Opus 4.8", "Fable 5", &[]));
    let next = apply_stream(&fallback, StreamKind::Text, "continuing", &done_opts());
    assert_eq!(next.len(), 2);
    assert!(next[0].fallback);
    assert!(next[1].body == "continuing" && !next[1].streaming);
    assert_eq!(
        apply_fallback(&[m! {body: "x"}], &fb("", "", &[])),
        [m! {body: "x"}]
    );
}

#[test]
fn retraction_without_uuid_matches_drops_the_unstamped_leg() {
    let list = vec![
        m! {me: true, body: "go"},
        m! {body: "refused text"},
        m! {tool: "Read"},
    ];
    let got = apply_fallback(&list, &fb("Opus 4.8", "", &["gone-frame"]));
    assert_eq!(got.len(), 2);
    assert!(got[0].me && got[1].fallback);
}

fn uuid_opts(uuid: &str) -> StreamOpts {
    StreamOpts {
        uuid: uuid.into(),
        ..StreamOpts::default()
    }
}

#[test]
fn stream_frames_and_thinking_tokens() {
    let first = apply_stream(&[], StreamKind::Text, "fable", &uuid_opts("a"));
    let next = apply_stream(&first, StreamKind::Text, "opus", &uuid_opts("b"));
    assert_eq!(next.len(), 2);
    assert!(next[0].body == "fable" && !next[0].streaming && next[0].provider_uuid == "a");
    assert!(next[1].body == "opus" && next[1].streaming && next[1].provider_uuid == "b");

    let thought = note_thinking_tokens(&[], 12, "fable-frame", "");
    assert!(
        thought[0].thinking && thought[0].tokens == 12 && thought[0].provider_uuid == "fable-frame"
    );
    let retracted = apply_fallback(&thought, &fb("Opus 4.8", "", &["fable-frame"]));
    assert_eq!(retracted.len(), 1);
    assert!(retracted[0].fallback);

    let first = note_thinking_tokens(&[], 326, "evt-1", "");
    let next = note_thinking_tokens(&first, 340, "evt-2", "");
    assert_eq!(next.len(), 1);
    assert!(next[0].streaming && next[0].tokens == 340 && next[0].provider_uuid == "evt-1");

    let mut list = note_thinking_tokens(&[], 326, "", "");
    list = apply_stream(&list, StreamKind::Thinking, "Act", &StreamOpts::default());
    list = note_thinking_tokens(&list, 340, "", "");
    list = apply_stream(
        &list,
        StreamKind::Thinking,
        "Actually",
        &StreamOpts::default(),
    );
    assert_eq!(list.len(), 1);
    assert!(
        list[0].thinking
            && list[0].streaming
            && list[0].body == "Actually"
            && list[0].tokens == 340
    );
}

#[test]
fn stream_settles_stay_in_the_current_tail() {
    let list = vec![
        m! {thinking: true, body: "need a decision"},
        m! {me: true, body: "use notes", ts: 5000},
    ];
    let next = apply_stream(&list, StreamKind::Thinking, "resuming", &done_opts());
    assert_eq!(next[0].body, "need a decision");
    assert_eq!(next[1].body, "use notes");
    assert!(next[2].thinking && next[2].body == "resuming" && !next[2].streaming);

    let list = vec![
        m! {thinking: true, body: "first block"},
        m! {tool: "Read", tool_id: "t1", result: "ok"},
    ];
    let next = apply_stream(&list, StreamKind::Thinking, "second block", &done_opts());
    assert_eq!(next[0].body, "first block");
    assert_eq!(next[1].tool, "Read");
    assert!(next[2].body == "second block" && !next[2].streaming);
    assert_eq!(settle_thinking(&list, "second block"), next);
    let settled = settle_text(&[m! {body: "x", streaming: true}], "xyz");
    assert_eq!(settled, [m! {body: "xyz"}]);
}

#[test]
fn stream_widens_only_marked_remount_rows() {
    let next = apply_stream(
        &[m! {body: "hello", widen_stream: true}],
        StreamKind::Text,
        "hello world",
        &StreamOpts::default(),
    );
    assert_eq!(next, [m! {body: "hello world", streaming: true}]);
    let early = apply_stream(
        &[m! {body: "Let me check.", widen_stream: true}],
        StreamKind::Text,
        "L",
        &StreamOpts::default(),
    );
    assert_eq!(early.len(), 2);
    assert_eq!(early[0], m! {body: "Let me check."});
    assert_eq!(early[1], m! {body: "L", streaming: true});
    let grown = apply_stream(
        &early,
        StreamKind::Text,
        "Later I will edit.",
        &StreamOpts::default(),
    );
    assert_eq!(grown.len(), 2);
    assert_eq!(grown[1], m! {body: "Later I will edit.", streaming: true});
    let next = apply_stream(
        &[m! {body: "Let me check."}],
        StreamKind::Text,
        "Let me check. Then I will edit.",
        &StreamOpts::default(),
    );
    assert_eq!(next.len(), 2);
    assert!(next[1].streaming);
    assert_eq!(
        apply_stream(&next, StreamKind::Text, "", &StreamOpts::default()),
        next
    );
}

#[test]
fn append_stream_extends_or_opens() {
    let list = append_stream(&[], StreamKind::Text, "a");
    let list = append_stream(&list, StreamKind::Text, "b");
    assert_eq!(list, [m! {body: "ab", streaming: true}]);
    let list = append_stream(&list, StreamKind::Thinking, "t");
    assert_eq!(list[0], m! {body: "ab"});
    assert_eq!(list[1], m! {thinking: true, body: "t", streaming: true});
}

#[test]
fn add_user_cases() {
    let list = add_user(&[m! {body: "assistant"}], "  please stop  ", NOW);
    assert_eq!(list.len(), 2);
    assert_eq!(list[1], m! {me: true, body: "please stop", ts: NOW});
    assert_eq!(
        add_user(&[m! {body: "assistant"}], "Please review the changes", NOW).len(),
        2
    );
    assert_eq!(
        add_user(&[m! {body: "assistant"}], "   ", NOW),
        [m! {body: "assistant"}]
    );
    let list = add_user(
        &[m! {me: true, body: "please stop", optimistic: true}],
        "please stop",
        NOW,
    );
    assert_eq!(list.len(), 1);
    assert!(!list[0].optimistic);
    let atts = vec![img("img_a")];
    let list = add_user(
        &[m! {me: true, body: "fix the layout", optimistic: true, attachments: atts.clone()}],
        &format!("{}fix the layout", attachment_marker("img_a")),
        NOW,
    );
    assert_eq!(list.len(), 1);
    assert!(list[0].me && !list[0].optimistic && list[0].attachments == atts);
    let list = add_user(&[m! {me: true, body: "please stop"}], "please stop", NOW);
    assert_eq!(list.len(), 2);
    let list = add_user(
        &[
            m! {me: true, body: "please stop", optimistic: true},
            m! {me: true, body: "please stop", optimistic: true},
        ],
        "please stop",
        NOW,
    );
    assert_eq!(list.len(), 2);
    assert!(!list[0].optimistic && list[1].optimistic);
}

#[test]
fn add_user_closes_the_live_row() {
    let list = add_user(&[m! {body: "working", streaming: true}], "steer", NOW);
    assert!(!list[0].streaming);
    assert!(list[1].me && list[1].body == "steer");
    let next = apply_stream(
        &list,
        StreamKind::Text,
        "ok after steer",
        &StreamOpts::default(),
    );
    assert_eq!(
        next.iter().filter(|m| m.streaming && !m.thinking).count(),
        1
    );
    assert!(!next[0].streaming);
    assert!(next[2].body == "ok after steer" && next[2].streaming);
    let next = apply_stream(
        &[
            m! {body: "working", streaming: true},
            m! {me: true, body: "steer", optimistic: true},
        ],
        StreamKind::Text,
        "ok after steer",
        &StreamOpts::default(),
    );
    assert_eq!(
        next.iter().filter(|m| m.streaming && !m.thinking).count(),
        1
    );
    assert!(!next[0].streaming);
    assert!(next[2].streaming);
}

#[test]
fn add_user_notices() {
    let list = add_user(
        &[m! {body: "x", streaming: true}],
        &format!("{DISPLAY_HIDDEN_SENTINEL}Explore done"),
        NOW,
    );
    assert_eq!(
        list[1],
        m! {note: true, note_label: "Subagent", body: "Explore done", ts: NOW}
    );
    assert!(!list[0].streaming);
    assert_eq!(add_user(&[], DISPLAY_HIDDEN_SENTINEL, NOW), []);
    assert_eq!(subagent_notice_label("plain"), None);
}

const GROK_TODO: &str =
    "call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0";
const GROK_README: &str =
    "call-9812a46f-b258-4ed8-8d09-55441deb1733-6\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_1";
const GROK_MAKEFILE: &str =
    "call-9812a46f-b258-4ed8-8d09-55441deb1733-7\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_2";

fn step() -> WorkspaceStep {
    WorkspaceStep {
        id: GROK_TODO.into(),
        tool: "Glob".into(),
        tool_input: "TODO.md".into(),
        input: r#"{"globPattern":"TODO.md"}"#.into(),
        ..WorkspaceStep::default()
    }
}

fn no_extras() -> ToolExtras {
    ToolExtras::default()
}

#[test]
fn repeated_steps_stay_one_row() {
    let mut list = Vec::new();
    for _ in 0..5 {
        list = add_tool(&list, &step());
    }
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].tool_id, GROK_TODO);
    assert!(list[0].result_pending);
    assert_eq!(list[0].input, r#"{"globPattern":"TODO.md"}"#);
    let list = add_tool(
        &[m! {tool: "Glob", tool_input: "TODO.md", result_pending: true}],
        &step(),
    );
    assert_eq!(list.len(), 1);
    assert!(list[0].tool_id == GROK_TODO && list[0].result_pending);
}

#[test]
fn parallel_steps_stay_distinct() {
    let mut list = add_tool(&[], &step());
    list = add_tool(
        &list,
        &WorkspaceStep {
            id: GROK_README.into(),
            tool_input: "README*".into(),
            input: r#"{"globPattern":"README*"}"#.into(),
            ..step()
        },
    );
    list = add_tool(
        &list,
        &WorkspaceStep {
            id: GROK_MAKEFILE.into(),
            tool_input: "Makefile".into(),
            input: r#"{"globPattern":"Makefile"}"#.into(),
            ..step()
        },
    );
    assert_eq!(
        list.iter().map(|m| m.tool_id.as_str()).collect::<Vec<_>>(),
        [GROK_TODO, GROK_README, GROK_MAKEFILE]
    );
    assert_eq!(
        list.iter().map(|m| m.input.as_str()).collect::<Vec<_>>(),
        [
            r#"{"globPattern":"TODO.md"}"#,
            r#"{"globPattern":"README*"}"#,
            r#"{"globPattern":"Makefile"}"#
        ]
    );
}

#[test]
fn blank_id_steps() {
    let blank = |tool_input: &str, input: &str| WorkspaceStep {
        id: String::new(),
        tool: "Glob".into(),
        tool_input: tool_input.into(),
        input: input.into(),
        ..WorkspaceStep::default()
    };
    let mut list = add_tool(&[], &blank("**/.golangci.*", ""));
    list = add_tool(&list, &blank("**/.golangci.*", ""));
    list = add_tool(&list, &blank("", r#"{"pattern":"**/.golangci.*"}"#));
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].input, r#"{"pattern":"**/.golangci.*"}"#);

    let across_me = add_tool(
        &[
            m! {tool: "Glob", result_pending: true, tool_input: "old"},
            m! {me: true, body: "next"},
        ],
        &blank("new", ""),
    );
    assert_eq!(across_me.len(), 3);
    assert_eq!(
        (
            across_me[0].tool_input.as_str(),
            across_me[2].tool_input.as_str()
        ),
        ("old", "new")
    );
    let across_end = add_tool(
        &[
            m! {tool: "Read", result_pending: true, tool_input: "old.ts"},
            m! {turn_end: true, duration_ms: 12},
        ],
        &WorkspaceStep {
            tool: "Read".into(),
            ..blank("new.ts", "")
        },
    );
    assert_eq!(across_end.len(), 3);
    assert_eq!(across_end[2].tool_input, "new.ts");

    let mut list = add_tool(&[], &blank("**/.golangci.*", ""));
    list.push(m! {me: true, body: "follow-up", queued: true, pending_id: "p1"});
    list = add_tool(&list, &blank("", r#"{"pattern":"**/.golangci.*"}"#));
    assert_eq!(list.iter().filter(|m| m.tool == "Glob").count(), 1);
    assert_eq!(list[0].input, r#"{"pattern":"**/.golangci.*"}"#);
    assert!(list[1].queued);
}

#[test]
fn attach_tool_results() {
    let clones = vec![
        m! {tool: "Glob", tool_id: GROK_TODO, result_pending: true, input: r#"{"globPattern":"TODO.md"}"#},
        m! {tool: "Glob", tool_id: GROK_TODO, result_pending: true, input: r#"{"globPattern":"TODO.md"}"#},
        m! {thinking: true, body: "next"},
    ];
    let out = attach_tool_result(&clones, GROK_TODO, "./TODO.md", false, &no_extras());
    let globs: Vec<&AgentMsg> = out.iter().filter(|m| m.tool == "Glob").collect();
    assert_eq!(globs.len(), 1);
    assert!(globs[0].result == "./TODO.md" && !globs[0].result_pending);
    assert!(out.iter().any(|m| m.thinking));

    let list = vec![
        m! {tool: "Glob", result_pending: true, input: r#"{"globPattern":"**"}"#},
        m! {tool: "Read", result_pending: true, input: r#"{"path":"README.md"}"#},
    ];
    let out = attach_tool_result(&list, "", "file contents", false, &no_extras());
    assert_eq!(out.len(), 2);
    assert!(out[1].result == "file contents" && !out[1].result_pending);
    assert!(out[0].result_pending);

    let mut list = add_tool(&[], &step());
    list = attach_tool_result(&list, GROK_TODO, "./TODO.md", false, &no_extras());
    assert!(!list[0].result_pending);
    list = add_tool(&list, &step());
    assert_eq!(list.len(), 2, "a settled row is not resurrected");
    assert!(list[0].result == "./TODO.md" && !list[0].result_pending);
    assert!(list[1].tool_id == GROK_TODO && list[1].result_pending);

    let refreshed = attach_tool_result(
        &[m! {tool: "x", tool_id: "t", input: "{}"}],
        "t",
        "ok",
        true,
        &ToolExtras {
            input: r#"{"q":1}"#.into(),
            tool_input: "q".into(),
        },
    );
    assert!(
        refreshed[0].result_error
            && refreshed[0].input == r#"{"q":1}"#
            && refreshed[0].tool_input == "q"
    );
    assert_eq!(
        attach_tool_result(&clones, "missing", "x", false, &no_extras()),
        clones
    );
}

#[test]
fn step_row_keys() {
    assert_ne!(step_row_key(GROK_TODO, 0), step_row_key(GROK_TODO, 1));
    assert_eq!(step_row_key("", 3), "i3");
}

fn nested(id: &str, tool: &str, parent: &str) -> WorkspaceStep {
    WorkspaceStep {
        id: id.into(),
        tool: tool.into(),
        parent_id: parent.into(),
        ..step()
    }
}

#[test]
fn nested_subagent_rows() {
    let mut list = add_tool(
        &[],
        &WorkspaceStep {
            description: "Map UI".into(),
            input: r#"{"prompt":"search"}"#.into(),
            ..nested("toolu_task_1", "Task", "")
        },
    );
    list = add_tool(
        &list,
        &WorkspaceStep {
            tool_input: "PRDetail".into(),
            input: r#"{"pattern":"PRDetail"}"#.into(),
            ..nested("t_nested", "Grep", "toolu_task_1")
        },
    );
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].tool, "Task");
    assert_eq!(list[0].children.len(), 1);
    let c = &list[0].children[0];
    assert!(c.tool == "Grep" && c.tool_id == "t_nested" && c.result_pending);

    let list = add_tool(&[], &nested("toolu_task_1", "Task", ""));
    let streamed = apply_stream(
        &list,
        StreamKind::Text,
        "Found foo.go",
        &StreamOpts {
            done: true,
            parent_id: "toolu_task_1".into(),
            ..StreamOpts::default()
        },
    );
    assert_eq!(streamed.len(), 1);
    assert!(streamed[0].body.is_empty());
    assert_eq!(streamed[0].children[0], m! {body: "Found foo.go"});

    let mut list = add_tool(&list, &nested("t_nested", "Grep", "toolu_task_1"));
    list = attach_tool_result(&list, "t_nested", "src/PR.svelte", false, &no_extras());
    assert_eq!(list.len(), 1);
    assert!(list[0].result_pending);
    assert!(list[0].children[0].result == "src/PR.svelte" && !list[0].children[0].result_pending);

    assert!(add_tool(&[], &nested("t_nested", "Grep", "toolu_missing")).is_empty());
    let ticked = note_thinking_tokens(&list, 7, "", "toolu_task_1");
    assert_eq!(ticked[0].children.last().map(|c| c.tokens), Some(7));
}

#[test]
fn patch_tool_input_refreshes_args_only() {
    let list = vec![m! {tool: "ask_user", tool_id: "t", result_pending: true, input: "{}"}];
    let out = patch_tool_input(
        &list,
        "t",
        &ToolExtras {
            input: r#"{"question":"Q"}"#.into(),
            tool_input: String::new(),
        },
    );
    assert!(out[0].input == r#"{"question":"Q"}"# && out[0].result_pending);
    assert_eq!(
        patch_tool_input(
            &list,
            "t",
            &ToolExtras {
                input: "{}".into(),
                ..no_extras()
            }
        ),
        list
    );
    assert_eq!(
        patch_tool_input(
            &list,
            "missing",
            &ToolExtras {
                tool_input: "x".into(),
                ..no_extras()
            }
        ),
        list
    );
}

#[test]
fn drop_optimistic_users() {
    let list = vec![
        m! {me: true, body: "steer", optimistic: true, pending_id: "p1"},
        m! {turn_end: true, duration_ms: 1200},
    ];
    assert_eq!(
        drop_optimistic_user(&list, "steer", "p1"),
        [m! {turn_end: true, duration_ms: 1200}]
    );
    let list = vec![
        m! {me: true, body: "steer", optimistic: true},
        m! {body: "working"},
    ];
    assert_eq!(
        drop_optimistic_user(&list, "steer", ""),
        [m! {body: "working"}]
    );
}

#[test]
fn rejected_steer_drafts() {
    let v = |xs: &[&str]| xs.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert_eq!(merge_rejected_steer_draft(&v(&["A"]), "C"), "A\n\nC");
    assert_eq!(
        merge_rejected_steer_draft(&v(&["A", "B"]), "C"),
        "A\n\nB\n\nC"
    );
    assert_eq!(
        merge_rejected_steer_draft(&v(&["A", "B"]), "B\n\nC"),
        "A\n\nB\n\nC"
    );
    assert_eq!(
        merge_rejected_steer_draft(&v(&["A1\n\nA2", "B"]), "A1\n\nA2"),
        "A1\n\nA2\n\nB"
    );
    assert_eq!(
        merge_rejected_steer_draft(&v(&["A1\n\nA2", "B"]), "A1\n\nA2\n\nC"),
        "A1\n\nA2\n\nB\n\nC"
    );
    let got =
        merge_rejected_steer_attachments(&[v(&["a", "b"])], &v(&["b", "c", ""]), |s: &String| {
            s.as_str()
        });
    assert_eq!(got, ["a", "b", "c"]);
}

#[test]
fn task_and_subagent_helpers() {
    assert!(is_task_tool("Task"));
    assert!(is_task_tool("task"));
    assert!(is_task_tool("Agent"));
    assert!(!is_task_tool("Bash"));
    assert!(!is_task_tool(""));
    let ack = "Async agent launched successfully.\n(This tool result is internal metadata – never quote or paste any part of it, including the agentId below, into a user-facing reply.)\n\nagentId: a2a1c3c855409bb10\nUse SendMessage with to: \"a2a1c3c855409bb10\"";
    assert!(is_subagent_launch_meta(ack));
    assert!(!is_subagent_launch_meta(
        "Explored the PR detail Svelte view; tabs live in PRDetail.svelte."
    ));
    assert!(!is_subagent_launch_meta(""));
    assert!(!is_subagent_launch_meta(
        "see Async agent launched successfully plus agentId and internal metadata in comments"
    ));
}

#[test]
fn collapse_duplicate_users() {
    let list = vec![
        m! {me: true, body: "go", optimistic: true},
        m! {tool: "Read"},
        m! {me: true, body: "go"},
    ];
    assert_eq!(
        collapse_duplicate_users_in_turn(&list),
        [m! {me: true, body: "go"}, m! {tool: "Read"}]
    );
    let resend = vec![
        m! {me: true, body: "go", optimistic: true},
        m! {body: "answer"},
        m! {me: true, body: "go"},
    ];
    assert_eq!(collapse_duplicate_users_in_turn(&resend), resend);
}

#[test]
fn send_attachments_and_ids() {
    let got = to_send_attachments(&[
        ChatAttachmentDraft {
            id: "a".into(),
            name: "a.png".into(),
            mime: "image/png".into(),
            data_url: "data:image/png;base64,AAA".into(),
        },
        ChatAttachmentDraft {
            id: "b".into(),
            data_url: "BBB".into(),
            ..ChatAttachmentDraft::default()
        },
    ]);
    assert_eq!((got[0].data.as_str(), got[1].data.as_str()), ("AAA", "BBB"));
    let id = new_attachment_id(NOW);
    let parts: Vec<&str> = id.split('_').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "img");
    assert_eq!(i64::from_str_radix(parts[1], 36).unwrap(), NOW);
    assert_eq!(parts[2].len(), 6);
    assert_ne!(new_attachment_id(NOW), id);
}
