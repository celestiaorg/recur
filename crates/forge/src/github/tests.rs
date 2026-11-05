//! GitHub forge tests: threads, snapshots, reactions, mentions and the
//! client, plus fake-`gh` runs of the command paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use pretty_assertions::assert_eq;
use recur_domain::{Check, Pr, PrComment, PrParticipant, PrReaction};

use super::checks::{
    apply_checks_tab_urls, graphql_command_error, parse_cli_checks, parse_graphql_checks, ref_glob,
    ruleset_applies_to_base,
};
use super::reactions::{apply_reactions, parse_reactions, reaction_subject_id};
use super::snapshot::{
    finish, flatten_json_array_pages, parse_snapshot_graphql, permission_can_write, timeline_args,
};
use super::thread::{
    Actor, ViewCommit, ViewCommitAuthor, apply_avatars, apply_review_threads,
    attach_review_comments, commit_logins, merge_comments, parse_commit_rollup,
    parse_inline_comments, parse_mentionable_users, parse_posted_inline_comment,
    parse_review_threads, parse_view,
};
use super::timeline::parse_timeline;
use super::*;
use crate::cli::write_script;
use crate::model::{enrich_commits, parse_time};
use crate::{CommitMeta, CreatePr};

fn at(s: &str) -> DateTime<Utc> {
    parse_time(s).unwrap()
}

fn encode_node(prefix: &str, type_id: u32, db: u64) -> String {
    let mut raw = type_id.to_be_bytes().to_vec();
    raw.extend(db.to_be_bytes());
    format!(
        "{prefix}_{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(raw)
    )
}

fn by_id<'a>(cs: &'a [PrComment], id: &str) -> &'a PrComment {
    cs.iter().find(|c| c.id == id).unwrap_or_else(|| {
        panic!(
            "missing {id} in {:?}",
            cs.iter().map(|c| &c.id).collect::<Vec<_>>()
        )
    })
}

fn rx(content: &str, count: i64, mine: bool) -> PrReaction {
    PrReaction {
        content: content.into(),
        count,
        mine,
    }
}

#[test]
fn node_database_ids() {
    assert_eq!(node_database_id("99"), Some(99));
    let id = encode_node("PRR", 8, 3864739160);
    assert_eq!(node_database_id(&id), Some(3864739160));
    let legacy = base64::engine::general_purpose::STANDARD.encode("012:PullRequestReview42");
    assert_eq!(node_database_id(&legacy), Some(42));
    assert_eq!(node_database_id("PRR_c"), None);
    assert_eq!(
        thread::permalink(
            "https://github.com/acme/app/pull/1/",
            "pullrequestreview",
            &id,
            ""
        ),
        "https://github.com/acme/app/pull/1#pullrequestreview-3864739160"
    );
}

#[test]
fn parse_pr_view() {
    let now = at("2026-08-21T12:00:00Z");
    let pr_node = encode_node("PR", 5, 5215972950);
    let raw = format!(
        r#"{{
        "id": "{pr_node}",
        "title": "Add OAuth",
        "state": "OPEN",
        "reviewDecision": "APPROVED",
        "headRefOid": "abc",
        "url": "https://github.com/acme/app/pull/12",
        "createdAt": "2026-08-20T10:00:00Z",
        "updatedAt": "2026-08-21T11:45:00Z",
        "author": {{"login": "loopbot"}},
        "reviewRequests": [{{"login": "avery"}}],
        "comments": [
            {{"id": "IC_1", "url": "https://github.com/acme/app/pull/12#issuecomment-1", "author": {{"login": "avery"}}, "body": "looks good", "createdAt": "2026-08-21T11:00:00Z"}}
        ],
        "reviews": [
            {{"id": "PRR_1", "url": "https://github.com/acme/app/pull/12#pullrequestreview-1", "author": {{"login": "sam"}}, "body": "ship it", "state": "APPROVED", "submittedAt": "2026-08-21T11:30:00Z"}},
            {{"id": "PRR_2", "author": {{"login": "bot"}}, "body": "", "state": "PENDING", "submittedAt": "2026-08-21T11:31:00Z"}}
        ]
    }}"#
    );
    let snap = parse_view(&raw, now).unwrap().snap;
    assert_eq!(
        (
            snap.title.as_str(),
            snap.state.as_str(),
            snap.review_decision.as_str(),
            snap.author_login.as_str()
        ),
        ("Add OAuth", "OPEN", "APPROVED", "loopbot")
    );
    assert_eq!(
        (snap.created_at.as_str(), snap.updated_at.as_str()),
        ("2026-08-20T10:00:00Z", "2026-08-21T11:45:00Z")
    );
    assert_eq!(snap.review_requests.len(), 1);
    assert_eq!(snap.review_requests[0].login, "avery");
    assert!(owner_repo_from_pr_url(&snap.url).is_some());
    assert_eq!(
        snap.comments.len(),
        3,
        "description + comment + review with body; pending skipped"
    );
    let c = &snap.comments;
    assert_eq!(
        (c[0].kind.as_str(), c[0].id.as_str()),
        ("description", "gh-description")
    );
    assert_eq!(
        c[0].url,
        "https://github.com/acme/app/pull/12#issue-5215972950"
    );
    assert_eq!(
        (
            c[1].author.as_str(),
            c[1].id.as_str(),
            c[1].author_login.as_str()
        ),
        ("avery", "gh-comment-IC_1", "avery")
    );
    assert_eq!(
        c[1].url,
        "https://github.com/acme/app/pull/12#issuecomment-1"
    );
    assert_eq!(c[1].body_md, "looks good");
    assert_eq!(c[1].verb, "commented");
    assert_eq!(
        (
            c[2].author.as_str(),
            c[2].id.as_str(),
            c[2].body_md.as_str()
        ),
        ("sam", "gh-review-PRR_1", "ship it")
    );
    assert_eq!(
        (c[2].verb.as_str(), c[2].kind.as_str()),
        ("approved", "comment")
    );
    assert_eq!(
        c[2].url,
        "https://github.com/acme/app/pull/12#pullrequestreview-1"
    );
}

#[test]
fn parse_pr_view_bodyless_review_and_merge_status() {
    let now = at("2026-08-21T12:00:00Z");
    let snap = parse_view(
        r#"{"title": "PR", "state": "OPEN", "url": "https://github.com/acme/app/pull/12", "author": {"login": "loopbot"},
        "reviews": [{"id": "PRR_9", "author": {"login": "sam"}, "body": "", "state": "CHANGES_REQUESTED", "submittedAt": "2026-08-21T11:30:00Z"}]}"#,
        now,
    )
    .unwrap()
    .snap;
    assert_eq!(snap.comments.len(), 2);
    let got = &snap.comments[1];
    assert_eq!(
        (got.kind.as_str(), got.author.as_str(), got.body_md.as_str()),
        ("action", "sam", "requested changes")
    );

    let snap = parse_view(
        r#"{"title": "Draft OAuth", "state": "OPEN", "reviewDecision": "REVIEW_REQUIRED", "mergeable": "CONFLICTING",
        "mergeStateStatus": "DIRTY", "isDraft": true, "url": "https://github.com/acme/app/pull/9", "author": {"login": "loopbot"}}"#,
        at("2026-08-22T12:00:00Z"),
    )
    .unwrap()
    .snap;
    assert!(snap.is_draft);
    assert_eq!(
        (
            snap.mergeable.as_str(),
            snap.merge_state_status.as_str(),
            snap.review_decision.as_str()
        ),
        ("CONFLICTING", "DIRTY", "REVIEW_REQUIRED")
    );
    assert!(parse_view("", now).is_err());
}

#[test]
fn parse_pr_view_bot_markdown() {
    let node = encode_node("PRR", 8, 3864739160);
    let raw = format!(
        r###"{{"title": "PR", "state": "OPEN", "reviewDecision": "", "headRefOid": "abc", "url": "https://github.com/acme/app/pull/1", "comments": [],
        "reviews": [{{"id": "{node}", "author": {{"login": "copilot-pull-request-reviewer", "type": "Bot", "avatarUrl": "https://avatars.githubusercontent.com/in/946600?v=4"}}, "body": "## Pull request overview\n\n**Changes:** table", "state": "COMMENTED", "submittedAt": "2026-08-21T11:30:00Z"}}]}}"###
    );
    let snap = parse_view(&raw, at("2026-08-21T12:00:00Z")).unwrap().snap;
    let review = by_id(&snap.comments, &format!("gh-review-{node}"));
    assert_eq!(
        review.url,
        "https://github.com/acme/app/pull/1#pullrequestreview-3864739160"
    );
    assert_eq!(review.author, "Copilot");
    assert_eq!(
        review.body_md,
        "## Pull request overview\n\n**Changes:** table"
    );
    assert!(!review.body_html.contains("##"));
    assert_eq!(
        review.avatar_url,
        "https://avatars.githubusercontent.com/in/946600?v=4"
    );
}

