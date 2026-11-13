//! PR automations end to end: the sweep's follow-up runs the prompt action a
//! person would run in the PR's own workspace.

use std::path::Path;
use std::sync::Arc;

use recur_domain::{AgentRole, Pr, Workspace};

use super::support::gh_pr;
use crate::loops::test_support::CaptureScript;
use crate::testing::{TempRepo, TestEngine, add_project, git, init_repo};
use crate::turn::test_support::maestro_fixture;

/// Stand-ins for the shipped `/update`, `/fix-checks` and `/address-review`:
/// each prints a marker prompt. Like the real ones, `/fix-checks` and
/// `/address-review` send nothing without the PR env, and `/address-review`
/// sends nothing until `review_marker` exists (no comment to answer).
fn install_actions(te: &TestEngine, review_marker: &Path) {
    let no_pr = "[ \"$RECUR_PR_SOURCE\" = gh ] && [ \"$RECUR_PR_NUMBER\" = 7 ] || { printf '%s\\n' '{\"prompt\":\"\",\"display\":\"No PR\"}'; exit 0; }\n";
    let dir = Path::new(&te.engine.bundle_roots().default_dir).join("AutoTest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let mut toml = String::from(
        "[bundle]\nname = \"AutoTest\"\nid = \"recur.autotest\"\nversion = \"1.0.0\"\n",
    );
    for (slash, body) in [
        (
            "update",
            "printf '%s\\n' '{\"prompt\":\"UPDATE-WIRE\",\"display\":\"Update\"}'".to_owned(),
        ),
        (
            "fix-checks",
            format!(
                "{no_pr}printf '%s\\n' '{{\"prompt\":\"FIX-CHECKS-WIRE\",\"display\":\"Fix checks\"}}'"
            ),
        ),
        (
            "address-review",
            format!(
                "{no_pr}if [ -f {:?} ]; then printf '%s\\n' '{{\"prompt\":\"REVIEW-WIRE\",\"display\":\"Address review\"}}'; else printf '%s\\n' '{{\"prompt\":\"\",\"display\":\"No review comments need a response\"}}'; fi",
                review_marker.to_string_lossy()
            ),
        ),
    ] {
        recur_exec::write_executable(
            dir.join("src").join(format!("{slash}.sh")),
            format!("#!/bin/sh\n{body}\n"),
            0o755,
        )
        .unwrap();
        toml.push_str(&format!(
            "\n[[prompt]]\nid = \"recur.autotest.{slash}\"\nname = \"{slash}\"\nslash = \"{slash}\"\nmode = \"agent\"\ninterpreter = \"shebang\"\nscript = \"src/{slash}.sh\"\n"
        ));
    }
    std::fs::write(dir.join("bundle.toml"), toml).unwrap();
}

struct Ax {
    te: TestEngine,
    repo: TempRepo,
    cap: Arc<CaptureScript>,
    ws: Workspace,
    pr: Pr,
    marker: tempfile::TempDir,
}

impl Ax {
    async fn new() -> Self {
        let cap = CaptureScript::new(|_| maestro_fixture("auto-sess", "Done."));
        let te = TestEngine::new(Some(cap.provider()));
        let repo = init_repo();
        let proj = add_project(&te.store, &repo.path);
        let ws = te
            .engine
            .create_workspace(&proj, "feature", "")
            .await
            .expect("create workspace");
        let marker = tempfile::tempdir().unwrap();
        install_actions(&te, &marker.path().join("review"));
        let pr = Pr {
            id: "gh-proj-7".into(),
            project_id: proj,
            workspace_id: ws.id.clone(),
            number: 7,
            branch: ws.branch.clone(),
            group: "you".into(),
            author_login: "josh".into(),
            viewer_login: "josh".into(),
            updated_at: "2026-11-13T10:00:00Z".into(),
            ..gh_pr(7, &ws.branch)
        };
        te.store.add_pr(pr.clone()).unwrap();
        let ax = Self {
            te,
            repo,
            cap,
            ws,
            pr,
            marker,
        };
        ax.settings(true, true, true);
        ax
    }

    fn settings(&self, conflicts: bool, checks: bool, review: bool) {
        let mut s = self.te.store.settings();
        s.auto_update_conflicts = conflicts;
        s.auto_fix_checks = checks;
        s.auto_address_review = review;
        self.te.store.set_settings(s);
    }

    fn pr(&self, f: impl FnOnce(&mut Pr)) {
        self.te.store.update_pr(&self.pr.id, f);
    }

    /// One sweep follow-up, waiting for the turns it started.
    async fn sweep(&self) {
        self.te.engine.run_pr_automations().await;
        let t = &self.te.engine.tasks.schedules;
        t.close();
        t.wait().await;
        t.reopen();
    }

    fn prompts(&self) -> Vec<String> {
        self.cap
            .with_role(AgentRole::Coder)
            .into_iter()
            .map(|r| r.prompt)
            .collect()
    }

