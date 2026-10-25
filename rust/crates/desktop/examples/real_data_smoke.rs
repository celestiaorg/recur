//! Real-data smoke: boots the desktop app on a sandbox copy of a long-lived Recur data dir and
//! drives every view, reporting panics, error toasts and hangs.
//!
//! `cargo run -p recur-desktop --example real_data_smoke -- <data dir> [sandbox dir]`
//! (`wails3 task smoke:rust DATA="<data dir>" [SANDBOX=<dir>]`)
//!
//! The source is only read. The sandbox gets:
//! - `data/`: a `VACUUM INTO` snapshot of `recur.db` (safe while the owning app runs) and the
//!   small state files and dirs (`*.json`, `workspaces/`, `app-chat/`, `open-files/`, bundles).
//! - `repos/<project>`: a hardlinked local clone of every project repo with all its branches,
//!   `origin` fetching from the real remote and pushing nowhere.
//! - `worktrees/<project>/<name>`: every live workspace's worktree, re-added from the clone at
//!   its branch (uncommitted changes in the real worktree are not carried over).
//!
//! Paths in the snapshot and the state files are rewritten to the sandbox, so nothing the app
//! does (git, terminals, setup scripts, LSP) reaches the real repos. Every agent turn goes to a
//! scripted provider that streams a canned reply, and the GitHub forge is read-only, so boot
//! recovery, auto-resume, scheduled prompts and the PR sweep run without spending or posting.
//! A second run against the same sandbox dir reuses it.
//!
//! The drive opens every project, every workspace and each of its chat tabs (transcripts
//! scrolled top to bottom), the Observatory's explorer, a file, the review and the terminal on
//! live workspaces, every PR on each detail tab, every Settings section, Briefing, Issues,
//! Schedules and each palette, and streams one chat turn. It exits non-zero on any panic
//! (contained or not), error toast, frame over 2s or UI stall over 20s, sampling the main thread
//! on a stall.
//!
//! It also times keystrokes in long chats, transcript and review scroll steps and workspace
//! switches against frame-time budgets (`Budget`), and fails when one is exceeded.
//!
//! To reproduce a finding: `RECUR_SMOKE_LIMIT=n` caps the workspaces and PRs visited,
//! `RECUR_SMOKE_FROM=n` starts the workspaces at the nth, `RECUR_SMOKE_WORKSPACE=<id>` visits
//! only that one (`RECUR_SMOKE_PR=<id>` likewise for PRs), `RECUR_SMOKE_WORKSPACES_ONLY=1`
//! stops after the workspaces and
//! `RECUR_SMOKE_RENDERS=1` lists the views each frame over 500ms rendered and
//! `RECUR_SMOKE_KEY_ROUNDS=n` types the keystroke sequence n times.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, IntoElement,
    Keystroke, Render, Window, WindowBounds, WindowOptions, point, px, size,
};
use recur_agent::{
    AgentEvent, Availability, CredentialDraft, Flags, ModelChoice, Provider, ProviderCaps,
    ProviderError, ProviderStatus, ToolDisplay, TurnContext, TurnRequest, TurnResult, UsageProbe,
};
use recur_app::{Shell, ShellOptions};
use recur_domain::{
    MergeCommitMessage, MergeMethod, PendingLineComment, Pr, PrComment, PrParticipant, PrSource,
    Settings, SquashCommitMessage,
};
use recur_forge::{Forge, ForgeError};
use recur_services::{AppConfig, AppOptions, AppServices};
use recur_view_core::prefs::CodePanelKey;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use recur_view_prs::detail::PrDetail;
use recur_view_prs::ui::PrTab;
use recur_view_workspace::detail::WorkspaceDetail;
use recur_view_workspace::view::WorkspaceView;

const PROMPT: &str = "summarize this workspace in one line";
const REPLY: &str =
    "Streaming a **scripted** reply from the real-data smoke.\n\n- one\n- two\n\nDone.";

fn main() {
    if let Some(root) = std::env::var_os("RECUR_SMOKE_ROOT") {
        std::process::exit(drive(PathBuf::from(root)));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(src) = args.first().map(PathBuf::from) else {
        eprintln!("usage: real_data_smoke <recur data dir> [sandbox dir]");
        std::process::exit(2);
    };
    let root = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("recur-real-smoke-{}", std::process::id()))
    });
    if let Err(err) = prepare(&src, &root) {
        eprintln!("prepare sandbox: {err}");
        std::process::exit(2);
    }
    let status = Command::new(std::env::current_exe().expect("current exe"))
        .env("RECUR_SMOKE_ROOT", &root)
        .env("RECUR_DATA_DIR", root.join("data"))
        .env("RECUR_WORKTREES_DIR", root.join("worktrees"))
        .status()
        .expect("run smoke");
    println!("sandbox kept at {}", root.display());
    std::process::exit(status.code().unwrap_or(1));
}

// ---- Sandbox

type Res<T> = Result<T, String>;

