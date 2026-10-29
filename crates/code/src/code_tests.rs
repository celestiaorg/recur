//! Port of `code_test.go`.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use recur_domain::{SearchOpts, TreeEntry};
use tokio_util::sync::CancellationToken;

use crate::drop_allow::{TEST_LOCK, set_max_external_allowed};
use crate::file::{
    MAX_CHAT_IMAGE_BYTES, MAX_EDIT_BYTES, MAX_IMAGE_BYTES, MAX_INLINE_BYTES, MAX_READ_BYTES,
    TEXT_PROBE_BYTES, preview_limit,
};
use crate::testutil::{git, tmp, write};
use crate::*;

const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn never() -> CancellationToken {
    CancellationToken::new()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn by_path(tree: Vec<TreeEntry>) -> HashMap<String, TreeEntry> {
    tree.into_iter().map(|e| (e.path.clone(), e)).collect()
}

fn sparse(path: &Path, len: u64) {
    fs::File::create(path).unwrap().set_len(len).unwrap();
}

#[test]
fn safe_join_rejects_escape() {
    let root = tmp();
    assert_eq!(
        safe_join(root.path(), "../outside")
            .unwrap_err()
            .to_string(),
        "path escapes root"
    );
    assert_eq!(
        safe_join(root.path(), "ok/file.go").unwrap(),
        root.path().join("ok/file.go")
    );
    assert_eq!(safe_join(root.path(), "/").unwrap(), root.path());
    assert_eq!(safe_join(root.path(), ".").unwrap(), root.path());
    assert!(safe_join(root.path(), "a/../../b").is_err());
}

#[tokio::test]
async fn tree_outline_search_symbols() {
    let root = tmp();
    let src = "package demo\n\nfunc Hello() {}\n\ntype Widget struct{}\n";
    write(root.path(), "demo.go", src);
    write(root.path(), "pkg/x.go", "package pkg\nfunc X() {}\n");
    write(root.path(), ".env", "X=1\n");
    fs::create_dir(root.path().join(".github")).unwrap();

    let tree = list_tree(root.path(), "", 2, "").await.unwrap();
    let names: Vec<&str> = tree.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&".env") && names.contains(&".github"),
        "tree should include hidden entries: {names:?}"
    );
    let pkg = tree.iter().find(|e| e.path == "pkg").unwrap();
    assert_eq!(pkg.children[0].path, "pkg/x.go", "depth 2 recurses");

    let out = outline(root.path(), "demo.go").unwrap();
    let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Hello", "Widget"]);
    assert_eq!((out[0].kind.as_str(), out[0].line), ("func", 3));

    let hits = search(&never(), root.path(), "Hello").unwrap().unwrap();
    assert_eq!(
        (hits[0].path.as_str(), hits[0].line, hits[0].col),
        ("demo.go", 3, 6)
    );

    let defs = find_defs(&never(), root.path(), "Hello").unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!((defs[0].path.as_str(), defs[0].line), ("demo.go", 3));
    let refs = find_refs(&never(), root.path(), "Hello").unwrap();
    assert!(refs.iter().any(|r| r.def && r.line == 3), "refs: {refs:?}");

    let fc = read_file(root.path(), "demo.go", true).unwrap();
    assert_eq!(fc.content, src);
    assert_eq!(fc.language, "go");
    write_file(root.path(), "demo.go", &format!("{src}\n")).unwrap();
}

