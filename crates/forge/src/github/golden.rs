//! Snapshot answers for the GitHub parsers and rules
//! (`tests/golden/github.json`), every field compared as JSON. Edit the fixture
//! by hand when the behavior changes on purpose.

use std::collections::HashMap;

use pretty_assertions::assert_eq;
use recur_domain::{
    Commit, MergeCommitMessage, MergeMethod, Pr, PrComment, PrSource, SquashCommitMessage,
};
use serde_json::{Value, json};

use super::avatar_url;
use super::checks::{parse_cli_checks, parse_graphql_checks};
use super::list::{ListedPr, parse_list};
use super::snapshot::{
    SNAPSHOT_QUERY, finish, flatten_json_array_pages, parse_snapshot_graphql, snapshot_query,
};
use crate::model::{
    apply_commit_avatars, enrich_commits, listed_attention_reasons, listed_behind,
    listed_merge_field, listed_review_requested, listed_state_group, normalize_time, parse_time,
    snapshot_participants, snapshot_reviewers,
};

fn golden() -> Value {
    serde_json::from_str(include_str!("../../tests/golden/github.json")).expect("golden json")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

/// The fixture writes empty lists as null; compare those with empty arrays.
fn json_of<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serialize")
}

fn or_empty(v: &Value) -> Value {
    if v.is_null() { json!([]) } else { v.clone() }
}

#[test]
fn queries_match_go() {
    let g = golden();
    assert_eq!(super::CHECKS_QUERY, s(&g["queries"]["checks"]));
    assert_eq!(SNAPSHOT_QUERY, s(&g["queries"]["snapshot"]));
    assert_eq!(snapshot_query(false), s(&g["queries"]["snapshotNoRollup"]));
}

#[test]
fn snapshot_matches_go() {
    let g = golden();
    let now = parse_time(s(&g["now"])).unwrap();
    let want = &g["snapshot"];
    let gh = parse_snapshot_graphql(s(&want["input"]), now).unwrap();
    let snap = finish(gh, &flatten_json_array_pages(s(&want["timeline"])), now);
    assert_eq!(snap.title, s(&want["title"]));
    assert_eq!(snap.state, s(&want["state"]));
    assert_eq!(snap.review_decision, s(&want["reviewDecision"]));
    assert_eq!(snap.viewer_login, s(&want["viewerLogin"]));
    assert_eq!(
        snap.viewer_can_write,
        want["viewerCanWrite"].as_bool().unwrap()
    );
    assert_eq!(
        json_of(&snap.allowed_merge_methods),
        or_empty(&want["allowedMergeMethods"])
    );
    assert_eq!(snap.node_id, s(&want["nodeID"]));
    assert_eq!(snap.merge_queued, want["mergeQueued"].as_bool().unwrap());

    let got: Vec<Value> = snap.comments.iter().map(json_of).collect();
    // The fixture's order for undated rows is arbitrary; Rust sorts them last.
    let mut go: Vec<Value> = want["comments"].as_array().unwrap().clone();
    go.sort_by_cached_key(|c| {
        let at = c["at"].as_str().and_then(parse_time);
        (c["kind"] != "description", at.is_none(), at)
    });
    assert_eq!(got.len(), go.len());
    for (g, w) in got.iter().zip(&go) {
        assert_eq!(g, w);
    }

    assert_eq!(json_of(&snap.commits), want["viewCommits"]);
    let stored = vec![
        Commit {
            hash: "abc1230".into(),
            author: "local".into(),
            ..Default::default()
        },
        Commit {
            sha: "def4560000000000".into(),
            hash: "def4560".into(),
            author: "Josh Dechant".into(),
            avatar_url: "x".into(),
            ..Default::default()
        },
    ];
    let mut enriched = enrich_commits(&stored, &snap);
    apply_commit_avatars(&mut enriched, &snap.author_avatars);
    assert_eq!(json_of(&enriched), want["enrichedCommits"]);

    let avatar = |l: &str, p: &str| avatar_url(l, p, 56);
    // Participants follow comment order, which differs from the fixture's above.
    let by_text = |v: &Value| {
        let mut rows: Vec<String> = v.as_array().unwrap().iter().map(Value::to_string).collect();
        rows.sort();
        rows
    };
    assert_eq!(
        by_text(&json_of(&snapshot_participants(
            &snap,
            &snap.comments,
            &avatar
        ))),
        by_text(&want["participants"])
    );
    assert_eq!(
        json_of(&snapshot_reviewers(&snap, &snap.comments, &avatar)),
        or_empty(&want["reviewers"])
    );
    let go_avatars: HashMap<String, String> =
        serde_json::from_value(want["authorAvatars"].clone()).unwrap();
    assert_eq!(snap.author_avatars, go_avatars);
}

