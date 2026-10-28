use crate::json::{Shape, Ty, field, strict_unmarshal};
use crate::path::{read_file, resolve_path, write_file};
use crate::tool::{Call, Tool, ToolError};

const SCHEMA: &str = r#"{"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["file_path","old_string","new_string"]}"#;

static IN: Shape = Shape::anon(&[
    field("file_path", Ty::OptStr),
    field("old_string", Ty::OptStr),
    field("new_string", Ty::OptStr),
    field("replace_all", Ty::Bool),
]);

pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Exact string replacement in a worktree file. Fails if old_string is not unique unless replace_all is true."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a = strict_unmarshal(&c.args, &IN)
            .map_err(|e| ToolError(format!("invalid Edit args: {e}")))?;
        let file_path = match a.opt_str("file_path") {
            Some(p) if !p.trim().is_empty() => p,
            _ => return Err("file_path is required".into()),
        };
        let old = match a.opt_str("old_string") {
            Some(s) if !s.is_empty() => s.as_bytes(),
            _ => return Err("old_string is required".into()),
        };
        let Some(new) = a.opt_str("new_string") else {
            return Err("new_string is required".into());
        };
        let replace_all = a.bool("replace_all");
        let abs = resolve_path(&c.worktree, file_path)?;
        // Byte-wise like Go strings, so non-UTF-8 files keep their bytes.
        let content = read_file(&abs)?;
        let hits = find_all(&content, old);
        if hits.is_empty() {
            return Err(format!("old_string not found in {file_path}").into());
        }
        if hits.len() > 1 && !replace_all {
            return Err(format!(
                "old_string matched {} times; provide more context or set replace_all",
                hits.len()
            )
            .into());
        }
        let replace = if replace_all { &hits[..] } else { &hits[..1] };
        let mut next = Vec::with_capacity(content.len());
        let mut at = 0;
        for &i in replace {
            next.extend_from_slice(&content[at..i]);
            next.extend_from_slice(new.as_bytes());
            at = i + old.len();
        }
        next.extend_from_slice(&content[at..]);
        write_file(&abs, &next)?;
        Ok(format!(
            "updated {file_path} ({} replacement(s))",
            hits.len()
        ))
    }
}

/// Start offsets of non-overlapping matches, left to right (Go strings.Count).
fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}
