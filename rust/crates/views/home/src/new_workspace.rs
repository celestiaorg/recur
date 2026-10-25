//! The store-driven flows of `lib/new-workspace.ts` the New workspace modal runs: revealing
//! the created workspace (open it from Welcome, else stay and park the opening prompt),
//! `startParkedOpening`, `waitForWorktree` and `linkLinearProject`. The pure launch rules are
//! `recur_engine::view::new_workspace`.

use std::cell::Cell;
use std::rc::Rc;

use futures::StreamExt as _;
use gpui_kit::{AppContext as _, AsyncApp, Entity};
use recur_domain::{ChatAttachment, Workspace};
use recur_engine::chat::surface::{CHAT_SURFACE_WORKSPACE, ChatSurface};
use recur_engine::view::chat::{
    ChatAttachmentDraft, is_chat_injected_reply, is_chat_stopped_reply, to_send_attachments,
};
use recur_engine::view::chat_surface::attach_chat_surface;
use recur_engine::view::mentions::compose_marked_text;
use recur_engine::view::new_workspace::is_parked_opening_failure;
use recur_ui::widgets::ToastKind;
use recur_view_core::modes::ws_creating;
use recur_view_core::state::SeedAttachment;
use recur_view_core::{AppState, ViewName, WorkspaceSeed, bridge_error_message, call, toast};

/// The first chat tab of a new workspace.
const FIRST_CHAT: &str = "PA";

/// `AbortController`: the flag a flow checks after each step, and a wake for waits it races.
#[derive(Clone, Default)]
pub struct Abort {
    flag: Rc<Cell<bool>>,
    wake: Rc<std::cell::RefCell<Vec<futures::channel::oneshot::Sender<()>>>>,
}

impl Abort {
    pub fn aborted(&self) -> bool {
        self.flag.get()
    }

    pub fn abort(&self) {
        self.flag.set(true);
        for tx in self.wake.borrow_mut().drain(..) {
            let _ = tx.send(());
        }
    }

    /// The same controller (`promptCtl === ac`).
    pub fn same(&self, other: &Abort) -> bool {
        Rc::ptr_eq(&self.flag, &other.flag)
    }

    /// Resolves when aborted.
    pub fn signal(&self) -> futures::channel::oneshot::Receiver<()> {
        let (tx, rx) = futures::channel::oneshot::channel();
        if self.aborted() {
            let _ = tx.send(());
        } else {
            self.wake.borrow_mut().push(tx);
        }
        rx
    }
}

/// The opening the modal hands the created workspace (`OpeningSeed`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpeningSeed {
    pub id: String,
    pub text: String,
    pub display: String,
    pub mode: String,
    pub model: String,
    pub flags: std::collections::BTreeMap<String, String>,
    pub simplify: Option<bool>,
    pub attachments: Vec<ChatAttachmentDraft>,
}

impl OpeningSeed {
    fn workspace_seed(&self) -> WorkspaceSeed {
        WorkspaceSeed {
            id: self.id.clone(),
            text: self.text.clone(),
            display: self.display.clone(),
            mode: self.mode.clone(),
            model: self.model.clone(),
            flags: self.flags.clone(),
            simplify: self.simplify,
            attachments: self
                .attachments
                .iter()
                .map(|a| SeedAttachment {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    mime: a.mime.clone(),
                    data_url: a.data_url.clone(),
                })
                .collect(),
        }
    }
}

pub fn wire_attachments(drafts: &[ChatAttachmentDraft]) -> Vec<ChatAttachment> {
    to_send_attachments(drafts)
        .into_iter()
        .map(|a| ChatAttachment {
            id: a.id,
            name: a.name,
            mime: a.mime,
            data: a.data,
        })
        .collect()
}