#[test]
fn outline_major_languages() {
    let root = tmp();
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "a.py",
            "class Widget:\n    def render(self):\n        pass\n\ndef helper():\n    return 1\n",
            &["Widget", "render", "helper"],
        ),
        (
            "a.rs",
            "pub struct Widget {}\npub fn render() {}\npub enum Kind { A }\npub trait Drawable {}\n",
            &["Widget", "render", "Kind", "Drawable"],
        ),
        (
            "A.java",
            "public class Widget {\n  public void render() {}\n}\n",
            &["Widget", "render"],
        ),
        (
            "a.kt",
            "class Widget\nfun render() {}\n",
            &["Widget", "render"],
        ),
        (
            "a.swift",
            "struct Widget {}\nfunc render() {}\n",
            &["Widget", "render"],
        ),
        (
            "a.rb",
            "class Widget\n  def render\n  end\nend\nmodule Helpers\nend\n",
            &["Widget", "render", "Helpers"],
        ),
        (
            "a.php",
            "<?php\nclass Widget {}\nfunction render() {}\n",
            &["Widget", "render"],
        ),
        (
            "a.cs",
            "public class Widget {\n  public void Render() {}\n}\n",
            &["Widget", "Render"],
        ),
        (
            "a.cpp",
            "class Widget {};\nint render() { return 0; }\n",
            &["Widget", "render"],
        ),
        (
            "a.dart",
            "class Widget {}\nvoid render() {}\n",
            &["Widget", "render"],
        ),
        (
            "a.zig",
            "pub fn render() void {}\npub const MAX = 1;\n",
            &["render", "MAX"],
        ),
    ];
    for (name, src, want) in cases {
        write(root.path(), name, src);
        let got: Vec<String> = outline(root.path(), name)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        for sym in *want {
            assert!(
                got.iter().any(|g| g == sym),
                "outline({name}) missing {sym}: {got:?}"
            );
        }
    }
}

#[test]
fn outline_directory_and_images_are_empty() {
    let root = tmp();
    fs::create_dir(root.path().join("vendor-lib")).unwrap();
    assert!(outline(root.path(), "vendor-lib").unwrap().is_empty());
    write(root.path(), "icon.png", PNG);
    assert!(outline(root.path(), "icon.png").unwrap().is_empty());
    assert!(outline(root.path(), "missing.go").is_err());
}

#[test]
fn read_file_images() {
    let root = tmp();
    for (name, data, mime) in [
        ("icon.png", &PNG[..], "image/png"),
        ("photo.JPEG", &[0xff, 0xd8, 0xff, 0xd9][..], "image/jpeg"),
    ] {
        write(root.path(), name, data);
        let fc = read_file(root.path(), name, false).unwrap();
        assert_eq!(fc.mime, mime, "{name}");
        assert!(fc.content.is_empty(), "{name} inlined image bytes");
        assert_eq!(fc.size, data.len() as i64);
        assert!(fc.language.is_empty(), "{name} language for an image");
    }
    // SVG is editable text (preview is a UI concern), not a binary image.
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
    write(root.path(), "logo.svg", svg);
    let fc = read_file(root.path(), "logo.svg", true).unwrap();
    assert!(fc.mime.is_empty());
    assert_eq!(fc.content, svg);
    // Non-image binary is still rejected.
    write(root.path(), "blob.bin", [0x00, 0x01, 0x02, 0xff]);
    assert_eq!(
        read_file(root.path(), "blob.bin", false)
            .unwrap_err()
            .to_string(),
        "binary or non-UTF-8 file"
    );
}

#[test]
fn read_file_video() {
    let root = tmp();
    write(
        root.path(),
        "clip.MOV",
        [0, 0, 0, 0x14, b'f', b't', b'y', b'p'],
    );
    let fc = read_file(root.path(), "clip.MOV", false).unwrap();
    assert_eq!(fc.mime, "video/quicktime");
    assert!(fc.content.is_empty());
    assert!(
        preview_limit("video/mp4") > MAX_READ_BYTES,
        "video is not capped"
    );
}

#[test]
fn read_file_pdf() {
    let root = tmp();
    write(root.path(), "doc.pdf", "%PDF-1.4\n");
    let fc = read_file(root.path(), "doc.pdf", false).unwrap();
    assert_eq!(fc.mime, "application/pdf");
    assert!(fc.content.is_empty());
    assert_eq!(fc.size, 9);
    assert_eq!(image_mime("doc.pdf"), "");
    assert_eq!(preview_mime("doc.pdf"), "application/pdf");
}

