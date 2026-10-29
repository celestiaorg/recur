use async_trait::async_trait;
use recur_domain::Agent;

use crate::Result;

/// Rust port of Go `app.AgentsService` (`frontend/bindings/recur/internal/app/agentsservice.ts`).
#[async_trait]
pub trait AgentsService: Send + Sync {
    async fn get_agent_trace(&self, workspace_id: &str, code: &str) -> Result<Agent>;

    async fn list_loop_agents(&self, workspace_id: &str) -> Result<Vec<Agent>>;
}