fn git(dir: &Path, args: &[&str]) -> Res<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn copy_dir(from: &Path, to: &Path) -> Res<()> {
    std::fs::create_dir_all(to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let dst = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &dst)
                .map_err(|e| format!("copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Mirrors the real worktree's changes against HEAD (staged or not) and its untracked, not
/// ignored files into the sandbox worktree. `git diff` and `git ls-files` only read the real
/// one.
fn carry_dirty(real: &Path, dst: &Path) {
    let changed = git(real, &["diff", "--name-only", "-z", "HEAD"]).unwrap_or_default();
    let untracked =
        git(real, &["ls-files", "-z", "--others", "--exclude-standard"]).unwrap_or_default();
    for rel in changed
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|r| !r.is_empty())
    {
        let from = real.join(rel);
        let to = dst.join(rel);
        if !from.exists() {
            let _ = std::fs::remove_file(&to);
            continue;
        }
        if !from.is_file() {
            continue;
        }
        if let Some(dir) = to.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::copy(&from, &to);
    }
}

fn prepare(src: &Path, root: &Path) -> Res<()> {
    let src = src
        .canonicalize()
        .map_err(|e| format!("{}: {e}", src.display()))?;
    if !src.join("recur.db").is_file() {
        return Err(format!("{} has no recur.db", src.display()));
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if root.starts_with(&src) || src.starts_with(&root) {
        return Err("the sandbox must not overlap the source data dir".into());
    }
    if root.join(".prepared").exists() {
        println!("reusing sandbox {}", root.display());
        return Ok(());
    }
    let data = root.join("data");
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let started = Instant::now();

    // A consistent snapshot through SQLite, readable while the owner writes.
    let db = data.join("recur.db");
    let _ = std::fs::remove_file(&db);
    let ro = rusqlite::Connection::open_with_flags(
        src.join("recur.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| format!("open source db: {e}"))?;
    ro.execute("VACUUM INTO ?1", [db.to_string_lossy().as_ref()])
        .map_err(|e| format!("snapshot db: {e}"))?;
    drop(ro);
    println!("snapshot recur.db ({:.0?})", started.elapsed());

    for entry in std::fs::read_dir(&src)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_file() && name.ends_with(".json") {
            std::fs::copy(&path, data.join(&name)).map_err(|e| format!("copy {name}: {e}"))?;
        } else if path.is_dir()
            && [
                "workspaces",
                "app-chat",
                "open-files",
                "Bundles",
                "BundleCache",
            ]
            .contains(&name.as_str())
        {
            copy_dir(&path, &data.join(&name))?;
        }
    }

    let conn = rusqlite::Connection::open(&db).map_err(|e| e.to_string())?;
    // Real path → sandbox path, for the snapshot and the state files.
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    moved.insert(
        src.to_string_lossy().into_owned(),
        data.to_string_lossy().into_owned(),
    );

    let projects: Vec<(String, String)> = {
        let mut st = conn
            .prepare("SELECT id, repo_path FROM projects")
            .map_err(|e| e.to_string())?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect()
    };
    let mut clones: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (id, repo) in &projects {
        let dst = root.join("repos").join(id);
        if !Path::new(repo).join(".git").exists() {
            println!(
                "project {id}: {repo} is not a git checkout; pointing it at {}",
                dst.display()
            );
            moved.insert(repo.clone(), dst.to_string_lossy().into_owned());
            continue;
        }
        let t = Instant::now();
        let parent = dst.parent().unwrap_or(&root);
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        git(
            parent,
            &["clone", "--quiet", "--local", repo, &dst.to_string_lossy()],
        )?;
        git(
            &dst,
            &[
                "fetch",
                "--quiet",
                "--update-head-ok",
                repo,
                "+refs/heads/*:refs/heads/*",
            ],
        )?;
        let remote = git(Path::new(repo), &["remote", "get-url", "origin"]).unwrap_or_default();
        if !remote.is_empty() {
            git(&dst, &["remote", "set-url", "origin", &remote])?;
        }
        git(
            &dst,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                "no-push://recur-smoke",
            ],
        )?;
        // The main checkout: the branch the real one is on, with its uncommitted files.
        if let Ok(branch) = git(
            Path::new(repo),
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        ) {
            let _ = git(&dst, &["checkout", "--quiet", &branch]);
        }
        carry_dirty(Path::new(repo), &dst);
        println!("cloned {id} ({:.0?})", t.elapsed());
        moved.insert(repo.clone(), dst.to_string_lossy().into_owned());
        clones.insert(id.clone(), dst);
    }
    for (id, repo) in &projects {
        conn.execute(
            "UPDATE projects SET repo_path = ?1 WHERE id = ?2",
            [moved.get(repo).cloned().unwrap_or_default(), id.clone()],
        )
        .map_err(|e| e.to_string())?;
    }

    let rows: Vec<(String, String)> = {
        let mut st = conn
            .prepare("SELECT id, doc_json FROM workspaces")
            .map_err(|e| e.to_string())?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .flatten()
            .collect()
    };
    let mut added = 0;
    for (id, doc) in rows {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) else {
            continue;
        };
        let wt = v["worktreePath"].as_str().unwrap_or("").to_string();
        if wt.is_empty() {
            continue;
        }
        let project = v["projectId"].as_str().unwrap_or("");
        let next = if let Some(m) = moved.get(&wt) {
            m.clone()
        } else {
            let name = Path::new(&wt)
                .file_name()
                .map_or_else(|| id.clone(), |n| n.to_string_lossy().into_owned());
            let dst = root.join("worktrees").join(project).join(&name);
            if let Some(clone) = clones.get(project)
                && Path::new(&wt).exists()
                && !dst.exists()
            {
                let branch = v["branch"].as_str().unwrap_or("");
                let has_branch = !branch.is_empty()
                    && git(
                        clone,
                        &[
                            "rev-parse",
                            "--verify",
                            "--quiet",
                            &format!("refs/heads/{branch}"),
                        ],
                    )
                    .is_ok();
                let path = dst.to_string_lossy().into_owned();
                let res = if has_branch {
                    git(
                        clone,
                        &["worktree", "add", "--quiet", "--force", &path, branch],
                    )
                } else {
                    git(clone, &["worktree", "add", "--quiet", "--detach", &path])
                };
                match res {
                    Ok(_) => {
                        added += 1;
                        carry_dirty(Path::new(&wt), &dst);
                    }
                    Err(err) => println!("workspace {id}: {err}"),
                }
            }
            dst.to_string_lossy().into_owned()
        };
        moved.insert(wt, next.clone());
        conn.execute(
            "UPDATE workspaces SET doc_json = json_set(doc_json, '$.worktreePath', ?1) WHERE id = ?2",
            [next, id],
        )
        .map_err(|e| e.to_string())?;
    }
    println!("added {added} worktrees");
    conn.execute(
        "UPDATE settings SET doc_json = json_set(doc_json, '$.cursorBridgePath', '') \
         WHERE json_extract(doc_json, '$.cursorBridgePath') IS NOT NULL",
        [],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);

    // Longest paths first, so a worktree under a repo is not rewritten as the repo.
    let mut pairs: Vec<(&String, &String)> = moved.iter().collect();
    pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    for entry in std::fs::read_dir(&data)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(mut body) = std::fs::read_to_string(&path)
        {
            for (from, to) in &pairs {
                body = body.replace(from.as_str(), to);
            }
            std::fs::write(&path, body).map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(root.join(".prepared"), "").map_err(|e| e.to_string())?;
    println!("sandbox ready in {:.0?}", started.elapsed());
    Ok(())
}

// ---- Services in the sandbox

/// Streams `REPLY` word by word; everything else is the real Claude provider's.
struct PacedProvider {
    real: Arc<dyn Provider>,
    turns: AtomicU64,
}

#[async_trait::async_trait]
impl Provider for PacedProvider {
    fn id(&self) -> &str {
        self.real.id()
    }
    fn label(&self) -> &str {
        self.real.label()
    }
    fn caps(&self) -> &ProviderCaps {
        self.real.caps()
    }
    fn availability(&self, settings: &Settings) -> Availability {
        self.real.availability(settings)
    }
    async fn status(&self, settings: &Settings) -> ProviderStatus {
        self.real.status(settings).await
    }
    async fn test_credentials(
        &self,
        settings: &Settings,
        draft: &CredentialDraft,
    ) -> ProviderStatus {
        self.real.test_credentials(settings, draft).await
    }
    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        self.real.models(settings).await
    }
    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice> {
        self.real.cached_models(settings)
    }
    fn resolve_model(&self, model: &str, flags: &Flags) -> String {
        self.real.resolve_model(model, flags)
    }
    fn context_window(&self, model: &str, flags: &Flags) -> i64 {
        self.real.context_window(model, flags)
    }
    fn model_label(&self, model: &str) -> String {
        self.real.model_label(model)
    }
    fn oneshot_flags(&self, model: &str) -> Flags {
        self.real.oneshot_flags(model)
    }
    fn resume_handle_fits(&self, handle: &str) -> bool {
        self.real.resume_handle_fits(handle)
    }
    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        self.real.display_tool(name, input)
    }
    fn display_tool_result(&self, name: &str, result: &str) -> String {
        self.real.display_tool_result(name, result)
    }
    fn usage_probe(&self) -> Option<Arc<dyn UsageProbe>> {
        None
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let n = self.turns.fetch_add(1, Ordering::SeqCst) + 1;
        let session_id = format!("smoke-{}-{n}", std::process::id());
        let started = Instant::now();
        cx.sink.emit(AgentEvent::Init {
            session_id: session_id.clone(),
            model: req.model.clone(),
            mcp_servers: Vec::new(),
        });
        for word in REPLY.split_inclusive(' ') {
            if cx.cancel.is_cancelled() {
                return Err(ProviderError::canceled());
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
            cx.sink.emit(AgentEvent::TextDelta {
                text: word.to_string(),
                parent: String::new(),
            });
        }
        cx.sink.emit(AgentEvent::Text {
            text: REPLY.to_string(),
            uuid: format!("{session_id}-text"),
            parent: String::new(),
        });
        let result = TurnResult {
            session_id,
            subtype: "success".into(),
            num_turns: 1,
            duration_ms: started.elapsed().as_millis().max(1) as i64,
            text: REPLY.to_string(),
            ..TurnResult::default()
        };
        cx.sink.emit(AgentEvent::TurnEnd(result.clone()));
        Ok(result)
    }
}

/// The real GitHub forge for reads; every write is refused.
struct ReadOnlyForge(Arc<dyn Forge>);

fn refused() -> ForgeError {
    ForgeError::failed("the real-data smoke does not write to GitHub")
}

#[async_trait::async_trait]
impl Forge for ReadOnlyForge {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn pr_source(&self) -> PrSource {
        self.0.pr_source()
    }
    fn label(&self) -> &str {
        self.0.label()
    }
    fn detect(&self, remote_url: &str) -> Option<String> {
        self.0.detect(remote_url)
    }
    fn imported_pr_id(&self, project_id: &str, number: i64) -> String {
        self.0.imported_pr_id(project_id, number)
    }
    fn avatar_url(&self, login: &str, provided: &str, size: u32) -> String {
        self.0.avatar_url(login, provided, size)
    }
    async fn available(&self) -> bool {
        self.0.available().await
    }
    async fn login(&self) -> String {
        self.0.login().await
    }
    async fn viewer(&self, repo: &Path, owner_repo: &str) -> recur_forge::Viewer {
        self.0.viewer(repo, owner_repo).await
    }
    async fn remote_repo(&self, repo: &Path, remote: &str) -> Result<String, ForgeError> {
        self.0.remote_repo(repo, remote).await
    }
    async fn list_prs(
        &self,
        repo: &Path,
        state: recur_forge::PrListState,
        limit: usize,
    ) -> Result<Vec<recur_forge::PrListRow>, ForgeError> {
        self.0.list_prs(repo, state, limit).await
    }
    async fn pr_state(
        &self,
        repo: &Path,
        number: i64,
    ) -> Result<recur_forge::PrStateProbe, ForgeError> {
        self.0.pr_state(repo, number).await
    }
    async fn snapshot(
        &self,
        repo: &Path,
        number: i64,
    ) -> Result<recur_forge::PrSnapshot, ForgeError> {
        self.0.snapshot(repo, number).await
    }
    async fn diff(&self, repo: &Path, number: i64) -> Result<String, ForgeError> {
        self.0.diff(repo, number).await
    }
    async fn checks(
        &self,
        repo: &Path,
        number: i64,
        pr_url: &str,
    ) -> Result<recur_forge::ChecksReport, ForgeError> {
        self.0.checks(repo, number, pr_url).await
    }
    fn merge_comments(
        &self,
        existing: &[PrComment],
        snap: &recur_forge::PrSnapshot,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<PrComment> {
        self.0.merge_comments(existing, snap, now)
    }
    async fn create_pr(&self, _: recur_forge::CreatePr<'_>) -> Result<i64, String> {
        Err(refused().to_string())
    }
    async fn push_head(
        &self,
        _: &Path,
        _: &Path,
        _: &Pr,
        _: &str,
        _: bool,
    ) -> Result<(), ForgeError> {
        Err(refused())
    }
    fn head_ref(&self, number: i64) -> recur_forge::HeadRef {
        self.0.head_ref(number)
    }
    async fn retarget_base(&self, _: &Path, _: i64, _: &str) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn edit_pr(&self, _: &Path, _: i64, _: &str, _: Option<&str>) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn set_draft(&self, _: &Path, _: i64, _: bool) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn branch_requires_pr(
        &self,
        repo: &Path,
        owner_repo: &str,
        branch: &str,
    ) -> Option<bool> {
        self.0.branch_requires_pr(repo, owner_repo, branch).await
    }
    async fn post_comment(
        &self,
        _: &Path,
        _: i64,
        _: &str,
        _: i64,
        _: &str,
    ) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn submit_review(
        &self,
        _: &Path,
        _: i64,
        _: &str,
        _: &str,
        _: &[PendingLineComment],
    ) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn edit_comment(&self, _: &Path, _: i64, _: &str, _: &str) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn delete_comment(&self, _: &Path, _: i64, _: &str) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn reply(
        &self,
        _: &Path,
        _: i64,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<Option<PrComment>, ForgeError> {
        Err(refused())
    }
    async fn set_thread_resolved(&self, _: &Path, _: &str, _: bool) -> Result<(), ForgeError> {
        Err(refused())
    }
    fn reaction_subject(&self, comment: &PrComment, pr: &Pr) -> Option<String> {
        self.0.reaction_subject(comment, pr)
    }
    async fn set_reaction(&self, _: &Path, _: &str, _: &str, _: bool) -> Result<(), ForgeError> {
        Err(refused())
    }
    async fn mentionable_users(
        &self,
        repo: &Path,
        pr_url: &str,
        query: &str,
    ) -> Vec<PrParticipant> {
        self.0.mentionable_users(repo, pr_url, query).await
    }
    async fn merge(
        &self,
        _: &Path,
        _: &Pr,
        _: &MergeMethod,
    ) -> Result<recur_forge::MergeOutcome, ForgeError> {
        Err(refused())
    }
    fn merge_failure_note(&self, err: &ForgeError) -> String {
        self.0.merge_failure_note(err)
    }
    fn merge_message(
        &self,
        pr: &Pr,
        method: &MergeMethod,
        squash: &SquashCommitMessage,
        merge: &MergeCommitMessage,
    ) -> recur_forge::MergeCommitText {
        self.0.merge_message(pr, method, squash, merge)
    }
    fn format_pr_body(&self, body: &str) -> String {
        self.0.format_pr_body(body)
    }
    fn title_limits(&self) -> (usize, usize) {
        self.0.title_limits()
    }
    fn failure_diag(&self, reason: &str) -> String {
        self.0.failure_diag(reason)
    }
    async fn fetch_image(&self, url: &str) -> Result<Option<recur_forge::ImageBytes>, ForgeError> {
        self.0.fetch_image(url).await
    }
    fn pr_label(&self, s: &recur_forge::PrSubject) -> String {
        self.0.pr_label(s)
    }
    fn pr_subject_prompt(&self, s: &recur_forge::PrSubject) -> String {
        self.0.pr_subject_prompt(s)
    }
    fn comment_url(&self, pr_url: &str, comment: &PrComment) -> String {
        self.0.comment_url(pr_url, comment)
    }
    fn commit_url(&self, pr_url: &str, sha: &str) -> String {
        self.0.commit_url(pr_url, sha)
    }
    fn blob_url(&self, pr_url: &str, git_ref: &str, path: &str, line: i64) -> String {
        self.0.blob_url(pr_url, git_ref, path, line)
    }
    fn check_run_url(&self, url: &str, pr_url: &str, pr_number: i64, status: &str) -> String {
        self.0.check_run_url(url, pr_url, pr_number, status)
    }
    fn checks_url(&self, pr_url: &str) -> String {
        self.0.checks_url(pr_url)
    }
}

// ---- Findings

#[derive(Default)]
struct Report {
    panics: Vec<String>,
    toasts: Vec<String>,
    stalls: Vec<String>,
    notes: Vec<String>,
    /// Frames over 500ms and the step they were drawn in.
    slow: Vec<(Duration, String)>,
    /// Timed interactions per budget.
    timed: BTreeMap<Budget, Vec<(Duration, String)>>,
}

/// The interactions the smoke times against frame-time budgets: a run fails when the 95th
/// percentile or the slowest sample of any of them is over its limit. The limits are for this
/// example's build (`dev`: the workspace at `opt-level = 0`, dependencies at 2); the release app
/// is several times faster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Budget {
    /// A key typed into the composer of a long chat, through its frame.
    Keystroke,
    /// The first key after mouse input: GPUI redraws the whole window, every cache off, when the
    /// input modality changes.
    FirstKey,
    /// One wheel step (0.9 of the viewport) of a transcript, through its frame: blocks coming
    /// into view hydrate.
    ScrollStep,
    /// One wheel step of a review: nearly a page of diff rows the frame has not laid out.
    ReviewScrollStep,
    /// Opening a workspace: the route change through its first frame.
    WorkspaceSwitch,
}

impl Budget {
    /// `(p95, max)`.
    fn limits(self) -> (Duration, Duration) {
        let ms = Duration::from_millis;
        match self {
            Budget::Keystroke => (ms(100), ms(150)),
            Budget::FirstKey => (ms(600), ms(900)),
            Budget::ScrollStep => (ms(400), ms(1200)),
            Budget::ReviewScrollStep => (ms(700), ms(1500)),
            Budget::WorkspaceSwitch => (ms(1000), ms(1500)),
        }
    }
}

fn timed(budget: Budget, took: Duration) {
    let s = step().lock().map(|s| s.clone()).unwrap_or_default();
    if let Ok(mut r) = report().lock() {
        r.timed.entry(budget).or_default().push((took, s));
    }
}

/// Pages of the review scrolled on each live workspace.
const REVIEW_PAGES: usize = 12;

/// A chat this long gets its keystrokes timed.
const LONG_CHAT_ROWS: usize = 200;

fn report() -> &'static Mutex<Report> {
    static R: OnceLock<Mutex<Report>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

fn step() -> &'static Mutex<String> {
    static S: OnceLock<Mutex<String>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

static BEAT: AtomicU64 = AtomicU64::new(0);
/// Transcript pane renders in the last frame drawn (stuck-scroll reports).
static TRANSCRIPT_RENDERS: AtomicU64 = AtomicU64::new(0);
/// `session.delta` events the UI received.
static DELTAS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn say(start: Instant, msg: impl AsRef<str>) {
    println!("[{:>7.1}s] {}", start.elapsed().as_secs_f32(), msg.as_ref());
}

fn at(start: Instant, s: impl Into<String>) {
    let s = s.into();
    say(start, &s);
    *step().lock().unwrap_or_else(|e| e.into_inner()) = s;
    BEAT.store(now_ms(), Ordering::SeqCst);
}

/// Panics anywhere (contained by `recur_ui::guard` or not) are findings.
fn count_panics() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let where_ = step().lock().map(|s| s.clone()).unwrap_or_default();
        let line = format!("{info} (during: {where_})");
        if let Ok(mut r) = report().lock() {
            r.panics.push(line);
        }
        prev(info);
    }));
}