#[test]
fn checks_match_go() {
    let g = golden();
    let now = parse_time(s(&g["now"])).unwrap();
    let want = &g["checks"];
    let (checks, ok) = parse_graphql_checks(s(&want["graphqlInput"]), now).unwrap();
    assert!(!want["graphqlErr"].as_bool().unwrap());
    assert_eq!(json_of(&checks), want["graphql"]);
    assert_eq!(ok, want["graphqlOK"].as_bool().unwrap());
    let (checks, ok) = parse_cli_checks(s(&want["cliInput"]), now).unwrap();
    assert!(!want["cliErr"].as_bool().unwrap());
    assert_eq!(json_of(&checks), want["cli"]);
    assert_eq!(ok, want["cliOK"].as_bool().unwrap());
}

#[test]
fn list_rows_match_go() {
    let g = golden();
    let rows: Vec<_> = parse_list(s(&g["list"]["input"]))
        .unwrap()
        .into_iter()
        .map(ListedPr::into_row)
        .collect();
    let want = g["list"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), want.len());
    for (r, w) in rows.iter().zip(want) {
        let (state, group) = listed_state_group(r, "me");
        let reasons = listed_attention_reasons(r, "me");
        let reasons = if reasons.is_empty() {
            Value::Null
        } else {
            json_of(&reasons)
        };
        let got = json!({
            "checks": if r.has_checks { json_of(&r.checks) } else { Value::Null },
            "checksOK": r.checks_ok,
            "hasChecks": r.has_checks,
            "participants": json_of(&r.participants),
            "reviewers": if r.reviewers.is_empty() { Value::Null } else { json_of(&r.reviewers) },
            "reasons": reasons,
            "reviewRequested": listed_review_requested(r, "me"),
            "state": state,
            "group": group,
            "behind": listed_behind(r),
            "mergeable": listed_merge_field("CONFLICTING", &r.mergeable),
            "createdAt": normalize_time(&r.created_at),
        });
        assert_eq!(&got, w);
    }
}

#[test]
fn merge_messages_match_go() {
    let g = golden();
    let pr = Pr {
        number: 12,
        source: PrSource::GitHub,
        title: "Add OAuth\nsecond".into(),
        commits: Some(vec![
            Commit {
                message: "b".into(),
                ..Default::default()
            },
            Commit {
                message: "a\nx".into(),
                ..Default::default()
            },
        ]),
        comments: Some(vec![PrComment {
            kind: "description".into(),
            body_md: " Why ".into(),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let mut got = Vec::new();
    for m in [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase] {
        for sq in [
            SquashCommitMessage::Title,
            SquashCommitMessage::TitleAndCommits,
            SquashCommitMessage::TitleAndDescription,
        ] {
            for mc in [
                MergeCommitMessage::Title,
                MergeCommitMessage::TitleAndDescription,
            ] {
                let t = super::merge::merge_message(&pr, &m, &sq, &mc);
                got.push(json!({"Subject": t.subject, "Body": t.body, "OmitBody": t.omit_body}));
            }
        }
    }
    assert_eq!(Value::Array(got), g["mergeMessages"]);
}
