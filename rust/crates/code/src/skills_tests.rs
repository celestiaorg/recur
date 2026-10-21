//! Port of `skills_test.go`.

use std::fs;
use std::path::Path;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::testutil::tmp;

fn write_skill(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("SKILL.md"), body).unwrap();
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn list_skills_union_roots() {
    let wt = tmp();
    let home = tmp();
    write_skill(
        &wt.path().join(".claude/skills"),
        "design-audit",
        "---\nname: design-audit\ndescription: >-\n  Audit UI polish.\n---\n# Design\n",
    );
    write_skill(
        &home.path().join(".cursor/skills"),
        "create-rule",
        "---\nname: create-rule\ndescription: \"Make a rule\"\n---\n",
    );
    write_skill(
        &home.path().join(".claude/skills"),
        "design-audit",
        "---\nname: design-audit\ndescription: user copy\n---\n",
    );
    write_skill(
        &wt.path().join(".cursor/skills"),
        "BadName",
        "---\nname: BadName\ndescription: no\n---\n",
    );
    write_skill(
        &wt.path().join(".cursor/skills"),
        "land-it",
        "---\nname: land-it\ndescription: Ship the PR\n---\n",
    );
    let got = list_skills_in(&CancellationToken::new(), s(wt.path()), s(home.path())).unwrap();
    let scopes: Vec<(&str, &str)> = got
        .iter()
        .map(|s| (s.name.as_str(), s.scope.as_str()))
        .collect();
    assert_eq!(
        scopes,
        [
            ("create-rule", "user"),
            ("design-audit", "workspace"),
            ("land-it", "workspace")
        ]
    );
    let audit = &got[1];
    assert_eq!(audit.description, "Audit UI polish.");
    assert!(audit.path.ends_with("SKILL.md"));
    assert!(audit.user_invocable);
}

#[test]
fn parse_description() {
    assert_eq!(parse_frontmatter_description("description: hello world\n"), "hello world");
    assert_eq!(
        parse_frontmatter_description("description: >\n  line one\n  line two\nname: x\n"),
        "line one line two"
    );
    assert_eq!(
        parse_frontmatter_description("description: |\n\n  after blank\n\n  not this\n"),
        "after blank"
    );
    assert_eq!(parse_frontmatter_description("name: x\n"), "");
}

#[test]
fn read_skill_body_strips_frontmatter() {
    let dir = tmp();
    let path = dir.path().join("SKILL.md");
    fs::write(&path, "---\nname: x\ndescription: d\n---\n\n# Hello\n").unwrap();
    assert_eq!(read_skill_body(s(&path)), "# Hello");
    fs::write(&path, "\u{feff}no frontmatter\n").unwrap();
    assert_eq!(read_skill_body(s(&path)), "\u{feff}no frontmatter");
    assert_eq!(read_skill_body("/nonexistent/SKILL.md"), "");
}

#[test]
fn list_skills_keeps_model_only() {
    let wt = tmp();
    let home = tmp();
    write_skill(
        &home.path().join(".claude/skills"),
        "hidden",
        "---\nname: hidden\ndescription: nope\nuser-invocable: false\ndisable-model-invocation: yes\n---\n# Hidden\n",
    );
    let got = list_skills_in(&CancellationToken::new(), s(wt.path()), s(home.path())).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "hidden");
    assert!(!got[0].user_invocable);
    assert!(skill_disables_model_invocation(&got[0].path));
}

#[test]
fn truncate_skill_desc_utf8() {
    let got = truncate_skill_desc(&"é".repeat(200), 160);
    assert!(got.ends_with('…'));
    assert_eq!(got.chars().count(), 80);
    assert_eq!(truncate_skill_desc("  a \n b ", 160), "a b");
}

#[test]
fn skill_resources_and_lookup() {
    let wt = tmp();
    let dir = wt.path().join(".agents/skills/tool");
    write_skill(&wt.path().join(".agents/skills"), "tool", "---\ndescription: d\n---\nbody\n");
    for rel in ["scripts/run.sh", "refs/a.md", "node_modules/x.js"] {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    }
    let skill_md = dir.join("SKILL.md");
    assert_eq!(
        list_skill_resources(s(&skill_md), 0),
        ["refs/a.md", "scripts/run.sh"]
    );
    assert_eq!(list_skill_resources(s(&skill_md), 1).len(), 1);
    let cancel = CancellationToken::new();
    assert_eq!(
        list_skills_in(&cancel, s(wt.path()), "").unwrap()[0].name,
        "tool"
    );
    assert!(skill_by_name(&cancel, s(wt.path()), "Bad Name").unwrap().is_none());
}
