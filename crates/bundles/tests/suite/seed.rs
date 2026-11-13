//! Bundle seeding, and seeding through a test run.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

use recur_bundles::*;

use crate::common::*;

fn map(entries: &[(&str, &str)]) -> MapSource {
    MapSource(
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>(),
    )
}

const DEMO_V1: &str = "[bundle]\nname = \"Demo\"\nid = \"d1\"\nversion = \"1.0.0\"\n[[command]]\nid = \"c1\"\nname = \"Hi\"\nbody = \"#!/bin/sh\\necho hi\\n\"\n";

#[test]
fn seed_defaults_idempotent() {
    let src = map(&[
        ("Demo/bundle.toml", DEMO_V1),
        ("Demo/src/a.ts", "export default (ctx) => ctx.input"),
    ]);
    let (_g, dest) = temp_dir();
    seed_defaults(&dest, &src, ".").unwrap();
    let stamp1 = std::fs::read_to_string(format!("{dest}/Demo/.recur-seed")).unwrap();
    seed_defaults(&dest, &src, ".").unwrap();
    let stamp2 = std::fs::read_to_string(format!("{dest}/Demo/.recur-seed")).unwrap();
    assert_eq!(stamp1, stamp2);
    assert_eq!(stamp1.len(), 65);
}

#[test]
fn seed_defaults_preserves_local_edits_until_ship_changes() {
    let src = map(&[("Demo/bundle.toml", DEMO_V1)]);
    let (_g, dest) = temp_dir();
    seed_defaults(&dest, &src, ".").unwrap();
    let p = format!("{dest}/Demo/bundle.toml");
    std::fs::write(&p, "edited locally").unwrap();
    seed_defaults(&dest, &src, ".").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "edited locally");
}

#[test]
fn seed_defaults_prunes_renamed_bundles_but_keeps_user_dirs() {
    let (_g, dest) = temp_dir();
    seed_defaults(&dest, &map(&[("Loop/bundle.toml", DEMO_V1)]), ".").unwrap();
    std::fs::create_dir_all(format!("{dest}/Mine")).unwrap();
    seed_defaults(&dest, &map(&[("Workflows/bundle.toml", DEMO_V1)]), ".").unwrap();
    let exists = |n: &str| std::path::Path::new(&format!("{dest}/{n}")).exists();
    assert!(!exists("Loop") && exists("Workflows/bundle.toml") && exists("Mine"));
}

#[test]
fn seed_defaults_refreshes_when_shipped_hash_changes() {
    let src = map(&[("Demo/bundle.toml", DEMO_V1), ("Demo/extra.txt", "old")]);
    let (_g, dest) = temp_dir();
    seed_defaults(&dest, &src, ".").unwrap();
    std::fs::write(format!("{dest}/Demo/stale.txt"), "gone").unwrap();
    let src2 = map(&[
        (
            "Demo/bundle.toml",
            "[bundle]\nname = \"Demo\"\nid = \"d1\"\nversion = \"1.1.0\"\n[[command]]\nid = \"c1\"\nname = \"Hi\"\nbody = \"#!/bin/sh\\necho v2\\n\"\n",
        ),
        ("Demo/extra.txt", "new"),
    ]);
    seed_defaults(&dest, &src2, ".").unwrap();
    let got = std::fs::read_to_string(format!("{dest}/Demo/bundle.toml")).unwrap();
    assert!(got.contains("1.1.0") && got.contains("echo v2"));
    assert!(!std::path::Path::new(&format!("{dest}/Demo/stale.txt")).exists());
    assert_eq!(
        std::fs::read_to_string(format!("{dest}/Demo/extra.txt")).unwrap(),
        "new"
    );
}

#[test]
fn seed_defaults_repo_bundles() {
    let (_g, dest) = temp_dir();
    seed_defaults(&dest, &DirSource(shipped_root()), ".").unwrap();
    for rel in [
        "Text/bundle.toml",
        "Text/.recur-seed",
        "Diff/bin/diff-clipboard.sh",
    ] {
        assert!(
            std::path::Path::new(&format!("{dest}/{rel}")).exists(),
            "{rel}"
        );
    }
    let mode = std::fs::metadata(format!("{dest}/Diff/bin/diff-clipboard.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
    assert!(!std::path::Path::new(&format!("{dest}/Text/tests")).exists());
}

#[test]
fn seed_defaults_embedded_matches_repo_stamps() {
    // The embedded tree must hash exactly like the repo tree (same stamps),
    // so startup never reseeds needlessly.
    let (_g1, from_dir) = temp_dir();
    let (_g2, from_embed) = temp_dir();
    seed_defaults(&from_dir, &DirSource(shipped_root()), ".").unwrap();
    seed_defaults(&from_embed, &SHIPPED_BUNDLES, ".").unwrap();
    let mut n = 0;
    for e in std::fs::read_dir(&from_dir).unwrap().flatten() {
        let stamp = e.path().join(".recur-seed");
        if stamp.exists() {
            let name = e.file_name().to_string_lossy().into_owned();
            let a = std::fs::read_to_string(&stamp).unwrap();
            let b = std::fs::read_to_string(format!("{from_embed}/{name}/.recur-seed")).unwrap();
            assert_eq!(a, b, "{name}");
            n += 1;
        }
    }
    assert!(n > 10);
    let (_g3, skills) = temp_dir();
    seed_defaults(&skills, &SHIPPED_SKILLS, ".").unwrap();
    assert!(std::path::Path::new(&format!("{skills}/create-bundle")).exists());
}

#[test]
fn seed_defaults_skips_tests_dir() {
    let src = map(&[
        ("Demo/bundle.toml", DEMO_V1),
        (
            "Demo/tests/case.toml",
            "[[test]]\nname = \"n\"\ncommand = \"c1\"\n[test.expect]\nstdout = \"hi\\n\"\n",
        ),
        ("Demo/src/a.ts", "export default (ctx) => ctx.input"),
    ]);
    let (_g, dest) = temp_dir();
    seed_defaults(&dest, &src, ".").unwrap();
    assert!(!std::path::Path::new(&format!("{dest}/Demo/tests")).exists());
    assert!(std::path::Path::new(&format!("{dest}/Demo/src/a.ts")).exists());
}

#[test]
fn seed_root_files_overwrite_on_change() {
    let (_g, dest) = temp_dir();
    seed_defaults(
        &dest,
        &map(&[
            ("tsconfig.json", "{}"),
            ("run.sh", "#!/bin/sh\n"),
            (".hidden", "x"),
        ]),
        ".",
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{dest}/tsconfig.json")).unwrap(),
        "{}"
    );
    let mode = std::fs::metadata(format!("{dest}/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
    assert!(!std::path::Path::new(&format!("{dest}/.hidden")).exists());
    seed_defaults(&dest, &map(&[("tsconfig.json", "{\"a\":1}")]), ".").unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{dest}/tsconfig.json")).unwrap(),
        "{\"a\":1}"
    );
}