/// A UI step that has not advanced in 20s is a stall; one stuck for 5 minutes ends the run.
fn watchdog() {
    std::thread::spawn(|| {
        let mut flagged = 0;
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let idle = now_ms().saturating_sub(BEAT.load(Ordering::SeqCst));
            if idle > 20_000 && flagged != BEAT.load(Ordering::SeqCst) {
                flagged = BEAT.load(Ordering::SeqCst);
                let s = step().lock().map(|s| s.clone()).unwrap_or_default();
                let line = format!("UI stalled {}s at: {s}", idle / 1000);
                println!("STALL {line}");
                // What the main thread is doing, for the report (macOS `sample`).
                let out = std::env::temp_dir().join(format!("recur-smoke-stall-{flagged}.txt"));
                let sampled = Command::new("sample")
                    .args([&std::process::id().to_string(), "5", "-file"])
                    .arg(&out)
                    .output()
                    .is_ok_and(|o| o.status.success());
                if sampled {
                    println!("STALL stacks in {}", out.display());
                }
                if let Ok(mut r) = report().lock() {
                    r.stalls.push(line);
                }
            }
            if idle > 300_000 {
                println!("STALL over 5 minutes; giving up");
                std::process::exit(3);
            }
        }
    });
}

// ---- The drive

struct WindowRoot(Entity<Shell>);

