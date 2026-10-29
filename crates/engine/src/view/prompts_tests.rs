use super::*;

#[test]
fn error_message_unwraps_runtime_errors() {
    let raw = r#"{"message":"prompt \"recur.chat.update\" exited 1: error: Your local changes to the following files would be overwritten by merge:\n\tREADME.md\nPlease commit your changes or stash them before you merge.\nAborting\nMerge with strategy ort failed.\ngit merge main failed","cause":{},"kind":"RuntimeError"}"#;
    assert_eq!(
        prompt_action_error_message(raw),
        "error: Your local changes to the following files would be overwritten by merge:\n\tREADME.md\nPlease commit your changes or stash them before you merge.\nAborting\nMerge with strategy ort failed.\ngit merge main failed"
    );
    assert_eq!(
        prompt_action_error_message(r#"prompt "recur.chat.commit" exited 1: nothing to commit"#),
        "nothing to commit"
    );
    assert_eq!(
        prompt_action_error_message(
            r#"{"message":"prompt \"x\" exited 2: no merge base ref","cause":{},"kind":"RuntimeError"}"#
        ),
        "no merge base ref"
    );
    assert_eq!(
        prompt_action_error_message("Unknown prompt /update"),
        "Unknown prompt /update"
    );
    assert_eq!(prompt_action_error_message("  "), "Prompt failed");
    assert_eq!(
        prompt_action_error_message(r#"PROMPT "x" EXITED 3:   "#),
        r#"PROMPT "x" EXITED 3:"#
    );
}

fn committed(text: &str) -> Option<(String, String)> {
    parse_committed_prompt_line(text)
}

fn pair(slash: &str, args: &str) -> Option<(String, String)> {
    Some((slash.into(), args.into()))
}

#[test]
fn committed_prompt_lines() {
    assert_eq!(committed("/linear"), pair("linear", ""));
    assert_eq!(committed("  /linear  "), pair("linear", ""));
    assert_eq!(committed("/linear ENG-123"), pair("linear", "ENG-123"));
    assert_eq!(
        committed("/linear   ENG-123  extra notes"),
        pair("linear", "ENG-123  extra notes")
    );
    assert_eq!(
        committed("/linear https://linear.app/acme/issue/ENG-123/slug"),
        pair("linear", "https://linear.app/acme/issue/ENG-123/slug")
    );
    assert_eq!(committed("please /linear ENG-123"), None);
    assert_eq!(committed("/Linear ENG-123"), None);
}

#[test]
fn action_query_gate() {
    let q = |start, end, query: &str| {
        Some(MentionQuery {
            start,
            end,
            query: query.into(),
        })
    };
    assert_eq!(prompt_action_query_at("/linear", 7), q(0, 7, "linear"));
    assert_eq!(prompt_action_query_at("/linear ", 7), q(0, 7, "linear"));
    assert_eq!(prompt_action_query_at("/linear ENG", 11), None);
    assert_eq!(prompt_action_query_at("x /linear", 9), None);
}

#[test]
fn args_query() {
    let takes = |s: &str| s == "linear" || s == "issue";
    let want = |args: &str, args_start| {
        Some(PromptArgsQuery {
            slash: "linear".into(),
            args: args.into(),
            args_start,
            arg: String::new(),
        })
    };
    assert_eq!(prompt_args_query_at("/linear ", 8, &takes), want("", 8));
    assert_eq!(
        prompt_args_query_at("/linear ENG-12", 14, &takes),
        want("ENG-12", 8)
    );
    assert_eq!(prompt_args_query_at("/linear", 7, &takes), None);
    assert_eq!(prompt_args_query_at("/commit ", 8, &takes), None);
    assert_eq!(
        prompt_args_query_at("/linear x", 3, &takes),
        None,
        "caret before args"
    );
}

fn info(slash: &str, takes_args: bool, args: &[(&str, &str, bool)]) -> PromptAction {
    PromptAction {
        slash: slash.into(),
        takes_args,
        args: args
            .iter()
            .map(|(name, placeholder, optional)| PromptArgSpec {
                name: (*name).into(),
                placeholder: (*placeholder).into(),
                optional: *optional,
            })
            .collect(),
        ..PromptAction::default()
    }
}

#[test]
fn params_from_args_cases() {
    let one = info("linear", true, &[("issue", "id", false)]);
    assert_eq!(
        params_from_args(Some(&one), "ENG-1 extra"),
        BTreeMap::from([("issue".to_owned(), "ENG-1 extra".to_owned())])
    );
    let poll = info(
        "poll",
        true,
        &[
            ("question", "", false),
            ("choice_1", "", false),
            ("choice_2", "", false),
        ],
    );
    let m = |q: &str, a: &str, b: &str| {
        BTreeMap::from([
            ("question".to_owned(), q.to_owned()),
            ("choice_1".to_owned(), a.to_owned()),
            ("choice_2".to_owned(), b.to_owned()),
        ])
    };
    assert_eq!(
        params_from_args(Some(&poll), "Which color? Red Blue extra"),
        m("Which", "color?", "Red Blue extra")
    );
    assert_eq!(
        params_from_args(Some(&poll), r#""Which color?" Red Blue extra"#),
        m("Which color?", "Red", "Blue extra")
    );
    assert!(params_from_args(None, "x").is_empty());
}

#[test]
fn optional_slots() {
    let all_optional = info("fix-checks", true, &[("check", "all", true)]);
    assert!(prompt_args_optional(Some(&all_optional)));
    let required = info("linear", true, &[("issue", "", false)]);
    assert!(!prompt_args_optional(Some(&required)));
    let mixed = info("mixed", true, &[("note", "", true), ("issue", "", false)]);
    assert!(!prompt_args_optional(Some(&mixed)));
    assert!(prompt_arg_slot_optional(Some(&mixed), "note"));
    assert!(!prompt_arg_slot_optional(Some(&mixed), "issue"));
    assert!(
        prompt_arg_slot_optional(Some(&mixed), ""),
        "falls back to the first slot"
    );
    assert!(!prompt_arg_slot_optional(None, ""));
}

#[test]
fn required_args_filled() {
    let mixed = info("mixed", true, &[("note", "", true), ("issue", "", false)]);
    let v = |xs: &[&str]| xs.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    assert!(prompt_required_args_filled(
        Some(&mixed),
        &v(&["", "ENG-1"])
    ));
    assert!(prompt_required_args_filled(
        Some(&mixed),
        &v(&["later", "ENG-1"])
    ));
    assert!(!prompt_required_args_filled(Some(&mixed), &v(&["", ""])));
    assert!(!prompt_required_args_filled(Some(&mixed), &v(&["note"])));
    assert!(!prompt_required_args_filled(None, &[]));
    assert!(!prompt_required_args_filled(None, &v(&["ENG-1"])));
    assert!(!prompt_required_args_filled(
        Some(&info("commit", false, &[])),
        &[]
    ));
    let all_optional = info("fix-checks", true, &[("check", "all", true)]);
    assert!(prompt_required_args_filled(Some(&all_optional), &[]));
    assert!(prompt_required_args_filled(Some(&all_optional), &v(&[""])));
}

#[test]
fn arg_specs_and_lookup() {
    let synthesized = PromptAction {
        slash: "linear".into(),
        takes_args: true,
        arg_placeholder: "TEAM-123".into(),
        ..PromptAction::default()
    };
    assert_eq!(
        prompt_arg_specs(Some(&synthesized)),
        [PromptArgSpec {
            name: "args".into(),
            placeholder: "TEAM-123".into(),
            optional: false
        }]
    );
    let inherits = PromptAction {
        arg_placeholder: "hint".into(),
        ..info("x", false, &[("a", "", false), ("", "skip", false)])
    };
    let specs = prompt_arg_specs(Some(&inherits));
    assert_eq!((specs.len(), specs[0].placeholder.as_str()), (1, "hint"));
    assert!(
        prompt_takes_args(Some(&inherits)),
        "implied by [[prompt.arg]]"
    );
    assert!(!prompt_takes_args(Some(&info("commit", false, &[]))));
    let list = vec![info("commit", false, &[]), info("linear", true, &[])];
    assert_eq!(
        find_prompt_by_slash("/LINEAR", &list).map(|p| p.slash.as_str()),
        Some("linear")
    );
    assert!(find_prompt_by_slash("nope", &list).is_none());
}

#[test]
fn settings_disable_prompts() {
    let set = Settings {
        disabled_bundle_prompts: vec!["a".into(), String::new()],
        ..Settings::default()
    };
    assert_eq!(disabled_prompt_ids(&set), ["a"]);
    assert!(!prompt_enabled(&set, "a"));
    assert!(prompt_enabled(&set, "b"));
    let list = vec![
        PromptAction {
            id: "a".into(),
            ..PromptAction::default()
        },
        PromptAction {
            id: "b".into(),
            ..PromptAction::default()
        },
    ];
    let kept = filter_enabled_prompts(&set, list);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].id, "b");
}

#[test]
fn prompt_context_defaults() {
    let ws = PromptWorkspace {
        id: "W1".into(),
        project_id: "P".into(),
        worktree_path: "/wt".into(),
        port: 0,
    };
    let ctx = build_prompt_context(&ws, None, "", "", BTreeMap::new());
    assert_eq!(
        (ctx.tab_size, ctx.soft_tabs, ctx.surface.as_str()),
        (4, true, "workspace")
    );
    assert_eq!(ctx.work_dir, "/wt");
    let ed = PromptEditor {
        path: Some("src/a.ts".into()),
        line_number: Some(3),
        soft_tabs: Some(false),
        ..PromptEditor::default()
    };
    let ctx = build_prompt_context(&ws, Some(&ed), "github_pr", "ENG-1", BTreeMap::new());
    assert_eq!(
        (
            ctx.file_path.as_str(),
            ctx.file_name.as_str(),
            ctx.line_number,
            ctx.soft_tabs
        ),
        ("src/a.ts", "a.ts", 3, false)
    );
    assert_eq!(
        (ctx.surface.as_str(), ctx.args.as_str()),
        ("github_pr", "ENG-1")
    );
}

#[derive(Debug)]
struct Tab {
    code: &'static str,
    title: &'static str,
    mode: &'static str,
    group: &'static str,
}

impl PromptTab for Tab {
    fn code(&self) -> &str {
        self.code
    }
    fn prompt_group(&self) -> &str {
        self.group
    }
}

const fn tab(
    code: &'static str,
    title: &'static str,
    mode: &'static str,
    group: &'static str,
) -> Tab {
    Tab {
        code,
        title,
        mode,
        group,
    }
}

const PA: Tab = tab("PA", "New chat", "agent", "");
const WORK: Tab = tab("PA", "Ship dark mode", "agent", "");
const OPS: Tab = tab("PA2", "Commit all changes", "agent", "ops");
const RV: Tab = tab("PA3", "Review", "review", "");
const MA: Tab = tab("MA", "Loop", "loop", "");
const UNUSED_RV: Tab = tab("PA4", "New chat", "review", "");

#[derive(Default)]
struct Extra {
    group: &'static str,
    busy: Vec<&'static str>,
    started: Option<Vec<&'static str>>,
    drafted: Vec<&'static str>,
    locked: Vec<&'static str>,
}

fn pick(tabs: &[Tab], active: &str, mode: &str, extra: Extra) -> PromptTabPick {
    let started: Vec<&str> = extra.started.clone().unwrap_or_else(|| {
        tabs.iter()
            .filter(|t| !t.title.is_empty() && t.title != "New chat")
            .map(|t| t.code)
            .collect()
    });
    let is_busy = |c: &str| extra.busy.contains(&c);
    let tab_mode = |t: &Tab| t.mode.to_owned();
    let started_f = |t: &Tab| started.contains(&t.code);
    let drafted_f = |t: &Tab| extra.drafted.contains(&t.code);
    let locked_f = |c: &str| extra.locked.contains(&c);
    pick_prompt_tab(&PromptTabQuery {
        tabs,
        active_code: active,
        mode,
        group: extra.group,
        is_busy: &is_busy,
        tab_mode: &tab_mode,
        started: Some(&started_f),
        drafted: Some(&drafted_f),
        conductor_locked: Some(&locked_f),
    })
}

fn grouped(group: &'static str) -> Extra {
    Extra {
        group,
        ..Extra::default()
    }
}

fn select(code: &str) -> PromptTabPick {
    PromptTabPick::Select(code.into())
}

#[test]
fn pick_prompt_tab_routing() {
    use PromptTabPick::{Create, Current};
    assert_eq!(pick(&[MA, PA], "PA", "agent", Extra::default()), Current);
    assert_eq!(
        pick(&[MA, WORK], "MA", "agent", Extra::default()),
        select("PA")
    );
    assert_eq!(pick(&[MA, WORK], "PA", "agent", grouped("ops")), Create);
    assert_eq!(
        pick(&[MA, WORK, OPS], "PA", "agent", grouped("ops")),
        select("PA2")
    );
    assert_eq!(pick(&[MA, OPS], "PA2", "agent", grouped("ops")), Current);
    let unused = tab("PA2", "New chat", "agent", "");
    assert_eq!(
        pick(&[MA, WORK, unused], "PA", "agent", grouped("ops")),
        select("PA2")
    );
    let draft = tab("PA", "New chat", "agent", "");
    assert_eq!(
        pick(
            &[MA, draft],
            "PA",
            "agent",
            Extra {
                group: "ops",
                started: Some(vec!["PA"]),
                ..Extra::default()
            }
        ),
        Create
    );
    let drafted = |code| Extra {
        group: "ops",
        drafted: vec![code],
        ..Extra::default()
    };
    assert_eq!(
        pick(&[MA, WORK, OPS], "PA", "agent", drafted("PA2")),
        Create
    );
    assert_eq!(pick(&[MA, OPS], "PA2", "agent", drafted("PA2")), Create);
    assert_eq!(
        pick(&[MA, WORK, RV], "PA", "review", grouped("review")),
        Create
    );
    let review_ops = tab("PA3", "Please review", "review", "review");
    assert_eq!(
        pick(
            &[MA, WORK, review_ops],
            "PA",
            "review",
            grouped("simplify-review")
        ),
        Create
    );
    assert_eq!(
        pick(&[MA, WORK, UNUSED_RV], "PA", "review", grouped("review")),
        select("PA4")
    );
    assert_eq!(
        pick(&[MA, OPS, RV], "PA2", "review", Extra::default()),
        select("PA3")
    );
    assert_eq!(pick(&[MA, RV], "PA3", "review", Extra::default()), Current);
    let a = tab("PA", "New chat", "agent", "");
    let b = tab("PA2", "New chat", "agent", "");
    assert_eq!(
        pick(&[MA, a, b], "MA", "agent", grouped("ops")),
        select("PA2")
    );
    assert_eq!(
        pick(
            &[MA, PA],
            "MA",
            "loop",
            Extra {
                locked: vec!["MA"],
                ..Extra::default()
            }
        ),
        Create
    );
    assert_eq!(
        pick(
            &[MA, PA],
            "PA",
            "agent",
            Extra {
                busy: vec!["PA"],
                ..Extra::default()
            }
        ),
        Create
    );
}