#[test]
fn parse_inline_line_notes() {
    let now = at("2026-08-21T12:00:00Z");
    let got = parse_inline_comments(
        r#"[{"id": 99, "html_url": "https://github.com/acme/app/pull/12#discussion_r99", "body": "nits", "path": "auth.go", "line": 12, "side": "RIGHT", "diff_hunk": "@@ -12,1 +12,1 @@\n-old\n+new", "created_at": "2026-08-21T11:55:00Z", "pull_request_review_id": 42, "user": {"login": "dependabot[bot]", "type": "Bot", "avatar_url": "https://avatars.githubusercontent.com/in/29110?v=4"}}]"#,
        now,
        "https://github.com/acme/app/pull/12",
    );
    assert_eq!(got.len(), 1);
    let c = &got[0];
    assert_eq!(
        (c.id.as_str(), c.author.as_str(), c.body_md.as_str()),
        ("gh-inline-99", "dependabot", "nits")
    );
    assert!(c.body_html.contains("auth.go:12"));
    assert_eq!(
        c.avatar_url,
        "https://avatars.githubusercontent.com/in/29110?v=4"
    );
    assert_eq!(c.url, "https://github.com/acme/app/pull/12#discussion_r99");
    assert_eq!(c.review_id, "42");
    assert_eq!((c.path.as_str(), c.line), ("auth.go", 12));
    assert!(c.diff_hunk.contains("+new"));

    let got = parse_inline_comments(
        r#"[
        {"id": 1, "body": "span", "path": "a.go", "line": 20, "start_line": 16, "side": "RIGHT", "start_side": "RIGHT", "created_at": "2026-08-21T11:55:00Z", "user": {"login": "avery"}},
        {"id": 2, "body": "stale", "path": "b.go", "line": null, "original_line": 9, "start_line": null, "original_start_line": 7, "side": "LEFT", "created_at": "2026-08-21T11:56:00Z", "user": {"login": "sam"}}
    ]"#,
        now,
        "https://github.com/acme/app/pull/12",
    );
    assert_eq!(
        (got[0].line, got[0].start_line, got[0].start_side.as_str()),
        (20, 16, "RIGHT")
    );
    assert!(got[0].body_html.contains("a.go:16-20") && !got[0].outdated);
    assert_eq!(
        (
            got[1].line,
            got[1].start_line,
            got[1].side.as_str(),
            got[1].outdated
        ),
        (9, 7, "LEFT", true)
    );
    assert!(got[1].body_html.contains("b.go:7-9") && got[1].body_html.contains("outdated"));
    assert_eq!(
        got[1].url,
        "https://github.com/acme/app/pull/12#discussion_r2"
    );
}

#[test]
fn parse_posted_reply() {
    let now = at("2026-08-21T12:00:00Z");
    let got = parse_posted_inline_comment(
        r#"{"id": 999, "html_url": "https://github.com/acme/app/pull/12#discussion_r999", "body": "thread reply", "path": "auth.go", "line": 12, "in_reply_to_id": 1, "created_at": "2026-08-21T11:59:00Z", "user": {"login": "avery"}}"#,
        now,
        "https://github.com/acme/app/pull/12",
    )
    .unwrap();
    assert_eq!(
        (
            got.id.as_str(),
            got.in_reply_to_id.as_str(),
            got.body_md.as_str()
        ),
        ("gh-inline-999", "gh-inline-1", "thread reply")
    );
    assert!(parse_posted_inline_comment("[]", now, "").is_none());
}

#[test]
fn attach_reviews_and_replies() {
    let now = at("2026-08-21T12:00:00Z");
    let node = encode_node("PRR", 8, 99);
    let raw = format!(
        r#"{{"id": "PR_1", "title": "Add OAuth", "state": "OPEN", "url": "https://github.com/acme/app/pull/12", "author": {{"login": "loopbot"}}, "createdAt": "2026-08-21T10:00:00Z",
        "reviews": [
            {{"id": "{node}", "url": "https://github.com/acme/app/pull/12#pullrequestreview-99", "author": {{"login": "sam"}}, "body": "testing again", "state": "COMMENTED", "submittedAt": "2026-08-21T11:30:00Z"}},
            {{"id": "PRR_empty", "url": "https://github.com/acme/app/pull/12#pullrequestreview-100", "author": {{"login": "sam"}}, "body": "", "state": "COMMENTED", "submittedAt": "2026-08-21T11:40:00Z"}}
        ]}}"#
    );
    let mut comments = parse_view(&raw, now).unwrap().snap.comments;
    comments.extend(parse_inline_comments(
        r#"[
        {"id": 1, "body": "hmm", "path": "apps/bridge/.env.example", "line": 17, "created_at": "2026-08-21T11:30:01Z", "pull_request_review_id": 99, "user": {"login": "sam"}},
        {"id": 2, "body": "later", "path": "a.go", "line": 1, "created_at": "2026-08-21T11:40:01Z", "pull_request_review_id": 100, "user": {"login": "sam"}},
        {"id": 3, "body": "orphan", "path": "b.go", "line": 2, "created_at": "2026-08-21T11:50:00Z", "user": {"login": "pat"}},
        {"id": 4, "body": "reply", "path": "apps/bridge/.env.example", "line": 17, "created_at": "2026-08-21T11:35:00Z", "pull_request_review_id": 101, "in_reply_to_id": 1, "user": {"login": "pat"}}
    ]"#,
        now,
        "https://github.com/acme/app/pull/12",
    ));
    attach_review_comments(&mut comments);
    let parent = by_id(&comments, &format!("gh-review-{node}")).clone();
    assert_eq!(parent.body_md, "testing again");
    assert_eq!(by_id(&comments, "gh-inline-1").parent_id, parent.id);
    let empty = by_id(&comments, "gh-review-PRR_empty").clone();
    assert_eq!(
        (
            empty.kind.as_str(),
            empty.verb.as_str(),
            empty.body_md.as_str()
        ),
        ("comment", "commented", "")
    );
    assert_eq!(by_id(&comments, "gh-inline-2").parent_id, empty.id);
    assert_eq!(by_id(&comments, "gh-inline-3").parent_id, "");
    assert!(
        !comments
            .iter()
            .any(|c| c.id == "gh-review-action-PRR_empty")
    );
    assert_eq!(
        by_id(&comments, "gh-inline-4").in_reply_to_id,
        "gh-inline-1"
    );
    assert_eq!(by_id(&comments, "gh-inline-4").parent_id, parent.id);
}

#[test]
fn reply_only_review_actions_are_hidden() {
    let now = at("2026-08-21T12:00:00Z");
    let mut comments = parse_view(
        r#"{"title": "T", "url": "https://github.com/acme/app/pull/12",
        "reviews": [{"id": "77", "author": {"login": "pat"}, "body": "", "state": "COMMENTED", "submittedAt": "2026-08-21T11:40:00Z"}]}"#,
        now,
    )
    .unwrap()
    .snap
    .comments;
    comments.extend(parse_inline_comments(
        r#"[{"id": 5, "body": "root", "path": "a.go", "line": 1, "created_at": "2026-08-21T11:30:00Z", "pull_request_review_id": 70, "user": {"login": "sam"}},
            {"id": 6, "body": "reply", "path": "a.go", "line": 1, "created_at": "2026-08-21T11:40:01Z", "pull_request_review_id": 77, "in_reply_to_id": 5, "user": {"login": "pat"}}]"#,
        now,
        "",
    ));
    attach_review_comments(&mut comments);
    assert_eq!(by_id(&comments, "gh-review-action-77").kind, "hidden");
}

#[test]
fn review_threads() {
    let threads = parse_review_threads(
        r#"{"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": [
        {"id": "PRRT_1", "isResolved": true, "comments": {"nodes": [{"databaseId": 1}, {"databaseId": 4}]}},
        {"id": "PRRT_2", "isResolved": false, "comments": {"nodes": [{"databaseId": 2}]}}
    ]}}}}}"#,
    );
    assert_eq!(threads.len(), 2);
    assert_eq!(
        (
            threads[0].id.as_str(),
            threads[0].resolved,
            threads[0].comment_ids.len()
        ),
        ("PRRT_1", true, 2)
    );
    let mut cs: Vec<PrComment> = ["gh-inline-1", "gh-inline-4", "gh-inline-2", "gh-comment-9"]
        .iter()
        .map(|id| PrComment {
            id: (*id).into(),
            ..Default::default()
        })
        .collect();
    apply_review_threads(&mut cs, &threads);
    assert_eq!((cs[0].thread_id.as_str(), cs[0].resolved), ("PRRT_1", true));
    assert_eq!((cs[1].thread_id.as_str(), cs[1].resolved), ("PRRT_1", true));
    assert_eq!(
        (cs[2].thread_id.as_str(), cs[2].resolved),
        ("PRRT_2", false)
    );
    assert_eq!(cs[3].thread_id, "");
}

const TIMELINE: &str = r#"[
    {"event": "committed", "sha": "3f1d02babcdef", "message": "Enable the tab\n\nbody", "author": {"name": "josh", "date": "2026-08-21T10:00:00Z"}},
    {"id": 88, "event": "review_requested", "created_at": "2026-08-21T10:05:00Z", "review_requester": {"login": "josh", "avatar_url": "https://avatars.githubusercontent.com/u/1?v=4"}, "requested_reviewer": {"login": "copilot-pull-request-reviewer", "type": "Bot"}},
    {"event": "commented", "created_at": "2026-08-21T10:06:00Z", "actor": {"login": "josh"}, "body": "skip me"},
    {"event": "reviewed", "submitted_at": "2026-08-21T10:07:00Z", "user": {"login": "copilot-pull-request-reviewer"}, "body": "dup"}
]"#;

#[test]
fn timeline_rows() {
    let now = at("2026-08-21T12:00:00Z");
    let pr = "https://github.com/acme/app/pull/12";
    let got = parse_timeline(TIMELINE, now, &HashMap::new(), &HashMap::new(), pr, "", "");
    assert_eq!(got.len(), 2, "commented/reviewed skipped");
    assert!(
        got[0].kind == "action"
            && got[0].body_md.contains("Enable the tab")
            && got[0].commit_sha == "3f1d02babcdef"
    );
    assert_eq!(got[0].avatar_url, "", "git author is not a GitHub login");
    assert_eq!(
        (got[1].author.as_str(), got[1].body_md.as_str()),
        ("josh", "requested a review from Copilot")
    );
    assert_eq!(
        got[1].avatar_url,
        "https://avatars.githubusercontent.com/u/1?v=4"
    );
    assert_eq!(got[1].url, "https://github.com/acme/app/pull/12#event-88");

    let logins = HashMap::from([("3f1d02babcdef999".to_owned(), "mycodecrafting".to_owned())]);
    let meta = HashMap::from([(
        "3f1d02babcdef999".to_owned(),
        CommitMeta {
            verified: true,
            check_status: "pass".into(),
            url: "https://github.com/acme/app/commit/3f1d02babcdef999".into(),
        },
    )]);
    let linked = parse_timeline(TIMELINE, now, &logins, &meta, pr, "", "");
    assert_eq!(linked.len(), 2);
    assert_eq!(linked[0].author, "mycodecrafting");
    assert_eq!(
        linked[0].avatar_url,
        "https://github.com/mycodecrafting.png?size=56"
    );
    assert!(
        linked[0].verified
            && linked[0].check_status == "pass"
            && linked[0].commit_sha == "3f1d02babcdef"
    );
    assert_eq!(
        linked[0].url,
        "https://github.com/acme/app/commit/3f1d02babcdef999"
    );
}

