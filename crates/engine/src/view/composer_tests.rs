//! The composer text model (caret placement and focus are tested with the
//! GPUI view).

use super::*;

fn files() -> HydrateOpts<'static> {
    HydrateOpts {
        chip_files: true,
        ..HydrateOpts::default()
    }
}

fn round_trip(wire: &str, opts: &HydrateOpts<'_>) -> Vec<Vec<Segment>> {
    let lines = hydrate_composer(wire, &[], opts);
    assert_eq!(serialize_composer(&lines), wire, "round trip");
    lines
}

fn file_paths(lines: &[Vec<Segment>]) -> Vec<(String, bool)> {
    lines
        .iter()
        .flatten()
        .filter_map(|s| match s {
            Segment::File { path, dir } => Some((path.clone(), *dir)),
            _ => None,
        })
        .collect()
}

fn arg(name: &str) -> PromptArgSpec {
    PromptArgSpec {
        name: name.into(),
        ..PromptArgSpec::default()
    }
}

#[test]
fn file_chips_round_trip() {
    let lines = round_trip("see @src/a.ts:12-40 please", &files());
    assert_eq!(file_paths(&lines), [("src/a.ts:12-40".into(), false)]);
    assert_eq!(
        super::super::mentions::file_mention_label("src/a.ts:12-40"),
        "a.ts (12-40)"
    );
    let lines = round_trip("see @src/a.ts:12:5-12:18 please", &files());
    assert_eq!(file_paths(&lines), [("src/a.ts:12:5-12:18".into(), false)]);
    round_trip("@PROGRESS.md please review", &files());
    round_trip("please review @PROGRESS.md", &files());
}

#[test]
fn directory_chips_round_trip() {
    let lines = round_trip("look at @src/ and @src/lib/", &files());
    assert_eq!(
        file_paths(&lines),
        [("src/".into(), true), ("src/lib/".into(), true)]
    );
    let chip = vec![vec![Segment::File {
        path: ".github/".into(),
        dir: true,
    }]];
    assert_eq!(serialize_composer(&chip), "@.github/");
    let lines = round_trip("@.github/ and @foo.bar/", &files());
    assert_eq!(
        file_paths(&lines),
        [(".github/".into(), true), ("foo.bar/".into(), true)]
    );
    let texts: Vec<&Segment> = lines[0]
        .iter()
        .filter(|s| matches!(s, Segment::Text(_)))
        .collect();
    assert_eq!(texts, [&Segment::Text(" and ".into())]);
}

#[test]
fn reserved_directory_names_stay_file_chips() {
    let lines = round_trip("see @terminal/", &files());
    assert!(
        !lines[0]
            .iter()
            .any(|s| matches!(s, Segment::Terminal { .. }))
    );
    assert_eq!(file_paths(&lines), [("terminal/".into(), true)]);
}

#[test]
fn chip_files_hydrate_repairs_boundaries() {
    let lines = hydrate_composer("what does it do?@frontend/src/a.ts:12-40", &[], &files());
    assert_eq!(
        serialize_composer(&lines),
        "what does it do? @frontend/src/a.ts:12-40"
    );
    assert_eq!(
        file_paths(&lines),
        [("frontend/src/a.ts:12-40".into(), false)]
    );
}

#[test]
fn typing_mode_leaves_tokens_as_text() {
    let lines = hydrate_composer("see @src/a.ts and /pdf", &[], &HydrateOpts::default());
    assert_eq!(
        lines,
        [vec![Segment::Text("see @src/a.ts and /pdf".into())]]
    );
}

