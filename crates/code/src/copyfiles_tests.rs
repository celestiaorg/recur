//! Copy-files tests.

use std::fs;
use std::path::Path;

use super::*;
use crate::testutil::{tmp, write};

fn never() -> CancellationToken {
    CancellationToken::new()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn pats(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn parse_patterns_rules() {
    assert_eq!(
        parse_patterns("\n.env*\n# comment\n  apps/**/.env.local  \n\\foo\\bar\n"),
        [".env*", "apps/**/.env.local", "foo/bar"]
    );
}

#[test]
fn match_dotenv_any_depth() {
    let p = pats(&[".env*"]);
    for rel in [".env", ".env.local", "apps/web/.env", "deploy/.env.mainnet"] {
        assert!(matches(rel, &p), "{rel}");
    }
    for rel in ["README.md", "src/env.ts", "apps/web/config.json", "", "."] {
        assert!(!matches(rel, &p), "{rel}");
    }
}

#[test]
fn match_root_relative() {
    let p = pats(&["apps/.env"]);
    assert!(matches("apps/.env", &p));
    assert!(!matches("apps/web/.env", &p), "no ** means no depth");
    assert!(!matches(".env", &p));
}

#[test]
fn match_double_star() {
    let p = pats(&["apps/**/.env.local"]);
    assert!(matches("apps/web/.env.local", &p));
    assert!(matches("apps/.env.local", &p));
    assert!(!matches("other/.env.local", &p));
}

#[test]
fn match_dir_any_depth() {
    let p = pats(&["secrets/"]);
    assert!(matches("secrets/a.txt", &p));
    assert!(matches("apps/secrets/a.txt", &p));
    assert!(!matches("secret/a.txt", &p));
}

#[test]
fn copy_and_preview() {
    let src = tmp();
    let dest = tmp();
    for (rel, body) in [
        (".env", "A=1\n"),
        ("apps/web/.env.local", "B=2\n"),
        ("deploy/.env.mainnet", "C=3\n"),
        ("README.md", "hello\n"),
        ("node_modules/.env", "SKIP=1\n"),
        (".git/.env", "nope\n"),
    ] {
        write(src.path(), rel, body);
    }
    fs::create_dir_all(src.path().join(".git/objects")).unwrap();

    let prev = preview(&never(), None, s(src.path()), ".env*\n");
    assert_eq!(prev.error, "");
    assert_eq!(prev.count, 3, "files={:?}", prev.files);
    assert_eq!(
        prev.files.unwrap(),
        [".env", "apps/web/.env.local", "deploy/.env.mainnet"]
    );

    let n = copy(&never(), None, s(src.path()), s(dest.path()), ".env*").unwrap();
    assert_eq!(n, 3);
    for rel in [".env", "apps/web/.env.local", "deploy/.env.mainnet"] {
        assert_eq!(
            fs::read(dest.path().join(rel)).unwrap(),
            fs::read(src.path().join(rel)).unwrap(),
            "{rel}"
        );
    }
    assert!(!dest.path().join("README.md").exists());
    assert!(
        !dest.path().join("node_modules/.env").exists(),
        "node_modules is skipped unless a glob names it"
    );
    assert!(
        !dest.path().join(".git/.env").exists(),
        ".git is never copied"
    );

    let named = preview(&never(), None, s(src.path()), "node_modules/.env\n");
    assert_eq!(named.files.unwrap(), ["node_modules/.env"]);
}

#[test]
fn copy_empty_patterns_is_noop() {
    let (a, b) = (tmp(), tmp());
    assert_eq!(
        copy(&never(), None, s(a.path()), s(b.path()), "\n# none\n").unwrap(),
        0
    );
    let prev = preview(&never(), None, s(a.path()), "\n");
    assert!(prev.files.is_none() && prev.error.is_empty());
}

#[test]
fn preview_missing_root() {
    let dir = tmp();
    let missing = dir.path().join("missing");
    let prev = preview(&never(), None, s(&missing), ".env*");
    assert_eq!(prev.error, format!("Checkout not found at {}", s(&missing)));
    assert_eq!(prev.count, 0);
}

#[test]
fn negation_is_rejected() {
    let text = ".env*\n!.env.example\n";
    assert_eq!(parse_patterns(text), [".env*"]);
    let prev = preview(&never(), None, s(tmp().path()), text);
    assert_eq!(prev.error, ERR_NEGATION);
    let err = copy(&never(), None, s(tmp().path()), s(tmp().path()), text).unwrap_err();
    assert_eq!(err.to_string(), ERR_NEGATION);
}

#[test]
fn copy_dot_dot_in_filename() {
    let (src, dest) = (tmp(), tmp());
    write(src.path(), ".env", "A=1\n");
    write(src.path(), ".env..bak", "B=2\n");
    assert_eq!(
        copy(&never(), None, s(src.path()), s(dest.path()), ".env*").unwrap(),
        2
    );
    assert_eq!(
        fs::read_to_string(dest.path().join(".env..bak")).unwrap(),
        "B=2\n"
    );
}

#[test]
fn copy_leaves_existing_dest() {
    let (src, dest) = (tmp(), tmp());
    write(src.path(), ".env", "from-src\n");
    write(dest.path(), ".env", "from-dest\n");
    assert_eq!(
        copy(&never(), None, s(src.path()), s(dest.path()), ".env").unwrap(),
        0
    );
    assert_eq!(
        fs::read_to_string(dest.path().join(".env")).unwrap(),
        "from-dest\n"
    );
}

#[test]
fn copy_respects_cancel_and_deadline() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        copy(&cancel, None, s(tmp().path()), s(tmp().path()), ".env*"),
        Err(Error::Canceled)
    ));
    let past = Instant::now();
    assert!(matches!(
        copy(
            &never(),
            Some(past),
            s(tmp().path()),
            s(tmp().path()),
            ".env*"
        ),
        Err(Error::DeadlineExceeded)
    ));
}

