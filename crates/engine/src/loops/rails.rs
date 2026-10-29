//! The PR-door rail: the policy findings bar a PR must pass.

use recur_domain::AutonomyLevel;

use crate::Engine;
use crate::guard::synthetic_policy;

impl Engine {
    /// The policy findings bar at the PR door: the workspace's stamped
    /// `[[policy]]`, else the autonomy's shipped policy (Balanced: medium+).
    /// Always names the reason when blocked.
    pub fn can_open_pr(&self, workspace_id: &str) -> (bool, String) {
        let mut th = synthetic_policy("", &AutonomyLevel::Balanced).findings_threshold;
        if let Some(ws) = self.store.workspace(workspace_id) {
            match self.resolve_guard(&ws.loop_id, &ws.policy_id, &ws.autonomy) {
                Ok(g) => th = g.policy.findings_threshold,
                Err(reason) => {
                    return (false, format!("{reason} — cannot evaluate the policy bar"));
                }
            }
        }
        if let Some(why) = self.findings_bar_blocks(workspace_id, &th) {
            return (false, format!("{why} — policy bar blocks opening a PR"));
        }
        (true, String::new())
    }
}