#[test]
fn artifact_chips() {
    let title = |id: &str| {
        if id == "abc-def" {
            "Notes".to_owned()
        } else {
            String::new()
        }
    };
    let opts = HydrateOpts {
        chip_files: true,
        artifact_title: Some(&title),
        ..HydrateOpts::default()
    };
    let lines = round_trip("see @artifact:abc-def", &opts);
    assert_eq!(
        lines[0][1],
        Segment::Artifact {
            id: "abc-def".into(),
            title: "Notes".into()
        }
    );
    let lines = round_trip("see @artifact", &files());
    assert!(matches!(&lines[0][1], Segment::Artifact { id, .. } if id.is_empty()));

    let lookup = |t: &str| {
        if t.to_lowercase() == "notes" {
            "abc-def".to_owned()
        } else {
            String::new()
        }
    };
    let opts = HydrateOpts {
        chip_files: true,
        artifact_id_for_title: Some(&lookup),
        artifact_title: Some(&title),
        ..HydrateOpts::default()
    };
    let lines = hydrate_composer("see @Notes please", &[], &opts);
    assert_eq!(serialize_composer(&lines), "see @artifact:abc-def please");
    assert!(lines[0].contains(&Segment::Artifact {
        id: "abc-def".into(),
        title: "Notes".into()
    }));
    let none = |_: &str| String::new();
    let opts = HydrateOpts {
        chip_files: true,
        artifact_id_for_title: Some(&none),
        ..HydrateOpts::default()
    };
    let lines = round_trip("ping @nobody", &opts);
    assert!(
        !lines[0]
            .iter()
            .any(|s| matches!(s, Segment::Artifact { .. }))
    );
}

#[test]
fn terminal_chips() {
    let title = |id: &str| {
        if id == "abc-def" {
            "dev".to_owned()
        } else {
            String::new()
        }
    };
    let opts = HydrateOpts {
        chip_files: true,
        terminal_title: Some(&title),
        ..HydrateOpts::default()
    };
    let lines = round_trip("fix @terminal:abc-def", &opts);
    assert_eq!(
        lines[0][1],
        Segment::Terminal {
            id: "abc-def".into(),
            title: "dev".into()
        }
    );
    let lines = round_trip("see @terminal", &files());
    assert!(matches!(&lines[0][1], Segment::Terminal { id, .. } if id.is_empty()));
}

#[test]
fn boundaries_after_terminal_and_artifact_chips() {
    for (chip, wire) in [
        (
            Segment::Artifact {
                id: "abc-def".into(),
                title: "Notes".into(),
            },
            "@artifact:abc-def",
        ),
        (
            Segment::Terminal {
                id: "abc-def".into(),
                title: "dev".into(),
            },
            "@terminal:abc-def",
        ),
    ] {
        let mut lines = vec![vec![chip, Segment::Text("why".into())]];
        assert_eq!(serialize_composer(&lines), format!("{wire}why"));
        ensure_chip_boundaries(&mut lines);
        assert_eq!(serialize_composer(&lines), format!("{wire} why"));
    }
}

#[test]
fn boundaries_before_chips() {
    let file = || Segment::File {
        path: "a.ts".into(),
        dir: false,
    };
    let mut lines = vec![vec![Segment::Text("see".into()), file()]];
    ensure_chip_boundaries(&mut lines);
    assert_eq!(serialize_composer(&lines), "see @a.ts");
    let mut lines = vec![vec![file(), Segment::Skill("pdf".into())]];
    ensure_chip_boundaries(&mut lines);
    assert_eq!(serialize_composer(&lines), "@a.ts /pdf");
    let mut lines = vec![vec![
        Segment::Text("see ".into()),
        Segment::Text("\u{200B}".into()),
        file(),
    ]];
    ensure_chip_boundaries(&mut lines);
    assert_eq!(
        serialize_composer(&lines),
        "see \u{200B}@a.ts",
        "fillers are skipped"
    );
}

fn linear() -> PromptChipSpec {
    PromptChipSpec {
        slash: "linear".into(),
        args: vec![PromptArgSpec {
            name: "issue".into(),
            placeholder: "TEAM-123".into(),
            optional: false,
        }],
    }
}

fn poll() -> PromptChipSpec {
    PromptChipSpec {
        slash: "poll".into(),
        args: vec![arg("question"), arg("choice_1"), arg("choice_2")],
    }
}

