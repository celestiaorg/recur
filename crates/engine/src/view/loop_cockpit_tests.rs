use pretty_assertions::assert_eq;
use recur_domain::{FeedItem, PrSource};

use super::*;
use crate::view::chat::{AgentMsg, ChatLog, ChatLogSession};

fn msg(body: &str, ts: Option<i64>) -> AgentMsg {
    AgentMsg {
        body: body.into(),
        ts,
        ..AgentMsg::default()
    }
}

fn me(body: &str, ts: Option<i64>) -> AgentMsg {
    AgentMsg {
        me: true,
        ..msg(body, ts)
    }
}

fn turn_end(ts: i64) -> AgentMsg {
    AgentMsg {
        turn_end: true,
        duration_ms: 1,
        ts: Some(ts),
        ..AgentMsg::default()
    }
}

fn thinking(body: &str, ts: Option<i64>) -> AgentMsg {
    AgentMsg {
        thinking: true,
        ..msg(body, ts)
    }
}

fn sess(id: &str, code: &str, name: &str, ts: i64, messages: Vec<AgentMsg>) -> FeedSess {
    FeedSess {
        id: id.into(),
        code: code.into(),
        name: name.into(),
        kind: name.to_lowercase(),
        ts,
        live_ts: None,
        messages,
    }
}

fn fact(id: &str, text: &str, ts: i64) -> FeedItem {
    FeedItem {
        id: id.into(),
        text: text.into(),
        ts,
        ..FeedItem::default()
    }
}

fn agent_fact(id: &str, text: &str, ts: i64, name: &str, code: &str) -> FeedItem {
    FeedItem {
        name: name.into(),
        agent_code: code.into(),
        ..fact(id, text, ts)
    }
}

fn live(codes: &[&str]) -> Vec<String> {
    codes.iter().map(|c| (*c).to_owned()).collect()
}

/// `m.body || m.log?.kind` (and `m.tool`, `factText`) as the TS tests read.
fn label(m: &AgentMsg) -> String {
    if !m.body.is_empty() {
        return m.body.clone();
    }
    if !m.tool.is_empty() {
        return m.tool.clone();
    }
    match m.log.as_deref() {
        Some(ChatLog::Fact { .. }) => "fact".into(),
        Some(ChatLog::Session(_)) => "session".into(),
        Some(ChatLog::Sessions { .. }) => "sessions".into(),
        None if m.turn_end => "completed".into(),
        None => String::new(),
    }
}

fn labels(v: &[AgentMsg]) -> Vec<String> {
    v.iter().map(label).collect()
}

/// `m.body || factText(m)`.
fn texts(v: &[AgentMsg]) -> Vec<String> {
    v.iter()
        .map(|m| match m.log.as_deref() {
            Some(ChatLog::Fact { text, .. }) if m.body.is_empty() => text.clone(),
            _ if m.body.is_empty() => m.tool.clone(),
            _ => m.body.clone(),
        })
        .collect()
}

fn session(m: &AgentMsg) -> &ChatLogSession {
    match m.log.as_deref() {
        Some(ChatLog::Session(s)) => s,
        other => panic!("not a session: {other:?}"),
    }
}

fn bodies(s: &ChatLogSession) -> Vec<String> {
    s.messages.iter().map(|m| m.body.clone()).collect()
}

#[test]
fn loop_cockpit_tab() {
    assert!(is_loop_cockpit_tab("MA", false));
    assert!(!is_loop_cockpit_tab("PA", false));
    assert!(is_loop_cockpit_tab("PA", true));
    assert!(is_loop_cockpit_tab("PA2", true));
}

#[test]
fn orders_maestro_fact_specialist_and_skips_pa() {
    let merged = merge_loop_cockpit(
        &[me("go", Some(1000)), msg("on it", Some(4000))],
        &[
            sess("s-co", "CO", "Coder", 3000, vec![msg("edited", None)]),
            sess("s-pa", "PA", "Agent", 2500, vec![msg("human chat", None)]),
            sess("s-ma", "MA", "Maestro", 500, vec![msg("dup", None)]),
        ],
        &[fact("f1", "Converged.", 2000)],
        &live(&["CO"]),
        "",
    );
    assert_eq!(labels(&merged), ["go", "fact", "session", "on it"]);
    match merged[1].log.as_deref() {
        Some(ChatLog::Fact {
            text, code, name, ..
        }) => assert_eq!(
            (text.as_str(), code.as_str(), name.as_str()),
            ("Converged.", "MA", "Maestro")
        ),
        other => panic!("{other:?}"),
    }
    let s = session(&merged[2]);
    assert_eq!(
        (s.id.as_str(), s.code.as_str(), s.live),
        ("s-co#0", "CO", true)
    );
    assert!(
        !merged
            .iter()
            .any(|m| matches!(m.log.as_deref(), Some(ChatLog::Session(s)) if s.code == "PA"))
    );
}

