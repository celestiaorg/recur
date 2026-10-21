//! Go `skill_mentions_test.go` (worktree-scope cases; the `$HOME` cases run
//! in `tests/skills_home.rs`, which owns its process env).

use std::path::Path;

use recur_agent::TurnRequest;

use super::*;

fn write_skill(root: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let dir = root.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("SKILL.md");
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn collects_mentions() {
    assert_eq!(
        collect_skill_mentions("/go-code-review please and /go-code-review again then /other"),
        ["go-code-review", "other"]
    );
    assert!(collect_skill_mentions("see /Users/josh/file.go").is_empty());
}

#[test]
fn wraps_activated_skills() {
    let wt = tempfile::tempdir().unwrap();
    let p = write_skill(
        wt.path(),
        "recur-t-go-review",
        "---\nname: recur-t-go-review\ndescription: Review Go changes.\n---\n# Go review\n\nCheck error handling.\n",
    );
    let skill_dir = p.parent().unwrap();
    std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
    std::fs::write(skill_dir.join("scripts/lint.sh"), "#!/bin/sh\n").unwrap();
    let w = wt.path().to_string_lossy();
    let input = "/recur-t-go-review look at internal/engine";
    let out = wrap_activated_skills(input, &w, None);
    let dir = recur_bundles::go::dir(&p.to_string_lossy());
    assert_eq!(
        out,
        format!(
            "<system_instruction>\nThe user activated skill(s) via /name. Follow each skill's instructions below.\nDo not search for these skills or re-load them — their content is already included.\n</system_instruction>\n\n<skill_content name=\"recur-t-go-review\">\n# Go review\n\nCheck error handling.\n\nSkill directory: {dir}\nRelative paths in this skill are relative to the skill directory.\n\n<skill_resources>\n  <file>scripts/lint.sh</file>\n</skill_resources>\n</skill_content>\n\nlook at internal/engine"
        )
    );
    assert_eq!(
        wrap_activated_skills("/recur-t-no-such-skill hi", &w, None),
        "/recur-t-no-such-skill hi"
    );
    let off = |n: &str| n != "recur-t-go-review";
    let dis = wrap_activated_skills(input, &w, Some(&off));
    assert!(!dis.contains("<skill_content name=\"recur-t-go-review\">"));
    assert!(!dis.contains("/recur-t-go-review"));
    assert!(dis.contains(
        "These skill mentions were ignored because they are disabled in Settings/Project: recur-t-go-review.\nDo not search for or load these skills."
    ));
    assert!(dis.contains("look at internal/engine"));
}

#[test]
fn adjacent_mentions() {
    let wt = tempfile::tempdir().unwrap();
    let bodies = [
        ("recur-t-sol", "SOLIDITY BODY"),
        ("recur-t-audit", "AUDIT BODY"),
        ("recur-t-gorev", "GO REVIEW BODY"),
    ];
    for (n, b) in bodies {
        write_skill(
            wt.path(),
            n,
            &format!("---\nname: {n}\ndescription: x.\n---\n{b}\n"),
        );
    }
    let w = wt.path().to_string_lossy();
    for (input, want) in [
        (
            "review vs origin/main /recur-t-sol /recur-t-audit",
            &["recur-t-sol", "recur-t-audit"][..],
        ),
        (
            "review vs origin/main /recur-t-sol /recur-t-audit /recur-t-gorev",
            &["recur-t-sol", "recur-t-audit", "recur-t-gorev"][..],
        ),
    ] {
        let out = wrap_activated_skills(input, &w, None);
        for (n, b) in bodies {
            let tag = format!("<skill_content name=\"{n}\">");
            assert_eq!(out.contains(&tag), want.contains(&n), "{input}: {n}");
            assert_eq!(out.contains(b), want.contains(&n), "{input}: {n} body");
        }
        assert!(out.contains("review vs origin/main"));
        let user = &out[out.rfind("</skill_content>").unwrap()..];
        for n in want {
            assert!(
                !user.contains(&format!("/{n}")),
                "{input}: /{n} left in body"
            );
        }
    }
}

#[test]
fn slash_activates_disable_model_invocation() {
    let wt = tempfile::tempdir().unwrap();
    write_skill(
        wt.path(),
        "recur-t-create-rule",
        "---\nname: recur-t-create-rule\ndescription: Make a rule\ndisable-model-invocation: true\n---\n# Create\n",
    );
    let act = wrap_activated_skills(
        "/recur-t-create-rule do it",
        &wt.path().to_string_lossy(),
        None,
    );
    assert!(act.contains("<skill_content name=\"recur-t-create-rule\">"));
}

#[test]
fn skips_non_invocable() {
    let wt = tempfile::tempdir().unwrap();
    write_skill(
        wt.path(),
        "recur-t-hidden",
        "---\nname: recur-t-hidden\ndescription: model only\nuser-invocable: false\n---\n# Hidden\n",
    );
    let input = "/recur-t-hidden please";
    assert_eq!(
        wrap_activated_skills(input, &wt.path().to_string_lossy(), None),
        input
    );
}

#[test]
fn join_append() {
    assert_eq!(join_append_prompt("a", "b"), "a\n\nb");
    assert_eq!(join_append_prompt("", "b"), "b");
    assert_eq!(join_append_prompt(" a ", " "), "a");
}

#[test]
fn skill_guidance_once() {
    let mut req = TurnRequest {
        allowed_tools: Some(vec![tool::LIST_SKILLS.into()]),
        ..TurnRequest::default()
    };
    apply_skill_guidance(&mut req);
    assert_eq!(req.append_system_prompt, SKILLS_TOOLS_APPEND_PROMPT);
    apply_skill_guidance(&mut req);
    assert_eq!(
        req.append_system_prompt.matches(tool::LIST_SKILLS).count(),
        1
    );
    let mut none = TurnRequest::default();
    apply_skill_guidance(&mut none);
    assert!(none.append_system_prompt.is_empty());
}

#[test]
fn comment_hygiene_on_write_capable_turns_only() {
    let mut coder = TurnRequest {
        allowed_tools: Some(vec!["Read".into(), "Edit".into()]),
        append_system_prompt: "notes".into(),
        ..TurnRequest::default()
    };
    apply_comment_hygiene(&mut coder);
    assert_eq!(
        coder.append_system_prompt,
        format!("notes\n\n{COMMENT_HYGIENE_APPEND_PROMPT}")
    );
    apply_comment_hygiene(&mut coder);
    assert_eq!(
        coder
            .append_system_prompt
            .matches(COMMENT_HYGIENE_NEEDLE)
            .count(),
        1
    );
    for tools in [None, Some(vec!["Read".into(), "Bash".into()])] {
        let mut ro = TurnRequest {
            allowed_tools: tools,
            ..TurnRequest::default()
        };
        apply_comment_hygiene(&mut ro);
        assert!(ro.append_system_prompt.is_empty());
    }
}

#[test]
fn strip_preserves_indentation() {
    let got = strip_activated_skill_mentions(
        "/go-code-review please\n\ndef main():\n    print('hi')\n",
        &["go-code-review".into()],
    );
    assert!(got.contains("    print('hi')"));
    assert!(!got.contains("/go-code-review"));
}

#[test]
fn strip_adjacent() {
    let names: Vec<String> = [
        "solidity-security",
        "smart-contract-audit",
        "go-code-review",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    for (input, want) in [
        ("/solidity-security /smart-contract-audit", ""),
        (
            "review vs origin/main /solidity-security /smart-contract-audit",
            "review vs origin/main",
        ),
        ("/solidity-security /smart-contract-audit\nkeep", "keep"),
        ("/solidity-security please /smart-contract-audit", "please"),
        ("x /solidity-security /smart-contract-audit y", "x y"),
        (
            "/solidity-security /smart-contract-audit /go-code-review",
            "",
        ),
        (
            "review vs origin/main /solidity-security /smart-contract-audit /go-code-review",
            "review vs origin/main",
        ),
        (
            "/solidity-security /smart-contract-audit /go-code-review\nkeep",
            "keep",
        ),
        (
            "x /solidity-security /smart-contract-audit /go-code-review y",
            "x y",
        ),
    ] {
        assert_eq!(
            strip_activated_skill_mentions(input, &names),
            want,
            "{input}"
        );
    }
}

#[test]
fn skill_content_truncates_long_bodies() {
    let body = "é".repeat(MAX_SKILL_BODY_BYTES);
    let out = format_skill_content("x", "/s/x/SKILL.md", &body);
    assert!(out.contains("\n\n…(skill truncated)…\n"));
    assert!(out.starts_with("<skill_content name=\"x\">\n"));
    assert!(out.contains("Skill directory: /s/x\n"));
}
