//! The website's demo story (`dataset.json`) as typed service fakes: the seed holds the
//! workspaces, PRs and inbox, and per-method overrides answer the chats, files, diffs, issues,
//! schedules, insights and terminals the captures show. Its dates move with the real clock.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Local, SecondsFormat, Utc};
use recur_domain::{
    ChangedFile, ChatItem, Commit, FeedItem, FileContent, FileDiff, FileDiffResult, Finding,
    InboxItem, OutlineSymbol, Policy, Pr, Project, Resource, ScheduledPrompt, SessionView,
    Settings, TreeEntry, UserQuestion, Workspace, WorkspaceChanges, WorkspaceChatTab,
    WorkspaceLandState, next_scheduled_time,
};
use recur_services::Services;
use recur_services::api::{
    AppChatSelection, AssignedIssue, EditorSession, LoopInfo, ModelGroup, Snapshot, TermInfo,
    TerminalDockChrome, ThemeInfo, ThemePreviewInfo, WorkspaceChatSelection,
};
use recur_services::fake::{Answer, Fake, Overrides, Seed};
use serde::Deserialize;
use serde_json::Value;

const MODEL: &str = "claude-code/claude-opus-5-5";
const APP_MODEL: &str = "claude-code/claude-sonnet-5-5";
/// The workspace with a live background process, and that process.
pub const BACKGROUND: &str = "payments-webhooks";
/// The workspace waiting on a question.
const ASKING: &str = "LOOP-2235";
const TERMINAL: &str = "term-api";

/// What one workspace's checkout, chats and review hold.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Worktree {
    tabs: Vec<WorkspaceChatTab>,
    chats: BTreeMap<String, Vec<ChatItem>>,
    /// Chat code to model, where it is not [`MODEL`].
    models: BTreeMap<String, String>,
    facts: Vec<FeedItem>,
    sessions: Vec<SessionView>,
    files: BTreeMap<String, String>,
    base_files: BTreeMap<String, String>,
    /// Against the base branch.
    diffs: Vec<FileDiff>,
    /// None of `diffs` is committed yet.
    uncommitted: bool,
    staged: Vec<String>,
    commits: Vec<Commit>,
    findings: Vec<Finding>,
}

impl Worktree {
    fn changes(&self) -> FileDiffResult {
        FileDiffResult {
            files: Some(self.diffs.clone()),
            truncated: false,
        }
    }

    fn working(&self) -> FileDiffResult {
        if self.uncommitted {
            self.changes()
        } else {
            FileDiffResult {
                files: Some(Vec::new()),
                truncated: false,
            }
        }
    }

    fn land(&self) -> WorkspaceLandState {
        WorkspaceLandState {
            base: "main".into(),
            dirty: self.uncommitted,
            ..Default::default()
        }
    }

    fn file(&self, path: &str) -> String {
        self.files.get(path).cloned().unwrap_or_default()
    }