#[test]
fn splits_one_session_into_a_card_per_turn() {
    let merged = merge_loop_cockpit(
        &[me("go", Some(500)), msg("coder round 2", Some(2500))],
        &[sess(
            "s-co",
            "CO",
            "Coder",
            1000,
            vec![
                me("fix rotation", Some(1000)),
                msg("first pass", Some(1100)),
                turn_end(1200),
                me("fix again", Some(3000)),
                msg("second pass", Some(3100)),
            ],
        )],
        &[],
        &live(&["CO"]),
        "",
    );
    let cards: Vec<&ChatLogSession> = merged
        .iter()
        .filter_map(|m| match m.log.as_deref() {
            Some(ChatLog::Session(s)) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 2);
    assert_eq!((cards[0].id.as_str(), cards[0].live), ("s-co#0", false));
    assert!(cards[0].messages.contains(&msg("first pass", Some(1100))));
    assert!(!bodies(cards[0]).contains(&"second pass".to_owned()));
    assert_eq!((cards[1].id.as_str(), cards[1].live), ("s-co#1", true));
    assert!(bodies(cards[1]).contains(&"second pass".to_owned()));
    assert_eq!(
        labels(&merged),
        ["go", "session", "coder round 2", "session"]
    );
}

#[test]
fn keeps_queued_follow_ups_at_the_end() {
    let queued = AgentMsg {
        queued: true,
        pending_id: "p1".into(),
        ..me("later", None)
    };
    let merged = merge_loop_cockpit(&[msg("hi", Some(1)), queued.clone()], &[], &[], &[], "");
    assert_eq!(merged.last(), Some(&queued));
}

#[test]
fn pins_the_opening_prompt_before_start_facts() {
    let merged = merge_loop_cockpit(
        &[
            me("create a cli game of pong", Some(3000)),
            thinking("inspect the repo", Some(3100)),
        ],
        &[],
        &[
            fact("f1", "Starting loop for: create a cli game of pong", 1000),
            fact("f2", "Converged.", 2000),
        ],
        &[],
        "",
    );
    assert_eq!(
        texts(&merged),
        [
            "create a cli game of pong",
            "Starting loop for: create a cli game of pong",
            "Converged.",
            "inspect the repo",
        ]
    );
}

#[test]
fn keeps_a_live_maestro_turn_in_array_order() {
    let merged = merge_loop_cockpit(
        &[
            me("pong", Some(500)),
            msg("planned", Some(800)),
            me("so we're done?", Some(5000)),
            thinking("check the PR", None),
            msg("I'll check the worktree", None),
            AgentMsg {
                tool: "Read".into(),
                description: "Read file".into(),
                ..AgentMsg::default()
            },
            AgentMsg {
                streaming: true,
                ..thinking("settled", None)
            },
        ],
        &[],
        &[
            fact("f1", "Starting loop for: pong", 1000),
            fact("f2", "Planning the change.", 2000),
        ],
        &[],
        "",
    );
    assert_eq!(
        texts(&merged),
        [
            "pong",
            "planned",
            "Starting loop for: pong",
            "Planning the change.",
            "so we're done?",
            "check the PR",
            "I'll check the worktree",
            "Read",
            "settled",
        ]
    );
}

#[test]
fn start_facts_precede_a_live_turn_with_no_user_bubble() {
    let merged = merge_loop_cockpit(
        &[
            AgentMsg {
                streaming: true,
                ..thinking("inspect the repo", None)
            },
            msg("I'll inspect the repository", None),
        ],
        &[],
        &[
            fact("f1", "Starting loop for: pong", 1000),
            fact("f2", "Planning the change.", 2000),
        ],
        &[],
        "create a cli game of pong",
    );
    assert_eq!(
        texts(&merged),
        [
            "create a cli game of pong",
            "Starting loop for: pong",
            "Planning the change.",
            "inspect the repo",
            "I'll inspect the repository",
        ]
    );
}

#[test]
fn goal_bubble_stays_ahead_of_an_optimistic_answer() {
    let goal = "create a cli game of pong";
    let now = chrono::Utc::now().timestamp_millis();
    let merged = merge_loop_cockpit(
        &[AgentMsg {
            optimistic: true,
            ..me("Approve and proceed", Some(now))
        }],
        &[],
        &[
            fact("f1", "Starting loop for: create a cli game of pong", 1000),
            fact("f2", "Converged.", 2000),
        ],
        &[],
        goal,
    );
    assert_eq!(
        texts(&merged),
        [
            goal,
            "Starting loop for: create a cli game of pong",
            "Converged.",
            "Approve and proceed",
        ]
    );
}

#[test]
fn does_not_duplicate_an_optimistic_goal_seed() {
    let goal = "create a cli game of pong";
    let merged = merge_loop_cockpit(
        &[AgentMsg {
            optimistic: true,
            ..me(goal, None)
        }],
        &[],
        &[],
        &[],
        goal,
    );
    let users: Vec<&str> = merged
        .iter()
        .filter(|m| m.me)
        .map(|m| m.body.as_str())
        .collect();
    assert_eq!(users, [goal]);
}

#[test]
fn completed_in_stays_after_its_turn() {
    let merged = merge_loop_cockpit(
        &[
            me("pong", Some(100)),
            me("so we're done?", Some(5000)),
            thinking("blast", Some(5100)),
            msg("I'll check", Some(5200)),
            AgentMsg {
                tool: "Read".into(),
                ts: Some(5300),
                ..AgentMsg::default()
            },
            AgentMsg {
                duration_ms: 17000,
                ..turn_end(5100)
            },
            thinking("settled", Some(5400)),
            msg("Yes. The loop has settled.", Some(5500)),
        ],
        &[],
        &[],
        &[],
        "",
    );
    assert_eq!(
        labels(&merged),
        [
            "pong",
            "so we're done?",
            "blast",
            "I'll check",
            "Read",
            "completed",
            "settled",
            "Yes. The loop has settled.",
        ]
    );
}

#[test]
fn specialist_session_excludes_maestro_and_pair_tabs() {
    assert!(is_specialist_session("CO"));
    assert!(!is_specialist_session("MA"));
    assert!(!is_specialist_session("PA2"));
    assert!(is_pair_chat_code("PA") && is_pair_chat_code("PA1") && !is_pair_chat_code("PAX"));
}

#[test]
fn live_codes_names_and_modes() {
    let mut got = specialist_live_codes("ws1", ["ws1-CO", "ws1-MA", "ws2-RV", "ws1-PA"], ["QA"]);
    got.sort();
    assert_eq!(got, ["CO", "QA"]);
    assert_eq!(specialist_name("CO"), "Coder");
    assert_eq!(specialist_name("RV"), "Reviewer");
    assert_eq!(specialist_name("PL"), "Planner");
    assert_eq!(specialist_name("SR"), "Security reviewer");
    for (code, mode) in [
        ("CO", "agent"),
        ("RV", "review"),
        ("PL", "plan"),
        ("SR", "review"),
        ("QA", "ask"),
        ("WR", "agent"),
        ("ED", "review"),
    ] {
        assert_eq!(specialist_mode(code), mode, "{code}");
    }
}

#[test]
fn inserts_a_working_card_for_a_live_specialist_without_a_session() {
    let merged = merge_loop_cockpit(&[me("go", Some(100))], &[], &[], &live(&["CO"]), "");
    let card = merged
        .iter()
        .find_map(|m| match m.log.as_deref() {
            Some(ChatLog::Session(s)) => Some(s),
            _ => None,
        })
        .expect("a card");
    assert_eq!(
        card,
        &ChatLogSession {
            id: "live-CO#0".into(),
            code: "CO".into(),
            name: "Coder".into(),
            messages: vec![],
            live: true,
        }
    );
}

#[test]
fn omits_phase_kickoff_facts() {
    let merged = merge_loop_cockpit(
        &[me("go", Some(100))],
        &[],
        &[agent_fact(
            "f-rv",
            "Reviewing the diff (iteration 1).",
            2000,
            "Reviewer",
            "RV",
        )],
        &live(&["RV"]),
        "",
    );
    assert_eq!(labels(&merged), ["go", "session"]);
    assert!(is_phase_kickoff_fact("Reviewing the diff (iteration 1)."));
    assert!(is_phase_kickoff_fact(
        "Planning the change and definition-of-done."
    ));
    assert!(!is_phase_kickoff_fact(
        "Converged after 1 iteration(s): 0 open findings."
    ));
}

#[test]
fn folds_commit_and_handoff_facts_into_the_specialist_timeline() {
    let commit = "Committed <code>abc</code> (+1 −0) to <code>main</code>.";
    let merged = merge_loop_cockpit(
        &[me("go", Some(100))],
        &[sess(
            "s-co",
            "CO",
            "Coder",
            1000,
            vec![msg("working", Some(1100)), msg("done", Some(2500))],
        )],
        &[
            agent_fact("f-c", commit, 2000, "Coder", "CO"),
            agent_fact(
                "f-h",
                "Handoff: raised the breakpoint.",
                2600,
                "Coder",
                "CO",
            ),
            fact("f-m", "Converged.", 3000),
        ],
        &[],
        "",
    );
    assert_eq!(labels(&merged), ["go", "session", "fact"]);
    let note = |label: &str, body: &str, ts: i64| AgentMsg {
        note: true,
        note_label: label.into(),
        note_html: true,
        ..msg(body, Some(ts))
    };
    assert_eq!(
        session(&merged[1]).messages,
        vec![
            msg("working", Some(1100)),
            note("Commit", commit, 2000),
            msg("done", Some(2500)),
            note("Handoff", "Handoff: raised the breakpoint.", 2600),
        ]
    );
    assert!(
        matches!(merged[2].log.as_deref(), Some(ChatLog::Fact { text, .. }) if text == "Converged.")
    );
    assert!(is_specialist_wrap_fact("Committed abc (+1 −0) to main."));
    assert!(is_specialist_wrap_fact("Handoff: raised the breakpoint."));
    assert!(!is_specialist_wrap_fact("Converged."));
}

#[test]
fn packs_consecutive_specialists_into_one_group() {
    let merged = merge_loop_cockpit(
        &[me("go", Some(100))],
        &[
            sess("s-co", "CO", "Coder", 1000, vec![msg("done", Some(1100))]),
            sess("s-rv", "RV", "Reviewer", 2000, vec![msg("ok", Some(2100))]),
        ],
        &[fact("f-m", "Converged.", 3000)],
        &[],
        "",
    );
    assert_eq!(labels(&merged), ["go", "sessions", "fact"]);
    match merged[1].log.as_deref() {
        Some(ChatLog::Sessions { id, sessions }) => {
            assert_eq!(id, "s-co#0");
            let got: Vec<(&str, &str)> = sessions
                .iter()
                .map(|s| (s.code.as_str(), s.name.as_str()))
                .collect();
            assert_eq!(got, [("CO", "Coder"), ("RV", "Reviewer")]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn does_not_pack_specialists_split_by_a_fact() {
    let merged = merge_loop_cockpit(
        &[me("go", Some(100))],
        &[
            sess("s-rv", "RV", "Reviewer", 1000, vec![msg("ok", Some(1100))]),
            sess("s-co", "CO", "Coder", 3000, vec![msg("done", Some(3100))]),
        ],
        &[fact("f-m", "Converged.", 2000)],
        &[],
        "",
    );
    assert_eq!(labels(&merged), ["go", "session", "fact", "session"]);
}

#[test]
fn first_turn_id_stays_stable_when_a_second_dispatch_lands() {
    let base = sess(
        "s-co",
        "CO",
        "Coder",
        1000,
        vec![msg("first", Some(1100)), turn_end(1200)],
    );
    let one = merge_loop_cockpit(&[], std::slice::from_ref(&base), &[], &[], "");
    let s = session(&one[0]);
    assert_eq!((s.id.as_str(), s.live), ("s-co#0", false));
    let mut more = base.clone();
    more.messages
        .extend([me("again", Some(2000)), msg("second", Some(2100))]);
    let two = merge_loop_cockpit(&[], &[more], &[], &live(&["CO"]), "");
    match two[0].log.as_deref() {
        Some(ChatLog::Sessions { id, sessions }) => {
            assert_eq!(id, "s-co#0");
            let got: Vec<(&str, bool)> = sessions.iter().map(|s| (s.id.as_str(), s.live)).collect();
            assert_eq!(got, [("s-co#0", false), ("s-co#1", true)]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn live_follow_up_without_timestamps_sits_after_later_maestro_rows() {
    let merged = merge_loop_cockpit(
        &[
            me("go", Some(100)),
            msg("delegated", Some(200)),
            me("approved", Some(5000)),
        ],
        &[FeedSess {
            live_ts: Some(6000),
            ..sess(
                "s-co",
                "CO",
                "Coder",
                300,
                vec![
                    msg("first", Some(300)),
                    turn_end(400),
                    AgentMsg {
                        streaming: true,
                        ..msg("second", None)
                    },
                ],
            )
        }],
        &[],
        &live(&["CO"]),
        "",
    );
    assert_eq!(
        labels(&merged),
        ["go", "delegated", "session", "approved", "session"]
    );
    let cards: Vec<&AgentMsg> = merged
        .iter()
        .filter(|m| matches!(m.log.as_deref(), Some(ChatLog::Session(_))))
        .collect();
    assert_eq!(
        (session(cards[0]).id.as_str(), session(cards[0]).live),
        ("s-co#0", false)
    );
    assert_eq!(
        (session(cards[1]).id.as_str(), session(cards[1]).live),
        ("s-co#1", true)
    );
    assert!(bodies(session(cards[1])).contains(&"second".to_owned()));
}

#[test]
fn maestro_resume_stays_after_the_human_answer() {
    let merged = merge_loop_cockpit(
        &[
            me("go", Some(100)),
            thinking("need a decision", Some(200)),
            AgentMsg {
                optimistic: true,
                ..me("use notes", Some(5000))
            },
            AgentMsg {
                streaming: true,
                ..thinking("resuming", None)
            },
        ],
        &[],
        &[],
        &[],
        "",
    );
    assert_eq!(
        labels(&merged),
        ["go", "need a decision", "use notes", "resuming"]
    );
}

#[test]
fn live_turn_break() {
    assert!(needs_live_turn_break(&[msg("done", None)]));
    assert!(!needs_live_turn_break(&[AgentMsg {
        streaming: true,
        ..msg("…", None)
    }]));
    assert!(!needs_live_turn_break(&[me("fix again", None)]));
    assert!(!needs_live_turn_break(&[AgentMsg {
        turn_end: true,
        ..AgentMsg::default()
    }]));
    assert!(!needs_live_turn_break(&[]));
}

fn pr_fact(
    number: i64,
    pr_id: Option<&str>,
    duration: Option<&str>,
    source: PrSource,
) -> LoopPrFact {
    LoopPrFact {
        number,
        pr_id: pr_id.map(str::to_owned),
        duration: duration.map(str::to_owned),
        source,
    }
}

#[test]
fn parse_loop_pr_facts() {
    assert_eq!(
        parse_loop_pr_fact(
            "Opened a Local PR <code>#157</code> (+721 −0 across 8 files) — ready for your review."
        ),
        Some(pr_fact(157, None, None, PrSource::Local))
    );
    assert_eq!(
        parse_loop_pr_fact(
            r#"Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>"#
        ),
        Some(pr_fact(157, Some("p1"), Some("12m 04s"), PrSource::Local))
    );
    assert_eq!(
        parse_loop_pr_fact(r#"<a class="loop-pr-link" data-pr="p1">Local PR #12</a>"#),
        Some(pr_fact(12, Some("p1"), None, PrSource::Local))
    );
    assert_eq!(
        parse_loop_pr_fact("Converged after 2 iteration(s): 0 open findings."),
        None
    );
    assert_eq!(
        parse_loop_pr_fact("Could not open the PR: gh missing"),
        None
    );
}

#[test]
fn github_fallback_note_is_not_an_opened_pr() {
    let html = "GitHub PR not opened (gh pr create failed: a pull request already exists: https://github.com/acme/r/pull/39) — opening a Local PR instead.";
    assert_eq!(parse_loop_pr_fact(html), None);
    let ctx = PrCompletionCtx {
        fact_ts: 1000 + 37 * 60_000 + 11_000,
        start_ts: Some(1000),
        ..PrCompletionCtx::default()
    };
    assert_eq!(loop_pr_completion(html, Some(&ctx)), None);
}

#[test]
fn wrap_body_prose_does_not_collide_with_pr_parsing() {
    let wrap = |body: &str| format!(r#"<div class="loop-wrap">{body}</div>"#);
    let reviewed = format!(
        "Completed in 12s.{}",
        wrap("<b>Done</b> Reviewed PR #39 and recorded the findings")
    );
    assert_eq!(parse_loop_pr_fact(&reviewed), None);
    assert_eq!(loop_pr_completion(&reviewed, None), None);
    assert_eq!(
        parse_loop_pr_fact(&format!(
            r#"Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">GitHub PR #157</a>{}"#,
            wrap("<b>Done</b> Fixed Local PR rendering")
        )),
        Some(pr_fact(157, Some("p1"), Some("12m 04s"), PrSource::GitHub))
    );
    assert_eq!(
        parse_loop_pr_fact(&format!(
            r#"Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>{}"#,
            wrap("<b>Notes</b> Settings dialog was not opened during testing")
        )),
        Some(pr_fact(157, Some("p1"), Some("12m 04s"), PrSource::Local))
    );
}

#[test]
fn format_loop_elapsed_ms_cases() {
    assert_eq!(format_loop_elapsed_ms(3000), "3s");
    assert_eq!(format_loop_elapsed_ms(90_000), "1m 30s");
    assert_eq!(format_loop_elapsed_ms(5 * 60_000), "5m");
    assert_eq!(format_loop_elapsed_ms(62 * 60_000), "1h 2m");
    assert_eq!(format_loop_elapsed_ms(12 * 60_000 + 4_000), "12m 04s");
    assert_eq!(format_loop_elapsed_ms(0), "");
}

#[test]
fn loop_pr_completions() {
    let completion = |duration: &str, pr_id: Option<&str>, label: &str| LoopPrCompletion {
        duration: duration.into(),
        pr_id: pr_id.map(str::to_owned),
        pr_label: label.into(),
    };
    assert_eq!(
        loop_pr_completion(
            r#"Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>"#,
            None
        ),
        Some(completion("12m 04s", Some("p1"), "Local PR #157"))
    );
    let legacy =
        "Opened a Local PR <code>#157</code> (+721 −0 across 8 files) — ready for your review.";
    let ctx = PrCompletionCtx {
        prs: vec![PrRef {
            id: "p1".into(),
            number: 157,
            workspace_id: "ws1".into(),
        }],
        workspace_id: "ws1".into(),
        fact_ts: 1000 + 12 * 60_000 + 4_000,
        start_ts: Some(1000),
    };
    assert_eq!(
        loop_pr_completion(legacy, Some(&ctx)),
        Some(completion("12m 04s", Some("p1"), "Local PR #157"))
    );
    let sentinel = PrCompletionCtx {
        fact_ts: chrono::Utc::now().timestamp_millis(),
        start_ts: Some(1),
        ..PrCompletionCtx::default()
    };
    assert_eq!(
        loop_pr_completion(legacy, Some(&sentinel)),
        Some(completion("", None, "Local PR #157")),
        "ts=1 is the opening-prompt sentinel, not a loop start"
    );
}

#[test]
fn loop_completion_wraps() {
    let wrap = r#"<div class="loop-wrap"><b>Issue</b> ACME-1 — Flaky checkout<br><b>Done</b> Retry the token</div>"#;
    let head = r#"Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>"#;
    let html = format!("{head}{wrap}");
    assert_eq!(
        loop_completion_wrap(&html),
        "<b>Issue</b> ACME-1 — Flaky checkout<br><b>Done</b> Retry the token"
    );
    assert_eq!(strip_loop_wrap(&html), head);
    assert_eq!(
        parse_loop_pr_fact(&html),
        Some(pr_fact(157, Some("p1"), Some("12m 04s"), PrSource::Local))
    );
    assert_eq!(loop_completion_wrap("Completed in 12s."), "");
    assert_eq!(strip_loop_wrap("Completed in 12s."), "Completed in 12s.");
}

/// The engine's wrap HTML is what the cockpit splits back out.
#[test]
fn engine_wrap_round_trips() {
    let wrap = crate::loops::wrap_up::loop_wrap_up_html("Root cause: nil deref.\nGuarded it.");
    let html = format!("Completed in 3s.{wrap}");
    assert_eq!(strip_loop_wrap(&html), "Completed in 3s.");
    assert!(loop_completion_wrap(&html).contains("Root cause: nil deref."));
}