#[test]
fn timeline_copilot_work() {
    let raw = r#"[
        {"event":"copilot_work_started","created_at":"2026-08-21T17:12:10Z","actor":{"login":"mycodecrafting"},"performed_via_github_app":{"slug":"copilot-pull-request-reviewer"}},
        {"event":"copilot_work_started","created_at":"2026-08-21T17:20:00Z","actor":{"login":"mycodecrafting"},"performed_via_github_app":{"slug":"copilot-swe-agent"}},
        {"event":"copilot_work_finished","created_at":"2026-08-21T17:25:00Z","actor":{"login":"mycodecrafting"},"performed_via_github_app":{"slug":"copilot-swe-agent"}},
        {"event":"copilot_work_finished_failure","created_at":"2026-08-21T17:26:00Z","actor":{"login":"mycodecrafting"}}
    ]"#;
    let got = parse_timeline(
        raw,
        at("2026-08-21T18:00:00Z"),
        &HashMap::new(),
        &HashMap::new(),
        "https://github.com/acme/app/pull/12",
        "",
        "",
    );
    let want = [
        "started reviewing on behalf of mycodecrafting",
        "started work on behalf of mycodecrafting",
        "finished work on behalf of mycodecrafting",
        "stopped work on behalf of mycodecrafting due to an error",
    ];
    assert_eq!(got.len(), 4);
    for (c, body) in got.iter().zip(want) {
        assert_eq!(
            (
                c.author.as_str(),
                c.kind.as_str(),
                c.body_md.as_str(),
                c.avatar_url.as_str()
            ),
            ("Copilot", "action", body, "")
        );
    }
}

#[test]
fn timeline_merge_events() {
    let now = at("2026-08-23T12:00:00Z");
    let raw = r#"[
        {"event":"merged","commit_id":"a3f73e2abcdef","created_at":"2026-08-23T04:00:00Z","actor":{"login":"mycodecrafting"}},
        {"event":"closed","commit_id":"a3f73e2abcdef","created_at":"2026-08-23T04:00:00Z","actor":{"login":"mycodecrafting"}},
        {"event":"head_ref_deleted","created_at":"2026-08-23T04:00:05Z","actor":{"login":"mycodecrafting"}},
        {"event":"labeled","created_at":"2026-08-23T03:50:00Z","actor":{"login":"mycodecrafting"},"label":{"name":"ready"}},
        {"event":"renamed","created_at":"2026-08-23T03:40:00Z","actor":{"login":"mycodecrafting"},"rename":{"from":"Old title","to":"New title"}}
    ]"#;
    let got = parse_timeline(
        raw,
        now,
        &HashMap::new(),
        &HashMap::new(),
        "https://github.com/acme/app/pull/12",
        "mycodecrafting/screening-cache",
        "mycodecrafting/reskin",
    );
    assert_eq!(got.len(), 4, "closed-on-merge skipped");
    assert_eq!(
        got[0].body_md,
        "merged commit a3f73e2 into mycodecrafting/reskin"
    );
    assert_eq!(
        (got[0].avatar_url.as_str(), got[0].commit_sha.as_str()),
        ("", "a3f73e2abcdef")
    );
    assert_eq!(
        (got[1].body_md.as_str(), got[1].avatar_url.as_str()),
        ("deleted the mycodecrafting/screening-cache branch", "")
    );
    assert_eq!(got[2].body_md, "added the ready label");
    assert_eq!(
        got[3].body_md,
        "changed the title from Old title to New title"
    );
    let closed = parse_timeline(
        r#"[{"event":"closed","created_at":"2026-08-23T04:00:00Z","actor":{"login":"mycodecrafting"}}]"#,
        now,
        &HashMap::new(),
        &HashMap::new(),
        "",
        "",
        "",
    );
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].body_md, "closed this");
}

