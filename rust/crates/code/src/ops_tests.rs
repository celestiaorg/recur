//! Ports of `ops_test.go` and `search_replace_test.go`.

use std::fs;

use recur_domain::SearchOpts;
use tokio_util::sync::CancellationToken;

use crate::testutil::{tmp, write};
use crate::*;

#[test]
fn create_mkdir_rename_copy_remove() {
    let root = tmp();
    let r = root.path();
    create_file(r, "src/hello.go").unwrap();
    assert_eq!(
        create_file(r, "src/hello.go").unwrap_err().to_string(),
        "src/hello.go already exists"
    );
    assert!(fs::read(r.join("src/hello.go")).unwrap().is_empty());

    mkdir(r, "src/pkg").unwrap();
    mkdir(r, "src/pkg").unwrap();
    assert_eq!(
        mkdir(r, "src/hello.go").unwrap_err().to_string(),
        "src/hello.go already exists"
    );
    create_file(r, "src/pkg/x.go").unwrap();
    fs::write(r.join("src/pkg/x.go"), "package pkg\n").unwrap();

    rename(r, "src/hello.go", "src/hi.go").unwrap();
    assert!(!r.join("src/hello.go").exists());
    rename(r, "src/hi.go", "src/hi.go").unwrap();

    copy(r, "src/pkg", "src/pkg2").unwrap();
    assert_eq!(
        fs::read_to_string(r.join("src/pkg2/x.go")).unwrap(),
        "package pkg\n"
    );
    copy(r, "src/hi.go", "src/hi_copy.go").unwrap();
    assert!(copy(r, "src/hi.go", "src/hi_copy.go").is_err());
    assert_eq!(
        copy(r, "src/hi.go", "src/hi.go").unwrap_err().to_string(),
        "source and destination are the same"
    );

    remove(r, "src/pkg2").unwrap();
    assert!(!r.join("src/pkg2").exists());
    remove(r, "src/hi_copy.go").unwrap();
    assert!(
        remove(r, "src/gone")
            .unwrap_err()
            .to_string()
            .starts_with("lstat ")
    );
}

#[test]
fn ops_reject_root_and_escape() {
    let root = tmp();
    let r = root.path();
    for err in [
        create_file(r, ""),
        mkdir(r, "."),
        remove(r, " / "),
        rename(r, "../x", "y"),
        copy(r, "a", "../b"),
    ] {
        assert!(err.is_err());
    }
    assert_eq!(create_file(r, "").unwrap_err().to_string(), "path required");
}

#[test]
fn copy_rename_refuse_self_nest() {
    let root = tmp();
    let r = root.path();
    mkdir(r, "a/b").unwrap();
    create_file(r, "a/b/f.go").unwrap();
    assert_eq!(
        copy(r, "a", "a/nested").unwrap_err().to_string(),
        "cannot copy into itself"
    );
    assert_eq!(
        rename(r, "a", "a/b/moved").unwrap_err().to_string(),
        "cannot move into itself"
    );
}

#[test]
fn copy_skips_symlinks_and_keeps_modes() {
    use std::os::unix::fs::PermissionsExt;
    let root = tmp();
    let r = root.path();
    write(r, "d/run.sh", "#!/bin/sh\n");
    fs::set_permissions(r.join("d/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", r.join("d/hosts")).unwrap();
    copy(r, "d", "e").unwrap();
    assert_eq!(
        fs::metadata(r.join("e/run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(fs::symlink_metadata(r.join("e/hosts")).is_err());
}

#[test]
fn abs_path_joins_under_root() {
    let root = tmp();
    assert_eq!(
        abs_path(root.path(), "src/x.go").unwrap(),
        root.path().join("src/x.go")
    );
}

#[test]
fn replace_in_files_case_insensitive() {
    let root = tmp();
    write(root.path(), "a.go", "Foo foo FOO\n");
    let res = replace_in_files(
        &CancellationToken::new(),
        root.path(),
        "foo",
        "bar",
        &SearchOpts::default(),
    )
    .unwrap();
    assert_eq!((res.files_changed, res.replacements), (1, 3));
    assert_eq!(
        fs::read_to_string(root.path().join("a.go")).unwrap(),
        "bar bar bar\n"
    );
}

#[test]
fn replace_in_files_regex_and_exact() {
    let root = tmp();
    write(root.path(), "a.go", "func Alpha()\nfunc Beta()\n");
    write(root.path(), "b.txt", "func Alpha()\n");
    let res = replace_in_files(
        &CancellationToken::new(),
        root.path(),
        r"func (\w+)",
        "fn $1",
        &SearchOpts {
            regex: true,
            glob: "*.go".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((res.files_changed, res.replacements), (1, 2));
    assert_eq!(
        fs::read_to_string(root.path().join("a.go")).unwrap(),
        "fn Alpha()\nfn Beta()\n"
    );
    let res = replace_in_files(
        &CancellationToken::new(),
        root.path(),
        "Alpha",
        "Gamma",
        &SearchOpts {
            case_sensitive: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((res.files_changed, res.replacements), (2, 2));
    assert_eq!(
        replace_in_files(
            &CancellationToken::new(),
            root.path(),
            " ",
            "x",
            &SearchOpts::default()
        )
        .unwrap_err()
        .to_string(),
        "empty search"
    );
}

#[test]
fn search_opts_regex_and_glob() {
    let root = tmp();
    write(root.path(), "a.go", "func Alpha()\n");
    write(root.path(), "b.txt", "func Alpha()\n");
    let res = search_opts(
        &CancellationToken::new(),
        root.path(),
        r"func\s+A\w+",
        &SearchOpts {
            regex: true,
            glob: "*.go".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let hits = res.hits.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "a.go");
    assert!(!res.truncated && res.skipped_large == 0);
}

/// Over-cap files are skipped and counted rather than a silent "no matches".
#[test]
fn search_opts_reports_skipped_large_files() {
    let root = tmp();
    write(
        root.path(),
        "big.go",
        "needle\n".repeat(256 * 1024 / 7 + 16),
    );
    let res = search_opts(
        &CancellationToken::new(),
        root.path(),
        "needle",
        &SearchOpts::default(),
    )
    .unwrap();
    assert!(res.hits.is_none());
    assert_eq!(res.skipped_large, 1);
}

#[test]
fn search_opts_honours_cancellation() {
    let root = tmp();
    write(root.path(), "a.go", "needle\n");
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        search_opts(&cancel, root.path(), "needle", &SearchOpts::default()),
        Err(Error::Canceled)
    ));
}

#[test]
fn search_skips_symlinks_out_of_root() {
    let root = tmp();
    let outside = tmp();
    write(outside.path(), "secret.go", "needle\n");
    write(root.path(), "in.go", "needle\n");
    std::os::unix::fs::symlink(
        outside.path().join("secret.go"),
        root.path().join("link.go"),
    )
    .unwrap();
    let hits = search(&CancellationToken::new(), root.path(), "needle")
        .unwrap()
        .unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].path, "in.go");
}

#[test]
fn list_files_and_fuzzy() {
    let root = tmp();
    write(root.path(), "pkg/x.go", "package pkg\n");
    write(root.path(), "README.md", "#\n");
    write(root.path(), "node_modules/m.js", "x");
    write(root.path(), "logo.png", "x");
    let files = list_files(&CancellationToken::new(), root.path()).unwrap();
    assert_eq!(files, ["README.md", "pkg/x.go"]);
    assert_eq!(match_files(&files, "pkg/x", 10), ["pkg/x.go"]);
}
