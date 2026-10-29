use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use recur_domain::{
    Agent, AutonomyLevel, Escalation, FINDING_ACTOR_ENGINE, FINDING_ACTOR_HUMAN, FeedItem,
    FileDiff, Finding, FindingStatus, IdType, InboxItem, Pr, PrSource, Project, ProjectGroup,
    RunCommand, Severity, Workspace, valid_id, verify_dev_cmd,
};
use recur_project::projfile;

use super::*;
use crate::store::{fill_missing_pr_times, inbox_targets_workspace};

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn ids(ps: &[Project]) -> Vec<String> {
    ps.iter().map(|p| p.id.clone()).collect()
}

fn run(name: &str, cmd: &str) -> RunCommand {
    RunCommand {
        name: name.into(),
        cmd: cmd.into(),
    }
}

fn proj(id: &str, repo: &Path) -> Project {
    Project {
        id: id.into(),
        name: id.into(),
        repo_path: s(repo),
        ..Default::default()
    }
}

fn write_toml(repo: &Path, body: &str) {
    fs::create_dir_all(repo.join(".recur")).unwrap();
    fs::write(repo.join(projfile::REL_PATH), body).unwrap();
}

fn read_toml(repo: &Path) -> String {
    fs::read_to_string(repo.join(projfile::REL_PATH)).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn finding(
    id: &str,
    ws: &str,
    iter: i64,
    file: &str,
    line: i64,
    sev: Severity,
    msg: &str,
    status: &str,
) -> Finding {
    Finding {
        id: id.into(),
        workspace_id: ws.into(),
        iteration: iter,
        file: file.into(),
        line,
        severity: sev,
        message: msg.into(),
        status: status.into(),
        ..Default::default()
    }
}

fn pr(id: &str, ws: &str, number: i64, source: PrSource) -> Pr {
    Pr {
        id: id.into(),
        workspace_id: ws.into(),
        number,
        source,
        ..Default::default()
    }
}

fn all_projects(st: &Store) -> Vec<Project> {
    st.projects().unwrap_or_default()
}

#[test]
fn seed_then_load_is_stable() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    assert_eq!(all_projects(&s1).len(), 3);
    assert_eq!(s1.workspaces().len(), 5);
    assert_eq!(s1.prs().len(), 3);
    assert!(s1.escalation("LOOP-2218").is_some());
    let feed_len = s1.feed("LOOP-2231").unwrap().len();
    assert!(feed_len > 0);
    s1.close().unwrap();

    let s2 = open_temp(dir.path());
    assert_eq!(all_projects(&s2).len(), 3);
    assert_eq!(s2.workspaces().len(), 5);
    let pr = s2.pr("482").unwrap();
    assert_eq!(pr.number, 482);
    assert_eq!(pr.files.unwrap().len(), 3);
    assert_eq!(s2.feed("LOOP-2231").unwrap().len(), feed_len);
}

#[test]
fn add_project_persists_across_reopen() {
    let home = tmp();
    crate::testenv::run(
        module_path!(),
        "add_project_persists_across_reopen",
        &[("HOME", home.path())],
        || {
            let home = std::env::var("HOME").unwrap();
            fs::create_dir_all(Path::new(&home).join("code").join("widgets")).unwrap();
            let dir = tmp();
            let s1 = open_temp(dir.path());
            let saved = s1
                .add_project(Project {
                    id: "widgets".into(),
                    name: "widgets".into(),
                    repo_path: "~/code/widgets".into(),
                    base_branch: "main".into(),
                    setup_cmd: "make setup".into(),
                    teardown_cmd: "make clean-wt".into(),
                    run_commands: vec![run("dev", "make dev PORT=$RECUR_PORT")],
                    app_url: "http://localhost:$RECUR_PORT".into(),
                    env: "VITE_PORT=$RECUR_PORT\n# dogfood\nRECUR_DATA_DIR=$HOME/tmp".into(),
                    write_project_file: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(saved.id, "widgets");
            s1.close().unwrap();

            let s2 = open_temp(dir.path());
            let got = s2.project("widgets").unwrap();
            assert_eq!(verify_dev_cmd(&got), "make dev PORT=$RECUR_PORT");
            assert_eq!(got.repo_path, "~/code/widgets");
            assert_eq!(got.teardown_cmd, "make clean-wt");
            assert!(got.env.contains("VITE_PORT=$RECUR_PORT") && got.env.contains("# dogfood"));
        },
    );
}

#[test]
fn reorder_projects_persists_across_reopen() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let mut reversed = ids(&all_projects(&s1));
    assert!(reversed.len() >= 2);
    reversed.reverse();
    s1.reorder_projects(&reversed).unwrap();
    assert_eq!(ids(&all_projects(&s1)), reversed);
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    assert_eq!(ids(&all_projects(&s2)), reversed);
}

#[test]
fn reorder_projects_rejects_bad_id_set() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let before = ids(&all_projects(&st));
    assert!(st.reorder_projects(&before[..1]).is_err());
    let mut extra = before.clone();
    extra.push("nope".into());
    assert!(st.reorder_projects(&extra).is_err());
    let dup = vec![before[0].clone(), before[0].clone(), before[1].clone()];
    assert!(
        st.reorder_projects(&dup)
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    assert_eq!(ids(&all_projects(&st)), before);
}

/// A closed DB makes add_project fail and leaves no phantom project.
#[test]
fn add_project_db_error_leaves_cache_unchanged() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let before = all_projects(&st).len();
    db(&st).close().unwrap();
    assert!(
        st.add_project(Project {
            id: "ghost".into(),
            name: "ghost".into(),
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(all_projects(&st).len(), before);
    assert!(st.project("ghost").is_none());
}

/// Mutating a fetched PR's nested slices never reaches the store's copy.
#[test]
fn pr_getter_deep_copy() {
    let st = Store::new_in_memory();
    let mut pr = st.pr("482").unwrap();
    let files = pr.files.as_mut().unwrap();
    let want_files = files.len();
    let orig_path = files[0].path.clone();
    files[0].path = "MUTATED/path".into();
    files.push(FileDiff {
        path: "extra".into(),
        ..Default::default()
    });
    if let Some(l) = files[0].lines.as_mut() {
        l[0].code_html = "MUTATED html".into();
    }
    let commits = pr.commits.as_mut().unwrap();
    let want_commits = commits.len();
    commits[0].message = "MUTATED message".into();

    let again = st.pr("482").unwrap();
    let files = again.files.unwrap();
    assert_eq!(files.len(), want_files);
    assert_eq!(files[0].path, orig_path);
    let commits = again.commits.unwrap();
    assert_eq!(commits.len(), want_commits);
    assert_ne!(commits[0].message, "MUTATED message");
}

#[test]
fn update_project_run_config_persists() {
    let dir = tmp();
    let repo = tmp();
    let s1 = open_temp(dir.path());
    let mut p = s1
        .add_project(Project {
            base_branch: "main".into(),
            ..proj("app", repo.path())
        })
        .unwrap();
    p.run_commands = vec![run("dev", "pnpm dev --port $RECUR_PORT --strictPort")];
    p.app_url = "http://127.0.0.1:$RECUR_PORT/health".into();
    p.write_project_file = true;
    s1.update_project_run_config(p).unwrap();
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    let got = s2.project("app").unwrap();
    assert_eq!(
        verify_dev_cmd(&got),
        "pnpm dev --port $RECUR_PORT --strictPort"
    );
    assert_eq!(got.app_url, "http://127.0.0.1:$RECUR_PORT/health");
}

#[test]
fn deleted_project_toml_does_not_resurrect_db() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    st.add_project(Project {
        setup_cmd: "make setup".into(),
        env: "A=1".into(),
        run_commands: vec![run("dev", "make dev")],
        write_project_file: true,
        ..proj("app", repo.path())
    })
    .unwrap();
    fs::remove_file(repo.path().join(projfile::REL_PATH)).unwrap();
    let got = st.project("app").unwrap();
    assert_eq!(got.setup_cmd, "");
    assert_eq!(verify_dev_cmd(&got), "");
    assert_eq!(got.env, "");
    assert!(!got.has_project_file);
}

#[test]
fn add_project_seeds_from_project_toml() {
    let dir = tmp();
    let repo = tmp();
    projfile::write(&Project {
        repo_path: s(repo.path()),
        setup_cmd: "pnpm install".into(),
        run_commands: vec![run("dev", "pnpm dev --port $RECUR_PORT")],
        app_url: "http://localhost:$RECUR_PORT".into(),
        env: "FOO=bar".into(),
        ..Default::default()
    })
    .unwrap();
    let st = open_temp(dir.path());
    let saved = st.add_project(proj("app", repo.path())).unwrap();
    assert_eq!(saved.setup_cmd, "pnpm install");
    assert_eq!(verify_dev_cmd(&saved), "pnpm dev --port $RECUR_PORT");
    assert_eq!(saved.app_url, "http://localhost:$RECUR_PORT");
    assert_eq!(saved.env, "FOO=bar");
}

#[test]
fn add_project_does_not_rewrite_existing_toml_without_opt_in() {
    let dir = tmp();
    let repo = tmp();
    projfile::write(&Project {
        repo_path: s(repo.path()),
        run_commands: vec![run("dev", "from-file")],
        ..Default::default()
    })
    .unwrap();
    let before = read_toml(repo.path());
    let st = open_temp(dir.path());
    let saved = st.add_project(proj("app", repo.path())).unwrap();
    assert!(saved.has_project_file);
    assert_eq!(verify_dev_cmd(&saved), "from-file");
    assert_eq!(read_toml(repo.path()), before);
}

#[test]
fn add_project_returns_overlay_when_file_exists() {
    let dir = tmp();
    let repo = tmp();
    write_toml(
        repo.path(),
        "[[run]]\nname = \"dev\"\ncmd = \"from-file\"\n[[run]]\nname = \"test\"\ncmd = \"go test\"\n",
    );
    let st = open_temp(dir.path());
    let saved = st.add_project(proj("app", repo.path())).unwrap();
    assert!(saved.has_project_file);
    assert_eq!(saved.run_commands.len(), 2);
    assert_eq!(saved.run_commands[1].name, "test");
}

#[test]
fn update_project_run_config_returns_overlay() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    let mut p = st.add_project(proj("app", repo.path())).unwrap();
    write_toml(repo.path(), "[[run]]\nname = \"dev\"\ncmd = \"file-cmd\"\n");
    p.open = false;
    let saved = st.update_project_run_config(p).unwrap();
    assert!(saved.has_project_file);
    assert_eq!(verify_dev_cmd(&saved), "file-cmd");
}

#[test]
fn update_project_run_config_unknown_errors() {
    let st = Store::new_in_memory();
    let err = st.update_project_run_config(Project {
        id: "nope".into(),
        ..Default::default()
    });
    assert_eq!(err.unwrap_err().to_string(), "project \"nope\" not found");
}

#[test]
fn project_toml_overlays_db() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    st.add_project(proj("app", repo.path())).unwrap();
    write_toml(repo.path(), "[[run]]\nname = \"dev\"\ncmd = \"file-cmd\"\n");
    assert_eq!(verify_dev_cmd(&st.project("app").unwrap()), "file-cmd");
}