#[test]
fn commit_rollup_and_authors() {
    let (meta, avatars) = parse_commit_rollup(
        r#"{"data": {"repository": {"pullRequest": {
        "author": {"login": "loopbot", "avatarUrl": "https://avatars.githubusercontent.com/u/2?v=4"},
        "reviews": {"nodes": [{"author": {"login": "copilot-pull-request-reviewer", "avatarUrl": "https://avatars.githubusercontent.com/in/946600?v=4"}}]},
        "commits": {"nodes": [
            {"commit":{"oid":"aaa","url":"https://github.com/acme/app/commit/aaa","signature":{"isValid":true},"statusCheckRollup":{"state":"SUCCESS"}}},
            {"commit":{"oid":"bbb","url":"https://github.com/acme/app/commit/bbb","signature":null,"statusCheckRollup":{"state":"FAILURE"}}}
        ]}}}}}"#,
    );
    assert!(
        meta["aaa"].verified && meta["aaa"].check_status == "pass" && !meta["aaa"].url.is_empty()
    );
    assert!(!meta["bbb"].verified && meta["bbb"].check_status == "fail");
    assert_eq!(
        avatars["copilot-pull-request-reviewer"],
        "https://avatars.githubusercontent.com/in/946600?v=4"
    );
    assert_eq!(
        avatars["copilot"],
        "https://avatars.githubusercontent.com/in/946600?v=4"
    );

    let logins = commit_logins(&[
        ViewCommit {
            oid: "3f1d02babcdef".into(),
            authors: vec![
                ViewCommitAuthor {
                    name: "Josh".into(),
                    ..Default::default()
                },
                ViewCommitAuthor {
                    login: "mycodecrafting".into(),
                    name: "Josh".into(),
                },
            ],
            ..Default::default()
        },
        ViewCommit {
            oid: "deadbeef".into(),
            authors: vec![ViewCommitAuthor {
                name: "no login".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    ]);
    assert_eq!(logins["3f1d02babcdef"], "mycodecrafting");
    assert!(!logins.contains_key("deadbeef"));
}

#[test]
fn avatars() {
    assert_eq!(
        avatar_url("mycodecrafting", "", 56),
        "https://github.com/mycodecrafting.png?size=56"
    );
    assert_eq!(
        avatar_url("dependabot[bot]", "", 56),
        "https://github.com/dependabot.png?size=56"
    );
    assert_eq!(
        avatar_url(
            "dependabot[bot]",
            "https://avatars.githubusercontent.com/in/29110?v=4",
            56
        ),
        "https://avatars.githubusercontent.com/in/29110?v=4"
    );
    assert_eq!(avatar_url("A Name", "", 56), "");
    let mut comments = vec![
        PrComment {
            id: "gh-review-1".into(),
            author: "Copilot".into(),
            author_login: "copilot-pull-request-reviewer".into(),
            avatar_url: "https://github.com/copilot-pull-request-reviewer.png?size=56".into(),
            ..Default::default()
        },
        PrComment {
            id: "gh-event-work".into(),
            author: "Copilot".into(),
            author_login: "copilot-pull-request-reviewer".into(),
            ..Default::default()
        },
        PrComment {
            id: "gh-comment-1".into(),
            author: "avery".into(),
            author_login: "avery".into(),
            avatar_url: "https://github.com/avery.png?size=56".into(),
            ..Default::default()
        },
    ];
    apply_avatars(
        &mut comments,
        &HashMap::from([
            (
                "copilot-pull-request-reviewer".to_owned(),
                "https://avatars.githubusercontent.com/in/946600?v=4".to_owned(),
            ),
            (
                "copilot".to_owned(),
                "https://avatars.githubusercontent.com/in/946600?v=4".to_owned(),
            ),
            (
                "avery".to_owned(),
                "https://avatars.githubusercontent.com/u/9?v=4".to_owned(),
            ),
        ]),
    );
    assert_eq!(
        comments[0].avatar_url,
        "https://avatars.githubusercontent.com/in/946600?v=4"
    );
    assert_eq!(comments[1].avatar_url, "");
    assert_eq!(
        comments[2].avatar_url,
        "https://avatars.githubusercontent.com/u/9?v=4"
    );
}

fn row(id: &str) -> PrComment {
    PrComment {
        id: id.into(),
        ..Default::default()
    }
}

#[test]
fn merge_comments_keeps_local_and_fresh() {
    let now = at("2026-08-21T12:00:00Z");
    let existing = [
        PrComment {
            author: "gone".into(),
            ..row("gh-comment-old")
        },
        PrComment {
            author: "Coder".into(),
            ..row("c-local")
        },
    ];
    let got = merge_comments(&existing, &[row("gh-comment-new")], false, false, now);
    assert_eq!(
        got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["gh-comment-new", "c-local"]
    );

    let existing = [
        PrComment {
            time: "1d".into(),
            at: "2026-08-01T00:00:00Z".into(),
            ..row("gh-inline-old")
        },
        PrComment {
            time: "now".into(),
            path: "auth.go".into(),
            ..row("gh-inline-new")
        },
        PrComment {
            at: "2026-08-21T11:59:00Z".into(),
            time: "1m".into(),
            ..row("gh-inline-recent")
        },
        row("c-local"),
    ];
    let got = merge_comments(&existing, &[row("gh-comment-1")], false, false, now);
    assert_eq!(
        got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [
            "gh-comment-1",
            "gh-inline-new",
            "gh-inline-recent",
            "c-local"
        ]
    );
}

#[test]
fn merge_comments_keeps_older_when_truncated() {
    let now = at("2026-08-21T12:00:00Z");
    let existing: Vec<PrComment> = (1..=101).map(|i| row(&format!("gh-comment-{i}"))).collect();
    let from_gh = existing[1..].to_vec();
    let kept = merge_comments(&existing, &from_gh, true, false, now);
    assert_eq!(kept.len(), 101);
    assert!(kept.iter().any(|c| c.id == "gh-comment-1"));
    let dropped = merge_comments(&existing, &from_gh, false, false, now);
    assert_eq!(dropped.len(), 100);
    assert!(!dropped.iter().any(|c| c.id == "gh-comment-1"));
    let reviews = [row("gh-review-1")];
    assert_eq!(merge_comments(&reviews, &[], false, true, now).len(), 1);
    assert!(merge_comments(&reviews, &[], false, false, now).is_empty());
}

const SNAPSHOT: &str = r#"{
  "data": {
    "viewer": {"login": "loopbot"},
    "repository": {
      "viewerPermission": "WRITE",
      "mergeCommitAllowed": false,
      "squashMergeAllowed": true,
      "rebaseMergeAllowed": false,
      "pullRequest": {
        "id": "PR_1", "title": "Add OAuth", "body": "please review", "state": "OPEN", "reviewDecision": "APPROVED",
        "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "isDraft": false, "isMergeQueueEnabled": true, "isInMergeQueue": true,
        "url": "https://github.com/acme/app/pull/12", "headRefOid": "abc123", "headRefName": "oauth", "baseRefName": "main",
        "createdAt": "2026-08-20T10:00:00Z", "updatedAt": "2026-08-21T11:45:00Z",
        "author": {"login": "loopbot", "avatarUrl": "https://avatars.githubusercontent.com/u/1?v=4"},
        "reactionGroups": [{"content":"HOORAY","viewerHasReacted":false,"reactors":{"totalCount":3}}],
        "reviewRequests": {"nodes":[
          {"requestedReviewer":{"__typename":"User","login":"avery","avatarUrl":"https://avatars.githubusercontent.com/u/2?v=4"}},
          {"requestedReviewer":{"__typename":"Bot","login":"copilot-pull-request-reviewer","avatarUrl":"https://avatars.githubusercontent.com/in/846209?v=4"}}
        ]},
        "assignees": {"nodes":[{"login":"sam","avatarUrl":"https://avatars.githubusercontent.com/u/3?v=4"}]},
        "comments": {"nodes":[
          {"id":"IC_1","databaseId":1001,"url":"https://github.com/acme/app/pull/12#issuecomment-1001","author":{"login":"avery"},"body":"looks good","createdAt":"2026-08-21T11:00:00Z",
            "reactionGroups":[{"content":"THUMBS_UP","viewerHasReacted":true,"reactors":{"totalCount":1}}]}
        ]},
        "reviews": {"nodes":[
          {"id":"PRR_1","databaseId":99,"url":"https://github.com/acme/app/pull/12#pullrequestreview-99","author":{"login":"sam"},"body":"ship it","state":"APPROVED","submittedAt":"2026-08-21T11:30:00Z"}
        ]},
        "commits": {"nodes":[
          {"commit":{"oid":"abc123","url":"https://github.com/acme/app/commit/abc123","messageHeadline":"Add OAuth","messageBody":"","committedDate":"2026-08-20T10:01:00Z","signature":{"isValid":true},"statusCheckRollup":{"state":"SUCCESS"},"authors":{"nodes":[{"name":"Loop Bot","user":{"login":"loopbot"}}]}}}
        ]},
        "reviewThreads": {"nodes":[
          {"id":"PRRT_1","isResolved":true,"diffSide":"RIGHT","startDiffSide":"RIGHT","comments":{"nodes":[
            {"id":"PRRC_1","databaseId":42,"url":"https://github.com/acme/app/pull/12#discussion_r42","author":{"login":"avery"},"body":"nit","createdAt":"2026-08-21T11:10:00Z","path":"auth.go","line":12,"originalLine":12,"outdated":true,"diffHunk":"@@ -12 +12 @@","pullRequestReview":{"databaseId":99},
              "reactionGroups":[{"content":"EYES","viewerHasReacted":false,"reactors":{"totalCount":1}}]}
          ]}}
        ]}
      }
    }
  }
}"#;

#[test]
fn snapshot_graphql() {
    let now = at("2026-08-21T12:00:00Z");
    let gh = parse_snapshot_graphql(SNAPSHOT, now).unwrap();
    let timeline = r#"[{"event":"copilot_work_started","created_at":"2026-08-21T11:40:00Z","actor":{"login":"avery"},"performed_via_github_app":{"slug":"copilot-swe-agent"}}]"#;
    let snap = finish(gh, timeline, now);
    assert_eq!(
        (
            snap.title.as_str(),
            snap.state.as_str(),
            snap.author_login.as_str(),
            snap.node_id.as_str()
        ),
        ("Add OAuth", "OPEN", "loopbot", "PR_1")
    );
    assert!(snap.merge_queue_enabled && snap.merge_queued);
    assert!(snap.viewer_login == "loopbot" && snap.viewer_can_write);
    assert_eq!(snap.allowed_merge_methods, ["squash"]);
    assert_eq!(
        snap.review_requests
            .iter()
            .map(|p| p.login.as_str())
            .collect::<Vec<_>>(),
        ["avery", "copilot-pull-request-reviewer"]
    );
    assert_eq!(snap.assignees.len(), 1);
    assert_eq!(snap.assignees[0].login, "sam");
    assert!(
        snap.commit_meta["abc123"].verified && snap.commit_meta["abc123"].check_status == "pass"
    );
    assert_eq!(
        by_id(&snap.comments, "gh-description").reactions,
        [rx("hooray", 3, false)]
    );
    let c = by_id(&snap.comments, "gh-comment-IC_1");
    assert_eq!(c.body_md, "looks good");
    assert_eq!(c.reactions, [rx("+1", 1, true)]);
    let r = by_id(&snap.comments, "gh-review-PRR_1");
    assert_eq!(
        (r.body_md.as_str(), r.verb.as_str()),
        ("ship it", "approved")
    );
    let i = by_id(&snap.comments, "gh-inline-42");
    assert_eq!(
        (i.path.as_str(), i.line, i.side.as_str(), i.outdated),
        ("auth.go", 12, "RIGHT", true)
    );
    assert_eq!((i.thread_id.as_str(), i.resolved), ("PRRT_1", true));
    assert_eq!(i.reactions, [rx("eyes", 1, false)]);
    assert_eq!(i.review_id, "99");
    assert!(snap.comments.iter().any(|c| c.kind == "action"
        && c.body_md == "started work on behalf of avery"
        && c.author == "Copilot"));
    assert_eq!(
        snap.author_avatar_url,
        "https://avatars.githubusercontent.com/u/1?v=4"
    );
    assert_eq!(snap.commits.len(), 1);
    assert_eq!(
        (
            snap.commits[0].author.as_str(),
            snap.commits[0].hash.as_str()
        ),
        ("loopbot", "abc123")
    );
    assert_eq!(snap.commit_authors["abc123"].name, "loopbot");
    let enriched = enrich_commits(&snap.commits, &snap);
    assert!(
        enriched[0].verified && enriched[0].avatar_url == "https://github.com/loopbot.png?size=56"
    );
}

#[test]
fn snapshot_auto_merge_and_left_threads() {
    for (enabled, want) in [(false, false), (true, true)] {
        let gql = format!(
            r#"{{"data":{{"viewer":{{"login":"me"}},"repository":{{"pullRequest":{{
            "id":"PR_1","state":"OPEN","url":"https://github.com/acme/app/pull/1",
            "isMergeQueueEnabled":{enabled},"isInMergeQueue":false,"autoMergeRequest":{{"enabledAt":"2026-08-21T11:00:00Z"}}}}}}}}}}"#
        );
        assert_eq!(
            parse_snapshot_graphql(&gql, Utc::now())
                .unwrap()
                .snap
                .merge_queued,
            want
        );
    }
    let null_auto = r#"{"data":{"repository":{"pullRequest":{"id":"PR_1","isMergeQueueEnabled":true,"autoMergeRequest":null}}}}"#;
    assert!(
        !parse_snapshot_graphql(null_auto, Utc::now())
            .unwrap()
            .snap
            .merge_queued
    );

    let gql = r#"{"data":{"repository":{"pullRequest":{"id":"PR_1","title":"Fix","url":"https://github.com/acme/app/pull/12",
        "reviewThreads":{"nodes":[{"id":"PRRT_L","diffSide":"LEFT","startDiffSide":"LEFT","comments":{"nodes":[
          {"databaseId":7,"url":"https://github.com/acme/app/pull/12#discussion_r7","author":{"login":"avery"},"body":"gone","createdAt":"2026-08-21T11:10:00Z","path":"old.go","line":9,"originalLine":9,"startLine":7,"originalStartLine":7,"diffHunk":"@@ -7,3 +7,3 @@"}
        ]}}]}}}}}"#;
    let snap = parse_snapshot_graphql(gql, at("2026-08-21T12:00:00Z"))
        .unwrap()
        .snap;
    let i = by_id(&snap.comments, "gh-inline-7");
    assert_eq!(
        (
            i.side.as_str(),
            i.start_side.as_str(),
            i.path.as_str(),
            i.line,
            i.start_line
        ),
        ("LEFT", "LEFT", "old.go", 9, 7)
    );
}

