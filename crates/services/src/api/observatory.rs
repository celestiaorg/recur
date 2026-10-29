use async_trait::async_trait;
use recur_domain::Finding;

use crate::Result;

/// ObservatoryService exposes Observatory UI affordances (findings today).
#[async_trait]
pub trait ObservatoryService: Send + Sync {
    /// AddFindingNote appends a human operator comment on a finding (Findings panel /
    /// chat digest). Publishes findings.updated so the UI refreshes.
    async fn add_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        body: &str,
    ) -> Result<Finding>;

    /// CreateFinding records a user note from the Observatory review surface as an
    /// open info finding (distinct from agent reviewer findings via Source=user).
    /// endLine is the inclusive end of a multiline span (0 or == line → single line).
    async fn create_finding(
        &self,
        workspace_id: &str,
        file: &str,
        line: i64,
        message: &str,
        end_line: i64,
    ) -> Result<Finding>;

    /// DeleteFindingNote removes one operator note from a finding. Publishes
    /// findings.updated so the UI refreshes.
    async fn delete_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        note_id: &str,
    ) -> Result<Finding>;

    /// ListFindings returns reviewer findings for a loop (all statuses).
    async fn list_findings(&self, workspace_id: &str) -> Result<Vec<Finding>>;

    /// SetFindingStatus updates one finding's status (open/resolved/ignored/invalid)
    /// from the Findings panel. Publishes findings.updated so the UI refreshes.
    async fn set_finding_status(
        &self,
        workspace_id: &str,
        finding_id: &str,
        status: &str,
    ) -> Result<Finding>;
}
