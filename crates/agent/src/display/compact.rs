//! Compact, still-parseable envelopes for listing-tool results over the chat
//! detail limit. A mid-JSON byte cut made the card dump the minified blob.

use recur_domain::{BriefingSnapshot, PresentWorkspaceResult, WorkspaceBrief, WorkspaceContext};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::null_default;
use super::{CHAT_DETAIL_LIMIT, is_list_findings_tool, is_tool, truncate};

/// The compact form of a listing tool's result (`list_findings`,
/// `list_artifacts`, `list_workspace_briefs`, `get_workspace_context`,
/// `present_workspace`); `None` for other tools or results that are not the
/// tool's envelope.
pub fn compact_tool_result(tool: &str, result: &str) -> Option<String> {
    if is_list_findings_tool(tool)
        && let Some(c) = list_findings(result)
    {
        return Some(c);
    }
    if is_tool(tool, "list_artifacts")
        && let Some(c) = list_artifacts(result)
    {
        return Some(c);
    }
    if is_tool(tool, "list_workspace_briefs")
        && let Some(c) = list_workspace_briefs(result)
    {
        return Some(c);
    }
    if is_tool(tool, "get_workspace_context")
        && let Some(c) = workspace_context(result)
    {
        return Some(c);
    }
    if is_tool(tool, "present_workspace")
        && let Some(c) = present_workspace(result)
    {
        return Some(c);
    }
    None
}

fn fits_as_is(result: &str) -> bool {
    result.len() <= CHAT_DETAIL_LIMIT
        && serde_json::from_str::<serde::de::IgnoredAny>(result).is_ok()
}

fn marshal<T: Serialize>(v: &T) -> Option<String> {
    serde_json::to_string(v).ok()
}

fn marshal_if_under<T: Serialize>(v: &T) -> Option<String> {
    marshal(v).filter(|b| b.len() <= CHAT_DETAIL_LIMIT)
}

/// Decodes a struct: `null` is the zero value.
fn decode<T: for<'de> Deserialize<'de> + Default>(raw: &str) -> Option<T> {
    serde_json::from_str::<Option<T>>(raw)
        .ok()
        .map(Option::unwrap_or_default)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct FindingsEnvelope {
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(deserialize_with = "null_default")]
    count: i64,
    findings: Option<Vec<Box<RawValue>>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ArtifactsEnvelope {
    #[serde(deserialize_with = "null_default")]
    count: i64,
    artifacts: Option<Vec<Box<RawValue>>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct FindingNote {
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    id: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    author: String,
    #[serde(deserialize_with = "null_default")]
    body: String,
    #[serde(
        rename = "createdAt",
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct FindingRow {
    #[serde(deserialize_with = "null_default")]
    id: String,
    #[serde(deserialize_with = "null_default")]
    file: String,
    #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
    line: i64,
    #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
    end_line: i64,
    #[serde(deserialize_with = "null_default")]
    severity: String,
    #[serde(deserialize_with = "null_default")]
    message: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    summary: String,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    status_reason: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    updated_by: String,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_default"
    )]
    notes: Vec<FindingNote>,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn list_findings(result: &str) -> Option<String> {
    if fits_as_is(result) {
        return Some(result.to_owned());
    }
    let env: FindingsEnvelope = decode(result)?;
    // Missing findings means an MCP wrapper or error object; [] is a real
    // empty list.
    let findings = env.findings?;
    let count = if env.count == 0 {
        findings.len() as i64
    } else {
        env.count
    };
    let mut rows: Vec<FindingRow> = findings.iter().filter_map(|f| decode(f.get())).collect();
    #[derive(Serialize)]
    struct Out<'a> {
        status: &'a str,
        count: i64,
        findings: Vec<FindingRow>,
    }
    // 800 keeps notes and longer prose; 120 drops notes and tightens text.
    // Both steps cut every variable field so the ladder is monotonic.
    let mut budget = 800;
    loop {
        let b = marshal(&Out {
            status: &env.status,
            count,
            findings: shrink_findings(&rows, budget),
        })?;
        if b.len() <= CHAT_DETAIL_LIMIT {
            return Some(b);
        }
        if budget > 120 {
            budget = 120;
        } else if rows.pop().is_none() {
            return Some(b);
        }
    }
}

fn shrink_findings(rows: &[FindingRow], budget: usize) -> Vec<FindingRow> {
    let keep_notes = budget > 120;
    let cut = |s: &mut String| {
        if s.len() > budget {
            *s = truncate(s, budget);
        }
    };
    rows.iter()
        .map(|r| {
            let mut r = r.clone();
            if keep_notes {
                r.notes.iter_mut().for_each(|n| cut(&mut n.body));
            } else {
                r.notes.clear();
            }
            cut(&mut r.message);
            cut(&mut r.summary);
            cut(&mut r.status_reason);
            r
        })
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ArtifactRow {
    #[serde(deserialize_with = "null_default")]
    id: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    workspace_id: String,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(deserialize_with = "null_default")]
    kind: String,
    #[serde(deserialize_with = "null_default")]
    path: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    created_at: String,
    #[serde(
        skip_serializing_if = "String::is_empty",
        deserialize_with = "null_default"
    )]
    created_by: String,
}