/// `waitForWorktree`: the workspace once create and setup finished, `None` if it failed, was
/// aborted or left the store.
pub async fn wait_for_worktree(
    state: &Entity<AppState>,
    id: &str,
    abort: &Abort,
    cx: &mut AsyncApp,
) -> Option<Workspace> {
    let current = |s: &AppState| -> Option<Option<Workspace>> {
        if abort.aborted() {
            return Some(None);
        }
        let Some(w) = s.workspace(id) else {
            return Some(None);
        };
        if !w.last_error.is_empty() || w.phase == "failed" {
            return Some(None);
        }
        if ws_creating(Some(w)) {
            return None;
        }
        if !w.worktree_path.trim().is_empty() {
            return Some(Some(w.clone()));
        }
        None
    };
    if let Some(now) = state.read_with(cx, |s, _| current(s)) {
        return now;
    }
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    let _sub = cx.update(|cx| {
        recur_view_core::observe_state_app(
            state,
            recur_view_core::Topics::ALL,
            move |_, _| {
                let _ = tx.unbounded_send(());
            },
            cx,
        )
    });
    let mut aborted = abort.signal();
    loop {
        futures::select_biased! {
            _ = aborted => return None,
            n = rx.next() => n?,
        }
        if let Some(v) = state.read_with(cx, |s, _| current(s)) {
            return v;
        }
    }
}

/// `revealCreatedWorkspace`: from Welcome, open the new workspace (seeding its opening);
/// anywhere else stay put and send the opening without mounting the workbench.
pub async fn reveal_created_workspace(
    state: &Entity<AppState>,
    seed: OpeningSeed,
    is_loop: bool,
    cx: &mut AsyncApp,
) {
    let from_welcome = state.read_with(cx, |s, _| s.view == ViewName::Welcome);
    if !from_welcome {
        if is_loop {
            return;
        }
        return start_parked_opening(state, seed, cx).await;
    }
    let id = seed.id.clone();
    state.update(cx, |s, cx| {
        if !is_loop {
            s.set_workspace_seed(seed.workspace_seed(), cx);
        }
        s.open_workspace(&id, false, cx);
    });
}

/// `persistOpeningChat`: create PA if needed, then remember the dock selection.
async fn persist_opening_chat(
    state: &Entity<AppState>,
    seed: &OpeningSeed,
    mode: &str,
    cx: &mut AsyncApp,
) -> recur_services::Result<()> {
    let services = state.read_with(cx, |s, _| s.services().clone());
    services.workspaces.list_workspace_chats(&seed.id).await?;
    services
        .workspaces
        .remember_workspace_chat_selection(
            &seed.id,
            FIRST_CHAT,
            &seed.model,
            &seed.flags,
            mode,
            seed.simplify,
        )
        .await
}