#[test]
fn snapshot_errors_and_truncation() {
    let now = at("2026-08-21T12:00:00Z");
    let perm = r#"{"data": {"viewer": {"login": "loopbot"}, "repository": {"viewerPermission": "WRITE", "pullRequest": {
        "id": "PR_1", "title": "Add OAuth", "state": "OPEN", "url": "https://github.com/acme/app/pull/12", "author": {"login": "loopbot"},
        "comments": {"nodes":[{"id":"IC_1","url":"https://github.com/acme/app/pull/12#issuecomment-1001","author":{"login":"avery"},"body":"looks good","createdAt":"2026-08-21T11:00:00Z"}]},
        "commits": {"nodes":[{"commit":{"oid":"abc123","messageHeadline":"Add OAuth","committedDate":"2026-08-20T10:01:00Z","authors":{"nodes":[{"name":"Loop Bot","user":{"login":"loopbot"}}]}}}]}}}},
        "errors": [{"message":"Resource not accessible by personal access token","path":["repository","pullRequest","commits","nodes",0,"commit","statusCheckRollup"]}]}"#;
    let snap = parse_snapshot_graphql(perm, now).unwrap().snap;
    assert_eq!(
        (snap.title.as_str(), snap.state.as_str()),
        ("Add OAuth", "OPEN")
    );
    by_id(&snap.comments, "gh-comment-IC_1");

    let truncated = r#"{"data": {"repository": {"pullRequest": {"id": "PR_1", "title": "Busy", "url": "https://github.com/acme/app/pull/12",
        "comments": {"pageInfo": {"hasPreviousPage": true}, "nodes":[{"id":"IC_new","url":"https://github.com/acme/app/pull/12#issuecomment-2","author":{"login":"avery"},"body":"newest","createdAt":"2026-08-21T11:00:00Z"}]},
        "reviews": {"pageInfo": {"hasPreviousPage": true}, "nodes":[]}}}}}"#;
    let snap = parse_snapshot_graphql(truncated, now).unwrap().snap;
    assert!(snap.comments_truncated && snap.reviews_truncated);

    assert!(parse_snapshot_graphql("", now).is_err());
    assert_eq!(
        parse_snapshot_graphql(r#"{"data":{"repository":{}},"errors":[{"message":"Could not resolve to a PullRequest"}]}"#, now).unwrap_err(),
        "pull request graphql: Could not resolve to a PullRequest"
    );
}

#[test]
fn snapshot_query_variants_and_timeline_pages() {
    assert!(snapshot_query(true).contains("statusCheckRollup"));
    let got = snapshot_query(false);
    assert!(!got.contains("statusCheckRollup"));
    assert!(
        got.contains("reviewThreads")
            && got.contains("comments")
            && got.contains("pageInfo { hasPreviousPage }")
    );
    assert_eq!(
        timeline_args("acme", "app", 12).join(" "),
        "api --paginate --slurp repos/acme/app/issues/12/timeline?per_page=100 -H Accept: application/vnd.github+json"
    );
    let flat = r#"[{"event":"committed","sha":"aaa"}]"#;
    assert_eq!(flatten_json_array_pages(flat), flat);
    let got = flatten_json_array_pages(
        r#"[[{"event":"committed","sha":"old"}],[{"event":"committed","sha":"new"}]]"#,
    );
    let acts = parse_timeline(
        &got,
        at("2026-08-21T12:00:00Z"),
        &HashMap::new(),
        &HashMap::new(),
        "",
        "",
        "",
    );
    assert_eq!(
        acts.iter()
            .map(|c| c.commit_sha.as_str())
            .collect::<Vec<_>>(),
        ["old", "new"]
    );
    assert!(
        permission_can_write("WRITE")
            && permission_can_write("admin")
            && permission_can_write("MAINTAIN")
    );
    assert!(
        !permission_can_write("READ")
            && !permission_can_write("TRIAGE")
            && !permission_can_write("")
    );
}

#[test]
fn reactions_parse_apply_and_subjects() {
    let raw = r#"{"data": {"repository": {"pullRequest": {"id": "PR_1",
        "reactionGroups": [{"content":"HOORAY","viewerHasReacted":false,"reactors":{"totalCount":3}}],
        "comments": {"nodes": [{"id":"IC_1","databaseId":1001,"reactionGroups":[
          {"content":"THUMBS_UP","viewerHasReacted":false,"reactors":{"totalCount":2}},
          {"content":"HEART","viewerHasReacted":true,"reactors":{"totalCount":1}}]}]},
        "reviews": {"nodes": [{"id":"PRR_1","databaseId":99,"reactionGroups":[{"content":"ROCKET","viewerHasReacted":true,"reactors":{"totalCount":1}}]}]},
        "reviewThreads": {"nodes": [{"comments":{"nodes":[{"id":"PRRC_1","databaseId":1,"reactionGroups":[{"content":"EYES","viewerHasReacted":false,"reactors":{"totalCount":1}}]}]}}]}
    }}}}"#;
    let (hits, node) = parse_reactions(raw).unwrap();
    assert_eq!(node, "PR_1");
    let mut cs = vec![
        row("gh-description"),
        row("gh-comment-IC_1"),
        row("gh-review-PRR_1"),
        PrComment {
            kind: "action".into(),
            ..row("gh-review-action-99")
        },
        row("gh-inline-1"),
    ];
    apply_reactions(&mut cs, &hits);
    assert_eq!(
        (cs[0].node_id.as_str(), cs[0].reactions.clone()),
        ("PR_1", vec![rx("hooray", 3, false)])
    );
    assert_eq!(
        (cs[1].node_id.as_str(), cs[1].reactions.clone()),
        ("IC_1", vec![rx("+1", 2, false), rx("heart", 1, true)])
    );
    assert_eq!(
        (cs[2].node_id.as_str(), cs[2].reactions.clone()),
        ("PRR_1", vec![rx("rocket", 1, true)])
    );
    assert_eq!(
        (cs[3].node_id.as_str(), cs[3].reactions.clone()),
        ("PRR_1", vec![rx("rocket", 1, true)])
    );
    assert_eq!(
        (cs[4].node_id.as_str(), cs[4].reactions.clone()),
        ("PRRC_1", vec![rx("eyes", 1, false)])
    );
    assert!(parse_reactions("").is_err());

    let pr = Pr {
        node_id: "PR_1".into(),
        ..Default::default()
    };
    assert_eq!(
        reaction_subject_id(&row("gh-description"), &pr).as_deref(),
        Some("PR_1")
    );
    let with_node = PrComment {
        node_id: "IC_1".into(),
        ..row("gh-comment-IC_1")
    };
    assert_eq!(
        reaction_subject_id(&with_node, &pr).as_deref(),
        Some("IC_1")
    );
    assert_eq!(
        reaction_subject_id(&row("gh-review-PRR_9"), &pr).as_deref(),
        Some("PRR_9")
    );
    assert_eq!(reaction_subject_id(&row("gh-inline-1"), &pr), None);
}

#[test]
fn mentionable_users_parse() {
    let nodes = r#"{"data":{"repository":{"mentionableUsers":{"nodes":[{"login":"octocat","name":"The Octocat","avatarUrl":"https://avatars.githubusercontent.com/u/1"},{"login":"","name":"No Login"},{"login":"hubot","name":"Hubot","avatarUrl":""}]}}}}"#;
    assert_eq!(
        parse_mentionable_users(nodes),
        [
            PrParticipant {
                login: "octocat".into(),
                name: "The Octocat".into(),
                avatar_url: "https://avatars.githubusercontent.com/u/1".into(),
                ..Default::default()
            },
            PrParticipant {
                login: "hubot".into(),
                name: "Hubot".into(),
                avatar_url: avatar_url("hubot", "", 56),
                ..Default::default()
            },
        ]
    );
    assert!(parse_mentionable_users("").is_empty());
    assert!(parse_mentionable_users("{").is_empty());
}

fn check(name: &str, detail: &str, passed: bool, status: &str, url: &str, started: &str) -> Check {
    Check {
        name: name.into(),
        detail: detail.into(),
        passed,
        status: status.into(),
        url: url.into(),
        started_at: started.into(),
    }
}

#[test]
fn cli_checks() {
    let now = at("2026-08-21T12:00:45Z");
    assert_eq!(parse_cli_checks("", now).unwrap(), (vec![], true));
    assert_eq!(
        parse_cli_checks("no checks reported on the \"recur/foo\" branch", now).unwrap(),
        (vec![], true)
    );
    let (got, ok) = parse_cli_checks(
        r#"[
        {"name":"test","state":"SUCCESS","bucket":"pass","workflow":"CI","description":"142 passed","startedAt":"2026-08-21T12:00:00Z","completedAt":"2026-08-21T12:01:04Z","link":"https://github.com/acme/app/actions/runs/1"},
        {"name":"lint","state":"FAILURE","bucket":"fail","workflow":"CI","description":"1 error","startedAt":"2026-08-21T12:00:00Z","completedAt":"2026-08-21T12:00:12Z"},
        {"name":"build","state":"PENDING","bucket":"pending","workflow":"CI","description":"","startedAt":"2026-08-21T12:00:00Z"},
        {"name":"deploy","state":"SKIPPED","bucket":"skipping","workflow":"Release","description":""}
    ]"#,
        now,
    )
    .unwrap();
    assert_eq!(
        got,
        [
            check(
                "test",
                "1m 4s",
                true,
                "pass",
                "https://github.com/acme/app/actions/runs/1",
                "2026-08-21T12:00:00Z"
            ),
            check("lint", "12s", false, "fail", "", "2026-08-21T12:00:00Z"),
            check("build", "45s", false, "pending", "", "2026-08-21T12:00:00Z"),
            check("deploy", "skipped", false, "skipping", "", ""),
        ]
    );
    assert!(!ok);
    let (got, ok) = parse_cli_checks(
        r#"[{"name":"test","state":"SUCCESS","bucket":"pass","startedAt":"2026-08-21T12:00:00Z","completedAt":"2026-08-21T12:00:08Z"},
            {"name":"deploy","state":"SKIPPED","bucket":"skipping"}]"#,
        now,
    )
    .unwrap();
    assert!(ok && got[0].detail == "8s");
    assert_eq!(
        parse_cli_checks(
            r#"[{"name":"ci","state":"IN_PROGRESS","workflow":"CI"}]"#,
            now
        )
        .unwrap(),
        (vec![check("ci", "pending", false, "pending", "", "")], true)
    );
    assert_eq!(
        parse_cli_checks(r#"[{"name":"license","state":"EXPECTED"}]"#, now).unwrap(),
        (
            vec![check("license", "pending", false, "pending", "", "")],
            true
        )
    );
    assert_eq!(
        parse_cli_checks(r#"[{"name":"ci","state":"STALE"}]"#, now).unwrap(),
        (
            vec![check("ci", "cancelled", false, "cancel", "", "")],
            false
        )
    );
    assert!(parse_cli_checks("GraphQL: Could not resolve to a PullRequest", now).is_err());
}

#[test]
fn graphql_checks_include_pending() {
    let now = at("2026-08-21T12:00:45Z");
    let raw = r#"{"data": {"repository": {"pullRequest": {"url": "https://github.com/acme/app/pull/11", "commits": {"nodes": [{"commit": {
        "statusCheckRollup": {"contexts": {"nodes": [
            {"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS", "detailsUrl": "https://github.com/acme/app/actions/runs/1/job/2", "permalink": "https://github.com/acme/app/runs/99", "startedAt": "2026-08-21T12:00:00Z", "checkSuite": {"workflowRun": {"workflow": {"name": "CI"}}}},
            {"__typename": "StatusContext", "context": "license/snyk", "state": "EXPECTED", "description": "Waiting for status to be reported"}
        ]}},
        "checkSuites": {"nodes": [
            {"status": "QUEUED", "app": {"name": "GitHub Actions"}, "workflowRun": {"url": "https://github.com/acme/app/actions/runs/2", "workflow": {"name": "Release"}}, "checkRuns": {"totalCount": 0}},
            {"status": "IN_PROGRESS", "workflowRun": {"workflow": {"name": "CI"}}, "checkRuns": {"totalCount": 1}}
        ]}}}]}}}}}"#;
    let (got, ok) = parse_graphql_checks(raw, now).unwrap();
    assert!(ok);
    assert_eq!(
        got,
        [
            check(
                "build",
                "45s",
                false,
                "pending",
                "https://github.com/acme/app/runs/99",
                "2026-08-21T12:00:00Z"
            ),
            check(
                "license/snyk",
                "pending",
                false,
                "pending",
                "https://github.com/acme/app/pull/11/checks",
                ""
            ),
            check(
                "Release",
                "pending",
                false,
                "pending",
                "https://github.com/acme/app/pull/11/checks",
                ""
            ),
        ]
    );
}