    fn base(&self, path: &str) -> String {
        self.base_files.get(path).cloned().unwrap_or_default()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dataset {
    projects: Vec<Project>,
    workspaces: Vec<Workspace>,
    pull_requests: Vec<Pr>,
    inbox: Vec<InboxItem>,
    questions: Vec<UserQuestion>,
    settings: Settings,
    policy: Policy,
    resource: Resource,
    model_catalog: Vec<ModelGroup>,
    usage: Snapshot,
    schedules: Vec<ScheduledPrompt>,
    issues: Vec<AssignedIssue>,
    app_chat: Vec<ChatItem>,
    /// The background process's output.
    terminal: String,
    worktrees: BTreeMap<String, Worktree>,
}

fn load() -> Dataset {
    let mut v: Value =
        serde_json::from_str(include_str!("dataset.json")).expect("dataset.json is JSON");
    let clock = v["clock"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .expect("dataset.json has a clock");
    let now = Utc::now();
    to_now(&mut v, now - clock.to_utc(), now);
    // Fact cards carry their data as a JSON string on the wire.
    let worktrees = v["worktrees"].as_object_mut().into_iter().flatten();
    for fact in worktrees
        .filter_map(|(_, wt)| wt.get_mut("facts")?.as_array_mut())
        .flatten()
    {
        let data = fact["data"].take();
        fact["data"] = data.to_string().into();
    }
    let mut d: Dataset = serde_json::from_value(v).expect("dataset.json matches the domain types");
    for s in &mut d.schedules {
        if !s.paused {
            s.next_run_at = next_scheduled_time(s, now)
                .expect("a valid schedule")
                .map_or(0, |t| t.timestamp_millis());
        }
        if let Some(w) = &mut s.watch_status {
            w.last_event_at += now.timestamp_millis();
        }
    }
    d
}

/// Moves the story's clock to now: RFC 3339 strings by `by`, and `{{t:-90}}` (a log time),
/// `{{at:90}}` and `{{due:90}}` (a wakeup's) placeholders to that many seconds from now.
fn to_now(v: &mut Value, by: Duration, now: DateTime<Utc>) {
    match v {
        Value::String(s) => {
            if let Ok(t) = DateTime::parse_from_rfc3339(s) {
                *s = (t.to_utc() + by).to_rfc3339_opts(SecondsFormat::Secs, true);
            } else if s.contains("{{") {
                *s = fill(s, now);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|v| to_now(v, by, now)),
        Value::Object(o) => o.values_mut().for_each(|v| to_now(v, by, now)),
        _ => {}
    }
}

fn fill(s: &str, now: DateTime<Utc>) -> String {
    let mut parts = s.split("{{");
    let mut out = parts.next().unwrap_or_default().to_string();
    for part in parts {
        let (expr, rest) = part.split_once("}}").expect("a closed placeholder");
        let (kind, secs) = expr.split_once(':').expect("a placeholder is kind:seconds");
        let at =
            (now + Duration::seconds(secs.parse().expect("whole seconds"))).with_timezone(&Local);
        match kind {
            "t" => out.push_str(&at.format("%H:%M:%S").to_string()),
            "at" => out.push_str(&at.format("%-I:%M %p").to_string()),
            _ => out.push_str(&at.to_rfc3339_opts(SecondsFormat::Secs, false)),
        }
        out.push_str(rest);
    }
    out
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = c.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - c.len()));
        for i in 0..4 {
            out.push(if i <= c.len() {
                char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize])
            } else {
                '='
            });
        }
    }
    out
}

fn shipped_themes() -> Vec<recur_bundles::Theme> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles");
    recur_bundles::discover_catalog(&recur_bundles::DiscoverRoots {
        user_dir: String::new(),
        default_dir: root.to_string_lossy().into_owned(),
    })
    .map(|c| c.themes)
    .unwrap_or_default()
}

fn list_tree(files: &BTreeMap<String, String>, path: &str) -> Vec<TreeEntry> {
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    let mut out: BTreeMap<String, TreeEntry> = BTreeMap::new();
    for file in files.keys() {
        let Some(rest) = file.strip_prefix(&prefix) else {
            continue;
        };
        let (name, dir) = match rest.split_once('/') {
            Some((name, _)) => (name, true),
            None => (rest, false),
        };
        out.entry(name.to_string()).or_insert_with(|| TreeEntry {
            name: name.to_string(),
            path: format!("{prefix}{name}"),
            dir,
            ..Default::default()
        });
    }
    out.into_values().collect()
}

fn status(diffs: FileDiffResult) -> BTreeMap<String, String> {
    diffs
        .files
        .into_iter()
        .flatten()
        .map(|f| (f.path, f.status))
        .collect()
}