#[test]
fn prompt_chips() {
    let specs = [linear()];
    let opts = HydrateOpts {
        chip_skills: true,
        prompt_chips: &specs,
        ..HydrateOpts::default()
    };
    let lines = round_trip("/linear ENG-1", &opts);
    assert_eq!(
        lines[0],
        [Segment::Prompt {
            spec: linear(),
            values: vec!["ENG-1".into()]
        }]
    );
    round_trip("/linear ", &opts);
    let specs = [poll()];
    let opts = HydrateOpts {
        chip_skills: true,
        prompt_chips: &specs,
        ..HydrateOpts::default()
    };
    let lines = round_trip("/poll Which Red Blue", &opts);
    assert_eq!(
        lines[0],
        [Segment::Prompt {
            spec: poll(),
            values: vec!["Which".into(), "Red".into(), "Blue".into()]
        }]
    );
    let typed = vec![vec![Segment::Prompt {
        spec: poll(),
        values: vec!["Which color?".into(), "Red".into(), "Blue".into()],
    }]];
    let wire = serialize_composer(&typed);
    assert_eq!(wire, r#"/poll "Which color?" Red Blue"#);
    assert_eq!(hydrate_composer(&wire, &[], &opts), typed);
}

#[test]
fn skills_chip_without_a_prompt_spec() {
    let opts = HydrateOpts {
        chip_skills: true,
        ..HydrateOpts::default()
    };
    let lines = round_trip("use /pdf now", &opts);
    assert_eq!(lines[0][1], Segment::Skill("pdf".into()));
}

#[test]
fn attachments_hydrate_and_orphans_append() {
    let atts = [
        ComposerAtt {
            id: "img_a".into(),
            data_url: "data:image/png;base64,AA".into(),
            name: "a.png".into(),
        },
        ComposerAtt {
            id: "img_b".into(),
            data_url: "data:image/png;base64,BB".into(),
            name: "b.png".into(),
        },
    ];
    let lines = hydrate_composer(
        "see ⟦attachment:img_a⟧ and ⟦attachment:gone⟧\nnext",
        &atts,
        &HydrateOpts::default(),
    );
    assert_eq!(lines[0][1], Segment::Attachment(atts[0].clone()));
    assert_eq!(lines[0][2], Segment::Text(" and ⟦attachment:gone⟧".into()));
    assert_eq!(lines[1].last(), Some(&Segment::Attachment(atts[1].clone())));
    assert_eq!(
        serialize_composer(&lines),
        "see ⟦attachment:img_a⟧ and ⟦attachment:gone⟧\nnext⟦attachment:img_b⟧"
    );
}

#[test]
fn prompt_args_with_trailing() {
    let specs = [PromptChipSpec {
        slash: "linear".into(),
        args: vec![arg("issue")],
    }];
    let opts = HydrateOpts {
        prompt_chips: &specs,
        ..HydrateOpts::default()
    };
    let lines = hydrate_composer("/linear ENG-1", &[], &opts);
    assert_eq!(
        prompt_chip_args_with_trailing(&lines).as_deref(),
        Some("ENG-1")
    );
    let mut same_line = lines.clone();
    same_line[0].push(Segment::Text(" skip mobile".into()));
    assert_eq!(
        prompt_chip_args_with_trailing(&same_line).as_deref(),
        Some("ENG-1 skip mobile")
    );
    let lines = hydrate_composer("/linear ENG-1\nskip mobile\nkeep it small", &[], &opts);
    assert_eq!(
        prompt_chip_args_with_trailing(&lines).as_deref(),
        Some("ENG-1\nskip mobile\nkeep it small")
    );
    assert_eq!(prompt_chip_args_with_trailing(&[vec![]]), None);
}

#[test]
fn prompt_wire_encoding() {
    assert_eq!(encode_prompt_wire("/linear", &[]), "/linear ");
    assert_eq!(
        encode_prompt_wire("linear", &[" ENG-1 ".into()]),
        "/linear ENG-1"
    );
    assert_eq!(
        encode_prompt_wire("poll", &["a b".into(), "".into(), r#"q"x\"#.into()]),
        r#"/poll "a b" "" "q\"x\\""#
    );
    assert_eq!(
        split_prompt_arg_tokens(r#" "a b" c  "d\"e" "#),
        ["a b", "c", "d\"e"]
    );
    let spec = poll();
    assert_eq!(
        parse_prompt_chip_values("Which color? Red Blue extra", &spec),
        ["Which", "color?", "Red Blue extra"]
    );
    assert_eq!(parse_prompt_chip_values("", &spec), ["", "", ""]);
    assert_eq!(
        parse_prompt_chip_values("  all of it ", &linear()),
        ["all of it"]
    );
}
