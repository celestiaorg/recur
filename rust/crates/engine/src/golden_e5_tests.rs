//! Go-generated golden fixtures for slice E5 (R2): squash / merge commit
//! messages, Local PR and workspace land messages, the open_pr tool result,
//! the send-back instruction, the workspace PR summary, diff hunks for line
//! notes, pending note locations and sides, review labels, PR URL and
//! checkout-head rules, hoist path and code remaps, diff text, thread keys
//! and the commit log merge (`tests/golden/go_e5.json`, generator
//! `tests/golden/gen_e5_golden.go.txt`).

use pretty_assertions::assert_eq;
use recur_domain::{
    Commit, FileDiff, MergeCommitMessage, MergeMethod, PendingLineComment, Pr, PrComment, PrSource,
    SquashCommitMessage, Workspace,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use recur_forge::Forge;

use crate::hoist::{hoist_remap_codes, rewrite_hoist_artifact_path};
use crate::prs::from_pr::checkout_pr_head_name;
use crate::prs::merge::{
    format_merge_commit_message, format_squash_commit_message, local_merge_commit_message,
};
use crate::prs::normalize_pr_url;
use crate::prs::ops::{
    comment_hunk_from_files, pending_loc, pending_side, resolve_thread_key, review_card_verb,
    review_label_state, split_pending_line, visible_diff_text,
};
use crate::prs::refresh::{commit_ref_matches, merge_commit_log};
use crate::prs::rework::{ERR_PR_UNAVAILABLE, distill_instruction};
use crate::prs::workspace::{
    format_open_pr_tool_result, workspace_merge_commit_message, workspace_pr_summary,
};

const GOLDEN: &str = include_str!("../tests/golden/go_e5.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden json")
}

fn cases<'a>(g: &'a Value, key: &str) -> &'a Vec<Value> {
    g[key]
        .as_array()
        .unwrap_or_else(|| panic!("no golden section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap_or_default()
}

fn de<T: DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("decode {v}: {e}"))
}

fn commit_sets(g: &Value) -> Vec<Vec<Commit>> {
    cases(g, "commits")
        .iter()
        .map(|c| if c.is_null() { Vec::new() } else { de(c) })
        .collect()
}

fn text(t: recur_forge::MergeCommitText) -> Value {
    json!({"subject": t.subject, "body": t.body, "omitBody": t.omit_body})
}

#[test]
fn squash_and_merge_messages() {
    let g = golden();
    let commits = commit_sets(&g);
    for c in cases(&g, "squash") {
        let n = &c["in"];
        let got = format_squash_commit_message(
            s(&n["title"]),
            i(&n["number"]),
            n["forge"].as_bool().unwrap(),
            &commits[i(&n["commits"]) as usize],
            s(&n["desc"]),
            &SquashCommitMessage::from(s(&n["style"])),
        );
        assert_eq!(text(got), c["out"], "squash {n}");
    }
    for c in cases(&g, "merge") {
        let n = &c["in"];
        let got = format_merge_commit_message(
            s(&n["title"]),
            i(&n["number"]),
            n["forge"].as_bool().unwrap(),
            s(&n["desc"]),
            &MergeCommitMessage::from(s(&n["style"])),
        );
        assert_eq!(text(got), c["out"], "merge {n}");
    }
}

#[test]
fn local_and_workspace_land_messages() {
    let g = golden();
    let prs: Vec<Pr> = de(&g["prs"]);
    let forge = recur_forge::testing::FakeForge::new();
    for c in cases(&g, "local_merge") {
        let n = &c["in"];
        let pr = &prs[i(&n["pr"]) as usize];
        let (method, sq, mg) = (
            MergeMethod::from(s(&n["method"])),
            SquashCommitMessage::from(s(&n["squash"])),
            MergeCommitMessage::from(s(&n["merge"])),
        );
        // Go's formatPRMergeMessage forge branch is the forge's merge_message (R8).
        let got = if pr.source == PrSource::Local {
            local_merge_commit_message(pr, &method, &sq, &mg)
        } else {
            forge.merge_message(pr, &method, &sq, &mg).combined()
        };
        assert_eq!(got, s(&c["out"]), "local merge {n}");
    }
    let commits = commit_sets(&g);
    for c in cases(&g, "workspace_merge") {
        let n = &c["in"];
        let ws = Workspace {
            id: "ws-1".into(),
            goal: s(&n["goal"]).into(),
            branch: "falafel".into(),
            ..Workspace::default()
        };
        let got = workspace_merge_commit_message(
            &ws,
            s(&n["base"]),
            &MergeMethod::from(s(&n["method"])),
            &commits[i(&n["commits"]) as usize],
            &SquashCommitMessage::from(s(&n["squash"])),
        );
        assert_eq!(got, s(&c["out"]), "workspace merge {n}");
    }
}

