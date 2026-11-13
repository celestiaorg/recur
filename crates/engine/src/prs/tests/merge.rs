//! Merging, commit messages and the PR rows a walk opens.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::{Provider, TurnRequest};
use recur_domain::{
    ATTENTION_REVIEW_REQUESTED, AutonomyLevel, Commit, DiffLine, FileDiff, MergeCommitMessage,
    MergeMethod, PendingLineComment, Pr, PrComment, PrSource, SquashCommitMessage, Workspace,
    WorkspaceState,
};
use recur_forge::Forge;
use recur_forge::model::{pr_description, single_line};

use super::support::*;
use crate::loops::open_pr::pr_fallback_markdown;
use crate::loops::test_support::{AsyncProvider, ConvergeRunner, success};
use crate::prs::merge::local_merge_commit_message;
use crate::prs::ops::{comment_hunk_from_files, pending_loc};
use crate::prs::refresh::{commit_ref_matches, merge_commit_log};
use crate::prs::workspace::workspace_merge_commit_message;
use crate::testing::{TestEngine, add_project, git_out, init_repo, wait_for};

fn message_pr() -> Pr {
    let commit = |m: &str| Commit {
        message: m.into(),
        ..Commit::default()
    };
    Pr {
        number: 42,
        branch: "recur/pepper-stew".into(),
        title: "Fix squash commit messages".into(),
        commits: Some(vec![
            commit("recur(Reviewer): nits"),
            commit("recur(Coder): use PR title for squash\n\nbody"),
            commit(""),
        ]),
        comments: Some(vec![PrComment {
            kind: "description".into(),
            body_md: "Prefer GitHub-style squash messages.".into(),
            ..PrComment::default()
        }]),
        ..Pr::default()
    }
}

#[test]
fn local_merge_commit_messages() {
    let pr = message_pr();
    let (sq, mg) = (
        SquashCommitMessage::TitleAndCommits,
        MergeCommitMessage::Title,
    );
    let squash = local_merge_commit_message(&pr, &MergeMethod::Squash, &sq, &mg);
    assert_eq!(
        squash,
        "Fix squash commit messages\n\n* recur(Coder): use PR title for squash\n* recur(Reviewer): nits\n\nRecur Local PR #42"
    );
    assert!(!squash.contains("pepper-stew"));
    assert_eq!(
        local_merge_commit_message(&pr, &MergeMethod::Squash, &SquashCommitMessage::Title, &mg),
        "Fix squash commit messages\n\nRecur Local PR #42"
    );
    assert_eq!(
        local_merge_commit_message(
            &pr,
            &MergeMethod::Squash,
            &SquashCommitMessage::TitleAndDescription,
            &mg
        ),
        "Fix squash commit messages\n\nPrefer GitHub-style squash messages.\n\nRecur Local PR #42"
    );
    assert_eq!(
        local_merge_commit_message(&pr, &MergeMethod::Merge, &sq, &mg),
        "Merge Local PR #42: Fix squash commit messages"
    );
    assert_eq!(
        local_merge_commit_message(
            &pr,
            &MergeMethod::Merge,
            &sq,
            &MergeCommitMessage::TitleAndDescription
        ),
        "Merge Local PR #42: Fix squash commit messages\n\nPrefer GitHub-style squash messages."
    );
    let empty = Pr {
        number: 7,
        branch: "recur/mystery".into(),
        ..Pr::default()
    };
    let got = local_merge_commit_message(&empty, &MergeMethod::Squash, &sq, &mg);
    assert!(
        got.starts_with("Workflow changes") && !got.contains("mystery"),
        "{got}"
    );
    let multi = Pr {
        number: 1,
        title: "Ship it\n\nmore detail".into(),
        ..Pr::default()
    };
    assert_eq!(
        local_merge_commit_message(&multi, &MergeMethod::Merge, &sq, &mg),
        "Merge Local PR #1: Ship it"
    );
    assert_eq!(
        local_merge_commit_message(&pr, &MergeMethod::from("nope"), &sq, &mg),
        "Merge Local PR #42: Fix squash commit messages",
        "an unknown method keeps the merge-commit fail-safe"
    );
    assert_eq!(
        local_merge_commit_message(&pr, &MergeMethod::Rebase, &sq, &mg),
        "",
        "rebase carries no message"
    );
}

