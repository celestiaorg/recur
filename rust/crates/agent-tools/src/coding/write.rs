use crate::json::{Shape, Ty, field, strict_unmarshal};
use crate::path::{dir, mkdir_all, resolve_path, write_file};
use crate::tool::{Call, Tool, ToolError};

const SCHEMA: &str = r#"{"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"]}"#;

static IN: Shape = Shape::anon(&[
    field("file_path", "FilePath", Ty::OptStr),
    field("content", "Content", Ty::OptStr),
]);

pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Create or overwrite a file in the worktree."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a = strict_unmarshal(&c.args, &IN)
            .map_err(|e| ToolError(format!("invalid Write args: {e}")))?;
        let file_path = match a.opt_str("file_path") {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Err("file_path is required".into()),
        };
        let Some(content) = a.opt_str("content") else {
            return Err("content is required".into());
        };
        let abs = resolve_path(&c.worktree, file_path)?;
        mkdir_all(&dir(&abs))?;
        write_file(&abs, content.as_bytes())?;
        Ok(format!("wrote {file_path}"))
    }
}
