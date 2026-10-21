use recur_domain::{Finding as StoredFinding, FindingNote};
use serde::Serialize;

use super::{is_zero, marshal, parse_severity, strict};
use crate::gostr::quote;
use crate::host::Finding;
use crate::json::{Shape, Ty, field};
use crate::names;
use crate::tool::{Call, Tool, ToolError};

const REPORT_FINDING_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "file": {"type": "string", "description": "Path of the file the finding is in"},
    "line": {"type": "integer", "description": "1-based start line (0 if not line-specific)"},
    "end_line": {"type": "integer", "description": "Inclusive end line for a range; omit or 0 for a single line"},
    "severity": {"type": "string", "enum": ["info","low","medium","high","critical"], "description": "Finding severity"},
    "message": {"type": "string", "description": "What is wrong and why"},
    "summary": {"type": "string", "description": "Optional markdown shown to the end user in the turn digest; omit to use message"}
  },
  "required": ["file","message","severity"]
}"#;

static REPORT_FINDING_IN: Shape = Shape::anon(&[
    field("file", "File", Ty::Str),
    field("line", "Line", Ty::Int),
    field("end_line", "EndLine", Ty::Int),
    field("severity", "Severity", Ty::Str),
    field("message", "Message", Ty::Str),
    field("summary", "Summary", Ty::Str),
]);

/// Structured JSON for the chat digest (id/file/summary); agents read the
/// same payload without scraping prose.
#[derive(Serialize)]
struct ReportedFinding<'a> {
    id: &'a str,
    file: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    line: i64,
    #[serde(rename = "endLine", skip_serializing_if = "is_zero")]
    end_line: i64,
    severity: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    summary: &'a str,
    status: &'a str,
}

pub struct ReportFindingTool;

#[async_trait::async_trait]
impl Tool for ReportFindingTool {
    fn name(&self) -> &'static str {
        names::REPORT_FINDING
    }

    fn description(&self) -> &'static str {
        "Report one structured code-review finding for the diff under review. The engine collects \
these to decide convergence; you cannot approve your own work. Optional summary is \
markdown shown to the end user in the turn digest (defaults to message). Do not restate \
filed findings in your closing reply — the app summarizes still-open filings."
    }

    fn schema(&self) -> String {
        REPORT_FINDING_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &REPORT_FINDING_IN)?;
        if a.str("file").trim().is_empty() {
            return Err("report_finding: file is required".into());
        }
        if a.str("message").trim().is_empty() {
            return Err("report_finding: message is required".into());
        }
        let Some(severity) = parse_severity(a.str("severity")) else {
            return Err(format!(
                "report_finding: severity must be one of info/low/medium/high/critical, got {}",
                quote(a.str("severity"))
            )
            .into());
        };
        let (line, end_line) = (a.int("line"), a.int("end_line"));
        if line < 0 {
            return Err("report_finding: line must be >= 0".into());
        }
        if end_line < 0 {
            return Err("report_finding: end_line must be >= 0".into());
        }
        if end_line > 0 {
            if line <= 0 {
                return Err("report_finding: line is required when end_line is set".into());
            }
            if end_line < line {
                return Err("report_finding: end_line must be >= line".into());
            }
        }
        // Normalize single-line ranges.
        let end = if end_line == line { 0 } else { end_line };
        let f = h.report_finding(
            &c.workspace_id,
            &c.agent_code,
            Finding {
                file: a.str("file").into(),
                line,
                end_line: end,
                severity,
                message: a.str("message").into(),
                summary: a.str("summary").trim().into(),
            },
        );
        marshal(&ReportedFinding {
            id: f.public_id(),
            file: &f.file,
            line: f.line,
            end_line: f.end_line,
            severity: f.severity.as_str(),
            message: &f.message,
            summary: &f.summary,
            status: &f.status,
        })
    }
}

const LIST_FINDINGS_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "status": {"type": "string", "enum": ["open","resolved","ignored","invalid","all"], "description": "Filter by status (default open)"}
  }
}"#;

static LIST_FINDINGS_IN: Shape = Shape::anon(&[field("status", "Status", Ty::Str)]);

#[derive(Serialize)]
struct NoteRow<'a> {
    id: &'a str,
    author: &'a str,
    body: &'a str,
    #[serde(rename = "createdAt")]
    created_at: &'a str,
}

fn note_rows(notes: &[FindingNote]) -> Vec<NoteRow<'_>> {
    notes
        .iter()
        .map(|n| NoteRow {
            id: &n.id,
            author: &n.author,
            body: &n.body,
            created_at: &n.created_at,
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FindingRow<'a> {
    id: &'a str,
    file: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    line: i64,
    #[serde(skip_serializing_if = "is_zero")]
    end_line: i64,
    severity: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    summary: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    status_reason: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    updated_by: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<NoteRow<'a>>,
}

#[derive(Serialize)]
struct FindingList<'a> {
    status: &'a str,
    count: usize,
    findings: Vec<FindingRow<'a>>,
}

