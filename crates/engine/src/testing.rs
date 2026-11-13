//! The engine test fixture: a SQLite store under a temp data dir, an engine
//! whose turns go to a pinned (scripted) provider, a bus recorder, temp git
//! repos, and the fixture bundle pack. Everything is per test: no process env
//! is read or written, so tests run in parallel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::Provider;
use recur_bundles::DiscoverRoots;
use recur_domain::Project;
use recur_store::Store;

use crate::Engine;
use crate::bus::{Bus, EngineEvent};
use crate::core::GitSetup;
use crate::registry::{ForgeRegistry, ProviderRegistry, TrackerRegistry};

/// Captures every published event, in order.
#[derive(Clone, Default)]
pub struct BusRecorder(Arc<Mutex<Vec<EngineEvent>>>);

impl BusRecorder {
    /// Subscribes a recorder to `bus`.
    pub fn attach(bus: &Bus) -> Self {
        let r = Self::default();
        let inner = Arc::clone(&r.0);
        bus.subscribe_fn(move |ev| inner.lock().push(ev.clone()));
        r
    }

    pub fn all(&self) -> Vec<EngineEvent> {
        self.0.lock().clone()
    }

    /// Every event with this name.
    pub fn by_name(&self, name: &str) -> Vec<EngineEvent> {
        self.0
            .lock()
            .iter()
            .filter(|e| e.name() == name)
            .cloned()
            .collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.lock().iter().any(|e| e.name() == name)
    }

    pub fn count(&self, name: &str) -> usize {
        self.0.lock().iter().filter(|e| e.name() == name).count()
    }
}

/// A temp git repo with one commit on `main`.
pub struct TempRepo {
    _dir: tempfile::TempDir,
    pub path: String,
}

