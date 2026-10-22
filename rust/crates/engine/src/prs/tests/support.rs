//! Shared E5 test scaffolding.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use recur_domain::{Check, Pr, PrSource, Project, Workspace};
use recur_forge::PrListRow;
use recur_forge::testing::FakeForge;

use crate::Engine;
use crate::testing::{TestEngine, TestOptions, git, init_repo};

/// One engine with a GitHub-shaped fake forge registered.
pub struct Px {
    pub te: TestEngine,
    pub forge: Arc<FakeForge>,
}

impl Px {
    pub fn new() -> Self {
        let forge = Arc::new(FakeForge::new());
        let te = TestEngine::with(TestOptions {
            forges: vec![forge.clone()],
            ..TestOptions::default()
        });
        te.engine.set_pr_sync_min_interval(Duration::ZERO);
        Self { te, forge }
    }

    pub fn eng(&self) -> &Arc<Engine> {
        &self.te.engine
    }

    pub fn add_project(&self, id: &str, repo: &str) {
        self.te
            .store
            .add_project(Project {
                id: id.into(),
                name: id.into(),
                repo_path: repo.into(),
                base_branch: "main".into(),
                ..Project::default()
            })
            .expect("add project");
    }

    pub fn add_ws(&self, ws: Workspace) -> Workspace {
        self.te.store.add_workspace(ws).expect("add workspace")
    }

    pub fn add_pr(&self, pr: Pr) -> Pr {
        self.te.store.add_pr(pr).expect("add pr")
    }

    pub fn pr(&self, id: &str) -> Pr {
        self.te.store.pr(id).expect("pr")
    }

    pub fn ws(&self, id: &str) -> Workspace {
        self.te.store.workspace(id).expect("workspace")
    }
}

/// A stored forge PR of project `acme`.
pub fn gh_pr(number: i64, branch: &str) -> Pr {
    Pr {
        id: format!("gh-acme-{number}"),
        project_id: "acme".into(),
        number,
        source: PrSource::GitHub,
        url: format!("https://github.com/acme/app/pull/{number}"),
        branch: branch.into(),
        base: "main".into(),
        title: "OAuth".into(),
        state: "Open".into(),
        group: "open".into(),
        ..Pr::default()
    }
}

/// An open list row of `acme/app`.
pub fn row(number: i64, title: &str, head: &str) -> PrListRow {
    PrListRow {
        number,
        title: title.into(),
        url: format!("https://github.com/acme/app/pull/{number}"),
        head_ref: head.into(),
        base_ref: "main".into(),
        author_login: "sam".into(),
        state: "OPEN".into(),
        checks_ok: true,
        ..PrListRow::default()
    }
}

/// A list row check as the forge's rollup parse yields it.
pub fn listed(name: &str, bucket: &str) -> Check {
    Check {
        name: name.into(),
        passed: bucket == "pass",
        status: bucket.into(),
        ..Check::default()
    }
}

/// Sets a row's rollup to these checks.
pub fn with_checks(mut r: PrListRow, checks: Vec<Check>) -> PrListRow {
    r.checks_ok = !checks.iter().any(|c| c.status == "fail");
    r.has_checks = !checks.is_empty();
    r.checks = checks;
    r
}

/// Temp dirs a test keeps alive.
pub struct Keep(#[allow(dead_code)] Vec<Box<dyn std::any::Any + Send>>);

/// Go `initCloneTrackingRemoteBranch`: a clone whose `local` branch tracks
/// `origin/<remote>`. Returns (dirs to keep alive, clone path).
pub fn clone_tracking(remote: &str, local: &str) -> (Keep, String) {
    let seed = init_repo();
    if remote != "main" {
        git(&seed.path, &["checkout", "-b", remote]);
        std::fs::write(Path::new(&seed.path).join("on-remote.txt"), "on remote\n").unwrap();
        git(&seed.path, &["add", "."]);
        git(&seed.path, &["commit", "-m", &format!("on {remote}")]);
        git(&seed.path, &["checkout", "main"]);
    }
    let bare_dir = tempfile::tempdir().unwrap();
    let bare = bare_dir.path().join("upstream.git");
    let cwd = tempfile::tempdir().unwrap();
    let cwd_s = cwd.path().to_string_lossy().into_owned();
    git(&cwd_s, &["clone", "--bare", &seed.path, bare.to_str().unwrap()]);
    let clone_dir = tempfile::tempdir().unwrap();
    let clone = clone_dir.path().join("work").to_string_lossy().into_owned();
    git(&cwd_s, &["clone", bare.to_str().unwrap(), &clone]);
    if local != remote {
        git(&clone, &["checkout", "-b", local, "--track", &format!("origin/{remote}")]);
    }
    (
        Keep(vec![Box::new(seed), Box::new(bare_dir), Box::new(cwd), Box::new(clone_dir)]),
        clone,
    )
}

/// Go `initCloneWithUpstreamFork`: a clone with origin and upstream remotes.
pub fn clone_with_upstream_fork() -> (Keep, String) {
    let seed = init_repo();
    let dirs: Vec<tempfile::TempDir> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let origin = dirs[0].path().join("origin.git").to_string_lossy().into_owned();
    let upstream = dirs[0].path().join("upstream.git").to_string_lossy().into_owned();
    let cwd = dirs[1].path().to_string_lossy().into_owned();
    git(&cwd, &["clone", "--bare", &seed.path, &origin]);
    git(&cwd, &["clone", "--bare", &seed.path, &upstream]);
    let clone = dirs[2].path().join("work").to_string_lossy().into_owned();
    git(&cwd, &["clone", &origin, &clone]);
    git(&clone, &["remote", "add", "upstream", &upstream]);
    git(&clone, &["fetch", "upstream"]);
    let mut keep: Vec<Box<dyn std::any::Any + Send>> = vec![Box::new(seed)];
    keep.extend(dirs.into_iter().map(|d| Box::new(d) as Box<dyn std::any::Any + Send>));
    (Keep(keep), clone)
}

/// A workspace row of project `acme`.
pub fn ws(id: &str, branch: &str) -> Workspace {
    Workspace {
        id: id.into(),
        project_id: "acme".into(),
        branch: branch.into(),
        ..Workspace::default()
    }
}