pub struct ListFindingsTool;

#[async_trait::async_trait]
impl Tool for ListFindingsTool {
    fn name(&self) -> &'static str {
        names::LIST_FINDINGS
    }

    fn description(&self) -> &'static str {
        "List reviewer findings for this workspace. Optional status filter: open (default), \
resolved, ignored, invalid, or all. Returns JSON with status, count, and \
findings[{id,file,line,severity,message,…}]."
    }

    fn schema(&self) -> String {
        LIST_FINDINGS_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let mut status = "open".to_string();
        let raw = c.args.trim();
        if !raw.is_empty() && raw != "null" {
            let a = strict(&c.args, &LIST_FINDINGS_IN)?;
            let s = a.str("status").trim();
            if !s.is_empty() {
                status = s.into();
            }
        }
        if !matches!(
            status.as_str(),
            "open" | "resolved" | "ignored" | "invalid" | "all"
        ) {
            return Err(format!(
                "list_findings: status must be open/resolved/ignored/invalid/all, got {}",
                quote(&status)
            )
            .into());
        }
        let list = h.list_findings(&c.workspace_id, &c.agent_code, &status)?;
        // Structured JSON so the chat UI can render finding rows (and agents can
        // still read ids/paths/messages without scraping prose).
        let findings = list
            .iter()
            .map(|f| FindingRow {
                id: f.public_id(),
                file: &f.file,
                line: f.line,
                end_line: f.end_line,
                severity: f.severity.as_str(),
                message: &f.message,
                summary: &f.summary,
                status: &f.status,
                status_reason: &f.status_reason,
                updated_by: &f.updated_by,
                notes: note_rows(&f.notes),
            })
            .collect();
        marshal(&FindingList {
            status: &status,
            count: list.len(),
            findings,
        })
    }
}

const UPDATE_FINDING_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "finding_id": {"type": "string", "description": "Finding id from list_findings or the fix prompt"},
    "status": {"type": "string", "enum": ["resolved","invalid"], "description": "resolved = fixed; invalid = disputed/wrong"},
    "reason": {"type": "string", "description": "Required when status is invalid — why the finding is wrong or out of scope"}
  },
  "required": ["finding_id","status"]
}"#;

static UPDATE_FINDING_IN: Shape = Shape::anon(&[
    field("finding_id", "FindingID", Ty::Str),
    field("status", "Status", Ty::Str),
    field("reason", "Reason", Ty::Str),
]);

/// Structured JSON for the chat card (id/status/file/reason).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdatedFinding<'a> {
    id: &'a str,
    status: &'a str,
    file: &'a str,
    #[serde(skip_serializing_if = "is_zero")]
    line: i64,
    #[serde(skip_serializing_if = "is_zero")]
    end_line: i64,
    #[serde(skip_serializing_if = "str::is_empty")]
    severity: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    message: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    summary: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    status_reason: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    updated_by: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<NoteRow<'a>>,
}

pub struct UpdateFindingTool;

#[async_trait::async_trait]
impl Tool for UpdateFindingTool {
    fn name(&self) -> &'static str {
        names::UPDATE_FINDING
    }

    fn description(&self) -> &'static str {
        "Update one finding's status. status must be resolved \
(the issue is fixed) or invalid (wrong/out of scope — reason required). Does not \
decide convergence; a later review still re-checks the diff."
    }

    fn schema(&self) -> String {
        UPDATE_FINDING_SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let h = c.host()?;
        let a = strict(&c.args, &UPDATE_FINDING_IN)?;
        if a.str("finding_id").trim().is_empty() {
            return Err("update_finding: finding_id is required".into());
        }
        if !matches!(a.str("status").trim(), "resolved" | "invalid") {
            return Err(format!(
                "update_finding: status must be resolved or invalid, got {}",
                quote(a.str("status"))
            )
            .into());
        }
        let f: StoredFinding = h.update_finding(
            &c.workspace_id,
            &c.agent_code,
            a.str("finding_id"),
            a.str("status"),
            a.str("reason"),
        )?;
        marshal(&UpdatedFinding {
            id: f.public_id(),
            status: &f.status,
            file: &f.file,
            line: f.line,
            end_line: f.end_line,
            severity: f.severity.as_str(),
            message: &f.message,
            summary: &f.summary,
            status_reason: &f.status_reason,
            updated_by: &f.updated_by,
            notes: note_rows(&f.notes),
        })
    }
}