#[test]
fn preview_and_copy_share_scan_cap() {
    let src = tmp();
    for name in [".env", ".env.local", ".env.prod"] {
        write(src.path(), name, "x\n");
    }
    let prev = preview_capped(&never(), None, s(src.path()), ".env*", 2);
    assert_eq!(prev.error, "more than 2 matching files; narrow the globs");
    let err = copy_capped(&never(), None, s(src.path()), s(tmp().path()), ".env*", 2).unwrap_err();
    assert_eq!(
        err.to_string(),
        "more than 2 matching files; narrow the globs"
    );
}

fn chmod_unreadable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    // Root can read mode-0000 files; the case does not apply then.
    fs::File::open(path).is_err()
}

#[test]
fn copy_skips_unreadable_without_failing() {
    let (src, dest) = (tmp(), tmp());
    write(src.path(), ".env", "secret\n");
    if !chmod_unreadable(&src.path().join(".env")) {
        return;
    }
    assert_eq!(
        copy(&never(), None, s(src.path()), s(dest.path()), ".env*").unwrap(),
        0
    );
    assert!(!dest.path().join(".env").exists());
}

#[test]
fn copy_existing_plus_unreadable_is_ok() {
    let (src, dest) = (tmp(), tmp());
    write(src.path(), ".env", "from-src\n");
    write(dest.path(), ".env", "from-dest\n");
    write(src.path(), ".env.bad", "nope\n");
    if !chmod_unreadable(&src.path().join(".env.bad")) {
        return;
    }
    assert_eq!(
        copy(&never(), None, s(src.path()), s(dest.path()), ".env*").unwrap(),
        0
    );
    assert_eq!(
        fs::read_to_string(dest.path().join(".env")).unwrap(),
        "from-dest\n"
    );
}

#[test]
fn preview_cancel() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let prev = preview(&cancel, None, s(tmp().path()), ".env*");
    assert_eq!(prev.error, "Scan timed out: narrow the globs");
}