#[test]
fn update_project_run_config_writes_toml() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    let mut p = st.add_project(proj("app", repo.path())).unwrap();
    p.setup_cmd = "make setup".into();
    p.env = "A=1".into();
    p.write_project_file = true;
    st.update_project_run_config(p.clone()).unwrap();
    let got = projfile::overlay(Project {
        repo_path: s(repo.path()),
        ..Default::default()
    });
    assert_eq!(got.setup_cmd, "make setup");
    assert_eq!(got.env, "A=1");
    p.setup_cmd = "make setup2".into();
    p.write_project_file = true;
    st.update_project_run_config(p).unwrap();
    let got = projfile::overlay(Project {
        repo_path: s(repo.path()),
        ..Default::default()
    });
    assert_eq!(got.setup_cmd, "make setup2");
}

#[test]
fn update_project_run_config_does_not_create_toml_without_opt_in() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    let mut p = st
        .add_project(Project {
            env: "SECRET=1".into(),
            ..proj("app", repo.path())
        })
        .unwrap();
    p.open = false;
    st.update_project_run_config(p.clone()).unwrap();
    assert!(!repo.path().join(projfile::REL_PATH).exists());
    p.setup_cmd = "make setup".into();
    st.update_project_run_config(p).unwrap();
    assert!(!repo.path().join(projfile::REL_PATH).exists());
}

#[test]
fn update_project_open_does_not_create_toml() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    let mut p = st
        .add_project(Project {
            open: true,
            ..proj("app", repo.path())
        })
        .unwrap();
    p.open = false;
    st.update_project_run_config(p).unwrap();
    assert!(!repo.path().join(projfile::REL_PATH).exists());
}

#[test]
fn pinned_run_persists_without_rewriting_toml() {
    let dir = tmp();
    let repo = tmp();
    let hand = "# keep me\n[[run]]\nname = \"dev\"\ncmd = \"pnpm dev\"\n";
    write_toml(repo.path(), hand);
    let st = open_temp(dir.path());
    let mut p = st.add_project(proj("app", repo.path())).unwrap();
    p.pinned_run = "dev".into();
    st.update_project_run_config(p).unwrap();
    assert_eq!(st.project("app").unwrap().pinned_run, "dev");
    assert_eq!(read_toml(repo.path()), hand);
    st.close().unwrap();
    let s2 = open_temp(dir.path());
    assert_eq!(s2.project("app").unwrap().pinned_run, "dev");
}

#[test]
fn skill_and_mcp_overrides_persist() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    let mut p = st.add_project(proj("app", repo.path())).unwrap();
    p.disabled_skills = vec!["noise-skill".into()];
    p.enabled_skills = vec!["special".into()];
    p.disabled_mcp = vec!["linear".into()];
    p.enabled_mcp = vec!["acme".into()];
    st.update_project_run_config(p).unwrap();
    let got = st.project("app").unwrap();
    assert_eq!(got.disabled_skills, ["noise-skill"]);
    assert_eq!(got.enabled_skills, ["special"]);
    assert_eq!(got.disabled_mcp, ["linear"]);
    assert_eq!(got.enabled_mcp, ["acme"]);
    st.close().unwrap();
    let s2 = open_temp(dir.path());
    let again = s2.project("app").unwrap();
    assert_eq!(again.disabled_skills, ["noise-skill"]);
    assert_eq!(again.enabled_mcp, ["acme"]);
}

#[test]
fn named_runs_persist_to_toml_and_pin_stays_local() {
    let dir = tmp();
    let repo = tmp();
    let st = open_temp(dir.path());
    let mut p = st.add_project(proj("app", repo.path())).unwrap();
    p.run_commands = vec![run("dev", "pnpm dev"), run("test", "go test ./...")];
    p.pinned_run = "test".into();
    p.write_project_file = true;
    let saved = st.update_project_run_config(p).unwrap();
    assert_eq!(saved.pinned_run, "test");
    let got = projfile::overlay(Project {
        repo_path: s(repo.path()),
        ..Default::default()
    });
    assert_eq!(got.run_commands.len(), 2);
    assert_eq!(got.run_commands[1].name, "test");
    assert_eq!(got.pinned_run, "");
    let from_db = st.project("app").unwrap();
    assert_eq!(from_db.pinned_run, "test");
    assert_eq!(from_db.run_commands.len(), 2);

    let mut p = from_db;
    p.run_commands = vec![run("dev", "pnpm dev")];
    p.pinned_run = "test".into();
    let cleared = st.update_project_run_config(p).unwrap();
    assert_eq!(cleared.pinned_run, "", "stale pin should drop");
}

#[test]
fn project_toml_does_not_rewrite_hand_file_on_open_toggle() {
    let dir = tmp();
    let repo = tmp();
    let hand = "# keep comments\n[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n";
    write_toml(repo.path(), hand);
    let st = open_temp(dir.path());
    let mut p = st
        .add_project(Project {
            open: true,
            ..proj("app", repo.path())
        })
        .unwrap();
    assert_eq!(verify_dev_cmd(&p), "make dev");
    p.open = false;
    st.update_project_run_config(p).unwrap();
    assert_eq!(read_toml(repo.path()), hand);
}

#[test]
fn update_project_open_does_not_revert_pulled_toml() {
    let dir = tmp();
    let repo = tmp();
    write_toml(
        repo.path(),
        "# keep me\n[[run]]\nname = \"dev\"\ncmd = \"old\"\n",
    );
    let st = open_temp(dir.path());
    let mut p = st
        .add_project(Project {
            open: true,
            ..proj("app", repo.path())
        })
        .unwrap();
    let pulled = "# pulled\n[[run]]\nname = \"dev\"\ncmd = \"new\"\n[[run]]\nname = \"test\"\ncmd = \"go test\"\n";
    write_toml(repo.path(), pulled);
    p.open = false;
    st.update_project_run_config(p).unwrap();
    assert_eq!(read_toml(repo.path()), pulled);
}

#[test]
fn add_project_write_project_file_overwrites_existing() {
    let dir = tmp();
    let repo = tmp();
    write_toml(
        repo.path(),
        "# team\n[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n[[run]]\nname = \"test\"\ncmd = \"go test\"\n",
    );
    let st = open_temp(dir.path());
    st.add_project(Project {
        run_commands: vec![run("dev", "from-form")],
        write_project_file: true,
        ..proj("app", repo.path())
    })
    .unwrap();
    let over = projfile::overlay(Project {
        repo_path: s(repo.path()),
        ..Default::default()
    });
    assert_eq!(over.run_commands, vec![run("dev", "from-form")]);
}

#[test]
fn project_open_and_hidden_persist() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let mut p = s1.project("storefront").unwrap();
    assert!(p.open);
    p.open = false;
    p.hidden = true;
    s1.update_project_run_config(p).unwrap();
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    let got = s2.project("storefront").unwrap();
    assert!(!got.open);
    assert!(got.hidden);
}