#[test]
fn graphql_checks_required_from_rulesets() {
    let now = at("2026-08-21T12:00:45Z");
    let raw = r#"{"data": {"repository": {"defaultBranchRef": {"name": "main"},
        "rulesets": {"nodes": [{"enforcement": "ACTIVE", "target": "BRANCH", "conditions": {"refName": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
            "rules": {"nodes": [{"type": "REQUIRED_STATUS_CHECKS", "parameters": {"requiredStatusChecks": [{"context": "CI Success"}]}}]}}]},
        "pullRequest": {"url": "https://github.com/acme/apollo/pull/204", "baseRef": {"name": "main"},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": null, "checkSuites": {"nodes": [
                {"status": "QUEUED", "app": {"name": "Depot Code Access"}, "workflowRun": null, "checkRuns": {"totalCount": 0}}]}}}]}}}}}"#;
    let (got, ok) = parse_graphql_checks(raw, now).unwrap();
    assert!(ok);
    assert_eq!(got.len(), 1);
    assert_eq!(
        (got[0].name.as_str(), got[0].status.as_str()),
        ("CI Success", "pending")
    );
    assert_eq!(got[0].url, "https://github.com/acme/apollo/pull/204/checks");
    let other = raw.replacen(
        r#""baseRef": {"name": "main"}"#,
        r#""baseRef": {"name": "develop"}"#,
        1,
    );
    assert_eq!(parse_graphql_checks(&other, now).unwrap(), (vec![], true));
}

#[test]
fn graphql_checks_errors_and_truncation() {
    let now = Utc::now();
    assert!(parse_graphql_checks(r#"{"data":{"repository":{"pullRequest":null}}}"#, now).is_err());
    assert!(
        parse_graphql_checks(
            r#"{"data": {"repository": {"pullRequest": {"url": "https://github.com/acme/app/pull/1", "commits": {"nodes": []}}}},
            "errors": [{"message": "Field 'rulesets' doesn't exist on type 'Repository'"}]}"#,
            now
        )
        .is_err()
    );
    let (got, ok) = parse_graphql_checks(
        r#"{"data": {"repository": {"pullRequest": {"url": "https://github.com/acme/app/pull/1",
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"pageInfo": {"hasNextPage": true},
        "nodes": [{"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"}]}}}}]}}}}}"#,
        now,
    )
    .unwrap();
    assert!(!ok && got.len() == 1 && got[0].name == "build" && got[0].passed);
    let (got, ok) = parse_graphql_checks(
        r#"{"data": {"repository": {"pullRequest": {"url": "https://github.com/acme/app/pull/1",
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [{"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS"}]}}}}]}}}},
        "errors": [{"message": "Field 'rulesets' doesn't exist on type 'Repository'"}]}gh: Field 'rulesets' doesn't exist on type 'Repository'"#,
        now,
    )
    .unwrap();
    assert!(ok && got.len() == 1 && got[0].status == "pending");

    let parse_err =
        "parse github checks graphql: invalid character 'g' looking for beginning of value"
            .to_owned();
    let got = graphql_command_error(
        "gh: authentication required for host github.com",
        Some("exit status 1"),
        parse_err.clone(),
    );
    assert!(got.contains("authentication required") && !got.contains("invalid character"));
    assert_eq!(
        graphql_command_error("", None, parse_err.clone()),
        parse_err
    );
}

#[test]
fn ref_globs() {
    assert!(ref_glob("refs/heads/**", "refs/heads/release/v2"));
    assert!(ruleset_applies_to_base(
        &["refs/heads/**".into()],
        &[],
        "release/v2",
        "main"
    ));
    assert!(!ref_glob("refs/heads/*", "refs/heads/release/v2"));
    assert!(ref_glob("refs/heads/naïve/*", "refs/heads/naïve/v1"));
    assert!(ref_glob("refs/heads/v?", "refs/heads/v2"));
    assert!(!ruleset_applies_to_base(
        &[],
        &["~ALL".into()],
        "main",
        "main"
    ));
    assert!(!ruleset_applies_to_base(&[], &[], "", "main"));
}

#[test]
fn checks_tab_urls() {
    let got = apply_checks_tab_urls(
        vec![
            check(
                "build",
                "",
                false,
                "pending",
                "https://github.com/acme/app/actions/runs/1/job/2",
                "",
            ),
            check(
                "lint",
                "",
                false,
                "pending",
                "https://github.com/acme/app/runs/99",
                "",
            ),
            check("test", "", false, "pending", "", ""),
            check(
                "typecheck",
                "",
                true,
                "pass",
                "https://github.com/acme/app/actions/runs/1/job/3",
                "",
            ),
        ],
        "https://github.com/acme/app/pull/11",
    );
    let urls: Vec<&str> = got.iter().map(|c| c.url.as_str()).collect();
    assert_eq!(
        urls,
        [
            "https://github.com/acme/app/pull/11/checks",
            "https://github.com/acme/app/runs/99",
            "https://github.com/acme/app/pull/11/checks",
            "https://github.com/acme/app/actions/runs/1/job/3",
        ]
    );
}

#[test]
fn subject_prompt_and_label() {
    let gh = GitHub::new();
    let s = PrSubject {
        number: 9,
        url: "https://github.com/acme/r/pull/9".into(),
        title: "Imported".into(),
        branch: "feat".into(),
        base: "main".into(),
        host: true,
    };
    let got = gh.pr_subject_prompt(&s);
    for want in [
        "GitHub pull request #9 (https://github.com/acme/r/pull/9)",
        r#"titled "Imported""#,
        "gh pr diff",
        "working copy",
        "(head feat into main)",
    ] {
        assert!(got.contains(want), "{want:?} in {got:?}");
    }
    assert!(!got.contains("workspace for that PR"));
    let worktree = gh.pr_subject_prompt(&PrSubject {
        host: false,
        ..s.clone()
    });
    assert!(worktree.contains("workspace for that PR's head"));
    assert_eq!(
        gh.pr_label(&PrSubject {
            url: String::new(),
            ..s.clone()
        }),
        "GitHub pull request #9"
    );
    assert_eq!(
        gh.pr_label(&PrSubject {
            number: 0,
            ..s.clone()
        }),
        "GitHub pull request (https://github.com/acme/r/pull/9)"
    );
    assert_eq!(
        gh.pr_label(&PrSubject::default()),
        "this GitHub pull request"
    );
    assert_eq!(gh.format_pr_body("short"), "short");
    let long = "x".repeat(60001);
    assert_eq!(
        gh.format_pr_body(&long),
        format!("{}\n\n…(truncated)…", "x".repeat(60000))
    );
    assert_eq!(
        gh.head_ref(7),
        HeadRef {
            remote_ref: "pull/7/head".into(),
            local_ref: "refs/recur/pr/7".into()
        }
    );
}

// ---- fake gh runs

struct FakeGh {
    dir: tempfile::TempDir,
    gh: GitHub,
}

impl FakeGh {
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = write_script(dir.path(), "gh", script);
        Self {
            gh: GitHub::with_gh(Gh::new(path)).with_merge_retry_delay(std::time::Duration::ZERO),
            dir,
        }
    }

    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }

    /// NUL-separated argv log written by `printf '%s\0' "$@" >> "$0.args"`.
    fn argv(&self) -> Vec<String> {
        let log = self.file("gh.args");
        log.trim_end_matches('\0')
            .split('\0')
            .map(str::to_owned)
            .collect()
    }

    fn flag_values(&self, flag: &str) -> Vec<String> {
        let args = self.argv();
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].clone())
            .collect()
    }
}

fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo on `main` with one commit.
fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "t@example.com"]);
    git_in(&repo, &["config", "user.name", "t"]);
    git_in(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README.md"), "hi\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    (dir, repo)
}

