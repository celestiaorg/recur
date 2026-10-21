//! ⇧⌘N / New Blank Workspace (`createBlankWorkspace` in `lib/new-workspace.ts`): a blank
//! Agent-mode workspace in the focused git project, without the modal.

use std::cell::Cell;

use gpui_kit::{App, Entity};
use recur_ui::widgets::ToastKind;
use recur_view_core::{AppState, ViewName, WorkspaceSeed, call, toast};

/// The first chat tab of a new workspace.
const FIRST_CHAT: &str = "PA";

thread_local! {
    static CREATING: Cell<bool> = const { Cell::new(false) };
}

/// `createBlankWorkspace`.
pub fn create_blank_workspace(state: &Entity<AppState>, cx: &mut App) {
    if CREATING.with(|c| c.replace(true)) {
        return;
    }
    let refresh = state.update(cx, |s, cx| {
        let o = &mut s.overlays;
        o.palette = false;
        o.shortcuts = false;
        o.open_in = false;
        o.commands = false;
        o.syntax_mode = false;
        o.run_menu = false;
        o.stop_menu = false;
        if o.new_task {
            s.close_new_task(cx);
        }
        s.overlays.new_project = false;
        cx.notify();
        s.refresh_projects(cx)
    });
    let state = state.clone();
    cx.spawn(async move |cx| {
        refresh.await;
        let project = state.read_with(cx, |s, _| s.git_project_for_new_workspace().cloned());
        let Some(project) = project else {
            state.update(cx, |s, cx| s.open_new_task("", None, cx));
            CREATING.with(|c| c.set(false));
            return;
        };
        let services = state.read_with(cx, |s, _| s.services().clone());
        let base = resolve_base_branch(&services, &project).await;
        let created = call(
            cx,
            services
                .workspaces
                .create_workspace(&project.id, "", &base, "agent", "", "", None),
            "Could not create the workspace",
        )
        .await;
        if let Some(ws) = created {
            let id = ws.id.clone();
            state.update(cx, |s, cx| s.upsert_workspace(ws, cx));
            cx.update(|cx| toast(ToastKind::Success, "Workspace created", cx));
            reveal_created_workspace(&state, &id, cx).await;
        }
        CREATING.with(|c| c.set(false));
    })
    .detach();
}

/// `resolveBaseBranch`: the project's base when it still resolves, else the first real tip;
/// empty or unborn repos keep the configured / `main` fallback.
async fn resolve_base_branch(services: &recur_services::Services, project: &recur_domain::Project) -> String {
    let fallback = match project.base_branch.trim() {
        "" => "main".to_string(),
        b => b.to_string(),
    };
    match services
        .projects
        .list_branches_for_path(&project.repo_path, &fallback)
        .await
    {
        Ok(list) if !list.is_empty() => {
            if list.iter().any(|b| b.name == fallback) {
                fallback
            } else {
                list.first()
                    .map(|b| b.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or(fallback)
            }
        }
        _ => fallback,
    }
}

/// `revealCreatedWorkspace` for a blank seed: open it from Welcome; anywhere else stay and
/// remember the first chat's selection (`startParkedOpening` with nothing to send).
async fn reveal_created_workspace(state: &Entity<AppState>, id: &str, cx: &mut gpui_kit::AsyncApp) {
    let seed = WorkspaceSeed {
        id: id.to_string(),
        mode: "agent".into(),
        ..WorkspaceSeed::default()
    };
    let from_welcome = state.read_with(cx, |s, _| s.view == ViewName::Welcome);
    state.update(cx, |s, cx| s.set_workspace_seed(seed, cx));
    if from_welcome {
        state.update(cx, |s, cx| s.open_workspace(id, false, cx));
        return;
    }
    let services = state.read_with(cx, |s, _| s.services().clone());
    let _ = async {
        services.workspaces.list_workspace_chats(id).await?;
        services
            .workspaces
            .remember_workspace_chat_selection(id, FIRST_CHAT, "", &Default::default(), "agent", None)
            .await
    }
    .await;
}
