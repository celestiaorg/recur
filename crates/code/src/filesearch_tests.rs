//! File search tests.

use recur_domain::path;

use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn basenames_of(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| path::base(p)).collect()
}

#[test]
fn rank_files_hyp_hyperlane() {
    let paths = s(&[
        "validators/decode-hyperlane-message.sh",
        "validators/hyperlane_validator2.sh",
        "hypothesis.txt",
        "README.md",
    ]);
    let hits = rank_files(None, &paths, &basenames_of(&paths), "hyp", 50);
    let seen: HashSet<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    assert!(
        seen.contains("validators/hyperlane_validator2.sh")
            && seen.contains("validators/decode-hyperlane-message.sh"),
        "hits={hits:?} missing hyperlane scripts"
    );
}

#[test]
fn rank_files_basename_beats_deep_path() {
    let mut paths = s(&["frontend/src/lib/deeply/nested/path/match-util.ts"]);
    for i in 0..15 {
        paths.push(format!("a{i:02}/match.md"));
    }
    let hits = rank_files(None, &paths, &basenames_of(&paths), "match", 50);
    assert_eq!(hits.len(), 16);
    assert!(
        hits.iter()
            .any(|h| h.path == "frontend/src/lib/deeply/nested/path/match-util.ts"),
        "nested match-util.ts missing from top 50"
    );
}

#[test]
fn filter_for_index_exclude() {
    let paths = s(&["src/a.go", "dist/bundle.js", "src/b.go", "dir/.DS_Store"]);
    let got = crate::fileindex::filter_for_index(
        paths,
        &compile_excludes(&s(&["dist/**", "**/.DS_Store"])),
    );
    assert_eq!(got, s(&["src/a.go", "src/b.go"]));
}

#[test]
fn exclude_to_rg_globs_shapes() {
    let globs = exclude_to_rg_globs(&s(&["**/.git", "**/.DS_Store", "dist/**", "build", ""]));
    let has = |g: &str| globs.iter().any(|x| x == g);
    assert!(has("!**/.DS_Store") && has("!**/.DS_Store/**"), "{globs:?}");
    assert!(has("!dist/**"), "{globs:?}");
    assert!(!has("!dist/**/**"), "should not double /**: {globs:?}");
    assert!(has("!build") && has("!build/**"), "{globs:?}");
    assert!(
        !has("!**/.git") && !has("!.git"),
        "default .git should be omitted: {globs:?}"
    );
}

#[test]
fn rank_files_camel_hump() {
    let paths = s(&["pkg/UserController.go", "pkg/other.go"]);
    let hits = rank_files(None, &paths, &basenames_of(&paths), "uc", 10);
    assert_eq!(
        hits.first().map(|h| h.path.as_str()),
        Some("pkg/UserController.go")
    );
}

#[test]
fn fast_exclude_matcher_defaults() {
    let ex = compile_excludes(&s(&["**/.git", "**/.DS_Store", "dist/**", "**/*.map"]));
    for (path, want) in [
        (".git", true),
        ("foo/.git", true),
        ("foo/.git/config", true),
        ("src/a.go", false),
        ("dir/.DS_Store", true),
        ("dist/bundle.js", true),
        ("distribute/x", false),
        ("app.js.map", true),
        ("app.js", false),
    ] {
        assert_eq!(excluded(path, &ex), want, "excluded({path:?})");
    }
}

#[test]
fn regex_exclude_fallback() {
    let ex = compile_excludes(&s(&["src/*.tmp", "a?c"]));
    assert!(excluded("src/x.tmp", &ex));
    assert!(!excluded("src/d/x.tmp", &ex));
    assert!(excluded("/abc", &ex));
    assert!(!excluded("abbc", &ex));
}

#[test]
fn word_boundary() {
    for (text, q, want) in [
        ("foo/bar_baz.go", "bar", true),
        ("foo/bar_baz.go", "baz", true),
        ("foo/bar_baz.go", "foo", true),
        ("foo/bar_baz.go", "ar", false),
        ("", "x", false),
    ] {
        assert_eq!(word_boundary_match(text, q), want, "({text:?},{q:?})");
    }
}

#[test]
fn unique_parent_dirs_from_files() {
    let got = unique_parent_dirs(&s(&[
        "src/lib/a.ts",
        "src/lib/b.ts",
        "README.md",
        "frontend/src/main.go",
    ]));
    assert_eq!(got, s(&["src/lib", "src", "frontend/src", "frontend"]));
}

#[test]
fn rank_files_and_dirs_merges() {
    let paths = s(&["src/lib/main.go", "src/app.ts", "README.md"]);
    let hits = rank_files_and_dirs(None, &paths, &basenames_of(&paths), "src", 50);
    let mut saw_dir = false;
    let mut saw_file = false;
    for h in &hits {
        let path = h.path.trim_end_matches('/');
        if (path == "src" || path == "src/lib") && h.dir {
            saw_dir = true;
        }
        if h.path == "src/app.ts" && !h.dir {
            saw_file = true;
        }
        assert!(
            !(h.dir && (path == "src/app.ts" || path == "README.md")),
            "file marked dir: {h:?}"
        );
    }
    assert!(saw_dir && saw_file, "hits={hits:?}");
}

#[test]
fn rank_hits_files_only_keeps_files_when_dirs_dominate() {
    let paths: Vec<String> = (0..50).map(|i| format!("pkg{i:02}/src/index.ts")).collect();
    let bases = basenames_of(&paths);
    let mixed = rank_files_and_dirs(None, &paths, &bases, "src", 50);
    assert!(
        mixed.iter().all(|h| h.dir),
        "mixed limit should fill with dirs"
    );
    let files = rank_hits(None, &paths, &bases, "src", 50, true);
    assert_eq!(files.len(), 50);
    for h in &files {
        assert!(!h.dir, "files-only marked dir: {h:?}");
        assert!(h.path.ends_with("/src/index.ts"), "{}", h.path);
    }
}

#[test]
fn rank_files_empty_query_and_cancel() {
    let paths = s(&["b", "a", "c"]);
    let hits = rank_files(None, &paths, &[], " ", 2);
    assert_eq!(
        hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert!(hits.iter().all(|h| h.score == 0));
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(rank_files(Some(&cancel), &paths, &[], "a", 2).is_empty());
}

#[test]
fn rank_files_ties_break_on_path() {
    let paths = s(&["b/x.go", "a/x.go", "c/x.go"]);
    let hits = rank_files(None, &paths, &basenames_of(&paths), "x.go", 2);
    assert_eq!(
        hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
        ["a/x.go", "b/x.go"]
    );
}
