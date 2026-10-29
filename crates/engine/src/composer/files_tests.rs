//! Go `file_mentions_test.go`.

use std::path::Path;

use super::*;

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "x").unwrap();
}

fn root() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().to_string_lossy().into_owned();
    (d, r)
}

#[test]
fn collects_mentions() {
    let (d, r) = root();
    touch(d.path(), "PROGRESS.md");
    touch(d.path(), "frontend/src/lib/agentchat.ts");
    let got = collect_file_mentions(
        "how does @PROGRESS.md compare to @frontend/src/lib/agentchat.ts ?",
        &r,
    );
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].chip, "PROGRESS.md");
    assert!(got[0].abs.ends_with("PROGRESS.md"));
    assert_eq!(got[1].chip, "frontend/src/lib/agentchat.ts");
    let abs = d.path().join("DECISIONS.md");
    std::fs::write(&abs, "x").unwrap();
    let got = collect_file_mentions(&format!("see @{}", abs.display()), &r);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].chip, "DECISIONS.md", "absolute chips relativize");
}

#[test]
fn requires_boundary() {
    let (d, r) = root();
    touch(d.path(), "acme.io");
    touch(d.path(), "fixture.py");
    for input in [
        "email john@acme.io please",
        "use @pytest.fixture here",
        "import @sveltejs/kit",
    ] {
        assert!(collect_file_mentions(input, &r).is_empty(), "{input}");
    }
    assert_eq!(collect_file_mentions("see @acme.io", &r).len(), 1);
    assert!(collect_file_mentions("see @nope.md", &r).is_empty());
}

#[test]
fn dedupes() {
    let (d, r) = root();
    let readme = d.path().join("README.md");
    std::fs::write(&readme, "hi").unwrap();
    let input = format!(
        "see @README.md and again @README.md and also @{}",
        readme.display()
    );
    let got = collect_file_mentions(&input, &r);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].chip, "README.md");
    let wrapped = wrap_attached_file_mentions(&input, &r);
    assert_eq!(wrapped.matches(" as @README.md").count(), 1);
    assert!(wrapped.matches("@README.md").count() >= 2);
}

#[test]
fn wraps_with_sizes() {
    let (d, r) = root();
    std::fs::write(d.path().join("README.md"), "hello world").unwrap();
    let got = wrap_attached_file_mentions("what is @README.md ?", &r);
    let abs = std::fs::canonicalize(d.path().join("README.md")).unwrap();
    assert_eq!(
        got,
        format!(
            "<system_instruction>\nThe user has attached these files. Read them before proceeding.\n- {} as @README.md (11 B)\n</system_instruction>\n\nwhat is @README.md ?",
            abs.display()
        )
    );
    assert_eq!(
        wrap_attached_file_mentions("hello there", &r),
        "hello there"
    );
}

#[test]
fn line_spans() {
    let (d, r) = root();
    touch(d.path(), "frontend/src/lib/agentchat.ts");
    let got = collect_file_mentions("look at @frontend/src/lib/agentchat.ts:22-48 please", &r);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].chip, "frontend/src/lib/agentchat.ts:22-48");
    assert_eq!((got[0].start_line, got[0].end_line), (22, 48));
    assert!(got[0].abs.ends_with("agentchat.ts"));
    touch(d.path(), "PROGRESS.md");
    assert_eq!(
        collect_file_mentions("see @PROGRESS.md and @PROGRESS.md:3", &r).len(),
        2
    );
    let wrapped = wrap_attached_file_mentions("see @PROGRESS.md:3", &r);
    assert!(wrapped.contains(" as @PROGRESS.md:3 ("));
    assert!(wrapped.contains("focus on line 3"));
    let abs = d.path().join("frontend/src/lib/agentchat.ts");
    let got = collect_file_mentions(&format!("see @{}:10", abs.display()), &r);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].chip, "frontend/src/lib/agentchat.ts:10");
}

#[test]
fn split_chip() {
    let s = split_file_mention_chip;
    assert_eq!(s("src/a.ts:12-40"), ("src/a.ts".into(), 12, 40, 0, 0));
    assert_eq!(s("src/a.ts:12"), ("src/a.ts".into(), 12, 0, 0, 0));
    assert_eq!(s("src/a.ts"), ("src/a.ts".into(), 0, 0, 0, 0));
    assert_eq!(s("src/a.ts:0"), ("src/a.ts".into(), 0, 0, 0, 0));
    assert_eq!(s("src/a.ts:0-12"), ("src/a.ts".into(), 0, 0, 0, 0));
    assert_eq!(s("src/a.ts:12:5-12:18"), ("src/a.ts".into(), 12, 0, 5, 18));
    assert_eq!(s("src/a.ts:12:5-13:20"), ("src/a.ts".into(), 12, 13, 5, 20));
    assert_eq!(s("src/a.ts:12:5-18"), ("src/a.ts".into(), 12, 18, 0, 0));
    let (p, start, _, sc, ec) = s("src/a.ts:12:0-12:18");
    assert_eq!((p.as_str(), start, sc, ec), ("src/a.ts", 12, 0, 0));
}

#[test]
fn invalid_line_attaches_whole_file() {
    let (d, r) = root();
    touch(d.path(), "src/a.ts");
    let got = collect_file_mentions("see @src/a.ts:0 please", &r);
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].chip.as_str(), got[0].start_line), ("src/a.ts", 0));
}