#[test]
fn preview_truncates_long_lists() {
    let src = tmp();
    for i in 0..MAX_PREVIEW_FILES + 3 {
        write(src.path(), &format!("cfg/{i:04}.local"), "x");
    }
    let prev = preview(&never(), None, s(src.path()), "*.local");
    assert_eq!(prev.count, (MAX_PREVIEW_FILES + 3) as i64);
    assert!(prev.truncated);
    assert_eq!(prev.files.unwrap().len(), MAX_PREVIEW_FILES);
}

/// `node_modules/` with three files (one executable), a nested dir and a
/// relative symlink.
fn node_modules(root: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    write(root, "node_modules/a/index.js", "a\n");
    write(root, "node_modules/b/index.js", "b\n");
    write(root, "node_modules/.bin/tool", "#!/bin/sh\n");
    let tool = root.join("node_modules/.bin/tool");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("../a", root.join("node_modules/b/link")).unwrap();
}

fn assert_node_modules(src: &Path, dest: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    for rel in [
        "node_modules/a/index.js",
        "node_modules/b/index.js",
        "node_modules/.bin/tool",
    ] {
        assert_eq!(
            fs::read(dest.join(rel)).unwrap(),
            fs::read(src.join(rel)).unwrap(),
            "{rel}"
        );
    }
    let mode = fs::metadata(dest.join("node_modules/.bin/tool"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(
        fs::read_link(dest.join("node_modules/b/link")).unwrap(),
        Path::new("../a"),
        "symlinks are copied as symlinks"
    );
}

#[test]
fn copy_dir_pattern_copies_whole_tree_past_cap() {
    let (src, dest) = (tmp(), tmp());
    node_modules(src.path());
    let n = copy_capped(
        &never(),
        None,
        s(src.path()),
        s(dest.path()),
        "node_modules/",
        2,
    )
    .unwrap();
    assert_eq!(n, 1, "the directory is one entry");
    assert_node_modules(src.path(), dest.path());
}

#[test]
fn copy_dir_pattern_merges_into_existing_dest() {
    let (src, dest) = (tmp(), tmp());
    node_modules(src.path());
    write(dest.path(), "node_modules/a/index.js", "kept\n");
    copy(
        &never(),
        None,
        s(src.path()),
        s(dest.path()),
        "node_modules/",
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dest.path().join("node_modules/a/index.js")).unwrap(),
        "kept\n"
    );
    assert_eq!(
        fs::read_to_string(dest.path().join("node_modules/b/index.js")).unwrap(),
        "b\n"
    );
}

#[test]
fn copy_tree_merges_and_stops_on_cancel() {
    let (src, dest) = (tmp(), tmp());
    node_modules(src.path());
    let ctx = Ctx {
        cancel: &never(),
        deadline: None,
    };
    let to = dest.path().join("node_modules");
    copy_tree(&ctx, s(&src.path().join("node_modules")), s(&to)).unwrap();
    assert_node_modules(src.path(), dest.path());

    let cancel = CancellationToken::new();
    cancel.cancel();
    let ctx = Ctx {
        cancel: &cancel,
        deadline: None,
    };
    assert!(matches!(
        copy_tree(&ctx, s(src.path()), s(tmp().path())),
        Err(Error::Canceled)
    ));
}

#[test]
fn preview_lists_dir_once_at_any_depth() {
    let src = tmp();
    node_modules(src.path());
    write(src.path(), "apps/web/node_modules/x/index.js", "x\n");
    write(src.path(), ".env", "A=1\n");
    let prev = preview(&never(), None, s(src.path()), "node_modules/\n.env\n");
    assert_eq!(prev.error, "");
    assert_eq!(
        prev.files.unwrap(),
        [".env", "apps/web/node_modules/", "node_modules/"]
    );
    assert_eq!(prev.count, 3);

    let anchored = preview(&never(), None, s(src.path()), "apps/*/node_modules/\n");
    assert_eq!(anchored.files.unwrap(), ["apps/web/node_modules/"]);
}
