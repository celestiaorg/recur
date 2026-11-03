use async_trait::async_trait;
use recur_domain::Artifact;

use crate::Result;

#[async_trait]
pub trait ArtifactsService: Send + Sync {
    async fn delete_artifact(&self, workspace_id: &str, id: &str) -> Result<()>;

    async fn list_artifacts(&self, workspace_id: &str) -> Result<Vec<Artifact>>;

    async fn present_artifact(&self, workspace_id: &str, id: &str) -> Result<()>;

    async fn rename_artifact(&self, workspace_id: &str, id: &str, title: &str) -> Result<Artifact>;

    async fn reveal_artifact(&self, workspace_id: &str, id: &str) -> Result<()>;
}