#[test]
fn chip_variants_at_start_and_end() {
    let (d, r) = root();
    touch(d.path(), "src/a.ts");
    touch(d.path(), "PROGRESS.md");
    touch(d.path(), ".gitignore");
    for (input, chip, start, end) in [
        ("@src/a.ts", "src/a.ts", 0, 0),
        ("@src/a.ts:12", "src/a.ts:12", 12, 0),
        ("@src/a.ts:12-40", "src/a.ts:12-40", 12, 40),
        ("@PROGRESS.md", "PROGRESS.md", 0, 0),
        ("@.gitignore", ".gitignore", 0, 0),
        ("@src/a.ts:0", "src/a.ts", 0, 0),
        ("@src/a.ts please review", "src/a.ts", 0, 0),
        ("@src/a.ts:12 please review", "src/a.ts:12", 12, 0),
        ("@src/a.ts:12-40 please review", "src/a.ts:12-40", 12, 40),
        ("@PROGRESS.md please review", "PROGRESS.md", 0, 0),
        ("@.gitignore please review", ".gitignore", 0, 0),
        ("@src/a.ts:0 please review", "src/a.ts", 0, 0),
        ("please review @src/a.ts", "src/a.ts", 0, 0),
        ("please review @src/a.ts:12", "src/a.ts:12", 12, 0),
        ("please review @src/a.ts:12-40", "src/a.ts:12-40", 12, 40),
        ("compare @PROGRESS.md", "PROGRESS.md", 0, 0),
        ("ignore @.gitignore", ".gitignore", 0, 0),
        ("see @src/a.ts:0", "src/a.ts", 0, 0),
        ("explain this @src/a.ts please", "src/a.ts", 0, 0),
        (
            "explain this @src/a.ts:12-40 please",
            "src/a.ts:12-40",
            12,
            40,
        ),
        ("@src/a.ts:12:5-12:18", "src/a.ts:12:5-12:18", 12, 0),
        (
            "please review @src/a.ts:12:5-12:18",
            "src/a.ts:12:5-12:18",
            12,
            0,
        ),
        ("@src/a.ts:12:5-13:20", "src/a.ts:12:5-13:20", 12, 13),
        (
            "explain this @src/a.ts:12:5-13:20 please",
            "src/a.ts:12:5-13:20",
            12,
            13,
        ),
    ] {
        let got = collect_file_mentions(input, &r);
        assert_eq!(got.len(), 1, "{input}");
        assert_eq!(
            (got[0].chip.as_str(), got[0].start_line, got[0].end_line),
            (chip, start, end),
            "{input}"
        );
    }
    for input in [
        "explain this@src/a.ts",
        "this@src/a.ts:12-40",
        "this@src/a.ts:12:5-12:18",
    ] {
        assert!(collect_file_mentions(input, &r).is_empty(), "{input}");
    }
}

#[test]
fn column_span_notes() {
    let (d, r) = root();
    let p = d.path().join("src").join("a.ts");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "alpha\nhello world\ngamma\n").unwrap();
    let got = collect_file_mentions("see @src/a.ts:2:7-2:11", &r);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].chip, "src/a.ts:2:7-2:11");
    assert_eq!(
        (got[0].start_line, got[0].start_col, got[0].end_col),
        (2, 7, 11)
    );
    let wrapped = wrap_attached_file_mentions("see @src/a.ts:2:7-2:11", &r);
    assert!(wrapped.contains(" as @src/a.ts:2:7-2:11 ("));
    assert!(wrapped.contains("focus on line 2, columns 7–11"));
    assert!(!wrapped.contains("“world”"));
    let cross = wrap_attached_file_mentions("see @src/a.ts:1:2-3:4", &r);
    assert!(cross.contains("focus on line 1 column 2 through line 3 column 4"));
    assert!(!cross.contains("lines 1–3, columns"));
    let same = wrap_attached_file_mentions("see @src/a.ts:2:7-2:7", &r);
    assert!(same.contains("focus on line 2, column 7"));
}

#[test]
fn directory_mentions() {
    let (d, r) = root();
    std::fs::create_dir_all(d.path().join("src/lib")).unwrap();
    touch(d.path(), "src/main.go");
    let one = |input: &str| {
        let got = collect_file_mentions(input, &r);
        assert_eq!(got.len(), 1, "{input}");
        assert!(got[0].dir, "{input}");
        got[0].chip.clone()
    };
    assert_eq!(one("look at @src"), "src");
    assert_eq!(one("look at @src/lib"), "src/lib");
    assert!(collect_file_mentions("email josh@src please", &r).is_empty());
    assert!(collect_file_mentions("see @terminal", &r).is_empty());
    assert_eq!(one("look at @src/"), "src");
    std::fs::create_dir_all(d.path().join("terminal")).unwrap();
    assert_eq!(one("look at @terminal/"), "terminal");
    std::fs::create_dir_all(d.path().join(".github")).unwrap();
    std::fs::create_dir_all(d.path().join("foo.bar")).unwrap();
    assert_eq!(one("look at @.github/"), ".github");
    assert_eq!(one("look at @foo.bar/"), "foo.bar");
    let wrapped = wrap_attached_file_mentions("look at @src", &r);
    assert!(wrapped.contains(
        "The user pointed at these directories. Explore them as needed; do not dump the whole tree."
    ));
    assert!(!wrapped.contains("The user has attached these files"));
    assert!(wrapped.contains(" as @src"));
    assert!(wrapped.trim().ends_with("look at @src"));
    let both = wrap_attached_file_mentions("see @src and @src/main.go", &r);
    assert!(both.contains("The user has attached these files. Read them before proceeding."));
    assert!(both.contains("The user pointed at these directories"));
}

#[test]
fn attach_sizes() {
    assert_eq!(format_attach_size(512), "512 B");
    assert_eq!(format_attach_size(1536), "1.5 KB");
    assert_eq!(format_attach_size(20 * 1024), "20 KB");
    assert_eq!(format_attach_size(3 * 1024 * 1024 + 300_000), "3.3 MB");
    assert_eq!(format_attach_size(12 * 1024 * 1024), "12 MB");
}