/// Runs git in `dir` with a stable test identity; panics on failure.
pub fn git(dir: &str, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git -C <dir> <args>` combined output.
pub fn git_out(dir: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(s) } else { Err(s) }
}

/// A fresh repo: `git init -b main`, test identity, README, one commit.
pub fn init_repo() -> TempRepo {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_string_lossy().into_owned();
    git(&path, &["init", "-b", "main"]);
    git(&path, &["config", "user.name", "test"]);
    git(&path, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").expect("write README");
    git(&path, &["add", "."]);
    git(&path, &["commit", "-m", "initial"]);
    TempRepo { _dir: dir, path }
}

/// Registers project "proj" on `repo` with base main.
pub fn add_project(store: &Store, repo: &str) -> String {
    let p = Project {
        id: "proj".into(),
        name: "proj".into(),
        repo_path: repo.into(),
        base_branch: "main".into(),
        ..Project::default()
    };
    store.add_project(p.clone()).expect("add project");
    p.id
}

fn has_run_config(p: &Project) -> bool {
    !p.setup_cmd.is_empty()
        || !p.teardown_cmd.is_empty()
        || !p.app_url.is_empty()
        || !p.env.is_empty()
        || !p.copy_files.is_empty()
        || !p.run_commands.is_empty()
}

/// Adds a project, writing its run config to `.recur/project.toml` first
/// (run config is read from the file).
pub fn add_run_project(store: &Store, p: Project) -> Project {
    if !p.repo_path.trim().is_empty() && has_run_config(&p) {
        recur_project::projfile::write(&p).expect("projfile write");
    }
    store.add_project(p).expect("add project")
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for e in std::fs::read_dir(src).expect("read_dir") {
        let e = e.expect("dir entry");
        let to = dst.join(e.file_name());
        if e.file_type().expect("file type").is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), &to).expect("copy");
        }
    }
}

/// Polls `cond` every 5 ms for 3× `d` (headroom for a loaded machine).
pub async fn wait_for(d: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + d * 3;
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The fixture pack: QA/writer/editor roles, a failing policy, and the
/// verify / converge / docs loops engine tests run.
pub const TEST_LOOPS_TOML: &str = r#"
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[role]]
id = "test.roles.qa"
name = "QA"
code = "QA"
mode = "ask"
tools = ["attach_evidence", "playwright", "report_result"]

[[role]]
id = "test.roles.writer"
name = "Writer"
code = "WR"
mode = "agent"

[[role]]
id = "test.roles.editor"
name = "Editor"
code = "ED"
mode = "review"

[[policy]]
id = "test.policies.fail"
name = "Fail at cap"
max_rounds = 4
on_max_rounds = "fail"
findings_threshold = { critical = 0, high = 0, medium = 0 }
plan_approval = "never"
extend_rounds = 1
on_fail = "fail"

[[loop]]
id = "test.loops.verify"
name = "Verify fixture"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
next = "verify"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop.phase]]
id = "verify"
uses = "verify"
role = "test.roles.qa"
next = "pr"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop.phase]]
id = "ondemand"
uses = "verify"
dispatch = "on_demand"
role = "test.roles.qa"

[[loop]]
id = "test.loops.verify-reject"
name = "Verify on_reject"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
next = "verify"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop.phase]]
id = "verify"
uses = "verify"
on_reject = "converge"
role = "test.roles.qa"
next = "pr"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop]]
id = "test.loops.converge"
name = "Converge reviews"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop]]
id = "test.loops.converge-pr"
name = "Converge then PR"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
next = "pr"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop]]
id = "test.loops.docs"
name = "Docs loop"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
findings_threshold = 0
[loop.phase.roles]
implementer = "test.roles.writer"
reviewer = "test.roles.editor"
"#;

/// Options for [`TestEngine::with`].
#[derive(Default)]
pub struct TestOptions {
    /// Every turn goes here (`None` fails turns closed).
    pub provider: Option<Arc<dyn Provider>>,
    /// Route by model ref over these instead of pinning `provider`.
    pub registered: Option<Vec<Arc<dyn Provider>>>,
    /// The boot git alert (title, body).
    pub git_warning: Option<(String, String)>,
    pub forges: Vec<Arc<dyn recur_forge::Forge>>,
    /// The Observatory terminals (`@terminal`, list/read_terminal).
    pub terminals: Option<Arc<dyn crate::terminals::TerminalSource>>,
    pub trackers: Vec<Arc<dyn recur_issues::IssueTracker>>,
    /// The MCP listen address (default a free port).
    pub mcp_addr: Option<String>,
}

/// One engine over a temp data dir.
pub struct TestEngine {
    pub engine: Arc<Engine>,
    pub store: Arc<Store>,
    pub events: BusRecorder,
    pub data_dir: PathBuf,
    _dir: tempfile::TempDir,
}

impl TestEngine {
    pub fn new(provider: Option<Arc<dyn Provider>>) -> Self {
        Self::with(TestOptions {
            provider,
            ..TestOptions::default()
        })
    }

    pub fn with(opts: TestOptions) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let data_dir = dir.path().to_path_buf();
        let store = Arc::new(Store::open(&data_dir.join("recur.db")).expect("store open"));
        let wt_root = data_dir.join("worktrees");
        std::fs::create_dir_all(&wt_root).expect("worktrees dir");
        let bundles = DiscoverRoots {
            user_dir: data_dir.join("user-bundles").to_string_lossy().into_owned(),
            default_dir: data_dir.join("Bundles").to_string_lossy().into_owned(),
        };
        std::fs::create_dir_all(&bundles.user_dir).expect("user bundles dir");
        std::fs::create_dir_all(&bundles.default_dir).expect("bundles dir");
        let (warning_title, warning) = opts.git_warning.unwrap_or_default();
        let bus = Arc::new(Bus::new());
        let events = BusRecorder::attach(&bus);
        let engine = Engine::build(
            Arc::clone(&store),
            bus,
            GitSetup {
                worktree_root: wt_root.to_string_lossy().into_owned(),
                warning_title,
                warning,
            },
            bundles,
            opts.terminals,
            match opts.registered {
                Some(list) => ProviderRegistry::new(list),
                None => ProviderRegistry::pinned(opts.provider),
            },
            ForgeRegistry::new(opts.forges),
            TrackerRegistry::new(opts.trackers),
            data_dir.clone(),
            opts.mcp_addr,
        );
        Self {
            engine,
            store,
            events,
            data_dir,
            _dir: dir,
        }
    }

    /// Writes a bundle pack under the default bundles root.
    pub fn install_test_pack(&self, name: &str, toml: &str) {
        let pack = Path::new(&self.engine.bundle_roots().default_dir).join(name);
        std::fs::create_dir_all(&pack).expect("mkdir pack");
        std::fs::write(pack.join("bundle.toml"), toml).expect("write bundle.toml");
    }

    /// The fixture pack.
    pub fn install_fixture_pack(&self) {
        self.install_test_pack("TestLoops", TEST_LOOPS_TOML);
    }

    /// Copies the shipped `bundles/Workflows` pack under the default bundles
    /// root.
    pub fn install_shipped_loop_pack(&self) {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles/Workflows");
        assert!(
            src.join("bundle.toml").exists(),
            "shipped Workflows pack missing at {src:?}"
        );
        let dst = Path::new(&self.engine.bundle_roots().default_dir).join("Workflows");
        copy_dir(&src, &dst);
    }

    /// A second engine over the same store and data dir (after
    /// a simulated restart), with its own bus.
    pub fn reopen(&self, provider: Option<Arc<dyn Provider>>) -> Arc<Engine> {
        self.reopen_on(Arc::clone(&self.store), provider)
    }

    /// [`Self::reopen`] over `store` (a store reopened from disk).
    pub fn reopen_on(&self, store: Arc<Store>, provider: Option<Arc<dyn Provider>>) -> Arc<Engine> {
        Engine::build(
            store,
            Arc::new(Bus::new()),
            GitSetup {
                worktree_root: self.engine.worktree_root().to_owned(),
                ..GitSetup::default()
            },
            self.engine.bundle_roots().clone(),
            None,
            ProviderRegistry::pinned(provider),
            ForgeRegistry::new(Vec::new()),
            TrackerRegistry::new(Vec::new()),
            self.data_dir.clone(),
            None,
        )
    }

    /// Joins the engine's tasks before the temp dirs go.
    pub async fn close(self) {
        self.engine.shutdown().await;
    }
}