#[test]
fn project_groups_crud_persists() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let mut g = s1
        .add_project_group(ProjectGroup {
            id: "work".into(),
            name: "Work".into(),
            open: true,
            ..Default::default()
        })
        .unwrap();
    assert!(g.id == "work" && g.name == "Work" && g.open);
    assert!(!g.created_at.is_empty() && !g.updated_at.is_empty());
    let created_at = g.created_at.clone();
    let mut p = s1.project("storefront").unwrap();
    p.group_id = "work".into();
    s1.update_project_run_config(p).unwrap();
    g.open = false;
    g.name = "Client work".into();
    let updated = s1.update_project_group(g).unwrap();
    assert_eq!(updated.created_at, created_at);
    assert!(!updated.updated_at.is_empty());
    assert!(s1.has_project_group("work"));
    assert_eq!(s1.project_group("work").unwrap().name, "Client work");
    s1.close().unwrap();

    let s2 = open_temp(dir.path());
    let groups = s2.project_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(groups[0].id == "work" && groups[0].name == "Client work" && !groups[0].open);
    assert_eq!(groups[0].created_at, created_at);
    assert!(!groups[0].updated_at.is_empty());
    assert_eq!(s2.project("storefront").unwrap().group_id, "work");
    assert!(
        s2.update_project_group(ProjectGroup {
            id: "nope".into(),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn delete_project_group_ungroups_members() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_project_group(ProjectGroup {
        id: "tmp".into(),
        name: "Tmp".into(),
        open: true,
        ..Default::default()
    })
    .unwrap();
    let mut p = st.project("storefront").unwrap();
    p.group_id = "tmp".into();
    st.update_project_run_config(p).unwrap();
    st.delete_project_group("tmp").unwrap();
    assert!(st.project_groups().is_none());
    assert_eq!(st.project("storefront").unwrap().group_id, "");
    st.delete_project_group("missing").unwrap();
}

#[test]
fn reorder_project_groups_persists() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    for id in ["a", "b", "c"] {
        s1.add_project_group(ProjectGroup {
            id: id.into(),
            name: id.into(),
            open: true,
            ..Default::default()
        })
        .unwrap();
    }
    let order: Vec<String> = ["c", "a", "b"].map(String::from).to_vec();
    s1.reorder_project_groups(&order).unwrap();
    assert!(s1.reorder_project_groups(&order[..1]).is_err());
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    let got: Vec<String> = s2
        .project_groups()
        .unwrap()
        .into_iter()
        .map(|g| g.id)
        .collect();
    assert_eq!(got, order);
}

#[test]
fn set_policy_toggle_persists() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let mut pol = s1.policy();
    pol.autonomy = AutonomyLevel::Aggressive;
    s1.set_policy(pol);
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    assert_eq!(s2.policy().autonomy, AutonomyLevel::Aggressive);
}

#[test]
fn mark_all_inbox_read_persists() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    assert!(s1.unread_count() > 0);
    s1.mark_all_inbox_read();
    assert_eq!(s1.unread_count(), 0);
    s1.close().unwrap();
    assert_eq!(open_temp(dir.path()).unread_count(), 0);
}

#[test]
fn mark_inbox_read_persists_one_row() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let before = s1.unread_count();
    s1.mark_inbox_read("ib1");
    assert_eq!(s1.unread_count(), before - 1);
    s1.close().unwrap();
    assert_eq!(open_temp(dir.path()).unread_count(), before - 1);
}

#[test]
fn answer_escalation_persists() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let (_, ok) = s1.answer_escalation("LOOP-2218", "Token bucket with refill", "");
    assert!(ok);
    let (again, ok) = s1.answer_escalation("LOOP-2218", "other", "");
    assert!(!ok, "second answer is a no-op");
    assert_eq!(again.unwrap().answer, "Token bucket with refill");
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    let e = s2.escalation("LOOP-2218").unwrap();
    assert_eq!(e.status, "answered");
    assert_eq!(e.answer, "Token bucket with refill");
    assert_eq!(e.choice, "Token bucket with refill");
    let (missing, ok) = s2.answer_escalation("nope", "x", "");
    assert!(missing.is_none() && !ok);
}

#[test]
fn mark_escalation_applied_persists() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    s1.mark_escalation_applied("LOOP-2218");
    assert!(
        !s1.escalation("LOOP-2218").unwrap().applied,
        "open rows are not applied"
    );
    s1.answer_escalation("LOOP-2218", "Queue & delay", "  free  ");
    let e = s1.escalation("LOOP-2218").unwrap();
    assert_eq!(e.answer, "free");
    assert_eq!(e.free_text, "free");
    s1.mark_escalation_applied("LOOP-2218");
    s1.close().unwrap();
    assert!(
        open_temp(dir.path())
            .escalation("LOOP-2218")
            .unwrap()
            .applied
    );
}

fn esc(id: &str, ws: &str, agent: &str, status: &str, question: &str) -> Escalation {
    Escalation {
        id: id.into(),
        workspace_id: ws.into(),
        agent_code: agent.into(),
        status: status.into(),
        question: question.into(),
        ..Default::default()
    }
}

#[test]
fn chat_escalations_do_not_clobber() {
    let st = Store::new_in_memory();
    st.add_escalation(esc("esc-a", "W1", "PA", "open", "A"));
    st.add_escalation(esc("esc-b", "W1", "PA2", "open", "B"));
    assert!(st.answer_escalation_for("W1", "PA2", "keep", "").1);
    let (got, ok) = st.answer_escalation_for("W1", "PA", "stop", "");
    assert!(ok, "PA row was overwritten by the sibling answer");
    let got = got.unwrap();
    assert_eq!(got.question, "A");
    assert_eq!(got.answer, "stop");
}

#[test]
fn escalation_prefers_open_chat_row_over_answered_workspace() {
    let st = Store::new_in_memory();
    st.add_escalation(Escalation {
        answer: "done".into(),
        ..esc("esc-old", "W1", "", "answered", "stale")
    });
    st.add_escalation(esc("esc-live", "W1", "PA", "open", "live"));
    assert_eq!(st.escalation("W1").unwrap().question, "live");
    assert!(st.has_open_escalation("W1"));
    let (ans, ok) = st.answer_escalation("W1", "yes", "");
    let ans = ans.unwrap();
    assert!(ok && ans.question == "live" && ans.answer == "yes");
    assert!(!st.has_open_escalation("W1"));
}

#[test]
fn findings_persist_and_resolve() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    for (i, c) in ['a', 'b', 'c'].into_iter().enumerate() {
        s1.add_finding(finding(
            &format!("F-1-{c}"),
            "L1",
            1,
            "main.go",
            i as i64 + 1,
            Severity::High,
            "bug",
            "open",
        ));
    }
    assert_eq!(s1.open_findings_count("L1"), 3);
    s1.resolve_findings("L1");
    assert_eq!(s1.open_findings_count("L1"), 0);
    s1.add_finding(finding(
        "F-2-a",
        "L1",
        2,
        "x.go",
        9,
        Severity::Low,
        "smell",
        "open",
    ));
    assert_eq!(s1.open_findings_count("L1"), 1);
    s1.close().unwrap();

    let s2 = open_temp(dir.path());
    let all = s2.findings("L1");
    assert_eq!(all.len(), 4);
    assert_eq!(s2.open_findings_count("L1"), 1);
    assert_eq!(all.iter().filter(|f| f.status == "resolved").count(), 3);
    assert_eq!(s2.open_findings("L1").unwrap().len(), 1);
    assert_eq!(s2.findings_by_status("L1", "resolved").unwrap().len(), 3);
    assert_eq!(s2.findings_by_status("L1", "all").unwrap().len(), 4);
    assert_eq!(s2.findings_by_status("L1", "").unwrap().len(), 4);
    assert!(s2.findings_by_status("L1", "ignored").is_none());
    assert!(s2.open_findings("nope").is_none());
}

#[test]
fn set_finding_status() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_finding(finding(
        "F-a",
        "L1",
        1,
        "a.go",
        1,
        Severity::High,
        "bug",
        "open",
    ));
    st.add_finding(finding(
        "F-b",
        "L1",
        1,
        "b.go",
        2,
        Severity::Low,
        "nit",
        "open",
    ));
    let got = st.set_finding_status("L1", "F-a", "ignored").unwrap();
    assert_eq!(got.status, "ignored");
    st.set_finding_status("L1", "F-b", "invalid").unwrap();
    assert_eq!(st.open_findings_count("L1"), 0);

    st.add_finding(finding(
        "F-c",
        "L1",
        2,
        "c.go",
        3,
        Severity::Medium,
        "x",
        "open",
    ));
    st.resolve_findings("L1");
    let by: BTreeMap<String, String> = st
        .findings("L1")
        .into_iter()
        .map(|f| (f.id, f.status))
        .collect();
    assert_eq!(by["F-a"], "ignored");
    assert_eq!(by["F-b"], "invalid");
    assert_eq!(by["F-c"], "resolved");

    let reopened = st.set_finding_status("L1", "F-a", "open").unwrap();
    assert_eq!(reopened.status, "open");
    assert!(st.set_finding_status("L1", "F-a", "bogus").is_err());
    assert!(st.set_finding_status("L1", "missing", "ignored").is_err());
}