fn add_bare_origin(dir: &Path, repo: &Path) {
    let bare = dir.join("origin.git");
    git_in(dir, &["init", "-q", "--bare", bare.to_str().unwrap()]);
    git_in(repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
}

const AUTH_OK: &str = "if [ \"$1\" = \"auth\" ]; then exit 0; fi\n";

#[tokio::test]
async fn availability_is_false_without_gh_and_cached() {
    let missing = GitHub::with_gh(Gh::new("/nonexistent/gh"));
    assert!(!missing.cli_available().await);

    let fake = FakeGh::new("#!/bin/sh\necho x >> \"$0.count\"\nexit 0\n");
    assert!(fake.gh.cli_available().await && fake.gh.cli_available().await);
    assert_eq!(
        fake.file("gh.count").lines().count(),
        1,
        "second call hits the cache"
    );
    fake.gh.reset_availability();
    assert!(fake.gh.cli_available().await);
    assert_eq!(fake.file("gh.count").lines().count(), 2);
}

#[tokio::test]
async fn pr_number_for_head_uses_list_not_view() {
    let fake = FakeGh::new(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo 'unknown flag: --head' >&2; exit 1; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"list\" ]; then echo '[{\"number\":42}]'; exit 0; fi\nexit 1\n",
    );
    assert_eq!(
        fake.gh
            .pr_number_for_head(Path::new(""), "feat", "main")
            .await,
        42
    );
    let joined = fake.argv().join(" ");
    assert!(!joined.contains("pr view"));
    for want in ["pr list", "--head feat", "--base main", "--state open"] {
        assert!(joined.contains(want), "{joined}");
    }
}

fn create<'a>(repo: &'a Path, title: &'a str, body: &'a str) -> CreatePr<'a> {
    CreatePr {
        repo,
        worktree: repo,
        branch: "feat",
        base: "main",
        title,
        body,
        draft: false,
    }
}

#[tokio::test]
async fn create_surfaces_push_failure_when_pr_exists() {
    let (_d, repo) = init_repo();
    let fake = FakeGh::new(&format!(
        "#!/bin/sh\n{AUTH_OK}if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then echo 'a pull request already exists: https://github.com/owner/repo/pull/99'; exit 1; fi\nexit 1\n"
    ));
    let reason = fake
        .gh
        .create(create(&repo, "Title", "Body"))
        .await
        .unwrap_err();
    assert!(reason.contains("git push failed"), "{reason}");
}

#[tokio::test]
async fn create_caps_title_at_256_chars() {
    let (_d, repo) = init_repo();
    let fake = FakeGh::new(&format!(
        "#!/bin/sh\n{AUTH_OK}printf '%s\\0' \"$@\" >> \"$0.args\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then echo 'https://github.com/owner/repo/pull/1'; exit 0; fi\nexit 1\n"
    ));
    let title = format!("{}\n{}", "x".repeat(200), "y".repeat(200));
    let _ = fake.gh.create(create(&repo, &title, "Body")).await;
    assert_eq!(fake.flag_values("--title"), ["x".repeat(200)]);
}

#[tokio::test]
async fn create_retries_once_on_title_too_long() {
    let (d, repo) = init_repo();
    add_bare_origin(d.path(), &repo);
    let fake = FakeGh::new(&format!(
        "#!/bin/sh\n{AUTH_OK}if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then\n  n=0\n  [ -f \"$0.count\" ] && n=$(cat \"$0.count\")\n  n=$((n+1))\n  printf '%s' \"$n\" > \"$0.count\"\n  printf '%s\\0' \"$@\" >> \"$0.args\"\n  if [ \"$n\" = \"1\" ]; then echo 'Title is too long (maximum is 256 characters) (createPullRequest)' >&2; exit 1; fi\n  echo 'https://github.com/owner/repo/pull/7'\n  exit 0\nfi\nexit 1\n"
    ));
    let title = "z".repeat(400);
    assert_eq!(fake.gh.create(create(&repo, &title, "Body")).await, Ok(7));
    assert_eq!(fake.file("gh.count"), "2");
    let titles = fake.flag_values("--title");
    assert_eq!(titles.len(), 2);
    assert!(titles[1].chars().count() <= 80);
}

#[tokio::test]
async fn create_does_not_retry_other_errors() {
    let (_d, repo) = init_repo();
    let fake = FakeGh::new(&format!(
        "#!/bin/sh\n{AUTH_OK}if [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then\n  n=0\n  [ -f \"$0.count\" ] && n=$(cat \"$0.count\")\n  printf '%s' \"$((n+1))\" > \"$0.count\"\n  echo 'HTTP 401' >&2\n  exit 1\nfi\nexit 1\n"
    ));
    let res = fake
        .gh
        .create(create(
            &repo,
            "Title is too long",
            "Body mentions Title is too long (maximum is 256 characters)",
        ))
        .await;
    assert!(res.is_err());
    assert_eq!(fake.file("gh.count"), "1");
}

#[tokio::test]
async fn create_without_gh_falls_back() {
    let gh = GitHub::with_gh(Gh::new("/nonexistent/gh"));
    let (_d, repo) = init_repo();
    assert_eq!(
        gh.create(create(&repo, "T", "B")).await.unwrap_err(),
        "gh CLI is not installed or not authenticated: using a Local PR instead"
    );
}

fn repo_with_remote(name: &str, owner_repo: &str) -> (tempfile::TempDir, PathBuf) {
    let (d, repo) = init_repo();
    git_in(
        &repo,
        &[
            "remote",
            "add",
            name,
            &format!("https://github.com/{owner_repo}.git"),
        ],
    );
    (d, repo)
}

#[tokio::test]
async fn branch_rules_use_the_remote_repo() {
    let (_d, repo) = repo_with_remote("upstream", "acme/parent");
    let fake = FakeGh::new(
        "#!/bin/sh\nif [ \"$1\" = \"repo\" ]; then echo 'gh repo view must not choose the default repository' >&2; exit 1; fi\npath=''\nfor a in \"$@\"; do case \"$a\" in repos/*) path=\"$a\" ;; esac; done\ncase \"$path\" in\n  repos/acme/parent/rules/branches/main)\n    paginate=0; slurp=0\n    for a in \"$@\"; do [ \"$a\" = \"--paginate\" ] && paginate=1; [ \"$a\" = \"--slurp\" ] && slurp=1; done\n    if [ \"$paginate\" != 1 ] || [ \"$slurp\" != 1 ]; then echo 'rules endpoint must paginate' >&2; exit 1; fi\n    echo '[[{\"type\":\"deletion\"}],[{\"type\":\"pull_request\"}]]'; exit 0 ;;\n  repos/acme/parent/branches/main/protection)\n    echo 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;\nesac\necho \"unexpected gh $*\" >&2\nexit 1\n",
    );
    assert_eq!(
        fake.gh
            .owner_repo_for_remote(&repo, "upstream")
            .await
            .unwrap(),
        "acme/parent"
    );
    assert!(fake.gh.owner_repo_for_remote(&repo, "nope").await.is_err());
    assert_eq!(
        fake.gh
            .branch_requires_pr(&repo, "acme/parent", "main")
            .await,
        Some(true)
    );
}

#[tokio::test]
async fn branch_rules_classic_protection() {
    let (_d, repo) = repo_with_remote("origin", "acme/app");
    let fake = FakeGh::new(
        "#!/bin/sh\npath=''\nfor a in \"$@\"; do case \"$a\" in repos/*) path=\"$a\" ;; esac; done\ncase \"$path\" in\n  repos/acme/app/rules/branches/main) echo '[]'; exit 0 ;;\n  repos/acme/app/branches/main/protection) echo '{\"required_pull_request_reviews\":{\"required_approving_review_count\":1}}'; exit 0 ;;\nesac\necho \"unexpected gh $*\" >&2\nexit 1\n",
    );
    assert_eq!(
        fake.gh.branch_requires_pr(&repo, "acme/app", "main").await,
        Some(true)
    );
    assert_eq!(
        fake.gh.branch_requires_pr(&repo, "nope", "main").await,
        None
    );
}

#[tokio::test]
async fn branch_rules_bypassable_pr_rule() {
    let (_d, repo) = repo_with_remote("origin", "acme/app");
    let fake = FakeGh::new(
        "#!/bin/sh\npath=''\nfor a in \"$@\"; do case \"$a\" in repos/*) path=\"$a\" ;; esac; done\ncase \"$path\" in\n  repos/acme/*/rules/branches/main) echo '[[{\"type\":\"deletion\",\"ruleset_id\":5},{\"type\":\"pull_request\",\"ruleset_id\":5}]]'; exit 0 ;;\n  repos/acme/app/rulesets/5) echo '{\"current_user_can_bypass\":\"always\"}'; exit 0 ;;\n  repos/acme/lib/rulesets/5) echo '{\"current_user_can_bypass\":\"pull_requests_only\"}'; exit 0 ;;\n  repos/acme/*/branches/main/protection) echo 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;\nesac\necho \"unexpected gh $*\" >&2\nexit 1\n",
    );
    assert_eq!(
        fake.gh.branch_requires_pr(&repo, "acme/app", "main").await,
        Some(false)
    );
    assert_eq!(
        fake.gh.branch_requires_pr(&repo, "acme/lib", "main").await,
        Some(true)
    );
}

#[tokio::test]
async fn close_runs_gh_pr_close() {
    let fake = FakeGh::new("#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\n");
    fake.gh.close(Path::new(""), 12).await.unwrap();
    assert_eq!(fake.argv(), ["pr", "close", "12"]);
    assert!(fake.gh.close(Path::new(""), 0).await.is_err());
    assert_eq!(fake.argv().len(), 3, "no gh call without a PR number");
}

#[tokio::test]
async fn mention_search_passes_string_fields() {
    let fake = FakeGh::new(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\necho '{\"data\":{\"repository\":{\"mentionableUsers\":{\"nodes\":[]}}}}'\n",
    );
    for q in ["123", "true", "false", "null"] {
        fake.gh
            .mentionable_users(Path::new(""), "https://github.com/123/true/pull/1", q)
            .await;
    }
    let raw = fake.flag_values("-f");
    for want in ["o=123", "r=true", "q=123", "q=true", "q=false", "q=null"] {
        assert!(raw.contains(&want.to_owned()), "-f {want} missing: {raw:?}");
    }
    assert!(
        fake.flag_values("-F")
            .iter()
            .all(|f| !f.starts_with("o=") && !f.starts_with("r=") && !f.starts_with("q="))
    );
    let before = fake.argv().len();
    assert!(
        fake.gh
            .mentionable_users(Path::new(""), "not-a-pr", "octo")
            .await
            .is_empty()
    );
    assert_eq!(fake.argv().len(), before, "no gh call without a PR URL");
}

