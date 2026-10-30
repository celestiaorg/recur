use recur_domain::Artifact;
use serde::Serialize;

use super::{EMPTY_OBJECT_SCHEMA, marshal, strict};
use crate::host::CreateArtifactRequest;
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const CREATE_ARTIFACT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string"},
    "kind": {"type": "string", "enum": ["html", "svg", "mermaid", "png", "jpeg", "csv", "json", "md"]},
    "content": {"type": "string", "description": "Text content (or base64 for png/jpeg)"}
  },
  "required": ["title", "kind", "content"]
}"#;

static CREATE_ARTIFACT_IN: Shape = Shape::anon(&[
    field("title", Ty::Str),
    field("kind", Ty::Str),
    field("content", Ty::Str),
]);

pub struct CreateArtifactTool;

#[async_trait::async_trait]
impl Tool for CreateArtifactTool {
    fn name(&self) -> &'static str {
        names::CREATE_ARTIFACT
    }

    fn description(&self) -> &'static str {
        "Create a standalone analytical artifact (html, svg, mermaid, png, jpeg, csv, json, or \
md) stored under the workspace data dir. Prefer this over dumping large tables into chat \
markdown. HTML may include interactive JS; preview runs it in an opaque sandboxed iframe \
(no parent access). https CDN scripts/styles and https/wss fetch work; http and localhost \
do not; localStorage is in-memory only; self-navigation still possible. Returns the \
artifact id and path."
    }

    fn schema(&self) -> String {
        CREATE_ARTIFACT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &CREATE_ARTIFACT_IN)?;
        if a.str("title").trim().is_empty() || a.str("kind").trim().is_empty() {
            return Err("create_artifact: title and kind are required".into());
        }
        let artifact = h
            .create_artifact(
                &c.workspace_id,
                &c.agent_code,
                CreateArtifactRequest {
                    title: a.str("title").into(),
                    kind: a.str("kind").into(),
                    content: a.str("content").into(),
                },
            )
            .await?;
        marshal(&artifact)
    }
}

#[derive(Serialize)]
struct ArtifactList<'a> {
    artifacts: &'a [Artifact],
    count: usize,
}

pub struct ListArtifactsTool;

#[async_trait::async_trait]
impl Tool for ListArtifactsTool {
    fn name(&self) -> &'static str {
        names::LIST_ARTIFACTS
    }

    fn description(&self) -> &'static str {
        "List artifacts created for this workspace (id, title, kind, path, createdAt)."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let list = h.list_artifacts(&c.workspace_id, &c.agent_code).await?;
        marshal(&ArtifactList {
            artifacts: &list,
            count: list.len(),
        })
    }
}

const READ_ARTIFACT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "id": {"type": "string", "description": "Artifact id from list_artifacts"},
    "workspace_id": {"type": "string", "description": "Read from one of this workspace's subagent children instead (id from read_child); default this workspace"}
  },
  "required": ["id"]
}"#;

static READ_ARTIFACT_IN: Shape =
    Shape::anon(&[field("id", Ty::Str), field("workspace_id", Ty::Str)]);

pub struct ReadArtifactTool;

#[async_trait::async_trait]
impl Tool for ReadArtifactTool {
    fn name(&self) -> &'static str {
        names::READ_ARTIFACT
    }

    fn description(&self) -> &'static str {
        "Read one workspace artifact by id (from list_artifacts). Returns JSON with id, title, \
kind, path, and content for text kinds. PNG/JPEG are binary and are not inlined: use \
present_artifact to show them. Prefer this over searching the git worktree or the Recur data dir."
    }

    fn schema(&self) -> String {
        READ_ARTIFACT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &READ_ARTIFACT_IN)?;
        if a.str("id").trim().is_empty() {
            return Err("read_artifact: id is required".into());
        }
        let child = a.str("workspace_id").trim();
        if !child.is_empty() && child != c.workspace_id {
            return Ok(h
                .read_child_artifact(&c.workspace_id, &c.agent_code, child, a.str("id"))
                .await?);
        }
        Ok(h.read_artifact(&c.workspace_id, &c.agent_code, a.str("id"))
            .await?)
    }
}

const PRESENT_ARTIFACT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "id": {"type": "string"}
  },
  "required": ["id"]
}"#;

static PRESENT_ARTIFACT_IN: Shape = Shape::anon(&[field("id", Ty::Str)]);

pub struct PresentArtifactTool;

#[async_trait::async_trait]
impl Tool for PresentArtifactTool {
    fn name(&self) -> &'static str {
        names::PRESENT_ARTIFACT
    }

    fn description(&self) -> &'static str {
        "Present one artifact to the user in the Artifacts panel / preview."
    }

    fn schema(&self) -> String {
        PRESENT_ARTIFACT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &PRESENT_ARTIFACT_IN)?;
        if a.str("id").trim().is_empty() {
            return Err("present_artifact: id is required".into());
        }
        Ok(
            h.present_artifact(&c.workspace_id, &c.agent_code, a.str("id"))
                .await,
        )
    }
}