#[test]
fn update_finding_status_engine_resolves_open_only() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_finding(finding(
        "F-open",
        "L1",
        1,
        "a.go",
        1,
        Severity::High,
        "bug",
        "open",
    ));
    st.add_finding(finding(
        "F-ig",
        "L1",
        1,
        "b.go",
        2,
        Severity::Low,
        "nit",
        "ignored",
    ));
    st.add_finding(finding(
        "F-inv",
        "L1",
        1,
        "c.go",
        3,
        Severity::Medium,
        "x",
        "invalid",
    ));
    let got = st
        .update_finding_status("L1", "F-open", "resolved", "", FINDING_ACTOR_ENGINE)
        .unwrap();
    assert_eq!(got.status, "resolved");
    assert_eq!(got.updated_by, FINDING_ACTOR_ENGINE);
    let got = st
        .update_finding_status("L1", "F-ig", "resolved", "", FINDING_ACTOR_ENGINE)
        .unwrap();
    assert_eq!(got.status, "ignored");
    let got = st
        .update_finding_status("L1", "F-inv", "resolved", "", FINDING_ACTOR_ENGINE)
        .unwrap();
    assert_eq!(got.status, "invalid");
    assert!(
        st.update_finding_status("L1", "F-open", "ignored", "", FINDING_ACTOR_ENGINE)
            .is_err()
    );
}

#[test]
fn update_finding_status_agent_rules() {
    let st = Store::new_in_memory();
    st.add_finding(finding(
        "F-1",
        "L1",
        1,
        "a.go",
        1,
        Severity::High,
        "bug",
        "open",
    ));
    st.add_finding(finding(
        "F-2",
        "L1",
        1,
        "a.go",
        2,
        Severity::High,
        "bug",
        "ignored",
    ));
    assert!(
        st.update_finding_status("L1", "F-1", "open", "", "coder")
            .is_err()
    );
    assert!(
        st.update_finding_status("L1", "F-1", "invalid", "  ", "reviewer")
            .is_err()
    );
    assert!(
        st.update_finding_status("L1", "F-1", "resolved", "", "bot")
            .is_err()
    );
    assert!(
        st.update_finding_status("L1", "F-2", "resolved", "", "coder")
            .is_err()
    );
    let got = st
        .update_finding_status("L1", "F-1", "invalid", "wrong file", "coder")
        .unwrap();
    assert_eq!(got.status_reason, "wrong file");
    assert_eq!(got.updated_by, "coder");
    let same = st
        .update_finding_status("L1", "F-1", "invalid", "different", "reviewer")
        .unwrap();
    assert_eq!(same.status_reason, "different");
    assert_eq!(same.updated_by, "reviewer");
    let human = st
        .update_finding_status("L1", "F-1", "resolved", "", "")
        .unwrap();
    assert_eq!(human.updated_by, FINDING_ACTOR_HUMAN);
    assert_eq!(human.status_reason, "");
}

#[test]
fn add_finding_note() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_finding(Finding {
        end_line: 4,
        summary: "short".into(),
        ..finding("F-n", "L1", 1, "a.go", 1, Severity::Low, "bug", "open")
    });
    let got = st
        .add_finding_note(
            "L1",
            "F-n",
            "  check the timeout path  ",
            FINDING_ACTOR_HUMAN,
        )
        .unwrap();
    assert_eq!(got.notes.len(), 1);
    let n = &got.notes[0];
    assert!(
        !n.id.is_empty()
            && n.body == "check the timeout path"
            && n.author == FINDING_ACTOR_HUMAN
            && !n.created_at.is_empty()
    );
    st.add_finding_note("L1", "F-n", "second", "").unwrap();
    let all = st.findings("L1");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].notes.len(), 2);
    assert!(
        st.add_finding_note("L1", "F-n", "   ", FINDING_ACTOR_HUMAN)
            .is_err()
    );
    assert!(
        st.add_finding_note("L1", "missing", "x", FINDING_ACTOR_HUMAN)
            .is_err()
    );
    assert!(st.add_finding_note("L1", "F-n", "x", "reviewer").is_err());
    st.close().unwrap();

    let s2 = Store::open(&db_file(dir.path())).unwrap();
    let reloaded = s2.findings("L1");
    assert_eq!(reloaded.len(), 1);
    let f = &reloaded[0];
    assert!(
        f.file == "a.go"
            && f.line == 1
            && f.end_line == 4
            && f.message == "bug"
            && f.summary == "short"
    );
    assert_eq!(f.notes.len(), 2);
    assert_eq!(f.notes[0].body, "check the timeout path");
    assert_eq!(f.notes[1].body, "second");
    assert_eq!(
        query_i64(
            &s2,
            "SELECT COUNT(*) FROM pragma_table_info('findings') WHERE name = 'doc_json'"
        ),
        0,
        "findings.doc_json should be gone"
    );
}

#[test]
fn add_finding_assigns_uuid_and_short_id() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let first = st.add_finding(Finding {
        workspace_id: "L1".into(),
        iteration: 1,
        file: "a.go".into(),
        line: 1,
        severity: Severity::High,
        message: "one".into(),
        status: "open".into(),
        ..Default::default()
    });
    let parsed = uuid::Uuid::parse_str(&first.id).unwrap();
    assert_eq!(parsed.get_version_num(), 7);
    assert!(
        valid_id(&first.short_id, &IdType::Finding),
        "{}",
        first.short_id
    );
    assert!(!first.created_at.is_empty());
    let second = st.add_finding(Finding {
        workspace_id: "L1".into(),
        file: "b.go".into(),
        message: "two".into(),
        status: "open".into(),
        ..Default::default()
    });
    assert_ne!(first.short_id, second.short_id);
    let other = st.add_finding(Finding {
        workspace_id: "L2".into(),
        short_id: first.short_id.clone(),
        file: "c.go".into(),
        message: "three".into(),
        status: "open".into(),
        ..Default::default()
    });
    assert_eq!(other.short_id, first.short_id, "unique per workspace");
    st.close().unwrap();

    let s2 = Store::open(&db_file(dir.path())).unwrap();
    let reloaded = s2.findings("L1");
    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded[0].id, first.id);
    assert_eq!(reloaded[0].short_id, first.short_id);
    s2.set_finding_status("L1", &first.short_id, "ignored")
        .unwrap();
    s2.set_finding_status("L1", &second.id, "ignored").unwrap();
}

fn bare_finding(ws: &str, short: &str, file: &str, msg: &str) -> Finding {
    Finding {
        workspace_id: ws.into(),
        short_id: short.into(),
        file: file.into(),
        message: msg.into(),
        status: "open".into(),
        ..Default::default()
    }
}

#[test]
fn add_finding_retries_short_id_collision() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let first = st.add_finding(bare_finding("L1", "find_aaaaaa", "a.go", "one"));
    assert_eq!(first.short_id, "find_aaaaaa");
    let second = st.add_finding(bare_finding("L1", "find_aaaaaa", "b.go", "two"));
    assert_ne!(second.short_id, first.short_id);
    assert!(valid_id(&second.short_id, &IdType::Finding));
}

#[test]
fn add_finding_retries_persisted_short_id_collision() {
    let dir = tmp();
    let st = open_temp(dir.path());
    exec(
        &st,
        "INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES ('id-disk','find_aaaaaa','L1','a.go','one','open',0)",
    )
    .unwrap();
    let got = st.add_finding(bare_finding("L1", "find_aaaaaa", "b.go", "two"));
    assert_ne!(got.short_id, "find_aaaaaa");
    assert!(valid_id(&got.short_id, &IdType::Finding));
    assert!(
        st.findings("L1").iter().any(|f| f.id == got.id),
        "cached after unique retry"
    );
}

#[test]
fn add_finding_does_not_cache_persist_failure() {
    let dir = tmp();
    let st = open_temp(dir.path());
    db(&st).close().unwrap();
    st.add_finding(bare_finding("WS-persist-fail", "", "a.go", "x"));
    assert!(st.findings("WS-persist-fail").is_empty());
}

#[test]
fn is_unique_constraint() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let ins = |id: &str, short: &str, file: Option<&str>, sort: i64| {
        db(&st).with(|c| {
            c.execute(
                "INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES (?,?,?,?,?,?,?)",
                rusqlite::params![id, short, "W", file, "m", "open", sort],
            )
            .map_err(|source| crate::Error::Sql { context: String::new(), source })
        })
    };
    let not_null = ins("x", "find_bbbbbb", None, 0).unwrap_err();
    assert!(!not_null.is_unique_constraint(), "{not_null}");
    ins("a", "find_cccccc", Some("a.go"), 0).unwrap();
    let unique = ins("b", "find_cccccc", Some("b.go"), 1).unwrap_err();
    assert!(unique.is_unique_constraint(), "{unique}");
}

#[test]
fn finding_identity_backfill() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    exec(
        &s1,
        "INSERT INTO findings (id, short_id, workspace_id, file, message, status, sort) VALUES ('F-legacy-123','','WS1','a.go','bug','open',0)",
    )
    .unwrap();
    s1.close().unwrap();
    let s2 = Store::open(&db_file(dir.path())).unwrap();
    let got = s2.findings("WS1");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "F-legacy-123");
    assert!(valid_id(&got[0].short_id, &IdType::Finding));
}

