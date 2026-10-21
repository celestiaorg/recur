use std::fs;
use std::path::Path;

use recur_domain::{Project, RunCommand, verify_dev_cmd};
use serde::Deserialize;

use super::write::encode_project_file;
use super::*;

fn run(name: &str, cmd: &str) -> RunCommand {
    RunCommand {
        name: name.into(),
        cmd: cmd.into(),
    }
}

fn repo(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

fn write_raw(dir: &Path, body: &str) {
    fs::create_dir_all(dir.join(".recur")).unwrap();
    fs::write(dir.join(REL_PATH), body).unwrap();
}

fn read_project_toml(dir: &Path) -> String {
    fs::read_to_string(dir.join(REL_PATH)).unwrap()
}

fn proj(dir: &Path) -> Project {
    Project {
        repo_path: repo(dir),
        ..Default::default()
    }
}

#[test]
fn load_missing_is_zero() {
    let dir = tempfile::tempdir().unwrap();
    let f = load(&repo(dir.path())).unwrap();
    assert_eq!(f, File::default());
    assert!(!exists(&repo(dir.path())));
}

#[test]
fn write_missing_repo_path_fails() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-repo");
    let p = Project {
        repo_path: repo(&missing),
        setup_cmd: "make setup".into(),
        ..Default::default()
    };
    let err = write(&p).unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert!(!missing.join(REL_PATH).exists());
}

#[test]
fn write_repo_path_is_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("not-a-dir");
    fs::write(&f, "x").unwrap();
    let p = Project {
        repo_path: repo(&f),
        setup_cmd: "make setup".into(),
        ..Default::default()
    };
    assert!(write(&p).unwrap_err().to_string().contains("is not a directory"));
}

#[test]
fn write_empty_repo_path_fails() {
    assert_eq!(
        write(&Project::default()).unwrap_err().to_string(),
        "write .recur/project.toml: empty repo path"
    );
}

#[test]
fn write_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project {
        repo_path: repo(dir.path()),
        setup_cmd: "pnpm install".into(),
        teardown_cmd: "rm -rf node_modules".into(),
        run_commands: vec![run("dev", "pnpm dev --port $RECUR_PORT")],
        app_url: "http://localhost:$RECUR_PORT".into(),
        env: "FOO=bar\n# comment\nBAZ=$HOME".into(),
        copy_files: ".env*\napps/**/.env.local".into(),
        ..Default::default()
    };
    write(&p).unwrap();
    let text = read_project_toml(dir.path());
    assert!(text.contains("setup_cmd") && text.contains("pnpm install"), "{text}");
    assert!(!text.contains("dev_cmd"), "{text}");
    assert!(!text.contains("\\n") && text.contains("env = \"\"\""), "{text}");
    assert!(text.contains("copy_files") && text.contains(".env*"), "{text}");
    assert!(text.starts_with("# Recur project settings"), "{text}");
    assert!(exists(&repo(dir.path())));

    let got = overlay(Project {
        repo_path: repo(dir.path()),
        name: "keep".into(),
        ..Default::default()
    });
    assert_eq!(got.name, "keep");
    assert!(got.has_project_file);
    assert_eq!(got.setup_cmd, p.setup_cmd);
    assert_eq!(got.teardown_cmd, p.teardown_cmd);
    assert_eq!(verify_dev_cmd(&got), verify_dev_cmd(&p));
    assert_eq!(got.app_url, p.app_url);
    assert_eq!(got.env, p.env);
    assert_eq!(got.copy_files, p.copy_files);
}

#[test]
fn overlay_omitted_keys_are_empty() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n");
    let got = overlay(Project {
        repo_path: repo(dir.path()),
        setup_cmd: "make setup".into(),
        env: "A=1".into(),
        copy_files: ".env*".into(),
        ..Default::default()
    });
    assert_eq!(verify_dev_cmd(&got), "make dev");
    assert_eq!(got.setup_cmd, "");
    assert_eq!(got.env, "");
    assert_eq!(got.copy_files, "");
}

#[test]
fn overlay_ignores_legacy_dev_cmd() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "dev_cmd = \"legacy\"\n");
    let got = overlay(proj(dir.path()));
    assert_eq!(verify_dev_cmd(&got), "");
    assert!(got.run_commands.is_empty());
}

#[test]
fn overlay_explicit_empty_clears() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "setup_cmd = \"\"\n");
    let got = overlay(Project {
        repo_path: repo(dir.path()),
        setup_cmd: "make setup".into(),
        ..Default::default()
    });
    assert_eq!(got.setup_cmd, "");
}

#[test]
fn overlay_recur_project_toml() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(
        dir.path(),
        include_str!("../../../../../internal/projfile/testdata/project.toml"),
    );
    let got = overlay(Project {
        repo_path: repo(dir.path()),
        env: "OLD=1".into(),
        ..Default::default()
    });
    assert!(got.has_project_file);
    assert!(got.setup_cmd.contains("go mod download"), "{}", got.setup_cmd);
    assert!(got.env != "OLD=1" && got.env.contains("RECUR_DATA_DIR"), "{}", got.env);
    assert!(got.run_commands.len() >= 2);
    assert_eq!(got.run_commands[0].cmd, "wails3 task dev");
}

