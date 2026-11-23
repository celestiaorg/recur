use super::{EMPTY_OBJECT_SCHEMA, strict};
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const INSTALL_BUNDLE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "path": {"type": "string", "description": "Bundle directory (holding bundle.toml), relative to the worktree root"}
  },
  "required": ["path"]
}"#;

static INSTALL_BUNDLE_IN: Shape = Shape::anon(&[field("path", Ty::Str)]);

pub struct InstallBundleTool;

#[async_trait::async_trait]
impl Tool for InstallBundleTool {
    fn name(&self) -> &'static str {
        names::INSTALL_BUNDLE
    }

    fn description(&self) -> &'static str {
        "Install a Recur Bundle you wrote in this worktree as a user bundle (~/.recur/bundles/<dir \
name>/) and reload bundles, so its workflows, roles, policies, prompts and commands are usable \
right away. The bundle is linted first: lint errors come back and nothing is installed. The \
human must approve every install (bundle shell commands run as them), including replacing a \
user bundle of the same name. Run `recur bundles lint <path>` first."
    }

    fn schema(&self) -> String {
        INSTALL_BUNDLE_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &INSTALL_BUNDLE_IN)?;
        let path = a.str("path").trim();
        if path.is_empty() {
            return Err("path is required".into());
        }
        Ok(
            h.install_bundle(&c.cancel, &c.workspace_id, &c.agent_code, path)
                .await?,
        )
    }
}

pub struct ReloadBundlesTool;

#[async_trait::async_trait]
impl Tool for ReloadBundlesTool {
    fn name(&self) -> &'static str {
        names::RELOAD_BUNDLES
    }

    fn description(&self) -> &'static str {
        "Reload Recur Bundles (Navigate > Reload Bundles) and list what is installed: every \
workflow, role and policy id with its bundle, plus packs skipped and why. Call it to find \
shipped ids to reuse before writing a bundle, or after editing an installed user bundle."
    }

    fn schema(&self) -> String {
        EMPTY_OBJECT_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        Ok(h.reload_bundles(&c.workspace_id, &c.agent_code).await?)
    }
}