#[test]
fn forge_merge_messages() {
    let forge = recur_forge::testing::FakeForge::new();
    let (sq, mg) = (
        SquashCommitMessage::TitleAndCommits,
        MergeCommitMessage::Title,
    );
    let gh = Pr {
        source: PrSource::GitHub,
        ..message_pr()
    };
    let got = forge.merge_message(&gh, &MergeMethod::Squash, &sq, &mg);
    assert_eq!(got.subject, "Fix squash commit messages (#42)");
    assert!(!got.combined().contains("Recur Local PR"));
    assert_eq!(
        got.body,
        "* recur(Coder): use PR title for squash\n* recur(Reviewer): nits"
    );
    assert!(!got.omit_body);
    let bare = Pr {
        number: 9,
        source: PrSource::GitHub,
        title: "Live title".into(),
        ..Pr::default()
    };
    let got = forge.merge_message(&bare, &MergeMethod::Squash, &sq, &mg);
    assert_eq!(got.subject, "Live title (#9)");
    assert!(got.omit_body);
    let untitled = Pr {
        number: 3,
        source: PrSource::GitHub,
        ..Pr::default()
    };
    assert_eq!(
        forge
            .merge_message(&untitled, &MergeMethod::Squash, &sq, &mg)
            .subject,
        ""
    );
    let got = forge.merge_message(&gh, &MergeMethod::Merge, &sq, &mg);
    assert_eq!(got.subject, "Fix squash commit messages (#42)");
    assert_eq!(got.body, "");
}

#[test]
fn workspace_merge_commit_messages() {
    let ws = Workspace {
        goal: "Ship dark mode".into(),
        ..Workspace::default()
    };
    let commits = vec![
        Commit {
            message: "wire settings toggle".into(),
            ..Commit::default()
        },
        Commit {
            message: "add theme tokens".into(),
            ..Commit::default()
        },
    ];
    assert_eq!(
        workspace_merge_commit_message(
            &ws,
            "main",
            &MergeMethod::Squash,
            &commits,
            &SquashCommitMessage::TitleAndCommits
        ),
        "Ship dark mode\n\n* add theme tokens\n* wire settings toggle"
    );
    assert_eq!(
        workspace_merge_commit_message(
            &ws,
            "main",
            &MergeMethod::Squash,
            &commits,
            &SquashCommitMessage::Title
        ),
        "Ship dark mode"
    );
    assert_eq!(
        workspace_merge_commit_message(
            &ws,
            "main",
            &MergeMethod::Merge,
            &commits,
            &SquashCommitMessage::TitleAndCommits
        ),
        "Merge Ship dark mode into main"
    );
    let unnamed = Workspace {
        goal: "New Workspace".into(),
        ..Workspace::default()
    };
    assert_eq!(
        workspace_merge_commit_message(
            &unnamed,
            "main",
            &MergeMethod::Merge,
            &[],
            &SquashCommitMessage::Title
        ),
        "Merge Workspace changes into main"
    );
}

/// A converge walk to pr_open on a fresh repo.
async fn walked_pr(task: &str) -> (TestEngine, crate::testing::TempRepo, String, Pr) {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, task, "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(10), "pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    wait_for(Duration::from_secs(5), "walk exit", || {
        t.engine.running_loop_by_id(&id).is_none()
    })
    .await;
    let pr = t.store.pr_by_workspace(&id).expect("a PR");
    (t, repo, id, pr)
}

#[tokio::test]
async fn squash_merge_commit_message_on_disk() {
    let goal = "Informative squash history";
    let (t, repo, _id, pr) = walked_pr(goal).await;
    assert!(
        t.engine
            .merge_pr(&pr.id, &MergeMethod::Squash)
            .await
            .is_ok()
    );
    let subj = git_out(&repo.path, &["log", "-1", "--pretty=%s", "main"]).unwrap();
    let subj = subj.trim();
    assert_eq!(subj, goal);
    assert!(!subj.contains("recur/") && !subj.contains("Merge "));
    let body = git_out(&repo.path, &["log", "-1", "--pretty=%b", "main"]).unwrap();
    assert!(body.contains("Recur Local PR #"), "{body}");
    t.close().await;
}

#[tokio::test]
async fn local_merge_advances_base() {
    let (t, repo, id, pr) = walked_pr("Mergeable feature").await;
    let before = git_out(&repo.path, &["rev-parse", "main"]).unwrap();
    let merged = t
        .engine
        .merge_pr(&pr.id, &MergeMethod::default())
        .await
        .unwrap();
    assert_eq!(
        (merged.state.as_str(), merged.group.as_str()),
        ("Merged", "merged")
    );
    assert_ne!(git_out(&repo.path, &["rev-parse", "main"]).unwrap(), before);
    assert_eq!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::Merged
    );
    assert!(
        t.engine
            .merge_pr(&pr.id, &MergeMethod::default())
            .await
            .is_ok(),
        "idempotent"
    );
    let notes: Vec<String> = t
        .store
        .feed(&id)
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.text)
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("Merge: Local PR <code>#")
                && n.ends_with("into <code>main</code> on disk. You were the only gate.")),
        "{notes:?}"
    );
    t.close().await;
}