#[test]
fn ensure_finding_created_at_rules() {
    let mut f = Finding {
        id: "F-x-1756378200000000000".into(),
        ..Default::default()
    };
    crate::store::ensure_finding_created_at(&mut f);
    assert_eq!(f.created_at, "2025-08-28T10:50:00Z");
    let mut f = Finding {
        id: "F-x-12".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        ..Default::default()
    };
    crate::store::ensure_finding_created_at(&mut f);
    assert_eq!(f.created_at, "2026-01-01T00:00:00Z");
    let mut f = Finding {
        id: "F-x-12".into(),
        ..Default::default()
    };
    crate::store::ensure_finding_created_at(&mut f);
    assert_eq!(f.created_at, "");
}

#[test]
fn findings_columns_migrate_from_doc_json() {
    let dir = tmp();
    open_temp(dir.path()).close().unwrap();
    raw(
        &db_file(dir.path()),
        &[
            "DROP TABLE finding_notes",
            "DROP TABLE findings",
            "CREATE TABLE findings (
			id TEXT PRIMARY KEY,
			workspace_id TEXT NOT NULL,
			doc_json TEXT NOT NULL,
			sort INTEGER NOT NULL DEFAULT 0
		)",
            r#"INSERT INTO findings (id, workspace_id, doc_json, sort) VALUES (
			'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
			'WS1',
			'{"id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","shortId":"find_abcxyz","workspaceId":"WS1","iteration":2,"file":"x.go","line":9,"endLine":12,"severity":"high","message":"nil","summary":"n","status":"open","notes":[{"id":"note-1","author":"human","body":"look","createdAt":"2026-01-01T00:00:01Z"}]}',
			0
		)"#,
            "DELETE FROM __migrations WHERE version = 18",
        ],
    );
    let s2 = Store::open(&db_file(dir.path())).unwrap();
    let got = s2.findings("WS1");
    assert_eq!(got.len(), 1);
    let f = &got[0];
    assert_eq!(f.id, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    assert_eq!(f.short_id, "find_abcxyz");
    assert!(f.iteration == 2 && f.file == "x.go" && f.line == 9 && f.end_line == 12);
    assert!(
        f.severity.as_str() == "high"
            && f.message == "nil"
            && f.summary == "n"
            && f.status == "open"
    );
    assert_eq!(f.notes.len(), 1);
    assert!(f.notes[0].id == "note-1" && f.notes[0].body == "look");
}

#[test]
fn delete_finding_note() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_finding(finding(
        "F-d",
        "L1",
        1,
        "a.go",
        1,
        Severity::Low,
        "bug",
        "open",
    ));
    let first = st
        .add_finding_note("L1", "F-d", "keep", FINDING_ACTOR_HUMAN)
        .unwrap();
    let second = st
        .add_finding_note("L1", "F-d", "drop", FINDING_ACTOR_HUMAN)
        .unwrap();
    let drop_id = second.notes[1].id.clone();
    let got = st.delete_finding_note("L1", "F-d", &drop_id).unwrap();
    assert_eq!(got.notes.len(), 1);
    assert_eq!(got.notes[0].id, first.notes[0].id);
    assert_eq!(got.notes[0].body, "keep");
    assert!(st.delete_finding_note("L1", "F-d", &drop_id).is_err());
    assert!(st.delete_finding_note("L1", "F-d", "").is_err());
    assert!(st.delete_finding_note("L1", "nope", "x").is_err());
}

/// A brand-new DB gets default policy/settings and a zeroed status bar but
/// no demo data.
#[test]
fn fresh_db_is_empty_by_default() {
    let dir = tmp();
    let st = Store::open(&db_file(dir.path())).unwrap();
    assert!(st.projects().is_none());
    assert!(st.workspaces().is_empty());
    assert!(st.policy().rails.unwrap_or_default().is_empty());
    assert!(!st.settings().model_agent.is_empty());
    assert!(!st.settings().bypass_permissions);
    let r = st.resource();
    assert!(r.loops_running == 0 && r.iters_today == 0 && r.needs_you == 0);
}

#[test]
fn github_branch_require_pr_persists() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let id = all_projects(&st)[0].id.clone();
    st.set_github_branch_require_pr(&id, "acme/app", "main", true);
    st.set_github_branch_require_pr(&id, "acme/app", "develop", false);
    st.set_github_branch_require_pr(&id, "acme/parent", "main", false);
    st.set_github_branch_require_pr(&id, " ", "main", true);
    let (got, at) = st
        .github_branch_require_pr(&id, "acme/app", "main")
        .unwrap();
    assert!(got && at.timestamp() > 0);
    st.close().unwrap();

    let s2 = open_temp(dir.path());
    let (got, at) = s2
        .github_branch_require_pr(&id, "acme/app", "main")
        .unwrap();
    assert!(got && at.timestamp() > 0);
    assert_eq!(
        s2.github_branch_require_pr(&id, "acme/app", "develop")
            .map(|r| r.0),
        Some(false)
    );
    assert_eq!(
        s2.github_branch_require_pr(&id, "acme/parent", "main")
            .map(|r| r.0),
        Some(false)
    );
    assert!(s2.github_branch_require_pr(&id, "", "main").is_none());
}

#[test]
fn clear_all_wipes_data_keeps_policy() {
    let dir = tmp();
    let st = open_temp(dir.path());
    assert!(!all_projects(&st).is_empty());
    st.clear_all().unwrap();
    assert!(st.projects().is_none());
    assert!(st.workspaces().is_empty());
    assert!(st.prs().is_empty());
    assert!(st.inbox().is_none());
    assert_eq!(st.policy().autonomy, AutonomyLevel::Balanced);
    assert_eq!(st.resource(), Default::default());
    st.close().unwrap();
    let s2 = open_temp(dir.path());
    assert!(
        s2.projects().is_none(),
        "demo came back after clear_all+reopen"
    );
    assert_eq!(s2.policy().autonomy, AutonomyLevel::Balanced);
}

#[test]
fn delete_project_cascades() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let deleted = st.delete_project("storefront").unwrap();
    assert!(!deleted.unwrap().is_empty());
    assert!(st.project("storefront").is_none());
    assert!(st.project("payments").is_some());
    assert!(st.workspace("LOOP-2231").is_none());
    assert!(st.workspace("LOOP-2218").is_some());
    assert!(st.pr("482").is_none());
    assert!(st.delete_project("missing").unwrap().is_none());
    st.close().unwrap();
    let s2 = open_temp(dir.path());
    assert!(s2.project("storefront").is_none());
    assert!(s2.workspace("LOOP-2218").is_some());
}

#[test]
fn delete_workspace_cascades() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.delete_workspace("LOOP-2231").unwrap();
    assert!(st.workspace("LOOP-2231").is_none());
    assert!(st.pr("482").is_none());
    assert!(st.project("storefront").is_some());
    assert!(st.workspace("LOOP-2218").is_some());
    assert!(st.agents("LOOP-2231").is_empty());
    assert!(st.feed("LOOP-2231").is_none());
    for it in st.inbox().unwrap() {
        assert!(
            it.target != "loop:LOOP-2231" && it.target != "pr:482",
            "{}",
            it.id
        );
    }
    st.close().unwrap();
    let s2 = open_temp(dir.path());
    assert!(s2.workspace("LOOP-2231").is_none());
    assert!(s2.pr("482").is_none());
    assert!(s2.workspace("LOOP-2218").is_some());
    assert!(s2.feed("LOOP-2231").is_none());
}

#[test]
fn delete_workspace_unknown_errors() {
    let dir = tmp();
    let st = open_temp(dir.path());
    assert!(st.delete_workspace("no-such-loop").is_err());
}

/// Prepends persist incrementally (one INSERT); newest-first order must
/// survive reloads, including after a patch renumbers the rows.
#[test]
fn feed_prepend_order_survives_reload() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    const WS: &str = "LOOP-FEED-ORDER";
    assert!(s1.feed(WS).is_none());
    for i in 0..25 {
        s1.prepend_feed(
            WS,
            FeedItem {
                id: format!("item-{i:02}"),
                agent_code: "CO".into(),
                name: "Coder".into(),
                text: format!("step {i}"),
                tool_use_id: format!("tu-{i:02}"),
                ..Default::default()
            },
        );
    }
    let mut want: Vec<String> = (0..25).rev().map(|i| format!("item-{i:02}")).collect();
    let feed_ids =
        |st: &Store| -> Vec<String> { st.feed(WS).unwrap().into_iter().map(|f| f.id).collect() };
    assert_eq!(feed_ids(&s1), want, "in memory");
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    assert_eq!(feed_ids(&s2), want, "after reload");

    let got = s2.patch_feed_by_tool_use_id(WS, "tu-10", |it| it.text = "patched".into());
    assert_eq!(got.unwrap().text, "patched");
    assert!(s2.patch_feed_by_tool_use_id(WS, "", |_| {}).is_none());
    assert!(s2.patch_feed_by_tool_use_id(WS, "nope", |_| {}).is_none());
    s2.prepend_feed(
        WS,
        FeedItem {
            id: "item-25".into(),
            agent_code: "CO".into(),
            name: "Coder".into(),
            ..Default::default()
        },
    );
    want.insert(0, "item-25".into());
    assert_eq!(feed_ids(&s2), want, "after patch + prepend");
    s2.close().unwrap();
    let s3 = open_temp(dir.path());
    assert_eq!(feed_ids(&s3), want, "after patch + prepend, reloaded");
    let patched = s3
        .feed(WS)
        .unwrap()
        .into_iter()
        .find(|f| f.id == "item-10")
        .unwrap();
    assert_eq!(patched.text, "patched");
}

