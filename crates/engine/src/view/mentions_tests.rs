//! The mention cases of frontend `lib/agentchat.test.ts`.

use super::*;

fn chip(
    path: &str,
    start_line: i64,
    end_line: i64,
    start_col: i64,
    end_col: i64,
) -> FileMentionRef {
    FileMentionRef {
        path: path.into(),
        start_line,
        end_line,
        start_col,
        end_col,
    }
}

#[test]
fn same_user_chat_body_rules() {
    let marked = format!("{}pr settings alignment", attachment_marker("img_a"));
    assert!(same_user_chat_body("pr settings alignment", &marked));
    assert!(!same_user_chat_body(
        &format!("{}pr settings alignment", attachment_marker("img_a")),
        &format!("{}pr settings alignment", attachment_marker("img_b")),
    ));
}

#[test]
fn format_file_mention_chips() {
    assert_eq!(format_file_mention_chip("src/a.ts", 0, 0, 0, 0), "src/a.ts");
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 0, 0, 0),
        "src/a.ts:12"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 40, 0, 0),
        "src/a.ts:12-40"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 0, 5, 18),
        "src/a.ts:12:5-12:18"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 13, 5, 20),
        "src/a.ts:12:5-13:20"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 12, 5, 18),
        "src/a.ts:12:5-12:18"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 0, 5, 5),
        "src/a.ts:12:5-12:5"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", -1, 10, 0, 0),
        "src/a.ts"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 12, 0, 0),
        "src/a.ts:12"
    );
    assert_eq!(
        format_file_mention_chip("src/a.ts", 12, 5, 0, 0),
        "src/a.ts:12"
    );
    assert_eq!(
        format_file_mention_chip("  src/a.ts  ", 3, 0, 0, 0),
        "src/a.ts:3"
    );
    assert_eq!(format_file_mention_chip("", 0, 0, 0, 0), "");
    assert_eq!(format_file_mention_chip("   ", 0, 0, 0, 0), "");
}

#[test]
fn parse_file_mention_chips() {
    assert_eq!(
        parse_file_mention_chip("src/a.ts"),
        chip("src/a.ts", 0, 0, 0, 0)
    );
    assert_eq!(
        parse_file_mention_chip("src/a.ts:12"),
        chip("src/a.ts", 12, 0, 0, 0)
    );
    assert_eq!(
        parse_file_mention_chip("src/a.ts:12-40"),
        chip("src/a.ts", 12, 40, 0, 0)
    );
    assert_eq!(
        parse_file_mention_chip("src/a.ts:12:5-12:18"),
        chip("src/a.ts", 12, 0, 5, 18)
    );
    assert_eq!(
        parse_file_mention_chip("src/a.ts:12:5-13:20"),
        chip("src/a.ts", 12, 13, 5, 20)
    );
    assert_eq!(
        parse_file_mention_chip("src/a.ts:12:5-12:5"),
        chip("src/a.ts", 12, 0, 5, 5)
    );
    assert_eq!(
        parse_file_mention_chip("src/a.ts:12:5-18"),
        chip("src/a.ts", 12, 18, 0, 0),
        "line 12 col 5 through line 18, not same-line columns"
    );
    for c in ["src/a.ts:0", "src/a.ts:0-12", "src/a.ts:0:5-12:18"] {
        assert_eq!(
            parse_file_mention_chip(c),
            chip("src/a.ts", 0, 0, 0, 0),
            "{c}"
        );
    }
    for c in [
        "src/a.ts:12-12",
        "src/a.ts:12-5",
        "src/a.ts:12:0-12:18",
        "src/a.ts:12:8-12:3",
    ] {
        assert_eq!(
            parse_file_mention_chip(c),
            chip("src/a.ts", 12, 0, 0, 0),
            "{c}"
        );
    }
    assert_eq!(parse_file_mention_chip("src/"), chip("src", 0, 0, 0, 0));
    assert_eq!(parse_file_mention_chip(":12"), chip(":12", 0, 0, 0, 0));
}

#[test]
fn file_mention_labels() {
    assert_eq!(file_mention_label("src/a.ts"), "a.ts");
    assert_eq!(file_mention_label("src/a.ts:12"), "a.ts (12)");
    assert_eq!(file_mention_label("src/a.ts:12-40"), "a.ts (12-40)");
    assert_eq!(
        file_mention_label("src/a.ts:12:5-12:18"),
        "a.ts (12:5-12:18)"
    );
    assert_eq!(
        file_mention_label("src/a.ts:12:5-13:20"),
        "a.ts (12:5-13:20)"
    );
    assert_eq!(file_mention_label("PROGRESS.md"), "PROGRESS.md");
}

#[test]
fn file_mention_tokens() {
    assert_eq!(file_mention_token("src/a.ts:12"), "@src/a.ts:12");
    assert_eq!(
        remove_file_mention_token("how does @src/a.ts compare", "src/a.ts"),
        "how does compare"
    );
    assert_eq!(
        remove_file_mention_token("see @src/a.ts", "src/a.ts"),
        "see"
    );
    assert_eq!(
        remove_file_mention_token("@src/a.ts please", "src/a.ts"),
        "please"
    );
    assert_eq!(
        remove_file_mention_token("look at @src/a.ts:12-40 please", "src/a.ts:12-40"),
        "look at please"
    );
    assert_eq!(remove_file_mention_token("x", " "), "x");
}