    fn feed(&self) -> String {
        self.te
            .store
            .feed(&self.ws.id)
            .unwrap_or_default()
            .iter()
            .map(|f| f.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn sent(prompts: &[String], wire: &str) -> usize {
    prompts.iter().filter(|p| p.contains(wire)).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_check_failure_runs_fix_checks_in_the_pr_workspace_once() {
    let ax = Ax::new().await;
    let fail = |ok: bool| {
        ax.pr(|p| {
            p.has_checks = true;
            p.checks_ok = ok;
        })
    };
    fail(false);
    ax.sweep().await;
    let got = ax.prompts();
    assert_eq!(sent(&got, "FIX-CHECKS-WIRE"), 1, "{got:?}");
    assert_eq!(
        ax.cap.with_role(AgentRole::Coder)[0].workspace_id,
        ax.ws.id,
        "the PR's own workspace"
    );
    assert!(
        ax.feed().contains("Checks failed on PR #7"),
        "{}",
        ax.feed()
    );
    ax.sweep().await;
    assert_eq!(sent(&ax.prompts(), "FIX-CHECKS-WIRE"), 1, "same occurrence");

    // A re-run: pending (nothing failed), then failing again.
    fail(true);
    ax.sweep().await;
    fail(false);
    ax.sweep().await;
    assert_eq!(sent(&ax.prompts(), "FIX-CHECKS-WIRE"), 2, "a new failure");

    ax.settings(true, false, true);
    fail(true);
    ax.sweep().await;
    fail(false);
    ax.sweep().await;
    assert_eq!(sent(&ax.prompts(), "FIX-CHECKS-WIRE"), 2, "turned off");
    ax.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_viewers_open_pr_with_a_live_workspace_runs() {
    let ax = Ax::new().await;
    ax.settings(false, false, false);
    ax.pr(|p| {
        p.has_checks = true;
        p.checks_ok = false;
    });
    ax.sweep().await;
    assert!(ax.prompts().is_empty(), "off by default");
    ax.settings(false, true, false);
    type Skip = (&'static str, fn(&mut Pr));
    let skip: [Skip; 4] = [
        ("someone else's PR", |p| p.author_login = "sam".into()),
        ("merged", |p| p.state = "Merged".into()),
        ("closed", |p| p.state = "Closed".into()),
        ("no workspace", |p| p.workspace_id.clear()),
    ];
    for (why, f) in skip {
        let before = ax.te.store.pr(&ax.pr.id).unwrap();
        ax.pr(f);
        ax.sweep().await;
        assert!(ax.prompts().is_empty(), "{why}");
        ax.te.store.update_pr(&ax.pr.id, |p| *p = before);
    }
    ax.te
        .store
        .update_workspace(&ax.ws.id, |w| w.run_started = true);
    ax.sweep().await;
    assert!(ax.prompts().is_empty(), "loop-owned");
    ax.te
        .store
        .update_workspace(&ax.ws.id, |w| w.run_started = false);
    ax.sweep().await;
    assert_eq!(sent(&ax.prompts(), "FIX-CHECKS-WIRE"), 1);
    ax.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_activity_runs_address_review_only_when_it_has_a_prompt() {
    let ax = Ax::new().await;
    ax.sweep().await;
    assert!(ax.prompts().is_empty(), "nothing to answer: no turn");
    assert!(!ax.feed().contains("Address review"));

    std::fs::write(ax.marker.path().join("review"), "").unwrap();
    ax.sweep().await;
    assert!(ax.prompts().is_empty(), "updated_at unchanged");

    ax.pr(|p| p.updated_at = "2026-11-13T10:10:00Z".into());
    ax.sweep().await;
    let got = ax.prompts();
    assert_eq!(sent(&got, "REVIEW-WIRE"), 1, "{got:?}");
    assert!(ax.feed().contains("Ran Address review automatically"));
    ax.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_update_with_base_makes_no_turn() {
    let ax = Ax::new().await;
    std::fs::write(Path::new(&ax.repo.path).join("base.txt"), "base side\n").unwrap();
    git(&ax.repo.path, &["add", "base.txt"]);
    git(&ax.repo.path, &["commit", "-m", "base change"]);
    ax.pr(|p| p.mergeable = "CONFLICTING".into());

    ax.sweep().await;
    assert!(ax.feed().contains("PR #7 has merge conflicts"));
    assert!(
        Path::new(&ax.ws.worktree_path).join("base.txt").exists(),
        "base merged in"
    );
    let got = ax.prompts();
    assert!(got.is_empty(), "{got:?}");
    ax.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_updates_with_base_then_sends_update() {
    let ax = Ax::new().await;
    let work = Path::new(&ax.ws.worktree_path).join("work.txt");
    std::fs::write(&work, "branch side\n").unwrap();
    git(&ax.ws.worktree_path, &["add", "work.txt"]);
    git(&ax.ws.worktree_path, &["commit", "-m", "branch change"]);
    std::fs::write(Path::new(&ax.repo.path).join("work.txt"), "base side\n").unwrap();
    git(&ax.repo.path, &["add", "work.txt"]);
    git(&ax.repo.path, &["commit", "-m", "base change"]);
    ax.pr(|p| p.mergeable = "CONFLICTING".into());

    ax.sweep().await;
    let got = ax.prompts();
    assert_eq!(sent(&got, "UPDATE-WIRE"), 1, "{got:?}");
    assert!(
        recur_git::Manager::new(&ax.repo.path, "")
            .update_in_progress(&ax.ws.worktree_path)
            .await,
        "Update with base left the markers for /update"
    );
    assert!(ax.feed().contains("PR #7 has merge conflicts"));
    ax.te.close().await;
}