#[test]
fn upsert_feed_replaces_or_prepends() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let item = |id: &str, text: &str| FeedItem {
        id: id.into(),
        text: text.into(),
        ..Default::default()
    };
    s1.upsert_feed("W", item("card", "one"));
    s1.upsert_feed("W", item("other", "x"));
    s1.upsert_feed("W", item("card", "two"));
    let texts =
        |st: &Store| -> Vec<String> { st.feed("W").unwrap().into_iter().map(|f| f.text).collect() };
    assert_eq!(texts(&s1), ["x", "two"]);
    s1.close().unwrap();
    assert_eq!(texts(&open_temp(dir.path())), ["x", "two"]);
}

#[test]
fn inbox_targets_workspace_includes_chat() {
    assert!(inbox_targets_workspace("chat:demo.ws:PA2", "demo.ws"));
    assert!(!inbox_targets_workspace("chat:other:PA", "demo.ws"));
    assert!(inbox_targets_workspace("workspace:demo.ws", "demo.ws"));
    assert!(inbox_targets_workspace("loop:demo.ws", "demo.ws"));
    assert!(!inbox_targets_workspace("chat:demo.ws", "demo.ws"));

    let st = Store::new_in_memory();
    st.add_inbox_item(InboxItem {
        id: "ib-chat-demo.ws-PA2".into(),
        kind: "chat".into(),
        target: "chat:demo.ws:PA2".into(),
        unread: true,
        ..Default::default()
    });
    st.add_inbox_item(InboxItem {
        id: "ib-keep".into(),
        target: "workspace:other".into(),
        unread: true,
        ..Default::default()
    });
    assert_eq!(st.clear_inbox_for_workspace("demo.ws"), 1);
    let ids: Vec<String> = st.inbox().unwrap().into_iter().map(|i| i.id).collect();
    assert!(!ids.contains(&"ib-chat-demo.ws-PA2".to_owned()));
    assert!(ids.contains(&"ib-keep".to_owned()));
}

#[test]
fn remove_chat_inbox_items() {
    let st = Store::new_in_memory();
    st.add_inbox_item(InboxItem {
        id: "ib-chat-ws-PA".into(),
        kind: "chat".into(),
        unread: true,
        ..Default::default()
    });
    st.add_inbox_item(InboxItem {
        id: "ib-keep".into(),
        kind: "pr".into(),
        unread: true,
        ..Default::default()
    });
    assert_eq!(st.remove_chat_inbox_items(), 1);
    let ids: Vec<String> = st.inbox().unwrap().into_iter().map(|i| i.id).collect();
    assert!(!ids.contains(&"ib-chat-ws-PA".to_owned()) && ids.contains(&"ib-keep".to_owned()));
    assert_eq!(st.remove_chat_inbox_items(), 0);
}

#[test]
fn inbox_add_remove_persist() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    s1.add_inbox_item(InboxItem {
        id: "ib-new".into(),
        target: "pr:9".into(),
        ..Default::default()
    });
    s1.add_inbox_item(InboxItem {
        id: "ib1".into(),
        title: "replaced".into(),
        ..Default::default()
    });
    assert_eq!(s1.inbox().unwrap()[0].id, "ib-new");
    assert!(s1.remove_inbox_item("ib2"));
    assert!(!s1.remove_inbox_item("ib2"));
    assert_eq!(s1.remove_inbox_by_target("view:pr"), 2);
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    let inbox = s2.inbox().unwrap();
    let ids: Vec<&str> = inbox.iter().map(|i| i.id.as_str()).collect();
    // As in Go, the upsert only rewrites doc_json, so existing rows keep
    // their seeded sort: the new head ties with ib1 at 0 and loads after it.
    assert_eq!(ids, ["ib1", "ib-new", "ib3", "ib5"]);
    assert_eq!(inbox[0].title, "replaced");
}

#[test]
fn add_pr_stamps_and_preserves_created_at() {
    let st = Store::new_in_memory();
    let first = st
        .add_pr(Pr {
            title: "one".into(),
            ..pr("p1", "", 1, Default::default())
        })
        .unwrap();
    assert!(!first.created_at.is_empty() && !first.updated_at.is_empty());
    let second = st
        .add_pr(Pr {
            title: "rewritten".into(),
            ..pr("p1", "", 1, Default::default())
        })
        .unwrap();
    assert_eq!(second.created_at, first.created_at);
    assert_eq!(second.title, "rewritten");
    assert!(!second.updated_at.is_empty());
    assert!(st.update_pr("p1", |p| p.created_at.clear()).is_some());
    let third = st
        .add_pr(Pr {
            title: "backfill".into(),
            ..pr("p1", "", 1, Default::default())
        })
        .unwrap();
    assert!(!third.created_at.is_empty());
}

#[test]
fn update_pr_does_not_infer_activity_from_writes() {
    let st = Store::new_in_memory();
    const FROZEN: &str = "2026-08-01T12:00:00Z";
    st.add_pr(Pr {
        title: "one".into(),
        created_at: FROZEN.into(),
        updated_at: FROZEN.into(),
        ..pr("p1", "", 0, PrSource::Local)
    })
    .unwrap();
    let got = st
        .update_pr("p1", |p| {
            p.files = Some(vec![FileDiff {
                path: "a.go".into(),
                ..Default::default()
            }])
        })
        .unwrap();
    assert_eq!(got.updated_at, FROZEN);
    let got = st
        .update_pr("p1", |p| p.updated_at = "2026-08-20T12:00:00Z".into())
        .unwrap();
    assert_eq!(got.updated_at, "2026-08-20T12:00:00Z");

    let gh = st
        .add_pr(Pr {
            title: "gh".into(),
            created_at: FROZEN.into(),
            updated_at: FROZEN.into(),
            ..pr("gh1", "", 0, PrSource::GitHub)
        })
        .unwrap();
    st.update_pr(&gh.id, |p| p.updated_at.clear()).unwrap();
    let got = st.update_pr(&gh.id, |p| p.behind_base = 3).unwrap();
    assert_eq!(got.updated_at, "", "empty GitHub UpdatedAt stays empty");
    let got = st
        .update_pr(&gh.id, |p| p.updated_at = "2026-08-27T09:00:00Z".into())
        .unwrap();
    assert_eq!(got.updated_at, "2026-08-27T09:00:00Z");
    assert!(st.update_pr("missing", |_| {}).is_none());
}

#[test]
fn fill_missing_pr_times_rules() {
    const KNOWN: &str = "2026-01-15T00:00:00Z";
    let mut both = Pr::default();
    assert!(!fill_missing_pr_times(&mut both));
    assert!(both.created_at.is_empty() && both.updated_at.is_empty());
    let mut c = Pr {
        created_at: KNOWN.into(),
        ..Default::default()
    };
    assert!(fill_missing_pr_times(&mut c));
    assert_eq!(c.updated_at, KNOWN);
    let mut u = Pr {
        updated_at: KNOWN.into(),
        ..Default::default()
    };
    assert!(fill_missing_pr_times(&mut u));
    assert_eq!(u.created_at, KNOWN);
    let mut full = Pr {
        created_at: KNOWN.into(),
        updated_at: "2026-02-01T00:00:00Z".into(),
        ..Default::default()
    };
    assert!(!fill_missing_pr_times(&mut full));
}

#[test]
fn load_backfills_missing_pr_times() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    s1.add_pr(Pr {
        title: "old local".into(),
        ..pr("legacy-local", "", 9, PrSource::Local)
    })
    .unwrap();
    s1.update_pr("legacy-local", |p| {
        p.created_at.clear();
        p.updated_at.clear();
    })
    .unwrap();
    s1.add_pr(Pr {
        title: "has created".into(),
        created_at: "2026-01-15T00:00:00Z".into(),
        ..pr("legacy-created", "", 10, PrSource::Local)
    })
    .unwrap();
    s1.update_pr("legacy-created", |p| p.updated_at.clear())
        .unwrap();
    s1.close().unwrap();

    let s2 = open_temp(dir.path());
    let empty = s2.pr("legacy-local").unwrap();
    assert!(empty.created_at.is_empty() && empty.updated_at.is_empty());
    let c = s2.pr("legacy-created").unwrap();
    assert_eq!(c.created_at, "2026-01-15T00:00:00Z");
    assert_eq!(c.updated_at, "2026-01-15T00:00:00Z");
    s2.close().unwrap();

    let s3 = open_temp(dir.path());
    let again = s3.pr("legacy-created").unwrap();
    assert_eq!(
        again.updated_at, "2026-01-15T00:00:00Z",
        "backfill persisted"
    );
    let still = s3.pr("legacy-local").unwrap();
    assert!(still.created_at.is_empty() && still.updated_at.is_empty());
}

