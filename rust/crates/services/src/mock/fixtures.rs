//! `frontend/e2e/helpers/fixtures.ts`: the shared e2e seed (loaded once) and
//! the per-test clone every test's bridge gets, so mutations cannot leak into
//! the next test. The page-level helpers there (clicking a sidebar row,
//! reading localStorage, asserting no page errors) belong to the view
//! harness; [`sidebar_workspace_id`] is the lookup they share.

use std::sync::OnceLock;

use super::Seed;

/// The e2e seed (`seed` in fixtures.ts).
pub fn base_seed() -> &'static Seed {
    static SEED: OnceLock<Seed> = OnceLock::new();
    SEED.get_or_init(Seed::load)
}

/// A per-test clone of [`base_seed`].
pub fn seed() -> Seed {
    base_seed().clone()
}

/// The workspace id behind the sidebar row for a seed goal (rows show
/// `short`, so specs match by id).
pub fn sidebar_workspace_id(goal: &str) -> Option<String> {
    base_seed().workspace_id_by_goal(goal)
}