#[test]
fn read_file_errors() {
    let root = tmp();
    assert_eq!(
        read_file(root.path(), "gone.txt", false)
            .unwrap_err()
            .to_string(),
        "not found: gone.txt"
    );
    fs::create_dir(root.path().join("d")).unwrap();
    assert_eq!(
        read_file(root.path(), "d", false).unwrap_err().to_string(),
        "d is a directory"
    );
    write(root.path(), "latin1.txt", [b'c', b'a', b'f', 0xe9]);
    assert_eq!(
        read_file(root.path(), "latin1.txt", false)
            .unwrap_err()
            .to_string(),
        "binary or non-UTF-8 file"
    );
}

#[tokio::test]
async fn image_at_base_reads_head() {
    let mut newer = PNG.to_vec();
    *newer.last_mut().unwrap() ^= 0xff;
    let root = tmp();
    git(root.path(), &["init", "-q", "-b", "main"]);
    write(root.path(), "icon.png", PNG);
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-q", "-m", "init"]);
    write(root.path(), "icon.png", &newer);

    let mut got = Vec::new();
    image_at_base(root.path(), "", "icon.png", Some(&mut got))
        .await
        .unwrap();
    assert_eq!(got, PNG, "want HEAD bytes, not the working tree");
    image_at_base::<Vec<u8>>(root.path(), "", "icon.png", None)
        .await
        .unwrap();

    let none: Option<&mut Vec<u8>> = None;
    assert_eq!(
        image_at_base(root.path(), "", "logo.svg", none)
            .await
            .unwrap_err()
            .to_string(),
        "not an image"
    );
    let err = image_at_base::<Vec<u8>>(root.path(), "", "missing.png", None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err}");
    assert!(
        image_at_base::<Vec<u8>>(root.path(), "", "../secret.png", None)
            .await
            .is_err()
    );
}

#[test]
fn prepare_chat_image_rules() {
    let _g = TEST_LOCK.lock();
    reset_external_paths();
    let dir = tmp();
    let png = dir.path().join("shot.png");
    fs::write(&png, PNG).unwrap();
    allow_external_path(s(&png));
    let (got, mime) = prepare_chat_image(s(&png)).unwrap();
    assert_eq!((got.as_path(), mime), (png.as_path(), "image/png"));

    assert_eq!(
        prepare_chat_image("shot.png").unwrap_err().to_string(),
        "path must be absolute"
    );
    assert_eq!(
        prepare_chat_image(" ").unwrap_err().to_string(),
        "missing path"
    );
    assert_eq!(
        prepare_chat_image("/a\0b").unwrap_err().to_string(),
        "invalid path"
    );
    let missing = dir.path().join("missing.png");
    assert_eq!(
        prepare_chat_image(s(&missing)).unwrap_err().to_string(),
        "path not allowed"
    );
    allow_external_path(s(&missing));
    assert!(matches!(
        prepare_chat_image(s(&missing)),
        Err(Error::NotFound(_))
    ));

    let txt = dir.path().join("notes.txt");
    fs::write(&txt, "hi").unwrap();
    allow_external_path(s(&txt));
    assert_eq!(
        prepare_chat_image(s(&txt)).unwrap_err().to_string(),
        "not a chat image"
    );
    let fake = dir.path().join("fake.png");
    fs::write(&fake, "not a png").unwrap();
    allow_external_path(s(&fake));
    assert_eq!(
        prepare_chat_image(s(&fake)).unwrap_err().to_string(),
        "not a chat image"
    );
    let huge = dir.path().join("huge.png");
    sparse(&huge, MAX_CHAT_IMAGE_BYTES + 1);
    allow_external_path(s(&huge));
    assert!(
        prepare_chat_image(s(&huge))
            .unwrap_err()
            .to_string()
            .starts_with("file too large")
    );
    let sub = dir.path().join("sub.png");
    fs::create_dir(&sub).unwrap();
    allow_external_path(s(&sub));
    assert_eq!(
        prepare_chat_image(s(&sub)).unwrap_err().to_string(),
        "sub.png is a directory"
    );
    reset_external_paths();
}