#[test]
fn backfill_pr_times_write_error() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_pr(Pr {
        title: "one".into(),
        created_at: "2026-01-15T00:00:00Z".into(),
        ..pr("p1", "", 1, PrSource::Local)
    })
    .unwrap();
    st.update_pr("p1", |p| p.updated_at.clear()).unwrap();
    db(&st).close().unwrap();
    let mut state = std::mem::take(&mut *st.state.write());
    assert!(db(&st).backfill_pr_times(&mut state).is_err());
}

#[test]
fn open_pr_by_workspace_prefers_github_over_local() {
    let st = Store::new_in_memory();
    let open = |id: &str, n: i64, src: PrSource| Pr {
        state: "In review".into(),
        group: "you".into(),
        ..pr(id, "ws-dup", n, src)
    };
    let local = st.add_pr(open("local-4", 4, PrSource::Local)).unwrap();
    let gh = st.add_pr(open("gh-156", 156, PrSource::GitHub)).unwrap();
    assert_eq!(st.open_pr_by_workspace("ws-dup").unwrap().id, gh.id);
    assert_eq!(st.pr_by_workspace("ws-dup").unwrap().id, gh.id);
    st.update_pr(&gh.id, |p| {
        p.state = "Merged".into();
        p.group = "merged".into();
    })
    .unwrap();
    assert_eq!(st.open_pr_by_workspace("ws-dup").unwrap().id, local.id);
    assert_eq!(st.pr_by_workspace("ws-dup").unwrap().id, gh.id);
    assert!(st.open_pr_by_workspace("none").is_none());
    assert!(st.pr_by_workspace("none").is_none());
}

#[test]
fn pr_by_workspace_prefers_open_over_closed() {
    let st = Store::new_in_memory();
    let open = st
        .add_pr(Pr {
            state: "Open".into(),
            group: "open".into(),
            ..pr("gh-13", "ws-r", 13, PrSource::GitHub)
        })
        .unwrap();
    st.add_pr(Pr {
        state: "Closed".into(),
        group: "merged".into(),
        ..pr("gh-9", "ws-r", 9, PrSource::GitHub)
    })
    .unwrap();
    assert_eq!(st.pr_by_workspace("ws-r").unwrap().id, open.id);
    st.update_pr(&open.id, |p| {
        p.state = "Merged".into();
        p.group = "merged".into();
    });
    assert_eq!(st.pr_by_workspace("ws-r").unwrap().id, "gh-9");
}

#[test]
fn pr_by_workspace_keeps_local_newer_than_closed_github() {
    let st = Store::new_in_memory();
    st.add_pr(Pr {
        state: "Closed".into(),
        group: "merged".into(),
        ..pr("gh-9", "ws-l", 9, PrSource::GitHub)
    })
    .unwrap();
    let local = st
        .add_pr(Pr {
            state: "In review".into(),
            group: "you".into(),
            ..pr("10", "ws-l", 10, PrSource::Local)
        })
        .unwrap();
    assert_eq!(st.pr_by_workspace("ws-l").unwrap().id, local.id);
}

#[test]
fn delete_pr() {
    let st = Store::new_in_memory();
    let p = st
        .add_pr(Pr {
            state: "In review".into(),
            ..pr("gone", "ws-x", 9, PrSource::Local)
        })
        .unwrap();
    assert!(st.delete_pr(&p.id));
    assert!(st.pr(&p.id).is_none());
    assert!(!st.delete_pr(&p.id));
}

#[test]
fn prs_list_rows_drop_commits_and_files() {
    let st = Store::new_in_memory();
    let row = st.prs().into_iter().find(|p| p.id == "482").unwrap();
    assert!(row.commits.is_none() && row.files.is_none());
    assert_eq!(row.comments.unwrap().len(), 6);
    let full = st.pr("482").unwrap();
    assert_eq!(full.files.unwrap().len(), 3);
}

/// The highest Local seed PR is #479; GitHub #482 must not set the mark.
const SEEDED_PR_HIGH_WATER: i64 = 479;

#[test]
fn next_pr_number_does_not_reclaim_after_workspace_delete() {
    let st = Store::new_in_memory();
    st.delete_workspace("LOOP-2224").unwrap();
    assert!(st.next_pr_number() > SEEDED_PR_HIGH_WATER);
}

#[test]
fn next_pr_number_persists_after_workspace_delete() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.delete_workspace("LOOP-2224").unwrap();
    st.close().unwrap();
    assert!(open_temp(dir.path()).next_pr_number() > SEEDED_PR_HIGH_WATER);
}

#[test]
fn pr_seq_migration_backfills_from_existing_prs() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_pr(pr("gh-9000", "LOOP-2224", 9000, PrSource::GitHub))
        .unwrap();
    // A promoted Local PR keeps the issued Local id; source/number become GitHub.
    st.add_pr(pr("500", "LOOP-2224", 9001, PrSource::GitHub))
        .unwrap();
    st.close().unwrap();
    raw(
        &db_file(dir.path()),
        &[
            "DROP TABLE pr_seq",
            "DELETE FROM __migrations WHERE version = 12",
        ],
    );
    let s2 = open_temp(dir.path());
    assert_eq!(
        query_i64(&s2, "SELECT last_number FROM pr_seq WHERE id = 1"),
        500
    );
}

#[test]
fn next_pr_number_allocations_are_monotonic() {
    let st = Store::new_in_memory();
    let n1 = st.next_pr_number();
    st.add_pr(pr(&n1.to_string(), "LOOP-2224", n1, PrSource::Local))
        .unwrap();
    assert!(st.delete_pr(&n1.to_string()));
    assert!(st.next_pr_number() > n1);
}

#[test]
fn next_pr_number_starts_at_one_on_empty_store() {
    let dir = tmp();
    let st = Store::open(&db_file(dir.path())).unwrap();
    assert_eq!(st.next_pr_number(), 1);
}

#[test]
fn clear_all_does_not_reclaim_pr_numbers() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.clear_all().unwrap();
    let got = st.next_pr_number();
    assert!(got > SEEDED_PR_HIGH_WATER);
    st.close().unwrap();
    assert!(open_temp(dir.path()).next_pr_number() > got);
}

#[test]
fn github_pr_number_does_not_ratchet_local_seq() {
    let st = Store::new_in_memory();
    let before = st.next_pr_number();
    st.add_pr(pr("gh-9000", "LOOP-2224", 9000, PrSource::GitHub))
        .unwrap();
    assert_eq!(st.next_pr_number(), before + 1);
}

#[test]
fn promote_to_github_does_not_ratchet_local_seq() {
    let st = Store::new_in_memory();
    let n = st.next_pr_number();
    st.add_pr(pr(&n.to_string(), "LOOP-2224", n, PrSource::Local))
        .unwrap();
    st.update_pr(&n.to_string(), |p| {
        p.source = PrSource::GitHub;
        p.number = 9000;
    })
    .unwrap();
    assert_eq!(st.next_pr_number(), n + 1);
}

fn workspace_in_file(path: &Path, id: &str) -> bool {
    Store::open(path).unwrap().workspace(id).is_some()
}

#[test]
fn checkpoint_flushes_wal_into_main_file() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let path = db_file(dir.path());
    db(&st)
        .with(|c| {
            c.query_row("PRAGMA wal_autocheckpoint=0", [], |_| Ok(()))
                .unwrap();
            Ok(())
        })
        .unwrap();
    st.add_workspace(Workspace {
        id: "wal-probe".into(),
        project_id: "storefront".into(),
        goal: "checkpoint probe".into(),
        ..Default::default()
    })
    .unwrap();
    let stale = dir.path().join("stale.db");
    fs::copy(&path, &stale).unwrap();
    assert!(
        !workspace_in_file(&stale, "wal-probe"),
        "main-file copy already has wal-probe"
    );
    let _ = fs::remove_file(&stale);
    let _ = fs::remove_file(dir.path().join("stale.db-wal"));
    let _ = fs::remove_file(dir.path().join("stale.db-shm"));
    db(&st).checkpoint();
    fs::copy(&path, &stale).unwrap();
    assert!(
        workspace_in_file(&stale, "wal-probe"),
        "checkpoint did not flush wal-probe"
    );
}

#[test]
fn workspace_update_and_count_group() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    assert_eq!(s1.count_group(&recur_domain::Group::Active), 3);
    let got = s1
        .update_workspace("LOOP-2229", |w| w.phase = "iteration 3".into())
        .unwrap();
    assert_eq!(got.phase, "iteration 3");
    assert!(s1.update_workspace("missing", |_| {}).is_none());
    s1.add_workspace(Workspace {
        id: "W-new".into(),
        agent_codes: Some(vec![]),
        ..Default::default()
    })
    .unwrap();
    s1.add_workspace(Workspace {
        id: "W-new".into(),
        goal: "updated".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(s1.workspaces().len(), 6);
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    assert_eq!(s2.workspace("LOOP-2229").unwrap().phase, "iteration 3");
    assert_eq!(s2.workspace("W-new").unwrap().goal, "updated");
    assert_eq!(s2.workspaces().last().unwrap().id, "W-new");
}

fn agent(id: &str, ws: &str, code: &str) -> Agent {
    Agent {
        id: id.into(),
        workspace_id: ws.into(),
        code: code.into(),
        ..Default::default()
    }
}

#[test]
fn agents_upsert_update_delete_persist() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    s1.upsert_agent("W", agent("W-PA", "W", "PA"));
    s1.upsert_agent("W", agent("W-CO", "W", "CO"));
    s1.upsert_agent(
        "W",
        Agent {
            name: "renamed".into(),
            ..agent("W-PA", "W", "PA")
        },
    );
    s1.update_agent("W", "CO", |a| a.unseen = true);
    s1.update_agent("W", "missing", |a| a.unseen = true);
    assert_eq!(s1.unseen_chat_count(), 1);
    assert_eq!(s1.agent("W", "W-CO").unwrap().code, "CO", "match by id too");
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    let list = s2.agents("W");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "renamed");
    assert!(list[1].unseen);
    s2.delete_agent("W", "PA");
    s2.delete_agent("W", "nope");
    s2.close().unwrap();
    assert_eq!(open_temp(dir.path()).agents("W").len(), 1);
}

