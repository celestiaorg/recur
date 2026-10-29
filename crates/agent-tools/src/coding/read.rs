use super::{MAX_READ_LINES, clip_line, is_binary};
use crate::json::{Shape, Ty, field, unmarshal};
use crate::path::{read_file, resolve_path};
use crate::tool::{Call, Tool, ToolError};

const SCHEMA: &str = r#"{"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["file_path"]}"#;

static IN: Shape = Shape::anon(&[
    field("file_path", Ty::Str),
    field("offset", Ty::Int),
    field("limit", Ty::Int),
]);

pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "Read"
    }

    fn description(&self) -> &'static str {
        "Read a file from the worktree. Returns cat -n format. Optional offset (1-based) and limit (default 2000 lines)."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a =
            unmarshal(&c.args, &IN).map_err(|e| ToolError(format!("invalid Read args: {e}")))?;
        let abs = resolve_path(&c.worktree, a.str("file_path"))?;
        let data = read_file(&abs)?;
        if is_binary(&data) {
            return Err("binary file".into());
        }
        let offset = a.int("offset").max(1);
        let limit = match a.int("limit") {
            n if n <= 0 => MAX_READ_LINES as i64,
            n => n,
        };
        let text = String::from_utf8_lossy(&data).into_owned();
        let mut lines: Vec<&str> = text.split('\n').collect();
        // Split leaves a trailing empty element when the file ends in newline.
        if lines.last() == Some(&"") {
            lines.pop();
        }
        let n = lines.len() as i64;
        if offset > n + 1 {
            return Err(format!("offset {offset} is past end of file ({n} lines)").into());
        }
        // Go int arithmetic: a huge limit wraps like it does there.
        let end = (offset - 1).wrapping_add(limit).min(n);
        let mut b = String::new();
        for i in (offset - 1)..end.max(offset - 1) {
            b.push_str(&format!("{:6}\t{}\n", i + 1, clip_line(lines[i as usize])));
        }
        if end < n {
            b.push_str(&format!(
                "\n<response clipped: showing lines {offset}–{end} of {n}>\n"
            ));
        }
        if b.is_empty() {
            return Ok("<empty file>\n".into());
        }
        Ok(b)
    }
}