/// `startParkedOpening`: remember the dock and send the opening prompt without mounting the
/// workbench. A pre-turn failure keeps the seed so opening the workspace can retry it.
pub async fn start_parked_opening(state: &Entity<AppState>, seed: OpeningSeed, cx: &mut AsyncApp) {
    let mode = if seed.mode.is_empty() {
        "agent".to_string()
    } else {
        seed.mode.clone()
    };
    let ids: Vec<&str> = seed.attachments.iter().map(|a| a.id.as_str()).collect();
    let marked = compose_marked_text(&seed.text, ids).trim().to_string();
    state.update(cx, |s, cx| s.set_workspace_seed(seed.workspace_seed(), cx));
    if marked.is_empty() && seed.attachments.is_empty() {
        let _ = persist_opening_chat(state, &seed, &mode, cx).await;
        return;
    }
    let services = state.read_with(cx, |s, _| s.services().clone());
    if let Err(err) = persist_opening_chat(state, &seed, &mode, cx).await {
        let msg = bridge_error_message(&err);
        cx.update(|cx| {
            toast(
                ToastKind::Error,
                if msg.is_empty() {
                    "Could not prepare the workspace chat".into()
                } else {
                    msg
                },
                cx,
            )
        });
        return;
    }
    let label = match seed.display.trim() {
        "" => marked.clone(),
        d => d.to_string(),
    };
    let (svc, id) = (services.clone(), seed.id.clone());
    cx.background_spawn(async move {
        let _ = svc
            .workspaces
            .apply_provisional_chat_titles(&id, FIRST_CHAT, &label)
            .await;
    })
    .detach();

    if wait_for_worktree(state, &seed.id, &Abort::default(), cx)
        .await
        .is_none()
    {
        return;
    }
    if state
        .update(cx, |s, cx| s.take_workspace_seed(&seed.id, cx))
        .is_none()
    {
        return;
    }
    let wire = attach_chat_surface(
        &marked,
        &ChatSurface {
            surface: CHAT_SURFACE_WORKSPACE.into(),
            ..Default::default()
        },
    );
    let reply = services
        .workspaces
        .send_workspace_message(
            &seed.id,
            FIRST_CHAT,
            &wire,
            &seed.model,
            &seed.flags,
            &wire_attachments(&seed.attachments),
            &mode,
            seed.display.trim(),
            false,
        )
        .await;
    match reply {
        Ok(reply) => {
            if is_chat_injected_reply(&reply) || is_chat_stopped_reply(&reply) {
                return;
            }
            let text = reply.trim().to_string();
            if !is_parked_opening_failure(&text) {
                return;
            }
            cx.update(|cx| toast(ToastKind::Error, text, cx));
            state.update(cx, |s, cx| s.set_workspace_seed(seed.workspace_seed(), cx));
        }
        Err(err) => {
            let msg = bridge_error_message(&err);
            cx.update(|cx| {
                toast(
                    ToastKind::Error,
                    if msg.is_empty() {
                        "The agent hit an error — try again.".into()
                    } else {
                        msg
                    },
                    cx,
                )
            });
            state.update(cx, |s, cx| s.set_workspace_seed(seed.workspace_seed(), cx));
        }
    }
}

/// The projects whose `linearProjects` change when `key` routes to `project_id`: it is added
/// there and removed from any other (a key routes to one project). Returns `(id, new list)`.
pub fn linear_link_edits(
    projects: &[recur_domain::Project],
    project_id: &str,
    key: &str,
) -> Vec<(String, String)> {
    if key.is_empty() {
        return Vec::new();
    }
    let same = |s: &str| s.to_lowercase() == key.to_lowercase();
    let mut out = Vec::new();
    for p in projects {
        let cur: Vec<&str> = p
            .linear_projects
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        let has = cur.iter().any(|s| same(s));
        if has == (p.id == project_id) {
            continue;
        }
        let next: Vec<&str> = if has {
            cur.into_iter().filter(|s| !same(s)).collect()
        } else {
            cur.into_iter().chain(std::iter::once(key)).collect()
        };
        out.push((p.id.clone(), next.join(", ")));
    }
    out
}

/// `linkLinearProject`: send this Linear key's issues to `project_id` from now on.
pub async fn link_linear_project(
    state: &Entity<AppState>,
    project_id: &str,
    key: &str,
    cx: &mut AsyncApp,
) {
    let (projects, services) =
        state.read_with(cx, |s, _| (s.projects.clone(), s.services().clone()));
    for (id, list) in linear_link_edits(&projects, project_id, key) {
        let Some(mut p) = projects.iter().find(|p| p.id == id).cloned() else {
            continue;
        };
        p.linear_projects = list;
        if let Some(saved) = call(
            cx,
            services.projects.update_project_run_config(&p),
            "Could not link Linear project",
        )
        .await
        {
            state.update(cx, |s, cx| {
                s.update_project(&id, |x| x.linear_projects = saved.linear_projects, cx)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_domain::Project;

    fn p(id: &str, lp: &str) -> Project {
        Project {
            id: id.into(),
            linear_projects: lp.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_linear_key_moves_to_one_project() {
        let list = vec![p("a", "Mobile, ENG"), p("b", ""), p("c", "eng")];
        assert_eq!(
            linear_link_edits(&list, "b", "ENG"),
            vec![
                ("a".into(), "Mobile".into()),
                ("b".into(), "ENG".into()),
                ("c".into(), "".into()),
            ]
        );
        assert!(linear_link_edits(&list, "a", "Mobile").is_empty());
        assert!(linear_link_edits(&list, "a", "").is_empty());
    }
}