fn q(start: usize, end: usize, query: &str) -> Option<MentionQuery> {
    Some(MentionQuery {
        start,
        end,
        query: query.into(),
    })
}

#[test]
fn caret_queries() {
    assert_eq!(mention_query_at("see @quer", 9), q(4, 9, "quer"));
    assert_eq!(mention_query_at("@src", 4), q(0, 4, "src"));
    assert_eq!(
        mention_query_at("john@acme", 9),
        None,
        "mid-token @ (email)"
    );
    assert_eq!(skill_query_at("run /foo", 8), q(4, 8, "foo"));
    assert_eq!(skill_query_at("/bar", 4), q(0, 4, "bar"));
    assert_eq!(skill_query_at("open /Users/x", 12), None, "absolute paths");
    assert_eq!(mention_query_at("é@x", 1), None, "not a char boundary");
    assert_eq!(mention_query_at("x", 9), None);
}

#[test]
fn strip_file_mentions_keeps_prose() {
    assert_eq!(
        js::trim(&strip_file_mentions("see @src/a.ts please")),
        "see  please"
    );
    assert_eq!(
        js::trim(&strip_file_mentions("see @src/a.ts:12-40 please")),
        "see  please"
    );
}

#[test]
fn repair_boundaries() {
    assert_eq!(
        repair_file_mention_boundaries("what does it do?@frontend/src/a.ts:1-2"),
        "what does it do? @frontend/src/a.ts:1-2"
    );
    assert_eq!(
        repair_file_mention_boundaries("look@src/a.ts:12:5-12:18"),
        "look @src/a.ts:12:5-12:18"
    );
    assert_eq!(
        repair_file_mention_boundaries("@src/a.ts:1@src/b.ts:2"),
        "@src/a.ts:1 @src/b.ts:2"
    );
    assert_eq!(
        repair_file_mention_boundaries("look@.github/"),
        "look @.github/"
    );
    assert_eq!(
        repair_file_mention_boundaries("see@foo.bar/"),
        "see @foo.bar/"
    );
    assert_eq!(
        repair_file_mention_boundaries("email john@acme.io please"),
        "email john@acme.io please"
    );
    assert_eq!(
        repair_file_mention_boundaries("see @src/a.ts please"),
        "see @src/a.ts please"
    );
}

fn insert(text: &str, caret: usize, strip: bool) -> (String, String) {
    prepare_file_mention_insert(text, caret, strip)
}

#[test]
fn prepare_insert() {
    assert_eq!(
        insert("explain this", 12, true),
        ("explain this ".into(), String::new())
    );
    assert_eq!(
        insert("explain this ", 13, true),
        ("explain this ".into(), String::new())
    );
    assert_eq!(insert("", 0, true), (String::new(), String::new()));
    assert_eq!(insert("see @quer", 9, true), ("see ".into(), String::new()));
    assert_eq!(insert("run /foo", 8, true), ("run ".into(), String::new()));
    assert_eq!(
        insert("hello world", 5, true),
        ("hello ".into(), " world".into())
    );
    let text = "see @src/a.ts";
    assert_eq!(
        insert(text, text.len(), true),
        ("see ".into(), String::new())
    );
    assert_eq!(
        insert(text, text.len(), false),
        ("see @src/a.ts ".into(), String::new())
    );
}

#[test]
fn extract_file_mentions_at_every_position() {
    for c in [
        "src/a.ts",
        "src/a.ts:12",
        "src/a.ts:12-40",
        "src/a.ts:12:5-12:18",
        "src/a.ts:12:5-13:20",
        "PROGRESS.md",
        ".gitignore",
    ] {
        assert_eq!(extract_file_mentions(&format!("@{c}")), [c], "sole {c}");
        assert_eq!(format_file_mention_ref(&parse_file_mention_chip(c)), c);
        assert_eq!(
            extract_file_mentions(&format!("@{c} please review")),
            [c],
            "start {c}"
        );
        assert_eq!(
            extract_file_mentions(&format!("please review @{c}")),
            [c],
            "end {c}"
        );
    }
}

#[test]
fn missing_leading_boundary_is_not_a_mention() {
    assert!(extract_file_mentions("explain this@src/a.ts").is_empty());
    assert!(extract_file_mentions("this@src/a.ts:12-40").is_empty());
    let (before, after) = insert("explain this", 12, true);
    let sep = if after.starts_with(' ') || after.starts_with('\n') {
        ""
    } else {
        " "
    };
    let wire = format!("{before}@src/a.ts{sep}{after}");
    assert_eq!(extract_file_mentions(&wire), ["src/a.ts"]);
    assert!(!file_at_hits(&wire).is_empty());
}

