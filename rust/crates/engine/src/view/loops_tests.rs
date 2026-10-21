use recur_domain::{Settings, Workspace};

use super::*;

fn loop_info(id: &str, name: &str) -> LoopInfo {
    LoopInfo {
        id: id.into(),
        name: name.into(),
        hint: String::new(),
    }
}

#[test]
fn enable_and_default_rules() {
    let set = Settings {
        disabled_bundle_loops: vec![DOCS_LOOP_ID.into()],
        disabled_bundle_policies: vec!["recur.policies.cautious".into()],
        default_loop: "  ".into(),
        default_policy: " recur.policies.aggressive ".into(),
        ..Settings::default()
    };
    let loops = vec![
        loop_info(DEFAULT_LOOP_ID, "To a PR"),
        loop_info(DOCS_LOOP_ID, "Docs"),
        loop_info("", "no id"),
    ];
    let ids: Vec<String> = enabled_loops(&loops, Some(&set))
        .into_iter()
        .map(|l| l.id)
        .collect();
    assert_eq!(ids, [DEFAULT_LOOP_ID]);
    assert_eq!(enabled_loops(&loops, None).len(), 2);
    let pols: Vec<String> = enabled_policies(&fallback_policies(), Some(&set))
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(pols, [DEFAULT_POLICY_ID, "recur.policies.aggressive"]);

    assert_eq!(
        default_loop_id(Some(&set)),
        DEFAULT_LOOP_ID,
        "blank falls back"
    );
    assert_eq!(default_loop_id(None), DEFAULT_LOOP_ID);
    assert_eq!(default_policy_id(Some(&set)), "recur.policies.aggressive");
    assert_eq!(default_policy_id(None), DEFAULT_POLICY_ID);

    let ws = Workspace {
        loop_id: CONVERGE_LOOP_ID.into(),
        ..Workspace::default()
    };
    assert_eq!(workspace_loop_id(Some(&ws), Some(&set)), CONVERGE_LOOP_ID);
    assert_eq!(
        workspace_policy_id(Some(&ws), Some(&set)),
        "recur.policies.aggressive"
    );
    assert_eq!(workspace_loop_id(None, None), DEFAULT_LOOP_ID);
}

#[test]
fn select_options_label_falls_back_to_id() {
    let got = loop_select_options(&[loop_info("x.loop", ""), loop_info("y", "Why")]);
    assert_eq!(got[0].label, "x.loop");
    assert_eq!(got[1].label, "Why");
    assert_eq!(
        policy_select_options(&fallback_policies())[1].value,
        DEFAULT_POLICY_ID
    );
}

#[test]
fn autonomy_from_policy_ids() {
    assert_eq!(
        autonomy_from_policy_id("recur.policies.Cautious"),
        "cautious"
    );
    assert_eq!(
        autonomy_from_policy_id("team.aggressive-ship"),
        "aggressive"
    );
    assert_eq!(
        autonomy_from_policy_id("recur.policies.balanced"),
        "balanced"
    );
    assert_eq!(autonomy_from_policy_id("custom"), "balanced");
}

/// The fallback catalog names loops and policies the shipped Loop pack
/// really defines.
#[test]
fn fallbacks_match_the_shipped_loop_pack() {
    let pack = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../bundles");
    let cat = recur_bundles::discover_catalog(&recur_bundles::DiscoverRoots {
        user_dir: String::new(),
        default_dir: pack.into(),
    })
    .unwrap();
    for l in fallback_loops() {
        let def = cat
            .loops
            .iter()
            .find(|d| d.id == l.id)
            .unwrap_or_else(|| panic!("{} not shipped", l.id));
        assert_eq!(def.name, l.name);
    }
    for p in fallback_policies() {
        let def = cat
            .policies
            .iter()
            .find(|d| d.id == p.id)
            .unwrap_or_else(|| panic!("{} not shipped", p.id));
        assert_eq!(def.name, p.name);
    }
}