#[test]
fn move_workspace_agents_merges_placeholder() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.upsert_agent(
        "W-host",
        Agent {
            session_id: "sess-host".into(),
            unseen: true,
            model: "sonnet".into(),
            status: "idle".into(),
            name: "Agent".into(),
            ..agent("h-PA", "W-host", "PA")
        },
    );
    st.upsert_agent(
        "W-live",
        Agent {
            status: "idle".into(),
            name: "Agent".into(),
            ..agent("l-PA", "W-live", "PA")
        },
    );
    st.move_workspace_agents("W-host", "W-live", &BTreeMap::new());
    let got = st.agent("W-live", "PA").unwrap();
    assert_eq!(got.session_id, "sess-host");
    assert_eq!(got.id, "l-PA");
    assert_eq!(got.workspace_id, "W-live");
    assert!(got.unseen);
    assert!(st.agent("W-host", "PA").is_none());
}

#[test]
fn move_workspace_agents_remaps_colliding_code() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.upsert_agent(
        "W-host",
        Agent {
            session_id: "sess-host".into(),
            ..agent("h-PA", "W-host", "PA")
        },
    );
    st.upsert_agent(
        "W-live",
        Agent {
            session_id: "sess-live".into(),
            ..agent("l-PA", "W-live", "PA")
        },
    );
    st.move_workspace_agents(
        "W-host",
        "W-live",
        &BTreeMap::from([("PA".to_owned(), "PA2".to_owned())]),
    );
    assert_eq!(st.agent("W-live", "PA").unwrap().session_id, "sess-live");
    let host = st.agent("W-live", "PA2").unwrap();
    assert_eq!(host.session_id, "sess-host");
    assert_eq!(host.id, "W-live-PA2");
    assert_eq!(host.workspace_id, "W-live");
    st.close().unwrap();
    let again = open_temp(dir.path());
    assert!(again.agents("W-host").is_empty());
    assert_eq!(again.agents("W-live").len(), 2);
}

#[test]
fn reassign_workspace_findings() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.add_finding(finding(
        "F-host",
        "W-host",
        0,
        "a.go",
        1,
        Severity::High,
        "bug",
        "open",
    ));
    st.add_finding(finding(
        "F-live",
        "W-live",
        0,
        "b.go",
        2,
        Severity::Low,
        "nit",
        "open",
    ));
    assert_eq!(st.reassign_workspace_findings("W-host", "W-live"), 1);
    let got = st.findings("W-live");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].id, "F-live");
    assert_eq!(got[1].id, "F-host");
    assert_eq!(got[1].workspace_id, "W-live");
    assert_eq!(st.open_findings_count("W-host"), 0);
    assert_eq!(st.reassign_workspace_findings("W-host", "W-live"), 0);
    st.close().unwrap();
    assert_eq!(open_temp(dir.path()).findings("W-live").len(), 2);
}

#[test]
fn pin_claude_default_models_migration_rewrites_legacy_aliases() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let mut set = st.settings();
    set.model_ask = "claude-code/sonnet".into();
    set.model_agent = "codex/gpt-5".into();
    set.model_workspace_title = "claude-code/haiku".into();
    st.set_settings(set);
    st.close().unwrap();
    raw(
        &db_file(dir.path()),
        &["DELETE FROM __migrations WHERE version = 17"],
    );
    let got = open_temp(dir.path()).settings();
    assert_eq!(got.model_ask, "claude-code/claude-sonnet-5-5");
    assert_eq!(got.model_agent, "codex/gpt-5");
    assert_eq!(
        got.model_workspace_title,
        "claude-code/claude-haiku-4-5-20251001"
    );
}

#[test]
fn analysis_newest_first_and_fallback() {
    let dir = tmp();
    let s1 = open_temp(dir.path());
    let run = |id: &str, ws: &str, ts: i64| recur_domain::AnalysisRun {
        id: id.into(),
        project_id: "storefront".into(),
        workspace_id: ws.into(),
        ts,
        ..Default::default()
    };
    s1.add_analysis(run("a1", "", 1));
    s1.add_analysis(run("a2", "W1", 2));
    assert_eq!(s1.latest_analysis("storefront", "").unwrap().id, "a2");
    assert_eq!(s1.latest_analysis("storefront", "W1").unwrap().id, "a2");
    assert_eq!(s1.latest_analysis("storefront", "W9").unwrap().id, "a1");
    assert!(s1.latest_analysis("payments", "").is_none());
    for i in 0..55 {
        s1.add_analysis(run(&format!("x{i}"), "", 10 + i));
    }
    s1.close().unwrap();
    let s2 = open_temp(dir.path());
    assert_eq!(s2.latest_analysis("storefront", "").unwrap().id, "x54");
}

#[test]
fn events_append_and_read_back() {
    let dir = tmp();
    let st = open_temp(dir.path());
    let ev = |t: &str, ts: i64| recur_domain::StoredEvent {
        workspace_id: "W".into(),
        agent_id: "W-CO".into(),
        r#type: t.into(),
        payload_json: "{}".into(),
        ts,
        ..Default::default()
    };
    let a = st.append_event(&ev("workspace.event", 1)).unwrap();
    let b = st.append_event(&ev("workspace.updated", 2)).unwrap();
    assert!(b > a);
    let got = st.events_for_loop("W").unwrap().unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].id, a);
    assert_eq!(got[1].r#type, "workspace.updated");
    assert_eq!(got[0].agent_id, "W-CO");
    assert!(st.events_for_loop("none").unwrap().is_none());
    assert_eq!(Store::new_in_memory().append_event(&ev("x", 1)).unwrap(), 0);
}

#[test]
fn attention_counts_roll_up() {
    let st = Store::new_in_memory();
    st.upsert_agent(
        "LOOP-2231",
        Agent {
            unseen: true,
            ..agent("x", "LOOP-2231", "PA")
        },
    );
    st.add_workspace(Workspace {
        id: "W-arch".into(),
        project_id: "storefront".into(),
        archived: true,
        ..Default::default()
    })
    .unwrap();
    st.upsert_agent(
        "W-arch",
        Agent {
            unseen: true,
            ..agent("y", "W-arch", "PA")
        },
    );
    st.add_inbox_item(InboxItem {
        id: "ib-chat-1".into(),
        kind: "chat".into(),
        unread: true,
        ..Default::default()
    });
    let c = st.attention_counts();
    assert_eq!(c.unseen, 1);
    assert_eq!(c.inbox_unread, 5);
    assert_eq!(c.dock, 1 + 4);
    assert_eq!(c.by_workspace.unwrap()["LOOP-2231"], 1);
    assert_eq!(c.by_project.unwrap()["storefront"], 1);
    assert_eq!(c.app_unseen, 0);
}

#[test]
fn schema_guard_refuses_newer_database() {
    let dir = tmp();
    open_temp(dir.path()).close().unwrap();
    raw(
        &db_file(dir.path()),
        &[
            "INSERT INTO __migrations (version, name, applied_at) VALUES (9999, 'from_the_future', 0)",
        ],
    );
    let err = Store::open(&db_file(dir.path())).err().unwrap();
    assert!(
        matches!(err, crate::Error::NewerSchema { ref versions } if versions == &[9999]),
        "{err}"
    );
    assert!(err.to_string().contains("9999"), "{err}");
}

#[test]
fn closed_store_reports_closed() {
    let dir = tmp();
    let st = open_temp(dir.path());
    st.close().unwrap();
    st.close().unwrap();
    let err = st
        .add_project(Project {
            id: "x".into(),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(err.to_string(), "sql: database is closed");
    assert!(st.get_session("x").is_none());
}

#[test]
fn in_memory_store_has_demo_and_skips_db() {
    let st = Store::new_in_memory();
    assert_eq!(st.projects().unwrap().len(), 3);
    assert_eq!(st.resource().iters_today, 37);
    assert!(st.close().is_ok());
    assert!(st.create_session(Default::default()).is_err());
    assert!(st.session_transcript("x").unwrap().is_none());
    assert!(st.scheduled_prompts().unwrap().is_empty());
    assert!(st.save_scheduled_prompt(&Default::default()).is_err());
    let _ = FindingStatus::Open;
}