fn list_artifacts(result: &str) -> Option<String> {
    if fits_as_is(result) {
        return Some(result.to_owned());
    }
    let env: ArtifactsEnvelope = decode(result)?;
    let artifacts = env.artifacts?;
    let count = if env.count == 0 {
        artifacts.len() as i64
    } else {
        env.count
    };
    let mut rows: Vec<ArtifactRow> = artifacts.iter().filter_map(|a| decode(a.get())).collect();
    #[derive(Serialize)]
    struct Out {
        count: i64,
        artifacts: Vec<ArtifactRow>,
    }
    // 800 keeps a readable title; 120 is the last shrink before dropping rows
    // so one oversized title cannot empty the list while count stays N.
    let mut budget = 800;
    loop {
        let artifacts = rows
            .iter()
            .map(|r| {
                let mut r = r.clone();
                if r.title.len() > budget {
                    r.title = truncate(&r.title, budget);
                }
                r
            })
            .collect();
        let b = marshal(&Out { count, artifacts })?;
        if b.len() <= CHAT_DETAIL_LIMIT {
            return Some(b);
        }
        if budget > 120 {
            budget = 120;
        } else if rows.pop().is_none() {
            return Some(b);
        }
    }
}

fn strip_brief(b: &mut WorkspaceBrief) {
    b.plan_summary.clear();
    b.last_summary.clear();
    b.active_agent_task.clear();
    b.secondary.clear();
}

fn list_workspace_briefs(result: &str) -> Option<String> {
    if fits_as_is(result) {
        return Some(result.to_owned());
    }
    let snap: BriefingSnapshot = decode(result)?;
    let mut rows = snap.workspaces?;
    rows.iter_mut().for_each(strip_brief);
    loop {
        let b = marshal(&BriefingSnapshot {
            generated_at: snap.generated_at,
            counts: snap.counts.clone(),
            workspaces: Some(rows.clone()),
        })?;
        if b.len() <= CHAT_DETAIL_LIMIT || rows.pop().is_none() {
            return Some(b);
        }
    }
}

fn workspace_context(result: &str) -> Option<String> {
    if fits_as_is(result) {
        return Some(result.to_owned());
    }
    let mut c: WorkspaceContext = decode(result)?;
    if c.brief.workspace_id.trim().is_empty() && c.brief.title.trim().is_empty() {
        return None;
    }
    c.conversation.clear();
    if let Some(b) = marshal_if_under(&c) {
        return Some(b);
    }
    c.plan_steps.iter_mut().for_each(|s| s.note.clear());
    if let Some(b) = marshal_if_under(&c) {
        return Some(b);
    }
    strip_brief(&mut c.brief);
    if let Some(b) = marshal_if_under(&c) {
        return Some(b);
    }
    c.plan_steps.clear();
    c.agents.clear();
    c.recent_chats.clear();
    c.open_asks.clear();
    c.pr = None;
    if let Some(b) = marshal_if_under(&c) {
        return Some(b);
    }
    marshal(&c)
}

fn present_workspace(result: &str) -> Option<String> {
    if fits_as_is(result) {
        return Some(result.to_owned());
    }
    let mut p: PresentWorkspaceResult = decode(result)?;
    if !p.ok && p.target.is_empty() && p.error.is_empty() {
        return None;
    }
    if let Some(b) = &mut p.brief {
        strip_brief(b);
    }
    if let Some(b) = marshal_if_under(&p) {
        return Some(b);
    }
    marshal(&p)
}