#[test]
fn open_pr_tool_results() {
    let g = golden();
    for c in cases(&g, "open_pr") {
        let n = &c["in"];
        let p = &n["pr"];
        let pr = Pr {
            number: i(&p["number"]),
            source: if p["forge"].as_bool().unwrap() {
                PrSource::GitHub
            } else {
                PrSource::Local
            },
            draft: p["draft"].as_bool().unwrap(),
            ..Pr::default()
        };
        let got = format_open_pr_tool_result(
            &pr,
            "GitHub",
            n["wantForge"].as_bool().unwrap(),
            n["hadOpen"].as_bool().unwrap(),
            n["wasLocal"].as_bool().unwrap(),
            s(&n["fail"]),
        );
        assert_eq!(got, s(&c["out"]), "open_pr {n}");
    }
}

#[test]
fn send_back_and_summary() {
    let g = golden();
    for c in cases(&g, "distill") {
        let notes: Vec<String> = if c["in"].is_null() {
            Vec::new()
        } else {
            de(&c["in"])
        };
        assert_eq!(
            distill_instruction(&notes),
            s(&c["out"]),
            "distill {}",
            c["in"]
        );
    }
    for c in cases(&g, "summary") {
        let n = &c["in"];
        let got = workspace_pr_summary(
            s(&n["name"]),
            i(&n["files"]),
            i(&n["add"]),
            i(&n["del"]),
            i(&n["commits"]),
        );
        assert_eq!(serde_json::to_value(got).unwrap(), c["out"], "summary {n}");
    }
    assert_eq!(ERR_PR_UNAVAILABLE, s(&g["small"]["err_pr_unavailable"]));
}

#[test]
fn line_note_hunks_and_locations() {
    let g = golden();
    let files: Vec<FileDiff> = de(&g["files"]);
    for c in cases(&g, "hunks") {
        let n = &c["in"];
        let got = comment_hunk_from_files(&files, s(&n["path"]), i(&n["line"]), s(&n["side"]));
        assert_eq!(got, s(&c["out"]), "hunk {n}");
    }
    for c in cases(&g, "pending_loc") {
        let pc: PendingLineComment = de(&c["in"]);
        assert_eq!(pending_loc(&pc), s(&c["out"]), "pending loc {}", c["in"]);
    }
    let small = &g["small"];
    for c in cases(small, "pending_side") {
        assert_eq!(pending_side(s(&c["in"])), s(&c["out"]));
    }
    for c in cases(small, "split_pending") {
        let (side, n) = split_pending_line(i(&c["in"]));
        assert_eq!(json!([side, n]), c["out"]);
    }
    for c in cases(small, "reviews") {
        let k = s(&c["in"]);
        let (label, state) = review_label_state(k);
        assert_eq!(
            json!([label, state, review_card_verb(k)]),
            c["out"],
            "review {k}"
        );
    }
    for c in cases(small, "diff_text") {
        assert_eq!(
            visible_diff_text(s(&c["in"])),
            s(&c["out"]),
            "diff text {}",
            c["in"]
        );
    }
}

#[test]
fn pr_urls_heads_and_hoist_rewrites() {
    let g = golden();
    let small = &g["small"];
    for c in cases(small, "pr_urls") {
        assert_eq!(normalize_pr_url(s(&c["in"])), s(&c["out"]));
    }
    for c in cases(small, "checkout_heads") {
        let n = &c["in"];
        let got = checkout_pr_head_name(s(&n[0]), s(&n[1]), s(&n[2]), n[3].as_bool().unwrap());
        assert_eq!(got, s(&c["out"]), "checkout head {n}");
    }
    for c in cases(small, "hoist_paths") {
        let n = &c["in"];
        let got = rewrite_hoist_artifact_path(s(&n[0]), s(&n[1]), s(&n[2]));
        assert_eq!(got, s(&c["out"]), "hoist path {n}");
    }
    let remap: BTreeMap<String, String> = de(&small["hoist_remap"]);
    for c in cases(small, "hoist_codes") {
        let order: Vec<String> = if c["in"].is_null() {
            Vec::new()
        } else {
            de(&c["in"])
        };
        let want: Vec<String> = if c["out"].is_null() {
            Vec::new()
        } else {
            de(&c["out"])
        };
        assert_eq!(
            hoist_remap_codes(&order, &remap),
            want,
            "hoist codes {}",
            c["in"]
        );
    }
}

#[test]
fn thread_keys_and_commit_log() {
    let g = golden();
    let thread: Vec<PrComment> = de(&g["thread"]);
    for c in cases(&g, "thread_keys") {
        assert_eq!(
            resolve_thread_key(&thread, s(&c["in"])),
            s(&c["out"]),
            "thread key {}",
            c["in"]
        );
    }
    let log = cases(&g, "commit_log");
    let existing: Vec<Commit> = de(&log[0]["in"]["existing"]);
    let fresh: Vec<Commit> = de(&log[0]["in"]["fresh"]);
    let want: Vec<Commit> = de(&log[0]["out"]);
    assert_eq!(merge_commit_log(&existing, fresh), want);
    for c in log[1]["out"].as_array().unwrap() {
        let r = s(&c["in"]);
        assert_eq!(
            commit_ref_matches(&existing[0], r),
            c["out"].as_bool().unwrap(),
            "ref {r}"
        );
    }
}