impl WindowRoot {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        recur_ui::style::window_root(self.0.clone())
    }
}

impl Render for WindowRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_chat::register(&mut r);
    recur_view_review::register(&mut r);
    recur_view_observatory::register(&mut r);
    recur_view_workspace::register(&mut r);
    recur_view_prs::register(&mut r);
    recur_view_settings::register(&mut r);
    recur_view_home::register(&mut r);
    recur_view_briefing::register(&mut r);
    recur_view_projects::register(&mut r);
    recur_view_palettes::register(&mut r);
    r
}

fn drive(root: PathBuf) -> i32 {
    let start = Instant::now();
    let data = root.join("data");
    let (writer, _log) = recur_store::applog::tee_app_log(&data);
    let fmt = tracing_subscriber::fmt().event_format(recur_store::applog::GoLogFormat);
    let _ = match writer {
        Some(w) => fmt.with_writer(w).try_init(),
        None => fmt.try_init(),
    };
    count_panics();
    recur_ui::guard::count_renders(true);
    BEAT.store(now_ms(), Ordering::SeqCst);
    watchdog();
    recur_platform::cliinstall::ensure_path();
    at(start, format!("boot on {}", data.display()));

    let store = match recur_store::Store::new() {
        Ok(s) => s,
        Err(err) => {
            println!("FAIL open store: {err}");
            return 1;
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let config = match AppConfig::from_env() {
        Ok(c) => c,
        Err(err) => {
            println!("FAIL config: {err}");
            return 1;
        }
    };
    let paced: Arc<dyn Provider> = Arc::new(PacedProvider {
        real: Arc::new(recur_provider_claude::ClaudeProvider::new(
            recur_provider_claude::ClaudeConfig::default(),
        )),
        turns: AtomicU64::new(0),
    });
    let forge: Arc<dyn Forge> =
        Arc::new(ReadOnlyForge(Arc::new(recur_forge::github::GitHub::new())));
    let svc = rt.block_on(AppServices::new(
        Arc::new(store),
        config,
        AppOptions {
            pin: Some(Some(paced)),
            forges: Some(vec![forge]),
            ..AppOptions::default()
        },
    ));
    let t = Instant::now();
    svc.recover();
    at(start, format!("recovered ({:.1?})", t.elapsed()));
    let services = svc.services();
    rt.spawn({
        let mut rx = services.subscribe();
        async move {
            while let Some(ev) = rx.recv().await {
                if matches!(ev, recur_services::AppEvent::SessionDelta(_)) {
                    DELTAS.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    rt.block_on(svc.start());
    at(start, "services started");

    let app = gpui_kit::application().with_assets(recur_ui::Assets);
    let handle = rt.handle().clone();
    let quit_svc = svc.clone();
    let code = Arc::new(AtomicU64::new(1));
    let exit = code.clone();
    app.run(move |cx: &mut App| {
        let quit_rt = handle.clone();
        let exit = exit.clone();
        cx.on_app_quit(move |_| {
            quit_rt.block_on(quit_svc.shutdown());
            std::process::exit(exit.load(Ordering::SeqCst) as i32);
            #[allow(unreachable_code)]
            async {}
        })
        .detach();
        let registry = registry();
        recur_app::init(&registry, cx);
        let storage = LocalStorage::open(data.join(recur_view_core::LOCAL_STORAGE_FILE));
        let state = AppState::install(services.clone(), storage, cx);
        let ctx = ViewContext {
            state: state.clone(),
            services: services.clone(),
        };
        let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let shell_slot = slot.clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(40.), px(40.)),
                        size: size(px(1380.), px(900.)),
                    })),
                    focus: false,
                    show: true,
                    ..Default::default()
                },
                move |window, cx| {
                    let shell =
                        cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx));
                    *shell_slot.borrow_mut() = Some(shell.clone());
                    let root = cx.new(|_| WindowRoot(shell));
                    cx.new(|cx| gpui_kit::base::Root::new(root, window, cx))
                },
            )
            .expect("open window");
        let window: AnyWindowHandle = window.into();
        let shell = slot.borrow().clone().expect("shell");
        cx.spawn(async move |cx: &mut AsyncApp| {
            run(start, cx, window, shell, state).await;
            let r = report().lock().unwrap_or_else(|e| e.into_inner());
            say(start, "---- summary");
            for n in &r.notes {
                say(start, format!("note: {n}"));
            }
            for p in &r.panics {
                say(start, format!("PANIC {p}"));
            }
            for t in &r.toasts {
                say(start, format!("ERROR TOAST {t}"));
            }
            for s in &r.stalls {
                say(start, format!("STALL {s}"));
            }
            let mut slow = r.slow.clone();
            slow.sort_by_key(|s| std::cmp::Reverse(s.0));
            say(
                start,
                format!("{} frames over 500ms; the slowest:", slow.len()),
            );
            for (d, s) in slow.iter().take(15) {
                say(start, format!("  {d:.0?} at: {s}"));
            }
            let mut over = 0;
            for (budget, samples) in &r.timed {
                let mut d: Vec<Duration> = samples.iter().map(|s| s.0).collect();
                d.sort();
                let pct = |p: f32| d[((d.len() - 1) as f32 * p).round() as usize];
                let (p95_limit, max_limit) = budget.limits();
                say(
                    start,
                    format!(
                        "budget {budget:?} (p95 {p95_limit:.0?}, max {max_limit:.0?}): {} samples, median {:.0?}, p95 {:.0?}, max {:.0?}",
                        d.len(),
                        pct(0.5),
                        pct(0.95),
                        pct(1.)
                    ),
                );
                if pct(0.95) > p95_limit {
                    over += 1;
                    say(start, format!("OVER BUDGET {budget:?} p95 {:.0?}", pct(0.95)));
                }
                for (took, s) in samples.iter().filter(|s| s.0 > max_limit) {
                    over += 1;
                    say(start, format!("OVER BUDGET {budget:?} {took:.0?} at: {s}"));
                }
            }
            let ok = r.panics.is_empty() && r.toasts.is_empty() && r.stalls.is_empty() && over == 0;
            say(
                start,
                format!(
                    "{} ({} panics, {} error toasts, {} stalls, {over} over budget)",
                    if ok { "PASS" } else { "FAIL" },
                    r.panics.len(),
                    r.toasts.len(),
                    r.stalls.len()
                ),
            );
            code.store(u64::from(!ok), Ordering::SeqCst);
            drop(r);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    1
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Draws one frame and returns how long it took.
fn draw(cx: &mut AsyncApp, window: AnyWindowHandle) -> Duration {
    let t = Instant::now();
    let _ = cx.update_window(window, |_, window, cx| {
        let drawn = window.draw(cx);
        drawn.clear(cx);
    });
    BEAT.store(now_ms(), Ordering::SeqCst);
    let took = t.elapsed();
    let renders = recur_ui::guard::take_render_counts();
    TRANSCRIPT_RENDERS.store(
        renders
            .iter()
            .filter(|(k, _)| k.ends_with("::TranscriptPane"))
            .map(|(_, n)| u64::from(*n))
            .sum(),
        Ordering::SeqCst,
    );
    if took > Duration::from_millis(500) {
        let s = step().lock().map(|s| s.clone()).unwrap_or_default();
        if took > Duration::from_secs(2) || std::env::var_os("RECUR_SMOKE_RENDERS").is_some() {
            let views: Vec<String> = renders
                .iter()
                .map(|(k, n)| format!("{}×{n}", k.rsplit("::").next().unwrap_or(k)))
                .collect();
            println!("slow frame {took:.0?} at {s}: {}", views.join(" "));
        }
        if let Ok(mut r) = report().lock() {
            r.slow.push((took, s));
        }
    }
    took
}

/// Draws frames (an unfocused window gets no display-link frames) and collects new error
/// toasts. A frame over 2s is a stall.
async fn frames(cx: &mut AsyncApp, window: AnyWindowHandle, n: usize) {
    for _ in 0..n {
        sleep(cx, 60).await;
        let took = draw(cx, window);
        stall_check(took);
        collect_toasts(cx);
    }
}

fn collect_toasts(cx: &mut AsyncApp) {
    static SEEN: OnceLock<Mutex<BTreeSet<u64>>> = OnceLock::new();
    let errors: Vec<(u64, String)> = cx.update(|cx| {
        let store = recur_ui::widgets::ToastStore::global(cx);
        store
            .read(cx)
            .toasts()
            .iter()
            .filter(|t| t.kind == recur_ui::widgets::ToastKind::Error)
            .map(|t| (t.id, t.text.to_string()))
            .collect()
    });
    let mut seen = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (id, text) in errors {
        if seen.insert(id) {
            let s = step().lock().map(|s| s.clone()).unwrap_or_default();
            println!("ERROR TOAST {text:?} at: {s}");
            if let Ok(mut r) = report().lock() {
                r.toasts.push(format!("{text:?} at: {s}"));
            }
        }
    }
}

/// Frames until `f` holds, for up to `secs`.
async fn until(
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    secs: u64,
    f: impl Fn(&mut AsyncApp) -> bool,
) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f(cx) {
            return true;
        }
        frames(cx, window, 1).await;
    }
    f(cx)
}

fn keys(cx: &mut AsyncApp, window: AnyWindowHandle, keys: &str) {
    for k in keys.split(' ') {
        if let Ok(ks) = Keystroke::parse(k) {
            let _ = cx.update_window(window, |_, window, cx| window.dispatch_keystroke(ks, cx));
        }
    }
}

fn type_text(cx: &mut AsyncApp, window: AnyWindowHandle, s: &str) {
    for ch in s.chars() {
        let k = if ch == ' ' {
            "space".to_string()
        } else {
            ch.to_string()
        };
        keys(cx, window, &k);
    }
}

fn note(msg: String) {
    println!("note: {msg}");
    if let Ok(mut r) = report().lock() {
        r.notes.push(msg);
    }
}

fn limit() -> usize {
    std::env::var("RECUR_SMOKE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX)
}

fn bound_view(cx: &mut AsyncApp) -> Option<Entity<WorkspaceView>> {
    cx.update(|cx| WorkspaceDetail::current(cx)?.read(cx).bound_view(cx))
}

async fn run(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    shell: Entity<Shell>,
    state: Entity<AppState>,
) {
    at(start, "waiting for boot");
    if !until(cx, window, 60, |cx| state.read_with(cx, |s, _| s.booted)).await {
        note("the app did not boot within 60s".into());
        return;
    }
    frames(cx, window, 10).await;
    let (projects, workspaces, prs) = state.read_with(cx, |s, _| {
        (
            s.projects
                .iter()
                .map(|p| (p.id.clone(), p.name.clone()))
                .collect::<Vec<_>>(),
            s.workspaces
                .iter()
                .map(|w| (w.id.clone(), w.archived, w.worktree_path.clone()))
                .collect::<Vec<_>>(),
            s.prs.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        )
    });
    at(
        start,
        format!(
            "booted: {} projects, {} workspaces, {} PRs",
            projects.len(),
            workspaces.len(),
            prs.len()
        ),
    );

    for (id, name) in &projects {
        at(start, format!("project {name}"));
        state.update(cx, |s, cx| s.open_project(id, cx));
        frames(cx, window, 6).await;
    }

    let mut streamed = false;
    // `RECUR_SMOKE_FROM=n` starts the workspace pass at the nth (1-based), to reproduce a late finding.
    let from = std::env::var("RECUR_SMOKE_FROM")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(0, |n| n.saturating_sub(1));
    let only = std::env::var("RECUR_SMOKE_WORKSPACE").ok();
    for (i, (id, archived, wt)) in workspaces
        .iter()
        .enumerate()
        .skip(from)
        .filter(|(_, (id, _, _))| only.as_ref().is_none_or(|o| o == id))
        .take(limit())
    {
        let live = !archived && !wt.is_empty() && Path::new(wt).is_dir();
        at(
            start,
            format!("workspace {}/{} {id}", i + 1, workspaces.len()),
        );
        if *archived {
            // Archived workspaces never bind a detail view (`liveWorkspaceIds`); the route
            // still has to draw.
            state.update(cx, |s, cx| s.open_workspace(id, false, cx));
            frames(cx, window, 2).await;
            continue;
        }
        visit_workspace(start, cx, window, &state, id, live).await;
        if live && !streamed {
            streamed = stream_turn(start, cx, window, &state, id).await;
        }
    }
    if !streamed {
        note("no live workspace to stream a chat turn in".into());
    }
    if std::env::var_os("RECUR_SMOKE_WORKSPACES_ONLY").is_some() {
        at(start, "done");
        return;
    }

    let only = std::env::var("RECUR_SMOKE_PR").ok();
    for (i, id) in prs
        .iter()
        .enumerate()
        .filter(|(_, id)| only.as_ref().is_none_or(|o| o == *id))
        .take(limit())
    {
        at(start, format!("PR {}/{} {id}", i + 1, prs.len()));
        state.update(cx, |s, cx| s.open_pr(id, cx));
        let detail = cx.update(|cx| {
            shell
                .read(cx)
                .route_view(ViewName::PrDetail)
                .and_then(|v| v.downcast::<PrDetail>().ok())
        });
        frames(cx, window, 8).await;
        let Some(detail) = detail else {
            note("PR detail is not registered".into());
            break;
        };
        for tab in [PrTab::Conv, PrTab::Files, PrTab::Commits, PrTab::Checks] {
            at(start, format!("PR {id} {tab:?}"));
            detail.update(cx, |d, cx| d.set_tab(tab, cx));
            frames(cx, window, 12).await;
        }
    }

    at(start, "Settings");
    for group in recur_view_settings::nav::nav() {
        for item in group.items {
            at(
                start,
                format!("Settings / {} / {}", group.group, item.label),
            );
            state.update(cx, |s, cx| s.open_settings(Some(item.id.id()), cx));
            frames(cx, window, 8).await;
        }
    }
    for (name, view) in [
        ("Briefing", ViewName::Briefing),
        ("Issues", ViewName::Issues),
        ("Schedules", ViewName::Schedules),
        ("PRs", ViewName::Pr),
    ] {
        at(start, name);
        state.update(cx, |s, cx| s.go(view, None, cx));
        frames(cx, window, 20).await;
    }

    at(start, "palettes");
    let live = workspaces
        .iter()
        .find(|(_, archived, wt)| !archived && !wt.is_empty() && Path::new(wt).is_dir())
        .map(|w| w.0.clone());
    state.update(cx, |s, cx| s.go(ViewName::Briefing, None, cx));
    frames(cx, window, 6).await;
    for (name, chord) in [
        ("Go to", "cmd-k"),
        ("Run Command", "cmd-shift-p"),
        ("Shortcuts", "cmd-shift-/"),
    ] {
        palette(start, cx, window, name, chord).await;
    }
    if let Some(id) = live {
        state.update(cx, |s, cx| s.open_workspace(&id, false, cx));
        frames(cx, window, 20).await;
        for (name, chord) in [
            ("Open in", "cmd-o"),
            ("Run", "cmd-r"),
            ("Quick Open", "cmd-p"),
        ] {
            palette(start, cx, window, name, chord).await;
        }
        at(start, "palette Language");
        state.update(cx, |s, cx| s.open_syntax_mode_picker(cx));
        frames(cx, window, 4).await;
        type_text(cx, window, "rust");
        frames(cx, window, 4).await;
        keys(cx, window, "down escape");
        frames(cx, window, 4).await;
    }
    at(start, "done");
}

async fn palette(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    name: &str,
    chord: &str,
) {
    at(start, format!("palette {name}"));
    keys(cx, window, chord);
    frames(cx, window, 6).await;
    type_text(cx, window, "re");
    frames(cx, window, 6).await;
    keys(cx, window, "down down up");
    frames(cx, window, 3).await;
    keys(cx, window, "escape");
    frames(cx, window, 4).await;
}

async fn visit_workspace(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: &Entity<AppState>,
    id: &str,
    live: bool,
) {
    sleep(cx, 60).await;
    let t = Instant::now();
    state.update(cx, |s, cx| s.open_workspace(id, false, cx));
    draw(cx, window);
    timed(Budget::WorkspaceSwitch, t.elapsed());
    let bound = until(cx, window, 15, |cx| {
        bound_view(cx).is_some_and(|v| cx.update(|cx| !v.read(cx).tab_codes().is_empty()))
    })
    .await;
    let Some(view) = bound_view(cx).filter(|_| bound) else {
        note(format!("workspace {id}: no chat tabs after 15s"));
        return;
    };
    let codes = view.read_with(cx, |v, _| v.tab_codes());
    for code in &codes {
        at(start, format!("workspace {id} chat {code}"));
        view.update(cx, |v, cx| v.select_chat_tab(code, cx));
        frames(cx, window, 4).await;
        // The transcript is loaded once its row count holds for a few frames.
        let chat = view.read_with(cx, |v, _| v.chat().clone());
        let mut last = usize::MAX;
        let mut stable = 0;
        for _ in 0..60 {
            let n = chat.read_with(cx, |c, _| c.props().messages.len());
            stable = if n == last { stable + 1 } else { 0 };
            last = n;
            if stable >= 3 {
                break;
            }
            frames(cx, window, 1).await;
        }
        scroll_transcript(start, cx, window, state, &chat, id, code).await;
    }
    if live {
        observatory(start, cx, window, state, &view, id).await;
    }
}

/// Top to bottom, a viewport at a time, with wheel events over the transcript as a user
/// scrolls; windowed blocks hydrate as they come into view, so the end is where the offset
/// stops growing.
async fn scroll_transcript(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: &Entity<AppState>,
    chat: &Entity<recur_view_chat::chat::AgentChat>,
    id: &str,
    code: &str,
) {
    at(start, format!("workspace {id} chat {code}: scroll"));
    let handle = chat.read_with(cx, |c, _| c.test_scroll().clone());
    move_mouse(cx, window, handle.bounds().center());
    frames(cx, window, 1).await;
    wheel(cx, window, &handle, 1e9);
    frames(cx, window, 3).await;
    let mut at_end = 0;
    let mut reached = false;
    let mut unmoved = 0;
    let mut trail: std::collections::VecDeque<(f32, f32, u64)> = Default::default();
    for _ in 0..2000 {
        let page = f32::from(handle.bounds().size.height).max(200.);
        let max = f32::from(handle.max_offset().y);
        let y = -f32::from(handle.offset().y);
        if trail.len() == 8 {
            trail.pop_front();
        }
        trail.push_back((y, max, TRANSCRIPT_RENDERS.load(Ordering::SeqCst)));
        if y >= max - 0.5 {
            at_end += 1;
            if at_end >= 3 {
                reached = true;
                break;
            }
        } else {
            at_end = 0;
        }
        sleep(cx, 60).await;
        let t = Instant::now();
        wheel(cx, window, &handle, -page * 0.9);
        draw(cx, window);
        let took = t.elapsed();
        timed(Budget::ScrollStep, took);
        stall_check(took);
        collect_toasts(cx);
        // A wheel that moves nothing short of the end: something took the events.
        unmoved = if -f32::from(handle.offset().y) == y && y < max - 0.5 {
            unmoved + 1
        } else {
            0
        };
        if unmoved >= 5 {
            let what = state.read_with(cx, |s, _| {
                format!(
                    "offset {y:.0} of {max:.0} at {:?} in {:?}; {} permission cards, {} questions, overlays {:?}",
                    handle.bounds().center(),
                    handle.bounds(),
                    s.permission_requests.len(),
                    s.user_questions.len(),
                    s.overlays
                )
            });
            let s = step().lock().map(|s| s.clone()).unwrap_or_default();
            println!("STUCK {what} at {s}");
            if let Ok(mut r) = report().lock() {
                r.stalls.push(format!(
                    "the transcript did not move under the wheel ({what}) at: {s}"
                ));
            }
            reached = true;
            break;
        }
    }
    if !reached {
        let s = step().lock().map(|s| s.clone()).unwrap_or_default();
        let line = format!(
            "the transcript never scrolled to its end (last offsets, ends and pane renders {trail:?}; scroller {:?}) at: {s}",
            handle.bounds()
        );
        println!("STALL {line}");
        if let Ok(mut r) = report().lock() {
            r.stalls.push(line);
        }
    }
    if chat.read_with(cx, |c, _| c.props().messages.len()) >= LONG_CHAT_ROWS {
        type_in_long_chat(start, cx, window, chat, id, code).await;
    }
}

/// One wheel event of `dy` pixels over the scroller (positive scrolls toward the top).
fn wheel(cx: &mut AsyncApp, window: AnyWindowHandle, handle: &gpui_kit::ScrollHandle, dy: f32) {
    wheel_at(cx, window, handle.bounds().center(), dy);
}

fn move_mouse(cx: &mut AsyncApp, window: AnyWindowHandle, at: gpui_kit::Point<gpui_kit::Pixels>) {
    let _ = cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
    });
}

/// One wheel event of `dy` pixels at `at`.
fn wheel_at(
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    at: gpui_kit::Point<gpui_kit::Pixels>,
    dy: f32,
) {
    let ev = gpui_kit::ScrollWheelEvent {
        position: at,
        delta: gpui_kit::ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Default::default(),
        touch_phase: gpui_kit::TouchPhase::Moved,
    };
    let _ = cx.update_window(window, |_, window, cx| {
        window.dispatch_event(gpui_kit::PlatformInput::ScrollWheel(ev), cx);
    });
}

/// Types into the composer and erases it again, timing each key through its frame.
async fn type_in_long_chat(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    chat: &Entity<recur_view_chat::chat::AgentChat>,
    id: &str,
    code: &str,
) {
    at(start, format!("workspace {id} chat {code}: type"));
    let before = chat.read_with(cx, |c, _| c.input().to_string());
    let _ = cx.update_window(window, |_, window, cx| {
        chat.update(cx, |c, cx| c.focus_composer(window, cx))
    });
    frames(cx, window, 2).await;
    let rounds = std::env::var("RECUR_SMOKE_KEY_ROUNDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let seq = "a b c backspace backspace backspace ".repeat(rounds);
    for (i, k) in seq.split_whitespace().enumerate() {
        sleep(cx, 60).await;
        let t = Instant::now();
        keys(cx, window, k);
        draw(cx, window);
        let took = t.elapsed();
        // The transcript was just scrolled with the wheel.
        timed(
            if i == 0 {
                Budget::FirstKey
            } else {
                Budget::Keystroke
            },
            took,
        );
        stall_check(took);
    }
    let after = chat.read_with(cx, |c, _| c.input().to_string());
    if after != before {
        note(format!(
            "typing in {id} {code} left the draft {after:?} (was {before:?})"
        ));
    }
}

fn stall_check(took: Duration) {
    if took > Duration::from_secs(2) {
        let s = step().lock().map(|s| s.clone()).unwrap_or_default();
        if let Ok(mut r) = report().lock() {
            r.stalls.push(format!("a frame took {took:.1?} at: {s}"));
        }
    }
}

async fn observatory(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: &Entity<AppState>,
    view: &Entity<WorkspaceView>,
    id: &str,
) {
    let obs = view.read_with(cx, |v, _| v.observatory().clone());
    at(start, format!("workspace {id}: explorer"));
    let _ = cx.update_window(window, |_, window, cx| {
        obs.update(cx, |o, cx| {
            o.select_code_panel(CodePanelKey::Files, window, cx)
        })
    });
    frames(cx, window, 8).await;
    let wt = state.read_with(cx, |s, _| {
        s.workspaces
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.worktree_path.clone())
            .unwrap_or_default()
    });
    let file = ["README.md", "Cargo.toml", "package.json", "go.mod"]
        .into_iter()
        .find(|f| Path::new(&wt).join(f).is_file())
        .map(str::to_string)
        .or_else(|| {
            std::fs::read_dir(&wt).ok()?.flatten().find_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (e.path().is_file() && !name.starts_with('.')).then_some(name)
            })
        });
    if let Some(file) = file {
        at(start, format!("workspace {id}: open {file}"));
        let _ = cx.update_window(window, |_, window, cx| {
            obs.update(cx, |o, cx| {
                o.open_path(recur_view_observatory::OpenRequest::new(file), window, cx)
                    .detach()
            })
        });
        frames(cx, window, 12).await;
    }
    at(start, format!("workspace {id}: review"));
    let restored = obs.read_with(cx, |o, cx| o.showing_review(cx));
    if !restored {
        obs.update(cx, |o, cx| o.click_review_toggle(cx));
    }
    frames(cx, window, 15).await;
    if obs.read_with(cx, |o, cx| o.showing_review(cx)) {
        at(start, format!("workspace {id}: review scroll"));
        let at_point = obs.read_with(cx, |o, _| o.main_bounds().center());
        let page = obs.read_with(cx, |o, _| f32::from(o.main_bounds().size.height));
        move_mouse(cx, window, at_point);
        for _ in 0..REVIEW_PAGES {
            sleep(cx, 60).await;
            let t = Instant::now();
            wheel_at(cx, window, at_point, -page.max(200.) * 0.9);
            draw(cx, window);
            let took = t.elapsed();
            timed(Budget::ReviewScrollStep, took);
            stall_check(took);
            collect_toasts(cx);
        }
        frames(cx, window, 4).await;
    }
    if !restored {
        obs.update(cx, |o, cx| o.click_review_toggle(cx));
        frames(cx, window, 4).await;
    }
    at(start, format!("workspace {id}: terminal"));
    state.update(cx, |s, cx| s.set_terminal_dock_open(id, true, cx));
    frames(cx, window, 20).await;
    state.update(cx, |s, cx| s.set_terminal_dock_open(id, false, cx));
    frames(cx, window, 4).await;
}

