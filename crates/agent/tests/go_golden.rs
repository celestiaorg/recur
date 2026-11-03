//! Snapshot answers for the text and display rules: constants, feed
//! summaries, chat detail bounds, listing compaction, failure classification,
//! attachment blocks. JSON outputs compare as values. Edit
//! `golden/go_agent.json` by hand when a rule changes on purpose.

use std::collections::BTreeMap;

use recur_agent::attach::{self, UserImage};
use recur_agent::display::{self, CHAT_DETAIL_LIMIT, FEED_INPUT_LIMIT, PLAN_MAX_PERSIST_CHARS};
use recur_agent::turn::format_steer_user_message;
use recur_agent::{ProviderError, TurnResult, error, permission};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Golden {
    constants: Constants,
    summaries: Vec<[String; 7]>,
    results: Vec<[String; 3]>,
    truncate: Vec<(String, usize, String, String)>,
    failures: Vec<Failure>,
    tidy: Vec<[String; 2]>,
    first_line: Vec<[String; 2]>,
    content_blocks: Vec<Blocks>,
    sanitize: Vec<[String; 2]>,
    ordinals: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Constants {
    transient_markers: Vec<String>,
    codex_login_expired: String,
    recur_tool_labels: BTreeMap<String, String>,
    permission_prompt_tool: String,
    attachment_marker: String,
    display_hidden_sentinel: String,
    feed_input_limit: usize,
    chat_detail_limit: usize,
    plan_max_persist_chars: usize,
    max_chat_attachment_bytes: usize,
    steer_message: String,
    image_mime: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Failure {
    err: Option<String>,
    subtype: String,
    text: String,
    stderr: String,
    is_error: bool,
    reason: String,
    transient: bool,
}

#[derive(Deserialize)]
struct Blocks {
    text: String,
    display: String,
    imgs: Option<Vec<String>>,
    blocks: Value,
    order: Option<Vec<String>>,
    ensure: String,
    strip: String,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("golden/go_agent.json")).unwrap()
}

fn report(what: &str, mismatches: Vec<String>) {
    assert!(
        mismatches.is_empty(),
        "{} {what} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

fn json(s: &str) -> Option<Value> {
    serde_json::from_str(s)
        .ok()
        .map(recur_domain::json::canonical)
}

fn same_json(got: &str, want: &str) -> bool {
    json(got).is_some_and(|g| Some(g) == json(want))
}

/// A listing compacted to the chat detail budget keeps different items once
/// the encoding differs, so only its envelope and size are comparable.
fn same_compaction(got: &str, want: &str) -> bool {
    let keys =
        |s: &str| json(s).and_then(|v| Some(v.as_object()?.keys().cloned().collect::<Vec<_>>()));
    got.len() <= CHAT_DETAIL_LIMIT && keys(got).is_some() && keys(got) == keys(want)
}

fn short(s: &str) -> String {
    let mut out: String = s.chars().take(120).collect();
    if out.len() < s.len() {
        out.push_str(&format!("…({} bytes)", s.len()));
    }
    out
}

#[test]
fn constants_match_go() {
    let c = golden().constants;
    let markers: Vec<&str> = error::TRANSIENT_MARKERS.to_vec();
    assert_eq!(markers, c.transient_markers);
    assert_eq!(error::CODEX_LOGIN_EXPIRED, c.codex_login_expired);
    let labels: BTreeMap<String, String> = permission::RECUR_TOOL_LABELS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(
        labels.len(),
        permission::RECUR_TOOL_LABELS.len(),
        "duplicate label key"
    );
    assert_eq!(labels, c.recur_tool_labels);
    assert_eq!(permission::PROMPT_TOOL, c.permission_prompt_tool);
    assert_eq!(attach::marker("ID"), c.attachment_marker);
    assert_eq!(attach::DISPLAY_HIDDEN_SENTINEL, c.display_hidden_sentinel);
    assert_eq!(FEED_INPUT_LIMIT, c.feed_input_limit);
    assert_eq!(CHAT_DETAIL_LIMIT, c.chat_detail_limit);
    assert_eq!(PLAN_MAX_PERSIST_CHARS, c.plan_max_persist_chars);
    assert_eq!(
        attach::MAX_CHAT_ATTACHMENT_BYTES,
        c.max_chat_attachment_bytes
    );
    assert_eq!(format_steer_user_message("SUMMARY"), c.steer_message);
    let mime: BTreeMap<String, String> = attach::CHAT_IMAGE_MIME
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(mime, c.image_mime);
}

#[test]
fn summaries_match_go() {
    let mut bad = Vec::new();
    for [
        tool,
        input,
        summary,
        chat_input,
        description,
        perm_summary,
        perm_label,
    ] in golden().summaries
    {
        let got = [
            (
                "summarize",
                display::summarize_tool_input(&tool, &input),
                summary,
            ),
            (
                "chat_input",
                display::chat_tool_input(&tool, &input),
                chat_input,
            ),
            (
                "description",
                display::tool_description(&tool, &input),
                description,
            ),
            (
                "permission summary",
                permission::summary(&tool, &input),
                perm_summary,
            ),
            (
                "permission label",
                permission::tool_label(&tool),
                perm_label,
            ),
        ];
        for (what, got, want) in got {
            if got != want && !same_json(&got, &want) {
                bad.push(format!(
                    "{what}({tool}, {}): rust {:?} go {:?}",
                    short(&input),
                    short(&got),
                    short(&want)
                ));
            }
        }
    }
    report("summary", bad);
}

#[test]
fn chat_results_match_go() {
    let mut bad = Vec::new();
    for [tool, result, want] in golden().results {
        let got = display::chat_tool_result(&tool, &result);
        if got != want && !same_json(&got, &want) && !same_compaction(&got, &want) {
            bad.push(format!(
                "chat_tool_result({tool}, {}): rust {:?} go {:?}",
                short(&result),
                short(&got),
                short(&want)
            ));
        }
    }
    report("chat result", bad);
}

#[test]
fn truncation_matches_go() {
    for (s, n, want, want_hard) in golden().truncate {
        assert_eq!(display::truncate(&s, n), want, "truncate({s:?}, {n})");
        assert_eq!(
            display::hard_truncate_runes(&s, n),
            want_hard,
            "hard({s:?}, {n})"
        );
    }
}

#[test]
fn failures_match_go() {
    let mut bad = Vec::new();
    for f in golden().failures {
        let res = TurnResult {
            subtype: f.subtype.clone(),
            text: f.text.clone(),
            stderr: f.stderr.clone(),
            is_error: f.is_error,
            ..Default::default()
        };
        let err = f.err.as_deref().map(ProviderError::failed);
        let got = error::classify(
            err.as_ref()
                .map(|e| e as &(dyn std::error::Error + 'static)),
            &res,
        );
        if got != (f.reason.clone(), f.transient) {
            bad.push(format!(
                "classify(err {:?}, stderr {}): rust {:?} go {:?}",
                f.err,
                short(&f.stderr),
                got,
                (f.reason, f.transient)
            ));
        }
    }
    report("failure", bad);
}

#[test]
fn error_lines_match_go() {
    let g = golden();
    for [s, want] in g.tidy {
        assert_eq!(error::tidy(&s), want, "tidy({})", short(&s));
    }
    for [s, want] in g.first_line {
        assert_eq!(error::first_line(&s), want, "first_line({})", short(&s));
    }
}

#[test]
fn content_blocks_match_go() {
    for c in golden().content_blocks {
        let imgs: Vec<UserImage> = c
            .imgs
            .unwrap_or_default()
            .into_iter()
            .map(|id| UserImage {
                name: format!("{id}.png"),
                mime: "image/png".into(),
                path: format!("workspaces/W/attachments/{id}.png"),
                id,
                ..Default::default()
            })
            .collect();
        let blocks = attach::user_content_blocks(&c.text, &c.display, &imgs);
        let got: Value = if blocks.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&serde_json::to_string(&blocks).unwrap()).unwrap()
        };
        assert_eq!(got, c.blocks, "blocks({:?}, {:?})", c.text, c.display);
        let order: Vec<String> = attach::images_in_marker_order(&c.text, &imgs)
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(order, c.order.unwrap_or_default(), "order({:?})", c.text);
        assert_eq!(
            attach::ensure_markers(&c.text, &imgs),
            c.ensure,
            "ensure({:?})",
            c.text
        );
        assert_eq!(
            attach::strip_markers(&c.text),
            c.strip,
            "strip({:?})",
            c.text
        );
    }
}

#[test]
fn attachment_ids_and_ordinals_match_go() {
    let g = golden();
    for [s, want] in g.sanitize {
        assert_eq!(attach::sanitize_attachment_id(&s), want, "sanitize({s:?})");
    }
    for (n, want) in g.ordinals.iter().enumerate() {
        assert_eq!(&attach::ordinal_word(n), want);
    }
}