#[test]
fn prepare_external_rules() {
    let _g = TEST_LOCK.lock();
    reset_external_paths();
    let dir = tmp();
    let csv = dir.path().join("export-111.csv");
    fs::write(&csv, "a,b\n1,2\n").unwrap();
    let secret = dir.path().join("secret.csv");
    fs::write(&secret, "a,b\n").unwrap();
    allow_external_path(s(&csv));
    let (got, mime) = prepare_external(s(&csv)).unwrap();
    assert_eq!((got.as_path(), mime), (csv.as_path(), ""));

    let pdf = dir.path().join("doc.pdf");
    fs::write(&pdf, "%PDF-1.4\n").unwrap();
    allow_external_path(s(&pdf));
    assert_eq!(prepare_external(s(&pdf)).unwrap().1, "application/pdf");

    assert_eq!(
        prepare_external(s(&secret)).unwrap_err().to_string(),
        "path not allowed"
    );
    assert!(prepare_external("export-111.csv").is_err());
    let missing = dir.path().join("missing.csv");
    allow_external_path(s(&missing));
    assert!(prepare_external(s(&missing)).is_err());
    allow_external_path(s(dir.path()));
    assert!(prepare_external(s(dir.path())).is_err());

    let fifo = dir.path().join("pipe.csv");
    let st = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(st.success());
    allow_external_path(s(&fifo));
    assert_eq!(
        prepare_external(s(&fifo)).unwrap_err().to_string(),
        "not a regular file"
    );
    reset_external_paths();
}

#[test]
fn external_allow_persists_across_reset() {
    let _g = TEST_LOCK.lock();
    reset_external_paths();
    let dir = tmp();
    let csv = dir.path().join("export-111.csv");
    fs::write(&csv, "a,b\n").unwrap();
    let allow_file = dir.path().join("external-allow.json");
    load_external_allow_file(s(&allow_file));
    allow_external_path(s(&csv));
    assert_eq!(
        fs::read_to_string(&allow_file).unwrap(),
        format!("[\"{}\"]", s(&csv))
    );
    reset_external_paths();
    assert!(prepare_external(s(&csv)).is_err(), "reset should forget");
    load_external_allow_file(s(&allow_file));
    prepare_external(s(&csv)).unwrap();
    reset_external_paths();
    load_external_allow_file("");
}

#[test]
fn external_allow_evicts_oldest() {
    let _g = TEST_LOCK.lock();
    reset_external_paths();
    set_max_external_allowed(2);
    let dir = tmp();
    let mk = |name: &str| {
        let p = dir.path().join(name);
        fs::write(&p, "x\n").unwrap();
        s(&p).to_string()
    };
    let (a, b, c) = (mk("a.csv"), mk("b.csv"), mk("c.csv"));
    let allow_file = dir.path().join("external-allow.json");
    load_external_allow_file(s(&allow_file));
    for p in [&a, &b, &c] {
        allow_external_path(p);
    }
    assert_eq!(
        prepare_external(&a).unwrap_err().to_string(),
        "path not allowed"
    );
    prepare_external(&b).unwrap();
    prepare_external(&c).unwrap();

    reset_external_paths();
    load_external_allow_file(s(&allow_file));
    assert!(
        prepare_external(&a).is_err(),
        "persisted list kept evicted a"
    );
    prepare_external(&b).unwrap();
    prepare_external(&c).unwrap();
    set_max_external_allowed(0);
    reset_external_paths();
    load_external_allow_file("");
}

#[test]
fn read_file_size_limit() {
    let root = tmp();
    sparse(&root.path().join("over.txt"), MAX_READ_BYTES as u64 + 1);
    assert_eq!(
        read_file(root.path(), "over.txt", false)
            .unwrap_err()
            .to_string(),
        format!("file too large ({} bytes)", MAX_READ_BYTES + 1)
    );
    sparse(&root.path().join("huge.png"), MAX_IMAGE_BYTES as u64 + 1);
    assert!(read_file(root.path(), "huge.png", false).is_err());
}

