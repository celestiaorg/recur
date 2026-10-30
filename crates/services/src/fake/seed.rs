//! The fake's backend state: the e2e seed (`seed.json`, a dump of the
//! demo-seeded store written by `cargo run -p recur-store --bin seeddump`)
//! plus what calls add as they run (extra chat tabs, editor sessions, …).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use recur_domain::{
    Agent, Escalation, FeedItem, InboxItem, Policy, Pr, Project, ProjectGroup, Resource, Settings,
    Workspace, WorkspaceChatTab,
};
use serde::Deserialize;

use crate::api::{EditorSession, ShellState};

#[derive(Debug, Clone, Default)]
pub struct Seed {
    /// In sidebar order (`ListWorkspaces` answers them reversed).
    pub workspaces: Vec<Workspace>,
    /// PR details; `ListPRs` answers them without files and commits.
    pub prs: Vec<Pr>,
    pub projects: Vec<Project>,
    pub project_groups: Vec<ProjectGroup>,
    pub inbox: Vec<InboxItem>,
    pub policy: Policy,
    pub settings: Settings,
    pub resource: Resource,
    pub feeds: BTreeMap<String, Vec<FeedItem>>,
    pub agents: BTreeMap<String, Vec<Agent>>,
    pub escalations: BTreeMap<String, Escalation>,
    /// Chat tabs per workspace beyond the default PA / MA tabs.
    pub chats: BTreeMap<String, Vec<WorkspaceChatTab>>,
    pub closed_chats: BTreeMap<String, Vec<WorkspaceChatTab>>,
    /// Workspaces whose Loop (MA) tab was closed.
    pub hidden_loop_tabs: BTreeSet<String>,
    pub chat_order: BTreeMap<String, Vec<String>>,
    /// `(loop id, policy id)` by `(workspace id, tab code)`.
    pub chat_loops: BTreeMap<(String, String), (String, String)>,
    /// Ask Recur tabs after the primary APP tab.
    pub app_chats: Vec<WorkspaceChatTab>,
    pub editor_sessions: BTreeMap<String, EditorSession>,
    pub shell_state: Option<ShellState>,
    pub expanded_schedules: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dump {
    loops: Vec<Workspace>,
    pr_by_id: BTreeMap<String, Pr>,
    prs: Vec<Pr>,
    projects: Vec<Project>,
    inbox: Vec<InboxItem>,
    policy: Policy,
    settings: Settings,
    resource: Resource,
    feed_by_loop: BTreeMap<String, Option<Vec<FeedItem>>>,
    agents_by_loop: BTreeMap<String, Vec<Agent>>,
    esc_by_loop: BTreeMap<String, Escalation>,
}

impl Seed {
    /// A copy of the e2e seed, every workspace given a mock worktree path
    /// (`/mock/worktrees/<id>`) so the Observatory lists files.
    pub fn e2e() -> Self {
        static SEED: OnceLock<Seed> = OnceLock::new();
        SEED.get_or_init(|| {
            let d: Dump = serde_json::from_str(include_str!("seed.json"))
                .expect("seed.json matches the domain types");
            let mut workspaces = d.loops;
            for w in &mut workspaces {
                if w.worktree_path.is_empty() {
                    w.worktree_path = format!("/mock/worktrees/{}", w.id);
                }
            }
            let mut by_id = d.pr_by_id;
            let prs = d
                .prs
                .into_iter()
                .map(|p| by_id.remove(&p.id).unwrap_or(p))
                .collect();
            Seed {
                workspaces,
                prs,
                projects: d.projects,
                inbox: d.inbox,
                policy: d.policy,
                settings: d.settings,
                resource: d.resource,
                feeds: d
                    .feed_by_loop
                    .into_iter()
                    .map(|(k, v)| (k, v.unwrap_or_default()))
                    .collect(),
                agents: d.agents_by_loop,
                escalations: d.esc_by_loop,
                ..Seed::default()
            }
        })
        .clone()
    }

    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn workspace_mut(&mut self, id: &str) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub fn pr(&self, id: &str) -> Option<&Pr> {
        self.prs.iter().find(|p| p.id == id)
    }

    pub fn pr_mut(&mut self, id: &str) -> Option<&mut Pr> {
        self.prs.iter_mut().find(|p| p.id == id)
    }

    /// The seed workspace id for a goal (sidebar rows show `short`, so specs
    /// match by id).
    pub fn workspace_id_by_goal(&self, goal: &str) -> Option<String> {
        self.workspaces
            .iter()
            .find(|w| w.goal == goal)
            .map(|w| w.id.clone())
    }
}