async fn stream_turn(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: &Entity<AppState>,
    id: &str,
) -> bool {
    at(start, format!("stream a chat turn in {id}"));
    let Some(view) = bound_view(cx) else {
        return false;
    };
    let chat = view.read_with(cx, |v, _| v.chat().clone());
    let before = chat.read_with(cx, |c, _| c.props().messages.len());
    let deltas = DELTAS.load(Ordering::SeqCst);
    let _ = cx.update_window(window, |_, window, cx| {
        chat.update(cx, |c, cx| c.focus_composer(window, cx))
    });
    frames(cx, window, 3).await;
    type_text(cx, window, PROMPT);
    frames(cx, window, 2).await;
    keys(cx, window, "enter");
    let saw_stream = std::cell::Cell::new(false);
    let ended = until(cx, window, 120, |cx| {
        chat.read_with(cx, |c, _| {
            let m = &c.props().messages;
            saw_stream.set(saw_stream.get() || m.iter().any(|m| m.streaming));
            m.len() > before && m.iter().skip(before).any(|m| m.turn_end)
        })
    })
    .await;
    let perms = state.read_with(cx, |s, _| s.permission_requests.len());
    let deltas = DELTAS.load(Ordering::SeqCst) - deltas;
    if !ended {
        note(format!("the streamed turn in {id} did not end within 120s"));
        return false;
    }
    let reply = chat.read_with(cx, |c, _| {
        c.props()
            .messages
            .iter()
            .skip(before)
            .filter(|m| !m.me && m.tool.is_empty() && !m.turn_end)
            .map(|m| m.body.clone())
            .collect::<Vec<_>>()
            .join(" / ")
    });
    say(
        start,
        format!(
            "turn ended; streamed rows seen: {}; {deltas} session.delta events; permissions {perms}; reply {reply:?}",
            saw_stream.get()
        ),
    );
    if deltas == 0 {
        note(format!("the turn in {id} streamed no session.delta events"));
    } else if !saw_stream.get() {
        note(format!(
            "the turn in {id} ended without a streaming row on screen"
        ));
    }
    true
}