#[test]
fn read_file_inlines_only_small_text() {
    let root = tmp();
    let small = "a".repeat(64);
    write(root.path(), "small.txt", &small);
    let fc = read_file(root.path(), "small.txt", true).unwrap();
    assert_eq!(fc.content, small);
    assert!(fc.writable && fc.mtime != 0);

    let big = "b".repeat(MAX_INLINE_BYTES as usize + 1);
    write(root.path(), "big.txt", &big);
    let fc = read_file(root.path(), "big.txt", false).unwrap();
    assert!(fc.content.is_empty(), "large text was inlined");
    assert_eq!(fc.size, MAX_INLINE_BYTES + 1);
    assert!(!fc.language.is_empty(), "language for streamed text");
}

#[test]
fn read_file_accepts_utf8_split_at_probe_boundary() {
    let root = tmp();
    // 8191 ASCII bytes, then a 3-byte rune that straddles the probe cut.
    let body = format!("{}世 trailing", "a".repeat(TEXT_PROBE_BYTES - 1));
    write(root.path(), "cjk.txt", &body);
    assert_eq!(
        read_file(root.path(), "cjk.txt", false).unwrap().content,
        body
    );
    let mut f = fs::File::open(root.path().join("cjk.txt")).unwrap();
    rewind_binary_check(&mut f).unwrap();
    let mut back = String::new();
    std::io::Read::read_to_string(&mut f, &mut back).unwrap();
    assert_eq!(back, body, "rewound to the start");

    write(root.path(), "nul.txt", b"ab\0cd");
    let mut f = fs::File::open(root.path().join("nul.txt")).unwrap();
    assert!(rewind_binary_check(&mut f).is_err());
}

