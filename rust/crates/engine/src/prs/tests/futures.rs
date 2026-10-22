//! Services poll E5's async methods from GPUI tasks, so every one of them
//! (and the hooks earlier slices await) must be a `Send` future.

use recur_domain::{MergeMethod, Project, Workspace};

use crate::Engine;

fn send<T: Send>(_: T) {}

fn e5_futures(e: &Engine, p: &Project, w: &Workspace) {
    let m = MergeMethod::Squash;
    send(e.sync_prs());
    send(e.run_pr_sweep());
    send(e.sweep_prs());
    send(e.project_forge("/repo"));
    send(e.branch_requires_pr(p, "origin", "main"));
    send(e.refresh_pr_conversation("pr"));
    send(e.refresh_pr_diff("pr"));
    send(e.refresh_pr_checks("pr"));
    send(e.refresh_pr_commits("pr"));
    send(e.refresh_pr_behind("pr"));
    send(e.ensure_commit_diff("pr", "abc"));
    send(e.add_pr_comment("pr", "a.rs", 1, "x", 0, ""));
    send(e.submit_review("pr", "approve", ""));
    send(e.update_pr_details("pr", "t", "b"));
    send(e.update_pr_comment("pr", "c", "b"));
    send(e.delete_pr_comment("pr", "c"));
    send(e.reply_pr_comment("pr", "c", "b"));
    send(e.toggle_pr_comment_reaction("pr", "c", "heart"));
    send(e.set_pr_thread_resolved("pr", "t", true));
    send(e.ready_pr("pr"));
    send(e.set_pr_draft("pr", true));
    send(e.search_pr_mentionable_users("pr", "jo"));
    send(e.merge_pr("pr", &m));
    send(e.merge_workspace("ws", &m));
    send(e.workspace_land_state("ws"));
    send(e.update_workspace_with_base("ws", "merge"));
    send(e.pull_workspace_pr("ws"));
    send(e.open_workspace_pr("ws", false));
    send(e.open_workspace_pr_with("ws", "t", "b", false));
    send(e.open_pr_from_agent("ws", "t", "b", false));
    send(e.set_workspace_base_branch("ws", "main"));
    send(e.send_back_to_loop("pr", &[]));
    send(e.maestro_send_back("ws", "x"));
    send(e.update_pr_with_base("pr", "merge"));
    send(e.create_workspace_from_pr("pr"));
    send(e.ensure_pr_chat_host("pr"));
    send(e.archive_pr_chat_hosts("pr", ""));
    send(e.link_workspace_to_matching_prs(w));
    send(e.fetch_open_pr_remote(w));
}

#[test]
fn e5_futures_are_send() {
    let px = super::support::Px::new();
    e5_futures(px.eng(), &Project::default(), &Workspace::default());
}