/// The fakes, with `session` restored as every workspace's editor session.
pub fn services(session: EditorSession) -> Services {
    let d = load();
    let seed = Seed {
        // In the website demo's order: `ListWorkspaces` answers the seed reversed, as its
        // transport does.
        workspaces: d.workspaces,
        prs: d.pull_requests,
        projects: d.projects,
        inbox: d.inbox,
        policy: d.policy,
        settings: d.settings,
        resource: d.resource,
        ..Seed::default()
    };

    let wt = Arc::new(d.worktrees);
    /// One override answered from the workspace's [`Worktree`] (empty for one without).
    macro_rules! from_worktree {
        ($o:ident, $method:ident, |$args:pat_param| $ws:expr, |$w:ident| $answer:expr) => {
            $o.$method({
                let wt = wt.clone();
                move |_, $args| {
                    let none = Worktree::default();
                    let $w = wt.get($ws.as_str()).unwrap_or(&none);
                    Ok($answer)
                }
            })
        };
    }
    let o = Overrides::default();
    let o = from_worktree!(o, list_workspace_chats, |ws| ws, |w| w.tabs.clone());
    let o = from_worktree!(o, workspace_chat_items, |(ws, code)| ws, |w| {
        w.chats.get(&code).cloned().unwrap_or_default()
    });
    let o = from_worktree!(o, workspace_chat_selection, |(ws, code)| ws, |w| {
        let tab = w.tabs.iter().find(|t| t.code == code);
        WorkspaceChatSelection {
            model: w.models.get(&code).map_or(MODEL, String::as_str).into(),
            mode: tab.map_or("agent", |t| t.mode.as_str()).into(),
            simplify: Some(false),
            ..Default::default()
        }
    });
    let o = from_worktree!(o, workspace_facts, |ws| ws, |w| w.facts.clone());
    let o = from_worktree!(o, workspace_sessions, |(ws, _, _)| ws, |w| w
        .sessions
        .clone());
    let o = from_worktree!(o, list_findings, |ws| ws, |w| w.findings.clone());
    let o = from_worktree!(o, list_commits, |(_, ws)| ws, |w| w.commits.clone());
    let o = from_worktree!(o, list_files, |(_, ws)| ws, |w| w
        .files
        .keys()
        .cloned()
        .collect());
    let o = from_worktree!(o, list_tree, |(_, ws, path)| ws, |w| list_tree(
        &w.files, &path
    ));
    let o = from_worktree!(o, read_file, |(_, ws, path)| ws, |w| {
        let content = w.file(&path);
        FileContent {
            language: if path.ends_with(".go") {
                "go"
            } else {
                "markdown"
            }
            .into(),
            size: content.len() as i64,
            path,
            content,
            writable: true,
            ..Default::default()
        }
    });
    let o = from_worktree!(o, file_at_base, |(_, ws, path)| ws, |w| w.base(&path));
    let o = from_worktree!(o, file_at_head, |(_, ws, path)| ws, |w| {
        if w.uncommitted {
            w.base(&path)
        } else {
            w.file(&path)
        }
    });
    let o = from_worktree!(o, file_diff, |(_, ws, path)| ws, |w| {
        w.diffs
            .iter()
            .find(|f| f.path == path)
            .cloned()
            .unwrap_or(FileDiff {
                path,
                ..Default::default()
            })
    });
    let o = from_worktree!(o, diff_vs_base, |(_, ws)| ws, |w| w.changes());
    let o = from_worktree!(o, diff_working, |(_, ws)| ws, |w| w.working());
    let o = from_worktree!(o, git_status, |(_, ws)| ws, |w| status(w.changes()));
    let o = from_worktree!(o, working_status, |(_, ws)| ws, |w| status(w.working()));
    let o = from_worktree!(o, staged_files, |(_, ws)| ws, |w| w.staged.clone());
    let o = from_worktree!(o, unstaged_files, |(_, ws)| ws, |w| status(w.working())
        .into_keys()
        .filter(|p| !w.staged.contains(p))
        .collect());
    let o = from_worktree!(o, list_changed_files, |ws| ws, |w| status(w.changes())
        .into_keys()
        .collect());
    let o = from_worktree!(o, workspace_land_state, |ws| ws, |w| w.land());
    let o = from_worktree!(o, fetch_working_copy_upstream, |ws| ws, |w| w.land());
    let o = from_worktree!(o, workspace_changes, |ws| ws, |w| WorkspaceChanges {
        base_branch: "main".into(),
        files: w
            .diffs
            .iter()
            .map(|f| ChangedFile {
                path: f.path.clone(),
                add: f.diff.add,
                del: f.diff.del,
            })
            .collect(),
    });

    let o = o
        .list_loops(|_, ()| {
            Ok(vec![LoopInfo {
                id: "recur.loops.pr".into(),
                name: "To a PR".into(),
                hint: "Plan, code, review, and open a pull request".into(),
                ..Default::default()
            }])
        })
        .app_chat_selection(|_, _| {
            Ok(AppChatSelection {
                model: APP_MODEL.into(),
                ..Default::default()
            })
        })
        .get_editor_session(move |_, _| Ok(Some(session.clone())))
        .outline(|_, _| {
            Ok(vec![OutlineSymbol {
                name: "refreshSession".into(),
                kind: "func".into(),
                line: 9,
                ..Default::default()
            }])
        })
        .insights(|_, (prev, start, end)| Ok(super::insights::report(prev, start, end)));
    let o = {
        let (app_chat, questions) = (d.app_chat, d.questions);
        o.app_chat_items(move |_, _| Ok(app_chat.clone()))
            .list_pending_questions(move |_, ()| Ok(questions.clone()))
    };
    // The fake files a workspace under Needs you by its group, which the sidebar counts on top of
    // its question.
    let o = o.get_briefing(|seed, ()| {
        let mut seed = seed.clone();
        if let Some(w) = seed.workspace_mut(ASKING) {
            w.group = "needs".into();
        }
        Answer::later(async move {
            let fake = Fake::new(seed, Overrides::default()).services();
            let mut b = fake.briefing.get_briefing().await?;
            for w in b.workspaces.iter_mut().flatten() {
                w.last_activity_at -= 2 * 60_000;
            }
            Ok(b)
        })
    });
    let o = {
        let schedules = d.schedules;
        o.list_scheduled_prompts(move |_, project| {
            Ok(schedules
                .iter()
                .filter(|s| project.is_empty() || s.project_id == project)
                .cloned()
                .collect())
        })
    };
    let o = {
        let (issues, catalog) = (d.issues, d.model_catalog);
        o.list_assigned_issues(move |_, ()| Ok(issues.clone()))
            .list_model_catalog(move |_, ()| Ok(catalog.clone()))
    };
    let o = {
        let (usage, refreshed) = (d.usage.clone(), d.usage);
        o.get_provider_usage(move |_, ()| Ok(usage.clone()))
            .refresh_provider_usage(move |_, ()| Ok(refreshed.clone()))
    };
    let terminals = |ws: &str| {
        if ws != BACKGROUND {
            return Vec::new();
        }
        vec![TermInfo {
            id: TERMINAL.into(),
            workspace_id: BACKGROUND.into(),
            cwd: "/example/payments-api/payments-webhooks".into(),
            shell: "/bin/zsh".into(),
            title: "api dev server".into(),
            created_at: (Utc::now() - Duration::seconds(190)).to_rfc3339(),
            busy: true,
            active: true,
            background: true,
            started_by: "PA".into(),
            ..Default::default()
        }]
    };
    let output = base64(d.terminal.as_bytes());
    let o = o
        .list_terminals(move |_, ws| Ok(terminals(&ws)))
        .restore_workspace_terminals(move |_, (ws, _, _)| Ok(terminals(&ws)))
        .attach_terminal(move |_, _| Ok(output.clone()))
        .list_terminal_dock_chrome(|_, ()| {
            Ok(vec![TerminalDockChrome {
                workspace_id: BACKGROUND.into(),
                open: true,
                ..Default::default()
            }])
        });

    // The shipped editor themes, so the capture reads the real one.
    let themes = shipped_themes();
    let o = {
        let themes = themes.clone();
        o.list_themes(move |_, ()| {
            Ok(themes
                .iter()
                .map(|t| ThemeInfo {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    path: t.path.clone(),
                    ui_theme: t.ui_theme.clone(),
                    order: t.order,
                    preview: ThemePreviewInfo {
                        bg: t.preview.bg.clone(),
                        fg: t.preview.fg.clone(),
                        r#fn: t.preview.func.clone(),
                        name: t.preview.name.clone(),
                        string: t.preview.string.clone(),
                    },
                    bundle_name: t.bundle_name.clone(),
                    source_root: t.source_root.clone(),
                })
                .collect())
        })
    };
    let o = o.read_theme(move |_, id| {
        let t = themes.iter().find(|t| t.id == id);
        t.and_then(|t| std::fs::read_to_string(&t.abs_path).ok())
            .ok_or_else(|| recur_services::Error::new(format!("theme {id} not found")))
    });

    Fake::new(seed, o).services()
}
