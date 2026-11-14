//! The website's demo story (`dataset.json`) as typed service fakes: the seed holds the
//! workspaces, PRs and inbox, and per-method overrides answer the chats, files, diffs, issues
//! and schedules the captures show.

use std::collections::BTreeMap;

use recur_domain::{
    ChatItem, Commit, FeedItem, FileContent, FileDiff, FileDiffResult, Finding, InboxItem,
    OutlineSymbol, Policy, Pr, Project, Resource, ScheduledPrompt, SessionView, Settings,
    TreeEntry, Workspace, WorkspaceChatTab,
};
use recur_services::Services;
use recur_services::api::{
    AppChatSelection, AssignedIssue, EditorSession, LoopInfo, ThemeInfo, ThemePreviewInfo,
    WorkspaceChatSelection,
};
use recur_services::fake::{Fake, Overrides, Seed};
use serde::Deserialize;

const MODEL: &str = "claude-code/claude-sonnet-4-6";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrReview {
    workspace_id: String,
    tabs: Vec<WorkspaceChatTab>,
    transcript: Vec<ChatItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dataset {
    workspace_id: String,
    projects: Vec<Project>,
    workspaces: Vec<Workspace>,
    pull_requests: Vec<Pr>,
    inbox: Vec<InboxItem>,
    settings: Settings,
    policy: Policy,
    resource: Resource,
    files: BTreeMap<String, String>,
    base_files: BTreeMap<String, String>,
    diffs: Vec<FileDiff>,
    transcript: Vec<ChatItem>,
    loop_transcript: Vec<ChatItem>,
    facts: Vec<FeedItem>,
    sessions: Vec<SessionView>,
    tabs: Vec<WorkspaceChatTab>,
    findings: Vec<Finding>,
    commits: Vec<Commit>,
    schedules: Vec<ScheduledPrompt>,
    issues: Vec<AssignedIssue>,
    app_chat: Vec<ChatItem>,
    pr_review: PrReview,
}

fn load() -> Dataset {
    let mut v: serde_json::Value =
        serde_json::from_str(include_str!("dataset.json")).expect("dataset.json is JSON");
    // Fact cards carry their data as a JSON string on the wire.
    for fact in v["facts"].as_array_mut().into_iter().flatten() {
        let data = fact["data"].take();
        fact["data"] = data.to_string().into();
    }
    serde_json::from_value(v).expect("dataset.json matches the domain types")
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

/// The fakes, with `session` restored as every workspace's editor session.
pub fn services(session: EditorSession) -> Services {
    let d = load();
    let seed = Seed {
        // In the website demo's order: `ListWorkspaces` answers the seed reversed, as its
        // transport does.
        workspaces: d.workspaces.clone(),
        prs: d.pull_requests.clone(),
        projects: d.projects.clone(),
        inbox: d.inbox.clone(),
        policy: d.policy.clone(),
        settings: d.settings.clone(),
        resource: d.resource.clone(),
        ..Seed::default()
    };

    let main_ws = d.workspace_id.clone();
    let host = d.pr_review.workspace_id.clone();
    let status: BTreeMap<String, String> = d
        .diffs
        .iter()
        .map(|f| (f.path.clone(), f.status.clone()))
        .collect();
    let changed: Vec<String> = d.diffs.iter().map(|f| f.path.clone()).collect();
    let diff_result = FileDiffResult {
        files: Some(d.diffs.clone()),
        truncated: false,
    };

    let o = Overrides::default();
    let o = {
        let (tabs, host_tabs, host) = (d.tabs.clone(), d.pr_review.tabs.clone(), host.clone());
        o.list_workspace_chats(move |_, ws| {
            Ok(if ws == host {
                host_tabs.clone()
            } else {
                tabs.clone()
            })
        })
    };
    let o = {
        let (main_ws, host) = (main_ws.clone(), host.clone());
        let (chat, maestro, review) = (
            d.transcript.clone(),
            d.loop_transcript.clone(),
            d.pr_review.transcript.clone(),
        );
        o.workspace_chat_items(move |_, (ws, code)| {
            Ok(if ws == host {
                review.clone()
            } else if ws == main_ws && code == "MA" {
                maestro.clone()
            } else {
                chat.clone()
            })
        })
    };
    let o = {
        let host = host.clone();
        o.workspace_chat_selection(move |_, (ws, code)| {
            let mode = match code.as_str() {
                "MA" => "loop",
                "PA2" => "review",
                _ if ws == host => "review",
                _ => "agent",
            };
            Ok(WorkspaceChatSelection {
                model: MODEL.into(),
                mode: mode.into(),
                simplify: Some(true),
                ..Default::default()
            })
        })
    };
    let o = {
        let (main_ws, facts) = (main_ws.clone(), d.facts.clone());
        o.workspace_facts(move |_, ws| {
            Ok(if ws == main_ws {
                facts.clone()
            } else {
                Vec::new()
            })
        })
    };
    let o = {
        let sessions = d.sessions.clone();
        o.workspace_sessions(move |_, _| Ok(sessions.clone()))
    };
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
                model: MODEL.into(),
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
        });
    let o = {
        let app_chat = d.app_chat.clone();
        o.app_chat_items(move |_, _| Ok(app_chat.clone()))
    };
    let o = {
        let schedules = d.schedules.clone();
        o.list_scheduled_prompts(move |_, project| {
            Ok(schedules
                .iter()
                .filter(|s| project.is_empty() || s.project_id == project)
                .cloned()
                .collect())
        })
    };
    let o = {
        let issues = d.issues.clone();
        o.list_assigned_issues(move |_, ()| Ok(issues.clone()))
    };
    let o = {
        let findings = d.findings.clone();
        o.list_findings(move |_, _| Ok(findings.clone()))
    };
    let o = {
        let commits = d.commits.clone();
        o.list_commits(move |_, _| Ok(commits.clone()))
    };
    let o = {
        let files = d.files.clone();
        o.list_files(move |_, _| Ok(files.keys().cloned().collect()))
    };
    let o = {
        let files = d.files.clone();
        o.list_tree(move |_, (_, _, path)| Ok(list_tree(&files, &path)))
    };
    let o = {
        let files = d.files.clone();
        o.read_file(move |_, (_, _, path)| {
            let content = files.get(&path).cloned().unwrap_or_default();
            Ok(FileContent {
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
            })
        })
    };
    let o = {
        let (base, files) = (d.base_files.clone(), d.files.clone());
        o.file_at_base(move |_, (_, _, path)| {
            Ok(base
                .get(&path)
                .or_else(|| files.get(&path))
                .cloned()
                .unwrap_or_default())
        })
    };
    let o = {
        let diffs = d.diffs.clone();
        o.file_diff(move |_, (_, _, path)| {
            Ok(diffs
                .iter()
                .find(|f| f.path == path)
                .cloned()
                .unwrap_or(FileDiff {
                    path,
                    ..Default::default()
                }))
        })
    };
    let o = {
        let r = diff_result.clone();
        o.diff_vs_base(move |_, _| Ok(r.clone()))
    };
    let o = {
        let r = diff_result.clone();
        o.diff_working(move |_, _| Ok(r.clone()))
    };
    let o = {
        let s = status.clone();
        o.git_status(move |_, _| Ok(s.clone()))
    };
    let o = {
        let s = status.clone();
        o.working_status(move |_, _| Ok(s.clone()))
    };
    let o = o.list_changed_files(move |_, _| Ok(changed.clone()));

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
