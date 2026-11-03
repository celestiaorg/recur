use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use recur_domain::{Project, Workspace};

use super::*;

fn env_map(env: &[String]) -> BTreeMap<String, String> {
    env.iter()
        .filter_map(|kv| match kv.find('=') {
            Some(i) if i > 0 => Some((kv[..i].to_owned(), kv[i + 1..].to_owned())),
            _ => None,
        })
        .collect()
}

fn write_dotenv(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn base(kvs: &[&str]) -> Option<Vec<String>> {
    Some(kvs.iter().map(|s| (*s).to_owned()).collect())
}

#[test]
fn load_dotenv_files_later_wins() {
    let repo = tempfile::tempdir().unwrap();
    write_dotenv(repo.path(), ".env", "A=from-env\nB=env\n");
    write_dotenv(repo.path(), ".env.local", "B=local\n");
    write_dotenv(repo.path(), ".recur/.env", "C=recur-env\n");
    let got = load_dotenv_files(&[&s(repo.path())]);
    assert_eq!(got["A"], "from-env");
    assert_eq!(got["B"], "local");
    assert_eq!(got["C"], "recur-env");
}

#[test]
fn load_dotenv_files_worktree_overrides_repo() {
    let repo = tempfile::tempdir().unwrap();
    let wt = tempfile::tempdir().unwrap();
    write_dotenv(repo.path(), ".env", "SECRET=repo\nSHARED=repo\n");
    write_dotenv(wt.path(), ".env", "SHARED=worktree\n");
    let got = load_dotenv_files(&[&s(repo.path()), &s(wt.path())]);
    assert_eq!(got["SECRET"], "repo");
    assert_eq!(got["SHARED"], "worktree");
}

#[test]
fn load_dotenv_files_missing_ok() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let got = load_dotenv_files(&[&s(a.path()), &s(&b.path().join("nope"))]);
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn load_dotenv_files_skips_oversized_and_dirs() {
    let repo = tempfile::tempdir().unwrap();
    write_dotenv(
        repo.path(),
        ".env",
        &format!("BIG={}\n", "x".repeat(256 << 10)),
    );
    fs::create_dir_all(repo.path().join(".env.local")).unwrap();
    write_dotenv(repo.path(), ".recur/.env", "OK=1\n");
    let got = load_dotenv_files(&[&s(repo.path()), &s(repo.path())]);
    assert_eq!(got, map(&[("OK", "1")]));
}

#[test]
fn environ_loads_dotenv_and_expands_project_env() {
    let repo = tempfile::tempdir().unwrap();
    write_dotenv(repo.path(), ".env", "API_KEY=sk-test\nNESTED=$API_KEY\n");
    write_dotenv(repo.path(), ".recur/.env.local", "LOCAL=1\n");
    let v = Vars {
        port: 9,
        repo_path: s(repo.path()),
        project_id: "p".into(),
        project_env: map(&[
            ("AUTH", "Bearer $API_KEY"),
            ("NESTED_COPY", "$NESTED"),
            ("PORT_COPY", "$RECUR_PORT"),
        ]),
        ..Default::default()
    };
    let got = env_map(&v.environ(base(&["PATH=/bin"])));
    assert!(!got.contains_key("API_KEY"));
    assert!(!got.contains_key("NESTED"));
    assert!(!got.contains_key("LOCAL"));
    assert_eq!(got["AUTH"], "Bearer sk-test");
    assert_eq!(got["NESTED_COPY"], "sk-test");
    assert_eq!(got["PORT_COPY"], "9");
}

#[test]
fn environ_db_project_env_may_set_path() {
    let v = Vars::from_workspace(
        &Workspace {
            id: "W1".into(),
            project_id: "p".into(),
            ..Default::default()
        },
        &Project {
            env: "PATH=/tmp/userbin\nDYLD_INSERT_LIBRARIES=/tmp/x.dylib\nOK=1\n".into(),
            ..Default::default()
        },
    );
    let got = env_map(&v.environ(base(&["PATH=/bin:/usr/bin"])));
    assert_eq!(got["PATH"], "/tmp/userbin");
    assert_eq!(got["DYLD_INSERT_LIBRARIES"], "/tmp/x.dylib");
    assert_eq!(got["OK"], "1");
}

#[test]
fn environ_project_toml_env_does_not_override_path() {
    let v = Vars::from_workspace(
        &Workspace {
            id: "W1".into(),
            project_id: "p".into(),
            ..Default::default()
        },
        &Project {
            has_project_file: true,
            env: "PATH=/tmp/evil\nDYLD_INSERT_LIBRARIES=/tmp/evil.dylib\nOK=1\n".into(),
            ..Default::default()
        },
    );
    let got = env_map(&v.environ(base(&["PATH=/bin:/usr/bin"])));
    assert_eq!(got["PATH"], "/bin:/usr/bin");
    assert!(!got.contains_key("DYLD_INSERT_LIBRARIES"));
    assert_eq!(got["OK"], "1");
}

#[test]
fn environ_dotenv_does_not_override_path() {
    let repo = tempfile::tempdir().unwrap();
    write_dotenv(
        repo.path(),
        ".env",
        "PATH=/tmp/evil:$PATH\nSHELL=/tmp/evilsh\nHOME=/tmp/fakehome\nDYLD_INSERT_LIBRARIES=/tmp/evil.dylib\nLD_PRELOAD=/tmp/evil.so\nFOO=$PATH\nOK=1\n",
    );
    let v = Vars {
        repo_path: s(repo.path()),
        project_id: "p".into(),
        project_env: map(&[("FOO", "$PATH")]),
        ..Default::default()
    };
    let got = env_map(&v.environ(base(&[
        "PATH=/bin:/usr/bin",
        "SHELL=/bin/zsh",
        "HOME=/Users/me",
    ])));
    assert_eq!(got["PATH"], "/bin:/usr/bin");
    assert_eq!(got["SHELL"], "/bin/zsh");
    assert_eq!(got["HOME"], "/Users/me");
    assert!(!got.contains_key("DYLD_INSERT_LIBRARIES"));
    assert!(!got.contains_key("LD_PRELOAD"));
    assert!(!got.contains_key("OK"));
    assert!(!got["FOO"].contains("/tmp/evil"), "{}", got["FOO"]);
}

#[test]
fn environ_recur_vars_win_over_dotenv() {
    let repo = tempfile::tempdir().unwrap();
    write_dotenv(repo.path(), ".env", "RECUR_PORT=1\nPORT=1\n");
    let v = Vars {
        port: 42,
        repo_path: s(repo.path()),
        project_id: "p".into(),
        ..Default::default()
    };
    let got = env_map(&v.environ(base(&["PATH=/bin"])));
    assert_eq!(got["RECUR_PORT"], "42");
    assert_eq!(got["PORT"], "42");
}

#[test]
fn environ_project_env_wins_over_dotenv() {
    let repo = tempfile::tempdir().unwrap();
    write_dotenv(repo.path(), ".env", "FOO=from-file\n");
    let v = Vars {
        repo_path: s(repo.path()),
        project_env: map(&[("FOO", "from-toml")]),
        ..Default::default()
    };
    let got = env_map(&v.environ(base(&["PATH=/bin"])));
    assert_eq!(got["FOO"], "from-toml");
}

#[test]
fn environ_defaults_to_host_environment() {
    let v = Vars::default();
    let got = env_map(&v.environ(None));
    if let Ok(path) = std::env::var("PATH") {
        assert_eq!(got["PATH"], path);
    }
    assert_eq!(got["RECUR_BRANCH"], "");
}

#[test]
fn parse_dotenv_cases() {
    let got = parse_dotenv(
        "\n# comment\nVITE_PORT=$RECUR_PORT\nexport FOO=bar\nBAZ=\"qux\"\nQ='single'\nbad line\n=nokey\n  SP = spaced  \nMIX=\"a'\nLATER=1\nLATER=2\n",
    );
    assert_eq!(got["VITE_PORT"], "$RECUR_PORT");
    assert_eq!(got["FOO"], "bar");
    assert_eq!(got["BAZ"], "qux");
    assert_eq!(got["Q"], "single");
    assert_eq!(got["SP"], "spaced");
    assert_eq!(got["MIX"], "\"a'");
    assert_eq!(got["LATER"], "2");
    assert_eq!(got.len(), 7);
}

#[test]
fn environ_injects_recur_and_project_env() {
    let v = Vars {
        port: 9246,
        workspace_id: "LOOP-1".into(),
        worktree_path: "/tmp/wt".into(),
        project_id: "recur".into(),
        branch: "mycodecrafting/smoky-shortcake".into(),
        project_env: map(&[
            ("VITE_PORT", "$RECUR_PORT"),
            ("RECUR_DATA_DIR", "/tmp/data-$RECUR_WORKSPACE_ID"),
            ("RECUR_INSTANCE_LABEL", "$RECUR_BRANCH"),
        ]),
        ..Default::default()
    };
    let got = env_map(&v.environ(base(&["PATH=/bin", "RECUR_PORT=1"])));
    assert_eq!(got["RECUR_PORT"], "9246");
    assert_eq!(got["PORT"], "9246");
    assert_eq!(got["RECUR_WORKSPACE_ID"], "LOOP-1");
    assert_eq!(got["RECUR_WORKTREE"], "/tmp/wt");
    assert_eq!(got["RECUR_BRANCH"], "mycodecrafting/smoky-shortcake");
    assert_eq!(
        got["RECUR_INSTANCE_LABEL"],
        "mycodecrafting/smoky-shortcake"
    );
    assert_eq!(got["VITE_PORT"], "9246");
    assert_eq!(got["RECUR_DATA_DIR"], "/tmp/data-LOOP-1");
}

#[test]
fn expand_known_leaves_unknown() {
    let got = expand_known(
        "x=$RECUR_PORT y=$NOPE_RECUR_TEST",
        &map(&[("RECUR_PORT", "9")]),
    );
    assert_eq!(got, "x=9 y=$NOPE_RECUR_TEST");
}

#[test]
fn expand_known_uses_home() {
    crate::testenv::run(
        module_path!(),
        "expand_known_uses_home",
        &[("HOME", "/Users/test")],
        || {
            let got = expand_known(
                "$HOME/Library/Recur-$RECUR_WORKSPACE_ID",
                &map(&[("RECUR_WORKSPACE_ID", "W1")]),
            );
            assert_eq!(got, "/Users/test/Library/Recur-W1");
        },
    );
}

#[test]
fn expand_matches_go_os_expand() {
    let m = |k: &str| format!("<{k}>");
    for (input, want) in [
        ("", ""),
        ("plain", "plain"),
        ("$A b", "<A> b"),
        ("${A}b", "<A>b"),
        ("${}x", "x"),
        ("${x", "x"),
        ("$$", "<$>"),
        ("${1}", "<1>"),
        ("$1x", "<1>x"),
        ("$-", "<->"),
        ("trail$", "trail$"),
        ("$ é", "$ é"),
        ("a$é", "a$é"),
        ("${a b}", "<a b>"),
        ("$A_1.x", "<A_1>.x"),
    ] {
        assert_eq!(expand(input, m), want, "expand({input:?})");
    }
}

#[test]
fn from_workspace() {
    let v = Vars::from_workspace(
        &Workspace {
            id: "W1".into(),
            project_id: "P1".into(),
            worktree_path: "/wt".into(),
            port: 3,
            branch: " feat/x ".into(),
            ..Default::default()
        },
        &Project {
            repo_path: "/repo".into(),
            env: "A=1\n# hi\nB=two".into(),
            ..Default::default()
        },
    );
    assert_eq!(v.port, 3);
    assert_eq!(v.workspace_id, "W1");
    assert_eq!(v.repo_path, "/repo");
    assert_eq!(v.branch, "feat/x");
    assert_eq!(v.project_env["A"], "1");
    assert_eq!(v.project_env["B"], "two");
}

#[test]
fn recur_map_includes_empty_branch() {
    let m = Vars {
        workspace_id: "W1".into(),
        project_id: "P1".into(),
        worktree_path: "/wt".into(),
        ..Default::default()
    }
    .recur_map();
    assert_eq!(m.get("RECUR_BRANCH").map(String::as_str), Some(""));
    assert!(!m.contains_key("PORT"));
}

#[test]
fn environ_empty_branch_hides_instance_label() {
    let v = Vars {
        workspace_id: "W1".into(),
        project_env: map(&[("RECUR_INSTANCE_LABEL", "$RECUR_BRANCH")]),
        ..Default::default()
    };
    let got = env_map(&v.environ(base(&["PATH=/bin"])));
    assert_eq!(got["RECUR_INSTANCE_LABEL"], "");
    assert_eq!(got["RECUR_BRANCH"], "");
}

#[test]
fn environ_branch_dollar_is_not_reexpanded() {
    crate::testenv::run(
        module_path!(),
        "environ_branch_dollar_is_not_reexpanded",
        &[("HOME", "/Users/test")],
        || {
            let v = Vars {
                branch: "feat/$HOME".into(),
                project_env: map(&[
                    ("RECUR_INSTANCE_LABEL", "$RECUR_BRANCH"),
                    ("COPY", "$RECUR_INSTANCE_LABEL"),
                ]),
                ..Default::default()
            };
            let got = env_map(&v.environ(base(&["PATH=/bin"])));
            assert_eq!(got["RECUR_INSTANCE_LABEL"], "feat/$HOME");
            assert_eq!(got["COPY"], "feat/$HOME");
            assert_eq!(got["RECUR_BRANCH"], "feat/$HOME");
        },
    );
}

#[test]
fn from_workspace_strips_toml_path() {
    let v = Vars::from_workspace(
        &Workspace {
            project_id: "p".into(),
            port: 9,
            ..Default::default()
        },
        &Project {
            id: "p".into(),
            repo_path: "/repo".into(),
            has_project_file: true,
            env: "PATH=/tmp/evil\nOK=1\n".into(),
            ..Default::default()
        },
    );
    assert_eq!(v.port, 9);
    assert_eq!(v.project_id, "p");
    assert_eq!(v.repo_path, "/repo");
    assert!(!v.project_env.contains_key("PATH"));
    assert_eq!(v.project_env["OK"], "1");
}