#[test]
fn overlay_malformed_yields_empty_run_config() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "[[[not toml");
    let got = overlay(Project {
        repo_path: repo(dir.path()),
        setup_cmd: "keep".into(),
        ..Default::default()
    });
    assert_eq!(got.setup_cmd, "");
    assert!(got.has_project_file);
    assert!(
        got.project_file_error.starts_with("parse .recur/project.toml: toml: "),
        "{}",
        got.project_file_error
    );
    // A second overlay hits the cache and reports the same error.
    let again = overlay(proj(dir.path()));
    assert_eq!(again.project_file_error, got.project_file_error);
}

#[test]
fn overlay_type_mismatch_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "setup_cmd = 5\n");
    let got = overlay(proj(dir.path()));
    assert!(!got.project_file_error.is_empty());
    assert_eq!(got.setup_cmd, "");
}

#[test]
fn overlay_missing_file_clears_db_run_config() {
    let dir = tempfile::tempdir().unwrap();
    let got = overlay(Project {
        repo_path: repo(dir.path()),
        setup_cmd: "make setup".into(),
        teardown_cmd: "make clean".into(),
        app_url: "http://localhost:3000".into(),
        env: "A=1".into(),
        copy_files: ".env*".into(),
        run_commands: vec![run("dev", "make dev")],
        ..Default::default()
    });
    assert_eq!(got.setup_cmd, "");
    assert_eq!(got.teardown_cmd, "");
    assert_eq!(got.app_url, "");
    assert_eq!(got.env, "");
    assert_eq!(got.copy_files, "");
    assert!(got.run_commands.is_empty());
    assert!(!got.has_project_file);
    assert_eq!(got.project_file_error, "");
}

#[test]
fn overlay_sees_rewrites_through_cache() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "setup_cmd = 'one'\n");
    assert_eq!(overlay(proj(dir.path())).setup_cmd, "one");
    write(&Project {
        repo_path: repo(dir.path()),
        setup_cmd: "two".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(overlay(proj(dir.path())).setup_cmd, "two");
}

#[test]
fn load_tilde_path() {
    let home = tempfile::tempdir().unwrap();
    crate::testenv::run(module_path!(), "load_tilde_path", &[("HOME", home.path())], || {
        let home = std::env::var("HOME").unwrap();
        let repo = Path::new(&home).join("code").join("app");
        fs::create_dir_all(&repo).unwrap();
        write(&Project {
            repo_path: repo.to_string_lossy().into_owned(),
            run_commands: vec![run("dev", "go run .")],
            ..Default::default()
        })
        .unwrap();
        let got = overlay(Project {
            repo_path: "~/code/app".into(),
            ..Default::default()
        });
        assert_eq!(verify_dev_cmd(&got), "go run .");
    });
}

#[test]
fn overlay_named_runs() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(
        dir.path(),
        "\ndev_cmd = \"legacy\"\n[[run]]\nname = \"dev\"\ncmd = \"wails3 task dev\"\n[[run]]\nname = \"test\"\ncmd = \"go test ./...\"\n",
    );
    let got = overlay(Project {
        repo_path: repo(dir.path()),
        pinned_run: "test".into(),
        ..Default::default()
    });
    assert_eq!(got.pinned_run, "test");
    assert_eq!(got.run_commands.len(), 2);
    assert_eq!(got.run_commands[1].name, "test");
    assert_eq!(verify_dev_cmd(&got), "wails3 task dev");
}

#[test]
fn write_named_runs() {
    let dir = tempfile::tempdir().unwrap();
    write(&Project {
        repo_path: repo(dir.path()),
        run_commands: vec![run("dev", "wails3 task dev"), run("test", "go test ./...")],
        ..Default::default()
    })
    .unwrap();
    let text = read_project_toml(dir.path());
    assert!(text.contains("[[run]]") && text.contains("go test"), "{text}");
    let got = overlay(proj(dir.path()));
    assert_eq!(got.run_commands.len(), 2);
    assert_eq!(got.run_commands[1].cmd, "go test ./...");
}

#[test]
fn write_multiline_env() {
    let dir = tempfile::tempdir().unwrap();
    let env = "RECUR_DATA_DIR=$HOME/tmp\nWAILS_VITE_PORT=$RECUR_PORT\n";
    write(&Project {
        repo_path: repo(dir.path()),
        env: env.into(),
        ..Default::default()
    })
    .unwrap();
    let text = read_project_toml(dir.path());
    assert!(!text.contains("\\n"), "{text}");
    assert!(text.contains("env = \"\"\""), "{text}");
    assert!(text.contains("WAILS_VITE_PORT=$RECUR_PORT"), "{text}");
    assert_eq!(overlay(proj(dir.path())).env, env);
}