#[test]
fn trailing_slash_directories() {
    assert!(extract_file_mentions("@src").is_empty());
    assert_eq!(extract_file_mentions("@src/"), ["src/"]);
    assert_eq!(extract_file_mentions("look at @src/ please"), ["src/"]);
    assert_eq!(extract_file_mentions("@src/lib/"), ["src/lib/"]);
    assert_eq!(extract_file_mentions("@terminal/"), ["terminal/"]);
    assert_eq!(extract_file_mentions("@.github/"), [".github/"]);
    assert_eq!(extract_file_mentions("@foo.bar/"), ["foo.bar/"]);
    assert_eq!(
        extract_file_mentions("look at @.github/ please"),
        [".github/"]
    );
    assert!(extract_terminal_mentions("@terminal/").is_empty());
    assert_eq!(dir_file_mention_path("src"), "src/");
    assert_eq!(dir_file_mention_path("src/lib/"), "src/lib/");
    assert!(is_dir_file_mention("src/"));
    assert!(!is_dir_file_mention("src/lib"));
}

#[test]
fn terminal_mentions() {
    assert_eq!(
        extract_terminal_mentions("see @terminal and @terminal:abc-def"),
        ["terminal", "terminal:abc-def"]
    );
    assert_eq!(terminal_mention_token("abc-def"), "@terminal:abc-def");
    assert_eq!(terminal_mention_token(""), "@terminal");
    assert_eq!(terminal_mention_label("terminal:abc", "dev"), "dev");
    assert_eq!(terminal_mention_label("terminal", ""), "terminal");
    assert_eq!(
        terminal_mention_label("@terminal:abcdef1234", ""),
        "abcdef12"
    );
    assert!(extract_terminal_mentions("write foo@terminal.com").is_empty());
    assert!(extract_terminal_mentions("see @terminals please").is_empty());
    assert_eq!(
        strip_composer_markers("errors in @terminal:abc please"),
        "errors in please"
    );
}

#[test]
fn artifact_mentions() {
    assert_eq!(
        extract_artifact_mentions("see @artifact and @artifact:abc-def"),
        ["artifact", "artifact:abc-def"]
    );
    assert_eq!(artifact_mention_token("abc-def"), "@artifact:abc-def");
    assert_eq!(artifact_mention_token(""), "@artifact");
    assert_eq!(artifact_mention_label("artifact:abc", "Notes"), "Notes");
    assert_eq!(artifact_mention_label("artifact", ""), "artifact");
    assert!(extract_artifact_mentions("write foo@artifact.com").is_empty());
    assert!(extract_artifact_mentions("see @artifacts please").is_empty());
    assert_eq!(
        strip_composer_markers("review @artifact:abc please"),
        "review please"
    );
}

#[test]
fn unique_artifact_titles() {
    let arts = [("art-1", "Notes"), ("art-2", "Chart")];
    assert_eq!(unique_artifact_id_for_title("Notes", arts), "art-1");
    assert_eq!(unique_artifact_id_for_title("notes", arts), "art-1");
    assert_eq!(unique_artifact_id_for_title("missing", arts), "");
    assert!(reserved_at_name("artifact"));
    assert_eq!(
        unique_artifact_id_for_title("app.ts", [("x", "app.ts")]),
        ""
    );
    assert!(at_name_is_file_mention("app.ts"));
    assert!(!at_name_is_file_mention("notes.md:v2"));
    assert_eq!(
        unique_artifact_id_for_title("notes.md:v2", [("x", "notes.md:v2")]),
        "x"
    );
    assert_eq!(
        unique_artifact_id_for_title("Notes", [("a", "Notes"), ("b", "notes")]),
        "",
        "duplicates"
    );
}

#[test]
fn loose_at_names_and_overlaps() {
    let hits = collect_loose_at_names("see @Notes and @terminal and @src/a.ts @x@y");
    assert_eq!(
        hits,
        [AtNameHit {
            index: 4,
            len: 6,
            token: "Notes".into()
        }]
    );
    assert!(token_span_overlaps(4, 6, &[(8, 3)]));
    assert!(!token_span_overlaps(4, 6, &[(10, 3)]));
}

#[test]
fn skill_and_attachment_markers() {
    assert_eq!(skill_mention_token(" pdf "), "/pdf");
    assert_eq!(
        strip_skill_mentions("use /pdf and /Users/x and /a_b ok"),
        "use and /Users/x and /a_b ok"
    );
    let marked = compose_marked_text("hello", ["img_a", "img_b"]);
    assert_eq!(marked, "hello\n⟦attachment:img_a⟧\n⟦attachment:img_b⟧");
    assert_eq!(compose_marked_text(&marked, ["img_a"]), marked);
    assert_eq!(extract_attachment_ids(&marked), ["img_a", "img_b"]);
    assert_eq!(strip_attachment_markers("a\n\n\n⟦attachment:x⟧b"), "a\n\nb");
}

#[test]
fn js_whitespace_bounds_tokens() {
    assert_eq!(extract_file_mentions("see\u{A0}@src/a.ts"), ["src/a.ts"]);
    assert_eq!(
        extract_terminal_mentions("@terminal\u{FEFF}x"),
        ["terminal"]
    );
}