#[tokio::test]
async fn merge_retries_then_enqueues_through_graphql() {
    let fake = FakeGh::new(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"-X\" ]; then\n  n=0\n  [ -f \"$0.put\" ] && n=$(cat \"$0.put\")\n  n=$((n+1)); printf '%s' \"$n\" > \"$0.put\"\n  if [ \"$n\" = \"1\" ]; then echo 'gh: Pull Request is not mergeable (HTTP 405)' >&2; exit 1; fi\n  echo 'gh: Changes must be made through the merge queue (HTTP 405)' >&2; exit 1\nfi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"merge\" ]; then echo 'GraphQL: Auto merge is not allowed for this repository (enablePullRequestAutoMerge)' >&2; exit 1; fi\nif [ \"$1\" = \"api\" ] && [ \"$2\" = \"graphql\" ]; then cat > \"$0.stdin\"; echo '{\"data\":{}}'; exit 0; fi\nexit 1\n",
    );
    let pr = Pr {
        number: 20,
        url: "https://github.com/acme/app/pull/20".into(),
        node_id: "PR_20".into(),
        ..Default::default()
    };
    assert_eq!(
        fake.gh
            .merge(Path::new(""), &pr, &MergeMethod::Squash)
            .await,
        Ok(MergeOutcome::Queued)
    );
    assert_eq!(fake.file("gh.put"), "2");
    assert_eq!(
        fake.file("gh.stdin"),
        r#"{"query":"mutation($id:ID!){enqueuePullRequest(input:{pullRequestId:$id}){mergeQueueEntry{id}}}","variables":{"id":"PR_20"}}"#
    );
}

#[tokio::test]
async fn graphql_mutation_errors_are_failures() {
    let fake = FakeGh::new(
        "#!/bin/sh\ncat > \"$0.stdin\"\necho '{\"errors\":[{\"message\":\"nope\"}]}'\n",
    );
    let err = fake
        .gh
        .set_thread_resolved(Path::new(""), "PRRT_1", false)
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        r#"gh api graphql unresolveReviewThread: {"errors":[{"message":"nope"}]}"#
    );
    assert_eq!(
        fake.file("gh.stdin"),
        r#"{"query":"mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{isResolved}}}","variables":{"id":"PRRT_1"}}"#
    );
    assert_eq!(
        fake.gh
            .set_thread_resolved(Path::new(""), " ", true)
            .await
            .unwrap_err()
            .message,
        "empty review thread id"
    );
}

#[tokio::test]
async fn checks_fall_back_to_cli_and_link_the_tab() {
    let fake = FakeGh::new(&format!(
        "#!/bin/sh\n{AUTH_OK}if [ \"$1\" = \"repo\" ]; then echo '{{\"nameWithOwner\":\"acme/app\"}}'; exit 0; fi\nif [ \"$1\" = \"api\" ]; then echo 'gh: Something went wrong' >&2; exit 1; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"checks\" ]; then echo '[{{\"name\":\"ci\",\"state\":\"PENDING\",\"bucket\":\"pending\"}}]'; exit 8; fi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{{\"url\":\"https://github.com/acme/app/pull/3\"}}'; exit 0; fi\nexit 1\n"
    ));
    let got = fake.gh.checks(Path::new(""), 3, "").await.unwrap();
    assert!(got.ok);
    assert_eq!(
        got.checks[0].url,
        "https://github.com/acme/app/pull/3/checks"
    );
    assert!(fake.gh.checks(Path::new(""), 0, "").await.is_err());
}

#[tokio::test]
async fn review_with_line_notes_posts_json() {
    let fake = FakeGh::new(
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"url\":\"https://github.com/acme/app/pull/5\",\"headRefOid\":\"abc\"}'; exit 0; fi\nprintf '%s\\0' \"$@\" >> \"$0.args\"\ncat > \"$0.stdin\"\necho '{}'\n",
    );
    let pending = [
        recur_domain::PendingLineComment {
            file: "a.go".into(),
            line: 9,
            start_line: 7,
            side: "left".into(),
            body: "look".into(),
            ..Default::default()
        },
        recur_domain::PendingLineComment {
            file: "b.go".into(),
            line: 0,
            body: "skipped".into(),
            ..Default::default()
        },
    ];
    fake.gh
        .submit_review(Path::new(""), 5, "approve", "", &pending)
        .await
        .unwrap();
    assert_eq!(
        fake.argv(),
        [
            "api",
            "repos/acme/app/pulls/5/reviews",
            "--method",
            "POST",
            "--input",
            "-"
        ]
    );
    assert_eq!(
        fake.file("gh.stdin"),
        r#"{"commit_id":"abc","body":"Approved","event":"APPROVE","comments":[{"path":"a.go","line":9,"side":"LEFT","body":"look","start_line":7,"start_side":"LEFT"}]}"#
    );
}

#[tokio::test]
async fn edits_and_deletes_route_by_comment_id() {
    let fake = FakeGh::new(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo '{\"url\":\"https://github.com/acme/app/pull/5\"}'; exit 0; fi\n[ \"$2\" = \"graphql\" ] && cat >> \"$0.stdin\"\necho '{}'\n",
    );
    let repo = Path::new("");
    fake.gh
        .edit_comment(repo, 5, "gh-inline-77", " new ")
        .await
        .unwrap();
    assert!(
        fake.argv()
            .join(" ")
            .contains("api repos/acme/app/pulls/comments/77 -X PATCH -f body=new")
    );
    fake.gh
        .edit_comment(repo, 5, "gh-review-PRR_1", "body")
        .await
        .unwrap();
    assert!(
        fake.file("gh.stdin")
            .contains("updatePullRequestReview(input:{pullRequestReviewId:$id,body:$body})")
    );
    fake.gh
        .delete_comment(repo, 5, "gh-inline-77")
        .await
        .unwrap();
    assert!(
        fake.argv()
            .join(" ")
            .contains("api repos/acme/app/pulls/comments/77 -X DELETE")
    );
    assert_eq!(
        fake.gh
            .delete_comment(repo, 5, "gh-review-1")
            .await
            .unwrap_err()
            .message,
        "unsupported comment id gh-review-1"
    );
    assert_eq!(
        fake.gh
            .edit_comment(repo, 5, "gh-inline-x", "b")
            .await
            .unwrap_err()
            .message,
        "invalid inline comment id \"x\""
    );
    assert_eq!(
        fake.gh
            .edit_comment(repo, 5, "gh-comment-1", " ")
            .await
            .unwrap_err()
            .message,
        "empty comment"
    );
}

#[tokio::test]
async fn reply_returns_the_posted_row() {
    let fake = FakeGh::new(
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ]; then echo '{\"url\":\"https://github.com/acme/app/pull/5\"}'; exit 0; fi\necho '{\"id\": 999, \"body\": \"thanks\", \"path\": \"a.go\", \"line\": 3, \"in_reply_to_id\": 1, \"created_at\": \"2026-08-21T11:59:00Z\", \"user\": {\"login\": \"avery\"}}'\n",
    );
    let got = fake
        .gh
        .reply(
            Path::new(""),
            5,
            "gh-inline-1",
            "thanks",
            "https://github.com/acme/app/pull/5",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (got.id.as_str(), got.in_reply_to_id.as_str()),
        ("gh-inline-999", "gh-inline-1")
    );
    assert_eq!(
        got.url,
        "https://github.com/acme/app/pull/5#discussion_r999"
    );
}

#[tokio::test]
async fn post_comment_falls_back_to_conversation() {
    let fake = FakeGh::new(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then echo 'boom' >&2; exit 1; fi\nexit 0\n",
    );
    fake.gh
        .post_comment(Path::new(""), 5, "a.go", 3, " hi ")
        .await
        .unwrap();
    assert_eq!(fake.flag_values("--body"), ["a.go:3\n\nhi"]);
}

#[tokio::test]
async fn list_and_state_probe() {
    let fake = FakeGh::new(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> \"$0.args\"\nif [ \"$2\" = \"list\" ]; then echo '[{\"number\":4,\"title\":\"T\",\"author\":{\"login\":\"sam\"},\"state\":\"OPEN\"}]'; exit 0; fi\necho '{\"state\":\"closed\",\"updatedAt\":\"2026-08-21T11:00:00Z\"}'\n",
    );
    let rows = fake
        .gh
        .list_prs(Path::new("/tmp"), PrListState::Merged, 20)
        .await
        .unwrap();
    assert_eq!((rows[0].number, rows[0].author_login.as_str()), (4, "sam"));
    assert!(
        fake.argv()
            .join(" ")
            .contains("--state merged --search sort:updated-desc")
    );
    assert_eq!(
        fake.gh.pr_state(Path::new(""), 4).await.unwrap().state,
        "CLOSED"
    );
    assert!(
        fake.gh
            .list_prs(Path::new(""), PrListState::Open, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn snapshot_fetch_combines_graphql_and_timeline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("snap.json"), SNAPSHOT).unwrap();
    let fake = FakeGh::new(&format!(
        "#!/bin/sh\nif [ \"$1\" = \"repo\" ]; then echo '{{\"nameWithOwner\":\"acme/app\"}}'; exit 0; fi\nif [ \"$2\" = \"graphql\" ]; then cat '{}'; exit 0; fi\necho '[[{{\"event\":\"reopened\",\"created_at\":\"2026-08-21T11:50:00Z\",\"actor\":{{\"login\":\"sam\"}}}}]]'\n",
        dir.path().join("snap.json").display()
    ));
    let snap = fake.gh.snapshot(Path::new(""), 12).await.unwrap();
    assert!(snap.comments.iter().any(|c| c.body_md == "reopened this"));
    assert_eq!(snap.comments[0].id, "gh-description");
    assert!(fake.gh.snapshot(Path::new(""), 0).await.is_err());
}

#[test]
fn actor_avatar_prefers_camel_case() {
    let a = Actor {
        avatar_url: " a ".into(),
        avatar_url_snake: "b".into(),
        ..Default::default()
    };
    assert_eq!(a.avatar(), "a");
}