#[tokio::test]
async fn pr_created_at_pr_open() {
    let (t, _repo, id, pr) = walked_pr("Add a real PR feature").await;
    assert_eq!(pr.source, PrSource::Local);
    assert_eq!((pr.state.as_str(), pr.group.as_str()), ("In review", "you"));
    assert_eq!(pr.attention_reasons, [ATTENTION_REVIEW_REQUESTED]);
    assert!(!pr.branch.is_empty());
    assert_eq!(pr.base, "main");
    let files = pr.files.clone().unwrap_or_default();
    assert!(pr.file_count > 0 && !files.is_empty());
    let work = files
        .iter()
        .find(|f| f.path == "work.txt")
        .expect("work.txt in the diff");
    assert!(work.diff.add > 0);
    assert!(pr.diff.add > 0);
    let commits = pr.commits.clone().unwrap_or_default();
    assert!(!commits.is_empty());
    assert!(!single_line(&commits[0].message).is_empty());
    let body = pr_fallback_markdown(
        "Add a real PR feature",
        &commits,
        pr.file_count,
        pr.diff.add,
        pr.diff.del,
        &["Converged to 0 independent-review findings.".to_owned()],
    );
    assert!(body.contains("Add a real PR feature"));
    assert!(body.contains("## Changes") && body.contains("versus the base branch"));
    assert!(body.contains("Converged to 0 independent-review findings."));
    assert!(!body.contains("Opened by Recur"));
    let desc = pr_description(&pr);
    assert!(
        desc.contains("Add a real PR feature") && desc.contains("## Changes"),
        "{desc}"
    );
    assert!(pr.summary.as_ref().is_some_and(|s| !s.is_empty()));
    assert!(!pr.focus.title.is_empty());
    assert!(
        pr.checks
            .iter()
            .flatten()
            .any(|c| c.name == "Independent review" && c.passed)
    );
    assert!(
        t.store
            .feed(&id)
            .unwrap_or_default()
            .iter()
            .any(|f| f.text.contains(&format!(r#"data-pr="{}""#, pr.id))
                && f.text.contains("Completed in"))
    );
    assert!(t.events.has("pr.updated"));

    // The PR survives a store reopen.
    let path = t.data_dir.join("recur.db");
    let reopened = recur_store::Store::open(&path).unwrap();
    let pr2 = reopened.pr_by_workspace(&id).expect("persisted");
    assert_eq!(pr2.file_count, pr.file_count);
    assert_eq!(
        pr2.files.map(|f| f.len()),
        pr.files.as_ref().map(|f| f.len())
    );
    assert_eq!(
        pr2.commits.map(|c| c.len()),
        pr.commits.as_ref().map(|c| c.len())
    );
    t.close().await;
}

/// The PR description one-shot answers with a workspace-wide title and summary;
/// everything else is converge.
fn describe_pr_converge() -> Arc<dyn Provider> {
    let inner = ConvergeRunner::new(&[0]).provider();
    AsyncProvider::wrap(move |req: TurnRequest, cx| {
        let inner = Arc::clone(&inner);
        async move {
            if req.prompt.contains("pull request title") {
                return Ok(success(
                    "",
                    "Cover the checkout flake\n\nLock the overlay so it cannot cover the composer, and restabilize the checkout tests versus the base branch.",
                ));
            }
            inner.run_turn(req, cx).await
        }
    })
}

const WIRE: &str = "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR.";
const LABEL: &str = "Review & fix to a PR";

async fn prompt_action_pr(
    provider: Arc<dyn Provider>,
) -> (TestEngine, crate::testing::TempRepo, Pr) {
    let repo = init_repo();
    let t = TestEngine::new(Some(provider));
    let proj = add_project(&t.store, &repo.path);
    let (rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            WIRE,
            LABEL,
            "",
            &AutonomyLevel::default(),
            "",
            "",
            None,
        )
        .await
        .unwrap();
    assert!(acquired);
    t.engine.dispatch(rec.clone()).await.unwrap();
    wait_for(Duration::from_secs(10), "pr_open", || {
        t.store
            .workspace(&rec.id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    wait_for(Duration::from_secs(5), "walk exit", || {
        t.engine.running_loop_by_id(&rec.id).is_none()
    })
    .await;
    let pr = t.store.pr_by_workspace(&rec.id).expect("a PR");
    (t, repo, pr)
}

#[tokio::test]
async fn local_pr_description_skips_prompt_action_wire() {
    let (t, _repo, pr) = prompt_action_pr(ConvergeRunner::new(&[0]).provider()).await;
    assert_eq!(pr.title, LABEL);
    let got = pr_description(&pr);
    assert!(
        !got.contains("The human already wrote") && !got.contains("Do not reopen scope"),
        "{got}"
    );
    assert!(got.contains("## Changes"), "{got}");
    t.close().await;
}

#[tokio::test]
async fn local_pr_description_covers_workspace_diff() {
    let (t, _repo, pr) = prompt_action_pr(describe_pr_converge()).await;
    assert_eq!(pr.title, "Cover the checkout flake");
    let got = pr_description(&pr);
    assert!(!got.contains("The human already wrote"), "{got}");
    assert!(!got.contains("## Changes"), "{got}");
    assert!(got.contains("versus the base branch"), "{got}");
    t.close().await;
}

#[tokio::test]
async fn maestro_context_includes_pr_thread() {
    let t = TestEngine::new(None);
    let ws = Workspace {
        id: "L-thread".into(),
        project_id: "P".into(),
        goal: "fix auth".into(),
        state: WorkspaceState::Paused,
        run_started: true,
        ..Workspace::default()
    };
    t.store.add_workspace(ws.clone()).unwrap();
    t.store
        .add_pr(Pr {
            id: "12".into(),
            workspace_id: ws.id.clone(),
            number: 12,
            title: "fix auth".into(),
            comments: Some(vec![PrComment {
                author: "ada".into(),
                path: "a.go".into(),
                line: 3,
                body_md: "off-by-one".into(),
                ..PrComment::default()
            }]),
            ..Pr::default()
        })
        .unwrap();
    let ctx = t.engine.maestro_context(&ws);
    assert!(
        ctx.pr_thread.contains("ada on a.go:3: off-by-one"),
        "{}",
        ctx.pr_thread
    );
    let prompt = crate::roles::maestro_system_prompt(&ctx);
    assert!(prompt.contains("off-by-one") && prompt.contains("reenter_phase"));
}

#[tokio::test]
async fn local_pr_activity_touches_updated_at() {
    let px = Px::new();
    let frozen = "2026-08-01T12:00:00Z";
    px.add_pr(Pr {
        id: "local-touch".into(),
        number: 10,
        source: PrSource::Local,
        title: "old".into(),
        state: "In review".into(),
        created_at: frozen.into(),
        updated_at: frozen.into(),
        ..Pr::default()
    });
    let st = &px.te.store;
    st.update_pr("local-touch", |p| {
        p.files = Some(vec![FileDiff {
            path: "a.go".into(),
            ..FileDiff::default()
        }])
    });
    assert_eq!(
        px.pr("local-touch").updated_at,
        frozen,
        "a cache fill does not bump"
    );
    let eng = px.eng();
    assert!(
        eng.add_pr_comment("local-touch", "a.go", 1, "draft note", 0, "")
            .await
            .1
    );
    assert_eq!(
        px.pr("local-touch").updated_at,
        frozen,
        "an unsubmitted draft does not bump"
    );
    assert!(
        eng.add_pr_comment("local-touch", "", 0, "please take a look", 0, "")
            .await
            .1
    );
    let got = px.pr("local-touch");
    assert_ne!(got.updated_at, frozen);
    assert_eq!(got.created_at, frozen);
    st.update_pr("local-touch", |p| p.updated_at = frozen.into());
    assert!(eng.submit_review("local-touch", "comment", "lgtm").await.1);
    assert_ne!(px.pr("local-touch").updated_at, frozen);
    st.update_pr("local-touch", |p| p.updated_at = frozen.into());
    let (updated, ok) = eng
        .update_pr_details("local-touch", "new title", "new body")
        .await;
    assert!(ok);
    assert_ne!(updated.updated_at, frozen);
}

#[tokio::test]
async fn local_review_flushes_pending_notes_under_the_card() {
    let px = Px::new();
    px.add_pr(Pr {
        id: "lr".into(),
        number: 3,
        source: PrSource::Local,
        files: Some(vec![FileDiff {
            path: "auth.go".into(),
            lines: Some(vec![
                DiffLine {
                    r#type: "hunk".into(),
                    code_html: "@@ -1 +1 @@".into(),
                    ..DiffLine::default()
                },
                DiffLine {
                    r#type: "add".into(),
                    new: "1".into(),
                    code_html: "x".into(),
                    ..DiffLine::default()
                },
            ]),
            ..FileDiff::default()
        }]),
        pending_comments: vec![PendingLineComment {
            id: "p1".into(),
            file: "auth.go".into(),
            line: 1,
            side: "RIGHT".into(),
            body: "nit".into(),
            ..PendingLineComment::default()
        }],
        ..Pr::default()
    });
    let (got, ok) = px.eng().submit_review("lr", "request", "needs work").await;
    assert!(ok);
    assert_eq!(got.state, "Changes requested");
    let cs = got.comments.unwrap();
    assert_eq!(cs.len(), 2);
    assert_eq!(cs[0].verb, "requested changes");
    assert_eq!(cs[1].parent_id, cs[0].id);
    assert_eq!(cs[1].diff_hunk, "@@ -1 +1 @@\n+x");
    assert_eq!(cs[1].body_html, "<code>auth.go:1</code>");
    assert!(got.pending_comments.is_empty());
    assert_eq!(got.reviewers.len(), 1, "{:?}", got.reviewers);
}

#[test]
fn commit_ref_matches_short_and_full_hash() {
    let c = Commit {
        hash: "3f1d02b".into(),
        sha: "3f1d02babcdef0123456789".into(),
        ..Commit::default()
    };
    for r in ["3f1d02b", "3F1D02B", "3f1d02babcdef0123456789"] {
        assert!(commit_ref_matches(&c, r), "{r}");
    }
    assert!(!commit_ref_matches(&c, "deadbeef"));
    assert!(!commit_ref_matches(&c, "3f1"));
}

#[test]
fn merge_commit_log_keeps_diffs_and_fills_body() {
    let existing = vec![Commit {
        hash: "3f1d02b".into(),
        message: "Enable the tab".into(),
        files: Some(vec![FileDiff {
            path: "a.ts".into(),
            ..FileDiff::default()
        }]),
        verified: true,
        check_status: "pass".into(),
        ..Commit::default()
    }];
    let fresh = vec![Commit {
        hash: "3f1d02b".into(),
        sha: "3f1d02babcdef".into(),
        message: "Enable the tab".into(),
        body: "Turn it on by default.".into(),
        author: "mycodecrafting".into(),
        when: "2h".into(),
        ..Commit::default()
    }];
    let got = merge_commit_log(&existing, fresh);
    assert_eq!(got.len(), 1);
    assert_eq!(
        (
            got[0].body.as_str(),
            got[0].author.as_str(),
            got[0].when.as_str()
        ),
        ("Turn it on by default.", "mycodecrafting", "2h")
    );
    assert_eq!(got[0].files.as_ref().unwrap()[0].path, "a.ts");
    assert!(got[0].verified);
    assert_eq!(got[0].check_status, "pass");
}

#[test]
fn comment_hunk_from_diff_files() {
    let line = |t: &str, old: &str, new: &str, html: &str| DiffLine {
        r#type: t.into(),
        old: old.into(),
        new: new.into(),
        code_html: html.into(),
        ..DiffLine::default()
    };
    let files = vec![FileDiff {
        path: "auth.go".into(),
        lines: Some(vec![
            line("hunk", "", "", "@@ -10,3 +10,3 @@"),
            line("ctx", "10", "10", " keep"),
            line("del", "11", "", r#"<span class="kw">old</span>"#),
            line("add", "", "11", "new &amp; better"),
        ]),
        ..FileDiff::default()
    }];
    let got = comment_hunk_from_files(&files, "auth.go", 11, "RIGHT");
    assert_eq!(got, "@@ -10,3 +10,3 @@\n  keep\n-old\n+new & better");
    assert_eq!(
        comment_hunk_from_files(&files, "auth.go", 11, "LEFT"),
        "@@ -10,3 +10,3 @@\n  keep\n-old"
    );
    assert_eq!(comment_hunk_from_files(&files, "nope.go", 11, "RIGHT"), "");
    assert_eq!(comment_hunk_from_files(&files, "auth.go", 99, "RIGHT"), "");
}

#[test]
fn pending_locations() {
    let pc = |line, start| PendingLineComment {
        file: "auth.go".into(),
        line,
        start_line: start,
        ..PendingLineComment::default()
    };
    assert_eq!(pending_loc(&pc(12, 10)), "auth.go:10-12");
    assert_eq!(pending_loc(&pc(12, 0)), "auth.go:12");
    assert_eq!(pending_loc(&pc(0, 0)), "auth.go");
}
