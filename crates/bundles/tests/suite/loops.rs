//! Bundle `[[loop]]` and `[[policy]]` parsing.

use std::collections::{BTreeMap, HashMap};

use recur_bundles::*;

use crate::common::*;

const LOOP_FIXTURE: &str = r#"
[bundle]
name = "Loop"
id = "recur.loop"

[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"

[[role]]
id = "recur.roles.planner"
name = "Planner"
code = "PL"
mode = "plan"

[[role]]
id = "recur.roles.coder"
name = "Coder"
code = "CO"
mode = "agent"
tools = ["update_plan_step"]

[[role]]
id = "recur.roles.reviewer"
name = "Reviewer"
code = "RV"
mode = "review"

[[policy]]
id = "recur.policies.balanced"
name = "Balanced"
max_rounds = 4
on_max_rounds = "escalate"
findings_threshold = { critical = 0, high = 0, medium = 0 }
plan_approval = "if_structured"
extend_rounds = 2

[[loop]]
id = "recur.loops.pr"
name = "To a PR"
conductor = "recur.roles.maestro"
entry = "planning"
reenter = ["planning", "converge"]

[[loop.phase]]
id = "planning"
uses = "plan"
role = "recur.roles.planner"
prompt = "/review later"
gate = "plan_approval"
next = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
next = "pr"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"

[loop.phase.prompts]
review = "/review"
fix = "/fix-findings"
rereview = "/re-review"

[[loop.phase]]
id = "pr"
uses = "ensure_pr"

[[loop.phase]]
id = "security"
uses = "converge"
dispatch = "on_demand"
max_rounds = 2
on_max_rounds = "proceed"
on_fail = "skip"

[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
"#;

const THREE_ROLES: &str = r#"
[bundle]
name = "X"
id = "x"
[[role]]
id = "recur.roles.maestro"
name = "Maestro"
code = "MA"
mode = "loop"
[[role]]
id = "recur.roles.coder"
name = "Coder"
code = "CO"
mode = "agent"
[[role]]
id = "recur.roles.reviewer"
name = "Reviewer"
code = "RV"
mode = "review"
"#;

fn parse_in_temp(toml: &str) -> Result<ParsedBundle, Error> {
    let (_g, dir) = temp_dir();
    must_write(&format!("{dir}/bundle.toml"), toml);
    parse_manifest_full(&dir, toml.as_bytes())
}

#[test]
fn parse_manifest_loop() {
    let parsed = parse_in_temp(LOOP_FIXTURE).unwrap();
    assert_eq!(parsed.roles.len(), 4);
    assert_eq!(parsed.policies.len(), 1);
    assert_eq!(parsed.policies[0].max_rounds, 4);
    assert_eq!(
        parsed.policies[0].findings_threshold.levels.get("medium"),
        Some(&0)
    );
    assert_eq!(parsed.loops.len(), 1);
    let lp = &parsed.loops[0];
    assert_eq!(lp.roles["conductor"], "recur.roles.maestro");
    let plan = lp.phase_by_id("planning").unwrap();
    assert_eq!(plan.roles["planner"], "recur.roles.planner");
    assert_eq!(plan.prompts["plan"], "/review later");
    let sec = lp.phase_by_id("security").unwrap();
    assert_eq!(sec.dispatch, "on_demand");
    assert_eq!(lp.graph_phase_ids(), ["planning", "converge"]);
    assert_eq!(lp.reenter_ids(), ["planning", "converge"]);
}

#[test]
fn parse_loop_rejects_next_cycle() {
    let toml = format!(
        "{THREE_ROLES}[[loop]]\nid = \"x.loops.cycle\"\nname = \"Cycle\"\nconductor = \"recur.roles.maestro\"\nentry = \"a\"\n[[loop.phase]]\nid = \"a\"\nuses = \"converge\"\nnext = \"b\"\n[loop.phase.roles]\nimplementer = \"recur.roles.coder\"\nreviewer = \"recur.roles.reviewer\"\n[[loop.phase]]\nid = \"b\"\nuses = \"converge\"\nnext = \"a\"\n[loop.phase.roles]\nimplementer = \"recur.roles.coder\"\nreviewer = \"recur.roles.reviewer\"\n"
    );
    let err = parse_in_temp(&toml).unwrap_err();
    assert_eq!(
        err.0,
        "loop \"x.loops.cycle\": next graph contains a cycle (b → a)"
    );
}

#[test]
fn parse_loop_rejects_on_demand_reenter() {
    let toml = format!(
        "{THREE_ROLES}[[loop]]\nid = \"x.loops.x\"\nname = \"X\"\nconductor = \"recur.roles.maestro\"\nentry = \"a\"\nreenter = [\"sec\"]\n[[loop.phase]]\nid = \"a\"\nuses = \"converge\"\n[loop.phase.roles]\nimplementer = \"recur.roles.coder\"\nreviewer = \"recur.roles.reviewer\"\n[[loop.phase]]\nid = \"sec\"\nuses = \"converge\"\ndispatch = \"on_demand\"\n[loop.phase.roles]\nimplementer = \"recur.roles.coder\"\nreviewer = \"recur.roles.reviewer\"\n"
    );
    let err = parse_in_temp(&toml).unwrap_err();
    assert_eq!(
        err.0,
        "loop \"x.loops.x\": reenter cannot list on-demand id \"sec\""
    );
}

#[test]
fn parse_loop_prompt_path_missing() {
    let toml = "[bundle]\nname = \"X\"\nid = \"x\"\n[[role]]\nid = \"recur.roles.maestro\"\nname = \"Maestro\"\ncode = \"MA\"\nmode = \"loop\"\n[[role]]\nid = \"recur.roles.planner\"\nname = \"Planner\"\ncode = \"PL\"\nmode = \"plan\"\n[[loop]]\nid = \"x.loops.x\"\nname = \"X\"\nconductor = \"recur.roles.maestro\"\nentry = \"planning\"\n[[loop.phase]]\nid = \"planning\"\nuses = \"plan\"\nrole = \"recur.roles.planner\"\nprompt = \"prompts/planing.md\"\n";
    let err = parse_in_temp(toml).unwrap_err();
    assert_eq!(
        err.0,
        "loop \"x.loops.x\" phase[0]: planning prompts.plan: missing path \"prompts/planing.md\""
    );
}

#[test]
fn same_or_stricter_rules() {
    let pol = FindingsThreshold {
        all_zero: false,
        levels: BTreeMap::from([
            ("critical".into(), 0),
            ("high".into(), 0),
            ("medium".into(), 0),
        ]),
    };
    let zero = FindingsThreshold {
        all_zero: true,
        ..Default::default()
    };
    assert!(same_or_stricter(&zero, &pol).is_ok());
    let loose = FindingsThreshold {
        all_zero: false,
        levels: BTreeMap::from([("critical".into(), 3)]),
    };
    assert!(same_or_stricter(&loose, &pol).is_err());
}

#[test]
fn looks_like_prompt_path_cases() {
    for (s, want) in [
        ("/review", false),
        ("@docs/spec.md", false),
        ("use the /skill to do x", false),
        ("prompts/plan.md", true),
        ("readme.md", true),
        ("just prose", false),
    ] {
        assert_eq!(looks_like_prompt_path(s), want, "{s}");
    }
}

#[test]
fn findings_block_rules() {
    let bal = FindingsThreshold {
        all_zero: false,
        levels: BTreeMap::from([
            ("critical".into(), 0),
            ("high".into(), 0),
            ("medium".into(), 0),
        ]),
    };
    assert!(!findings_block(
        &bal,
        &HashMap::from([("low".to_string(), 3)])
    ));
    assert!(findings_block(
        &bal,
        &HashMap::from([("medium".to_string(), 1)])
    ));
    let zero = FindingsThreshold {
        all_zero: true,
        ..Default::default()
    };
    assert!(findings_block(
        &zero,
        &HashMap::from([("info".to_string(), 1)])
    ));
}

#[test]
fn discover_loop_user_wins() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    write_bundle(&def, "Loop", LOOP_FIXTURE);
    write_bundle(
        &user,
        "Loop",
        "[bundle]\nname = \"Loop\"\nid = \"recur.loop\"\n[[policy]]\nid = \"recur.policies.balanced\"\nname = \"Ship it\"\nmax_rounds = 9\non_max_rounds = \"proceed\"\nplan_approval = \"never\"\n",
    );
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    let p = find_policy(&cat.policies, "recur.policies.balanced").expect("missing");
    assert_eq!(p.name, "Ship it");
    assert_eq!(p.max_rounds, 9);
}

#[test]
fn parse_shipped_loop_pack() {
    let dir = repo_path("bundles/Loop");
    let data = std::fs::read(format!("{dir}/bundle.toml")).unwrap();
    let parsed = parse_manifest_full(&dir, &data).unwrap();
    let lp = find_loop(&parsed.loops, DEFAULT_LOOP_ID).expect("missing recur.loops.pr");
    assert!(lp.prompts["mission"].contains("Ship a correct"));
    assert!(lp.phase_by_id("pr").is_some());
    assert!(lp.phase_by_id("security").is_some());
    assert_eq!(lp.phase_by_id("converge").unwrap().next, "pr");

    let cv = find_loop(&parsed.loops, CONVERGE_LOOP_ID).expect("converge");
    assert_eq!(cv.entry, "converge");
    let ph = cv.phase_by_id("converge").unwrap();
    assert_eq!((ph.start.as_str(), ph.next.as_str()), ("review", ""));
    assert!(cv.prompts["mission"].contains("already wrote"));

    let cv_pr = find_loop(&parsed.loops, CONVERGE_PR_LOOP_ID).expect("converge-pr");
    assert_eq!(cv_pr.entry, "converge");
    let ph = cv_pr.phase_by_id("converge").unwrap();
    assert_eq!((ph.start.as_str(), ph.next.as_str()), ("review", "pr"));
    assert!(cv_pr.phase_by_id("pr").is_some());
    assert!(cv_pr.prompts["mission"].contains("then open a PR"));

    let docs = find_loop(&parsed.loops, DOCS_LOOP_ID).expect("docs");
    assert_eq!(docs.entry, "planning");
    assert!(
        docs.reenter.iter().any(|r| r == "planning")
            && docs.reenter.iter().any(|r| r == "converge")
    );
    assert!(docs.prompts["mission"].contains("Do not open a PR"));
    let docs_conv = docs.phase_by_id("converge").unwrap();
    assert!(docs_conv.findings.as_ref().is_some_and(|f| f.all_zero));
    assert_eq!(docs_conv.next, "done");
    assert_eq!(docs_conv.roles["implementer"], "recur.roles.writer");
    assert_eq!(docs_conv.roles["reviewer"], "recur.roles.editor");
    assert_eq!(docs.phase_by_id("done").unwrap().uses, VERB_ARTIFACT);
    assert!(docs.phase_by_id("pr").is_none());
    assert!(find_loop(&parsed.loops, STEP_LOOP_ID).is_none());

    let sec = find_loop(&parsed.loops, SECURITY_LOOP_ID).expect("security");
    assert_eq!(sec.entry, "converge");
    assert!(sec.prompts["mission"].contains("Do not reopen product scope"));
    let sec_ph = sec.phase_by_id("converge").unwrap();
    assert_eq!(
        (sec_ph.start.as_str(), sec_ph.next.as_str()),
        ("review", "")
    );
    assert_eq!(sec_ph.roles["implementer"], "recur.roles.coder");
    assert_eq!(sec_ph.roles["reviewer"], "recur.roles.security-reviewer");
    assert!(sec.phase_by_id("pr").is_none());

    assert!(find_role(&parsed.roles, "recur.roles.writer").is_some());
    assert!(find_role(&parsed.roles, "recur.roles.editor").is_some());
    assert!(find_policy(&parsed.policies, DEFAULT_POLICY_ID).is_some());
    assert!(parsed.roles.len() >= 8, "roles: {}", parsed.roles.len());
}

#[test]
fn phase_findings_integer_n_lints() {
    let toml = format!(
        "{THREE_ROLES}[[loop]]\nid = \"x.loops.x\"\nname = \"X\"\nconductor = \"recur.roles.maestro\"\nentry = \"c\"\n[[loop.phase]]\nid = \"c\"\nuses = \"converge\"\nfindings_threshold = 3\n[loop.phase.roles]\nimplementer = \"recur.roles.coder\"\nreviewer = \"recur.roles.reviewer\"\n"
    );
    let err = parse_in_temp(&toml).unwrap_err();
    assert_eq!(
        err.0,
        "loop \"x.loops.x\" phase[0]: phase findings_threshold integer must be 0 (got 3)"
    );
}

#[test]
fn policy_from_autonomy_maps() {
    assert_eq!(
        policy_from_autonomy(" Cautious "),
        "recur.policies.cautious"
    );
    assert_eq!(
        policy_from_autonomy("aggressive"),
        "recur.policies.aggressive"
    );
    assert_eq!(policy_from_autonomy(""), DEFAULT_POLICY_ID);
    assert_eq!(
        allowed_role_modes(VERB_VERIFY, "verifier"),
        ["ask", "review", "agent"]
    );
    assert!(allowed_role_modes(VERB_ENSURE_PR, "x").is_empty());
}
