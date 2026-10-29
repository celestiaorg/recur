use std::path::Path;

use crate::paths::*;

fn run_isolated(name: &str, envs: &[(&str, &Path)], body: impl FnOnce()) {
    crate::testenv::run(module_path!(), name, envs, body);
}

#[test]
fn worktrees_dir_uses_data_dir_when_overridden() {
    let data = tempfile::tempdir().unwrap();
    run_isolated(
        "worktrees_dir_uses_data_dir_when_overridden",
        &[
            (DATA_DIR_ENV, data.path()),
            (WORKTREES_DIR_ENV, Path::new("")),
        ],
        || {
            let data = std::env::var(DATA_DIR_ENV).unwrap();
            assert_eq!(worktrees_dir().unwrap(), Path::new(&data).join("worktrees"));
        },
    );
}

#[test]
fn worktrees_dir_explicit_override() {
    let wt = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    run_isolated(
        "worktrees_dir_explicit_override",
        &[(WORKTREES_DIR_ENV, wt.path()), (DATA_DIR_ENV, data.path())],
        || {
            let wt = std::env::var(WORKTREES_DIR_ENV).unwrap();
            assert_eq!(worktrees_dir().unwrap(), Path::new(&wt));
        },
    );
}

/// Without overrides the Rust app uses its own roots, side by side with Go.
#[test]
fn defaults_are_side_by_side_roots() {
    let home = tempfile::tempdir().unwrap();
    run_isolated(
        "defaults_are_side_by_side_roots",
        &[
            ("HOME", home.path()),
            (DATA_DIR_ENV, Path::new("")),
            (WORKTREES_DIR_ENV, Path::new("")),
            ("XDG_CONFIG_HOME", Path::new("")),
        ],
        || {
            let home = std::env::var("HOME").unwrap();
            let cfg = if cfg!(target_os = "macos") {
                Path::new(&home).join("Library/Application Support")
            } else {
                Path::new(&home).join(".config")
            };
            assert_eq!(data_dir().unwrap(), cfg.join("Recur"));
            assert_eq!(
                worktrees_dir().unwrap(),
                Path::new(&home).join(".recur/worktrees")
            );
            assert_eq!(
                recur_skills_dir().unwrap(),
                Path::new(&home).join(".recur/recur-skills")
            );
            assert_eq!(
                user_bundles_dir().unwrap(),
                Path::new(&home).join(".recur/bundles")
            );
            assert_eq!(bundles_dir().unwrap(), cfg.join("Recur/Bundles"));
            assert_eq!(bundles_cache_dir().unwrap(), cfg.join("Recur/BundleCache"));
            assert_eq!(open_sock_path().unwrap(), cfg.join("Recur/open.sock"));
            assert_eq!(open_files_dir().unwrap(), cfg.join("Recur/open-files"));
            assert_eq!(
                codex_default_seed_auth_path(),
                format!("{home}/.codex/auth.json")
            );
        },
    );
}

#[test]
fn default_worktrees_dir_migrates_legacy_entries() {
    let home = tempfile::tempdir().unwrap();
    run_isolated(
        "default_worktrees_dir_migrates_legacy_entries",
        &[
            ("HOME", home.path()),
            (DATA_DIR_ENV, Path::new("")),
            (WORKTREES_DIR_ENV, Path::new("")),
            ("XDG_CONFIG_HOME", Path::new("")),
        ],
        || {
            let legacy = data_dir().unwrap().join("worktrees");
            std::fs::create_dir_all(legacy.join("proj")).unwrap();
            std::fs::write(legacy.join("LOOP-1"), "x").unwrap();
            let dir = worktrees_dir().unwrap();
            assert!(dir.join("LOOP-1").exists() && dir.join("proj").is_dir());
            assert!(!legacy.exists(), "emptied legacy dir is removed");
        },
    );
}

#[test]
fn migrate_legacy_worktrees_skips_populated_root() {
    let legacy = tempfile::tempdir().unwrap();
    let new_root = tempfile::tempdir().unwrap();
    std::fs::write(legacy.path().join("LOOP-1"), "x").unwrap();
    std::fs::write(new_root.path().join("existing"), "y").unwrap();
    migrate_legacy_worktrees(legacy.path(), new_root.path());
    assert!(legacy.path().join("LOOP-1").exists());
    assert!(!new_root.path().join("LOOP-1").exists());

    std::fs::remove_file(new_root.path().join("existing")).unwrap();
    migrate_legacy_worktrees(legacy.path(), new_root.path());
    assert!(new_root.path().join("LOOP-1").exists());
}

#[test]
fn app_chat_dir_is_managed_not_a_worktree() {
    let data = tempfile::tempdir().unwrap();
    run_isolated(
        "app_chat_dir_is_managed_not_a_worktree",
        &[(DATA_DIR_ENV, data.path())],
        || {
            let data = std::env::var(DATA_DIR_ENV).unwrap();
            let got = app_chat_dir().unwrap();
            assert_eq!(got, Path::new(&data).join("app-chat"));
            assert!(got.is_dir());
            assert_eq!(data_dir().unwrap(), Path::new(&data));
        },
    );
}

#[test]
fn workspace_data_dirs() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let ws = workspace_dir(root, "W1").unwrap();
    assert_eq!(ws, root.join("workspaces").join("W1"));
    let art = artifacts_dir(root, "W1").unwrap();
    assert_eq!(art, root.join("workspaces/W1/artifacts"));
    let att = attachments_dir(root, "W1").unwrap();
    assert_eq!(att, root.join("workspaces/W1/attachments"));
    let shot = screenshots_dir(root, "W1").unwrap();
    assert_eq!(shot, root.join("workspaces/W1/screenshots"));
    for d in [art, att, shot] {
        assert!(d.is_dir(), "{}", d.display());
    }
    assert!(workspace_dir(root, "").is_err());
    assert_eq!(logs_dir(root).unwrap(), root.join("logs"));
    assert_eq!(db_path(root), root.join("recur.db"));
}

#[test]
fn recur_skills_dir_under_home() {
    let home = tempfile::tempdir().unwrap();
    run_isolated(
        "recur_skills_dir_under_home",
        &[("HOME", home.path())],
        || {
            let home = std::env::var("HOME").unwrap();
            assert_eq!(
                recur_skills_dir().unwrap(),
                Path::new(&home).join(".recur").join("recur-skills")
            );
        },
    );
}

#[test]
fn missing_home_is_an_error() {
    run_isolated(
        "missing_home_is_an_error",
        &[("HOME", Path::new(""))],
        || {
            assert!(recur_skills_dir().is_err());
            assert_eq!(codex_default_seed_auth_path(), "");
        },
    );
}
