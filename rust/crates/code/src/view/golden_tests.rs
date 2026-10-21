//! Replays `tests/golden/ts_view.json`, recorded from the frontend modules by
//! `tests/golden/gen_ts_view.mjs.txt`.

use std::collections::BTreeMap;

use recur_domain::TreeEntry;
use serde::Deserialize;

use super::{changed_tree, finding_paths, skills};
use crate::fuzzy::fuzzy_filter;

#[derive(Deserialize)]
struct Golden {
    labels: Vec<String>,
    ranked: BTreeMap<String, Vec<(String, i64)>>,
    paths: Vec<String>,
    built: Vec<TreeEntry>,
    order: Vec<String>,
    locations: Vec<(String, String, String, String, String)>,
    candidates: Vec<String>,
    matches: Vec<(String, String)>,
}

fn golden() -> Golden {
    serde_json::from_str(include_str!("../../tests/golden/ts_view.json")).expect("golden json")
}

#[test]
fn fuzzy_matches_the_ui() {
    let g = golden();
    for (q, want) in &g.ranked {
        let got: Vec<(String, i64)> = fuzzy_filter(&g.labels, q, |s| s.clone(), 10)
            .into_iter()
            .map(|h| (h.label, h.score))
            .collect();
        assert_eq!(&got, want, "fuzzy_filter({q:?})");
    }
}

#[test]
fn changed_tree_matches_the_ui() {
    let g = golden();
    assert_eq!(changed_tree::build_tree_from_paths(&g.paths), g.built);
    assert_eq!(changed_tree::order_changed_paths(&g.paths), g.order);
}

#[test]
fn skills_and_finding_paths_match_the_ui() {
    let g = golden();
    for (path, scope, home, location, inferred) in &g.locations {
        assert_eq!(&skills::skill_location(path, scope, home), location, "{path}");
        assert_eq!(&skills::infer_home_from_skill_path(path), inferred, "{path}");
    }
    for (path, want) in &g.matches {
        assert_eq!(finding_paths::match_finding_path(path, &g.candidates), want, "{path:?}");
    }
}