#[test]
fn prepare_serve_resolves_without_loading() {
    let root = tmp();
    write(root.path(), "a/b.txt", "x");
    let (abs, mime) = prepare_serve(root.path(), "a/b.txt").unwrap();
    assert_eq!((abs, mime), (root.path().join("a/b.txt"), ""));
    assert!(matches!(
        prepare_serve(root.path(), "gone"),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn write_file_rejects_over_edit_cap() {
    let root = tmp();
    let err = write_file(root.path(), "huge.txt", &"a".repeat(MAX_EDIT_BYTES + 1)).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("file too large to save ({} bytes)", MAX_EDIT_BYTES + 1)
    );
}

#[tokio::test]
async fn list_tree_annotates_git_status() {
    let root = tmp();
    git(root.path(), &["init", "-q", "-b", "main"]);
    write(root.path(), "tracked.go", "package t\n");
    write(root.path(), "pkg/x.go", "package pkg\n");
    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "-q", "-m", "init"]);
    // Modify, delete, add, untracked.
    write(root.path(), "tracked.go", "package t\nfunc X() {}\n");
    fs::remove_file(root.path().join("pkg/x.go")).unwrap();
    write(root.path(), "added.go", "package t\n");
    git(root.path(), &["add", "added.go"]);
    write(root.path(), "scratch.go", "package t\n");

    let tree = by_path(list_tree(root.path(), "", 1, "main").await.unwrap());
    assert_eq!(tree["tracked.go"].status, "m");
    assert_eq!(tree["added.go"].status, "a");
    assert_eq!(tree["scratch.go"].status, "u");

    let pkg = list_tree(root.path(), "pkg", 1, "main").await.unwrap();
    assert!(
        pkg.iter().any(|e| e.path == "pkg/x.go" && e.status == "d"),
        "deleted pkg/x.go missing: {pkg:?}"
    );
    let status = git_status(root.path(), "").await;
    assert_eq!(status.get("scratch.go").map(String::as_str), Some("u"));
    assert_eq!(
        file_at_base(root.path(), "main", "tracked.go")
            .await
            .unwrap(),
        "package t\n"
    );
    let diff = file_diff(root.path(), "main", "tracked.go").await.unwrap();
    assert_eq!(diff.path, "tracked.go");

    assert!(matches!(
        list_tree(root.path(), "nope", 1, "").await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn list_tree_annotates_ignored() {
    let root = tmp();
    git(root.path(), &["init", "-q", "-b", "main"]);
    write(root.path(), ".gitignore", "scratch.bin\nbuild/\n");
    write(root.path(), "tracked.go", "package t\n");
    write(root.path(), ".github/ci.yml", "name: ci\n");
    write(root.path(), "build/out.txt", "x\n");
    write(root.path(), "scratch.bin", "x");
    git(root.path(), &["add", ".gitignore", "tracked.go", ".github"]);
    git(root.path(), &["commit", "-q", "-m", "init"]);

    let tree = by_path(list_tree(root.path(), "", 1, "").await.unwrap());
    assert!(tree["scratch.bin"].ignored);
    assert!(tree["build"].ignored);
    assert!(!tree[".gitignore"].ignored);
    assert!(!tree[".github"].ignored);
    assert!(!tree["tracked.go"].ignored);

    let build = list_tree(root.path(), "build", 1, "").await.unwrap();
    assert!(
        build.iter().any(|e| e.path == "build/out.txt" && e.ignored),
        "{build:?}"
    );

    let batch = list_trees(
        root.path(),
        &["".into(), "/build".into(), ".".into(), "gone".into()],
        1,
        "",
    )
    .await
    .unwrap();
    assert_eq!(batch.keys().collect::<Vec<_>>(), ["", "build"]);
    let ignored = |key: &str, path: &str| {
        batch[key]
            .iter()
            .find(|e| e.path == path)
            .is_some_and(|e| e.ignored)
    };
    assert!(ignored("", "scratch.bin"));
    assert!(!ignored("", ".gitignore"));
    assert!(ignored("build", "build/out.txt"));
}

#[tokio::test]
async fn list_tree_treats_symlink_dirs_as_dirs() {
    let root = tmp();
    write(root.path(), "real/inside.go", "package real\n");
    write(root.path(), "file.go", "package t\n");
    std::os::unix::fs::symlink("real", root.path().join("link")).unwrap();
    std::os::unix::fs::symlink("file.go", root.path().join("filelink")).unwrap();
    std::os::unix::fs::symlink("missing", root.path().join("dangling")).unwrap();

    let tree = by_path(list_tree(root.path(), "", 1, "").await.unwrap());
    assert!(tree["real"].dir);
    assert!(tree["link"].dir, "symlink to dir");
    assert!(!tree["file.go"].dir);
    assert!(!tree["filelink"].dir, "symlink to file");
    assert!(!tree["dangling"].dir, "broken symlink");
    // Dirs first, then case-insensitive names.
    let order: Vec<String> = list_tree(root.path(), "", 1, "")
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(order, ["link", "real", "dangling", "file.go", "filelink"]);

    let kids = list_tree(root.path(), "link", 1, "").await.unwrap();
    assert!(kids.iter().any(|e| e.path == "link/inside.go" && !e.dir));
    let deep = list_tree(root.path(), "", 2, "").await.unwrap();
    let link = deep.iter().find(|e| e.path == "link").unwrap();
    assert!(link.children.iter().any(|e| e.path == "link/inside.go"));
}

#[test]
fn resolve_root_prefers_worktree() {
    let repo = tmp();
    let wt = tmp();
    assert_eq!(
        resolve_root(s(repo.path()), s(wt.path())).unwrap(),
        wt.path()
    );
    assert_eq!(
        resolve_root_info(s(repo.path()), "").unwrap(),
        (repo.path().to_path_buf(), false)
    );
    assert_eq!(
        resolve_root("", "").unwrap_err().to_string(),
        "no repository path"
    );
    let missing = repo.path().join("nope");
    assert_eq!(
        resolve_root(s(&missing), "").unwrap_err().to_string(),
        format!("repository path \"{}\" is not a directory", s(&missing))
    );
}

#[test]
fn resolve_root_info_missing_worktree_does_not_fall_back() {
    let repo = tmp();
    write(repo.path(), "only-in-repo.txt", "x\n");
    let missing = repo.path().join("gone-wt");
    let err = resolve_root_info(s(repo.path()), s(&missing)).unwrap_err();
    assert!(matches!(err, Error::WorktreeMissing(_)), "{err}");
    // The UI's isWorktreeMissing sniffs this text.
    assert!(err.to_string().starts_with("worktree is not a directory"));
}

#[test]
fn write_file_is_atomic_and_preserves_mode() {
    let root = tmp();
    let script = root.path().join("run.sh");
    recur_exec::write_executable(&script, "#!/bin/sh\necho old\n", 0o755).unwrap();
    write_file(root.path(), "run.sh", "#!/bin/sh\necho new\n").unwrap();
    assert_eq!(
        fs::read_to_string(&script).unwrap(),
        "#!/bin/sh\necho new\n"
    );
    assert_eq!(
        fs::metadata(&script).unwrap().permissions().mode() & 0o777,
        0o755
    );

    write_file(root.path(), "sub/dir/new.txt", "hello").unwrap();
    let new = root.path().join("sub/dir/new.txt");
    assert_eq!(fs::read_to_string(&new).unwrap(), "hello");
    assert_eq!(
        fs::metadata(&new).unwrap().permissions().mode() & 0o777,
        0o644
    );

    for e in fs::read_dir(root.path()).unwrap() {
        let name = e.unwrap().file_name();
        assert!(
            !name.to_string_lossy().contains(".tmp"),
            "temp file left behind: {name:?}"
        );
    }
}

/// The walk root is never skipped, even when its name starts with a dot (a temp dir such as
/// `/tmp/.tmpXXXX`), as `files.rs` already did. Go's `search.go` and `symbols.go` skip it.
#[test]
fn search_replace_and_symbols_walk_a_dot_named_root() {
    let tmp = tmp();
    let root = tmp.path().join(".dotroot");
    write(&root, "a.go", "package a\n\nfunc Hello() {}\n");
    write(&root, ".hidden/b.go", "func Hello() {}\n");
    let hits = search(&never(), &root, "hello")
        .unwrap()
        .unwrap_or_default();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(find_defs(&never(), &root, "Hello").unwrap().len(), 1);
    let replaced = replace_in_files(&never(), &root, "Hello", "Bye", &SearchOpts::default());
    assert_eq!(replaced.unwrap().files_changed, 1);
    assert!(
        fs::read_to_string(root.join(".hidden/b.go"))
            .unwrap()
            .contains("Hello")
    );
}

#[test]
fn search_opts_pages_and_matches_case() {
    let root = tmp();
    write(root.path(), "a.go", "Foo\nfoo\nFOO\n");
    let all = search_opts(&never(), root.path(), "foo", &SearchOpts::default()).unwrap();
    assert_eq!(all.hits.as_ref().unwrap().len(), 3);
    let exact = search_opts(
        &never(),
        root.path(),
        "foo",
        &SearchOpts {
            case_sensitive: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(exact.hits.unwrap()[0].line, 2);
    let page = search_opts(
        &never(),
        root.path(),
        "foo",
        &SearchOpts {
            limit: 1,
            offset: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let hits = page.hits.unwrap();
    assert_eq!((hits.len(), hits[0].line, page.truncated), (1, 2, true));
    let empty = search_opts(&never(), root.path(), "  ", &SearchOpts::default()).unwrap();
    assert!(empty.hits.is_none());
    let none = search_opts(&never(), root.path(), "zzz", &SearchOpts::default()).unwrap();
    assert!(none.hits.is_none() && !none.truncated);
    let bad = search_opts(
        &never(),
        root.path(),
        "(",
        &SearchOpts {
            regex: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        bad.to_string()
            .starts_with("invalid regex: regex parse error"),
        "{bad}"
    );
}

#[test]
fn match_files_and_extract_idents() {
    let paths: Vec<String> = ["pkg/x.go", "README.md", "src/Main.rs"]
        .map(String::from)
        .to_vec();
    assert_eq!(match_files(&paths, "", 2), ["pkg/x.go", "README.md"]);
    assert_eq!(match_files(&paths, "MAIN", 0), ["src/Main.rs"]);
    assert_eq!(match_files(&paths, "pxg", 0), ["pkg/x.go"]);
    assert_eq!(
        extract_idents("let Foo = barBaz(Foo, ab, xyzw)"),
        ["Foo", "barBaz", "xyzw"]
    );
}