#[test]
fn write_multiline_run_cmd() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = "go test ./...\ngo vet ./...\n";
    write(&Project {
        repo_path: repo(dir.path()),
        run_commands: vec![run("check", cmd)],
        ..Default::default()
    })
    .unwrap();
    let text = read_project_toml(dir.path());
    assert!(!text.contains("\\n"), "{text}");
    assert!(text.contains("cmd = \"\"\""), "{text}");
    let got = overlay(proj(dir.path()));
    assert_eq!(got.run_commands, vec![run("check", cmd.trim())]);
}

#[test]
fn write_merges_multiline_run_cmd() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "# keep\n[[run]]\nname = \"dev\"\ncmd = \"make dev\"\n");
    write(&Project {
        repo_path: repo(dir.path()),
        run_commands: vec![run("dev", "make setup\nmake dev\n")],
        ..Default::default()
    })
    .unwrap();
    let text = read_project_toml(dir.path());
    assert!(text.contains("# keep"), "{text}");
    assert!(text.contains("cmd = \"\"\"") && !text.contains("\\n"), "{text}");
    let got = overlay(proj(dir.path()));
    assert_eq!(got.run_commands, vec![run("dev", "make setup\nmake dev")]);
}

#[test]
fn write_merges_comments_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(
        dir.path(),
        "# team notes\n\n# secrets stay in .env\nenv = \"\"\"\nFOO=old\n\"\"\"\n\nextra = \"keep-me\"\n\n[[run]]\n# local override\nname = \"dev\"\ncmd = \"make dev\"\n",
    );
    write(&Project {
        repo_path: repo(dir.path()),
        env: "FOO=new\nBAR=2\n".into(),
        run_commands: vec![run("dev", "make dev"), run("test", "go test")],
        ..Default::default()
    })
    .unwrap();
    let text = read_project_toml(dir.path());
    for want in [
        "# team notes",
        "# secrets stay in .env",
        "extra = \"keep-me\"",
        "# local override",
    ] {
        assert!(text.contains(want), "dropped {want:?}:\n{text}");
    }
    let got = overlay(proj(dir.path()));
    assert_eq!(got.env, "FOO=new\nBAR=2\n");
    assert_eq!(got.run_commands.len(), 2);
    assert_eq!(got.run_commands[1].cmd, "go test");
}

#[test]
fn write_adds_runs_when_file_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project {
        repo_path: repo(dir.path()),
        setup_cmd: "make setup".into(),
        env: "A=1".into(),
        ..Default::default()
    };
    write(&p).unwrap();
    let text = read_project_toml(dir.path());
    assert!(!text.contains("[[run]]") && !text.contains("[run."), "{text}");
    p.run_commands = vec![run("dev", "make dev")];
    write(&p).unwrap();
    let text = read_project_toml(dir.path());
    assert!(!text.contains("[run.0]"), "{text}");
    assert!(text.contains("[[run]]") && text.contains("make dev"), "{text}");
    assert_eq!(overlay(proj(dir.path())).run_commands, vec![run("dev", "make dev")]);
}

#[test]
fn write_heals_indexed_run_tables() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(
        dir.path(),
        "setup_cmd = 'make setup'\n\n[run.0]\nname = 'dev'\ncmd = 'old'\n",
    );
    write(&Project {
        repo_path: repo(dir.path()),
        setup_cmd: "make setup".into(),
        run_commands: vec![run("dev", "make dev")],
        ..Default::default()
    })
    .unwrap();
    assert!(!read_project_toml(dir.path()).contains("[run.0]"));
    assert_eq!(overlay(proj(dir.path())).run_commands, vec![run("dev", "make dev")]);
}

#[test]
fn write_malformed_rewrites() {
    let dir = tempfile::tempdir().unwrap();
    write_raw(dir.path(), "[[[not toml");
    write(&Project {
        repo_path: repo(dir.path()),
        env: "A=1".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(read_project_toml(dir.path()).starts_with("# Recur project settings"));
    assert_eq!(overlay(proj(dir.path())).env, "A=1");
}

#[derive(Deserialize)]
struct WriteCase {
    name: String,
    existing: Option<String>,
    project: Project,
    output: String,
}

#[derive(Deserialize)]
struct Golden {
    write: Vec<WriteCase>,
}

/// Go's encodeProjectFile output (fresh file and in-place merges), recorded
/// by `tests/fixtures/gen_projfile.go.txt`.
#[test]
fn encode_matches_go() {
    let golden: Golden =
        serde_json::from_str(include_str!("../../tests/fixtures/projfile_golden.json")).unwrap();
    assert!(golden.write.len() >= 14);
    for case in golden.write {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.toml");
        if let Some(existing) = &case.existing {
            fs::write(&path, existing).unwrap();
        }
        let got = encode_project_file(&path.to_string_lossy(), &case.project).unwrap();
        assert_eq!(got, case.output, "{}", case.name);
    }
}
