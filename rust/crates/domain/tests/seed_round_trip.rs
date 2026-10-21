//! The e2e seed fixture (real Go seed data dumped by cmd/seeddump) decodes
//! into the domain types and re-encodes to the same JSON values.

use std::collections::BTreeMap;

use recur_domain::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn round_trip<T: DeserializeOwned + Serialize>(seed: &Value, key: &str) {
    let original = &seed[key];
    assert!(!original.is_null(), "seed has no {key}");
    let typed: T =
        serde_json::from_value(original.clone()).unwrap_or_else(|e| panic!("{key}: {e}"));
    let back = serde_json::to_value(&typed).unwrap();
    pretty_assertions::assert_eq!(&back, original, "{key}");
}

#[test]
fn seed_fixture_round_trips() {
    let seed: Value =
        serde_json::from_str(include_str!("../../../../frontend/e2e/fixtures/seed.json")).unwrap();
    round_trip::<Vec<Project>>(&seed, "projects");
    round_trip::<Vec<Workspace>>(&seed, "loops");
    round_trip::<Vec<Pr>>(&seed, "prs");
    round_trip::<Vec<InboxItem>>(&seed, "inbox");
    round_trip::<Policy>(&seed, "policy");
    round_trip::<Settings>(&seed, "settings");
    round_trip::<Resource>(&seed, "resource");
    round_trip::<BTreeMap<String, Workspace>>(&seed, "loopById");
    round_trip::<BTreeMap<String, Option<Vec<FeedItem>>>>(&seed, "feedByLoop");
    round_trip::<BTreeMap<String, Option<Vec<Agent>>>>(&seed, "agentsByLoop");
    round_trip::<BTreeMap<String, Escalation>>(&seed, "escByLoop");
    round_trip::<BTreeMap<String, Pr>>(&seed, "prById");

    let keys: Vec<_> = seed.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "agentsByLoop",
            "escByLoop",
            "feedByLoop",
            "inbox",
            "loopById",
            "loops",
            "policy",
            "prById",
            "projects",
            "prs",
            "resource",
            "settings"
        ],
        "a new seed section needs a round-trip check"
    );
}

#[test]
fn seed_semantics_survive_decoding() {
    let seed: Value =
        serde_json::from_str(include_str!("../../../../frontend/e2e/fixtures/seed.json")).unwrap();
    let loops: Vec<Workspace> = serde_json::from_value(seed["loops"].clone()).unwrap();
    assert!(
        loops
            .iter()
            .all(|w| w.group.is_known() && w.state.is_known() && w.autonomy.is_known())
    );
    let prs: Vec<Pr> = serde_json::from_value(seed["prs"].clone()).unwrap();
    assert!(prs.iter().all(|p| p.source.is_known()));
}
