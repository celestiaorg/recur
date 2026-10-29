//! The chat turn-end findings digest (frontend `stillOpenFindingsForTurn`
//! in `lib/agentchat.ts`): findings filed in a turn and still open, from
//! modern JSON `report_finding` results or legacy prose results matched
//! through the tool input.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::chat::AgentMsg;
use super::js;

/// An operator note on a digest finding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnDigestNote {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// One still-open finding for the digest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnDigestFinding {
    pub id: String,
    pub file: String,
    pub line: i64,
    pub end_line: i64,
    pub severity: String,
    /// The finding message (accordion title).
    pub message: String,
    /// The end-user body: the summary, else the message.
    pub text: String,
    pub notes: Vec<TurnDigestNote>,
}

/// The finding fields the digest reads from the workspace store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DigestFindingRef {
    pub id: String,
    pub short_id: String,
    pub file: String,
    pub line: i64,
    pub end_line: i64,
    pub severity: String,
    pub message: String,
    pub summary: String,
    pub status: String,
    pub notes: Vec<TurnDigestNote>,
}

/// The public id: the short id when set (frontend `findingPublicId`).
pub fn finding_public_id(id: &str, short_id: &str) -> String {
    if short_id.is_empty() { id } else { short_id }.to_owned()
}

/// Whether `r` is this finding's UUID or short id.
pub fn finding_matches_ref(f: &DigestFindingRef, r: &str) -> bool {
    !r.is_empty() && (f.id == r || (!f.short_id.is_empty() && f.short_id == r))
}

fn is_report_finding_tool(tool: &str) -> bool {
    tool.ends_with("__report_finding") || tool == "report_finding"
}

fn is_update_finding_tool(tool: &str) -> bool {
    tool.ends_with("__update_finding") || tool == "update_finding"
}

static WS_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("[{}]+", js::WS_CLASS)).expect("ws re"));

fn norm_finding_text(s: &str) -> String {
    js::trim(&WS_RUN.replace_all(s, " ")).to_owned()
}

fn digest_notes(notes: &[TurnDigestNote]) -> Vec<TurnDigestNote> {
    notes
        .iter()
        .filter(|n| !js::trim(&n.body).is_empty())
        .cloned()
        .collect()
}

fn or_str(a: &str, b: &str) -> String {
    if a.is_empty() { b } else { a }.to_owned()
}

fn or_num(a: i64, b: i64) -> i64 {
    if a > 0 { a } else { b }
}

/// A report_finding row resolved to its finding (JSON result, else a store
/// match on the tool input, else the input itself).
fn resolve_reported_finding(
    m: &AgentMsg,
    findings: &[DigestFindingRef],
) -> Option<DigestFindingRef> {
    if !m.result.is_empty()
        && !m.result_error
        && let Some(raw) = js::parse_object(&m.result)
        && js::truthy(raw.get("id"))
    {
        let id = js::string_of(raw.get("id"));
        let store = findings.iter().find(|f| finding_matches_ref(f, &id));
        let s = |k: &str| js::str_field(raw.get(k)).to_owned();
        let n = |k: &str| js::num_field(raw.get(k));
        return Some(DigestFindingRef {
            id: store.map_or(id.clone(), |f| finding_public_id(&f.id, &f.short_id)),
            short_id: String::new(),
            file: or_str(store.map_or("", |f| &f.file), &s("file")),
            line: or_num(store.map_or(0, |f| f.line), n("line")),
            end_line: or_num(store.map_or(0, |f| f.end_line), n("endLine")),
            severity: or_str(store.map_or("", |f| &f.severity), &s("severity")),
            message: or_str(store.map_or("", |f| &f.message), &s("message")),
            summary: or_str(store.map_or("", |f| &f.summary), &s("summary")),
            status: or_str(
                store.map_or("", |f| &f.status),
                &or_str(&s("status"), "open"),
            ),
            notes: store.map_or_else(Vec::new, |f| digest_notes(&f.notes)),
        });
    }
    let p: Map<String, Value> = js::parse_object(&m.input)?;
    let s = |k: &str| js::str_field(p.get(k)).to_owned();
    let n = |k: &str| js::num_field(p.get(k));
    let (file, message, line, end_line, severity, summary) = (
        s("file"),
        s("message"),
        n("line"),
        n("end_line"),
        s("severity"),
        s("summary"),
    );
    if file.is_empty() && message.is_empty() {
        return None;
    }
    let msg = norm_finding_text(&message);
    let hit = findings
        .iter()
        .find(|f| f.file == file && f.line == line && norm_finding_text(&f.message) == msg)
        .or_else(|| {
            (!file.is_empty())
                .then(|| {
                    findings
                        .iter()
                        .find(|f| f.file == file && f.line == line && f.severity == severity)
                })
                .flatten()
        });
    Some(match hit {
        Some(f) => DigestFindingRef {
            id: finding_public_id(&f.id, &f.short_id),
            short_id: String::new(),
            file: or_str(&f.file, &file),
            line: or_num(f.line, line),
            end_line: or_num(f.end_line, end_line),
            severity: or_str(&f.severity, &severity),
            message: or_str(&f.message, &message),
            summary: or_str(&f.summary, &summary),
            status: or_str(&f.status, "open"),
            notes: digest_notes(&f.notes),
        },
        // No store row yet (or already purged): surface the filing itself.
        None => DigestFindingRef {
            file,
            line,
            end_line,
            severity,
            message,
            summary,
            status: "open".into(),
            ..DigestFindingRef::default()
        },
    })
}

/// The still-open findings filed in the turn that ends at `end_idx`.
pub fn still_open_findings_for_turn(
    messages: &[AgentMsg],
    end_idx: usize,
    findings: &[DigestFindingRef],
) -> Vec<TurnDigestFinding> {
    let end_idx = end_idx.min(messages.len());
    let start = messages[..end_idx]
        .iter()
        .rposition(|m| m.turn_end || m.me)
        .map_or(0, |i| i + 1);
    let mut filed = Vec::new();
    let mut closed = HashSet::new();
    for m in &messages[start..end_idx] {
        if m.tool.is_empty() || m.result_error {
            continue;
        }
        if is_report_finding_tool(&m.tool) {
            filed.extend(resolve_reported_finding(m, findings));
            continue;
        }
        if is_update_finding_tool(&m.tool) {
            let p = js::parse_object(&m.input).unwrap_or_default();
            let id = js::str_field(p.get("finding_id"));
            let status = js::str_field(p.get("status"));
            if !id.is_empty() && (status == "resolved" || status == "invalid") {
                let live = findings.iter().find(|f| finding_matches_ref(f, id));
                closed
                    .insert(live.map_or(id.to_owned(), |f| finding_public_id(&f.id, &f.short_id)));
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for f in filed {
        let key = if f.id.is_empty() {
            format!("{}|{}|{}", f.file, f.line, norm_finding_text(&f.message))
        } else {
            f.id.clone()
        };
        if !seen.insert(key) || (!f.id.is_empty() && closed.contains(&f.id)) {
            continue;
        }
        if or_str(&f.status, "open") != "open" {
            continue;
        }
        // Prefer the live store fields when there is an id.
        let mut notes = digest_notes(&f.notes);
        let mut message = norm_finding_text(&f.message);
        let mut summary = norm_finding_text(&f.summary);
        if !f.id.is_empty() {
            let live = findings.iter().find(|x| finding_matches_ref(x, &f.id));
            if live.is_some_and(|l| or_str(&l.status, "open") != "open") {
                continue;
            }
            if let Some(l) = live {
                if !l.notes.is_empty() {
                    notes = digest_notes(&l.notes);
                }
                let lm = norm_finding_text(&l.message);
                if !lm.is_empty() {
                    message = lm;
                }
                let ls = norm_finding_text(&l.summary);
                if !ls.is_empty() {
                    summary = ls;
                }
            }
        }
        out.push(TurnDigestFinding {
            id: f.id,
            file: f.file,
            line: f.line,
            end_line: f.end_line,
            severity: f.severity,
            text: if summary.is_empty() {
                message.clone()
            } else {
                summary
            },
            message,
            notes,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(tool: &str, input: &str, result: &str) -> AgentMsg {
        AgentMsg {
            tool: tool.into(),
            input: input.into(),
            result: result.into(),
            ..AgentMsg::default()
        }
    }

    fn me(body: &str) -> AgentMsg {
        AgentMsg {
            me: true,
            body: body.into(),
            ..AgentMsg::default()
        }
    }

    fn end() -> AgentMsg {
        AgentMsg {
            turn_end: true,
            duration_ms: 12000,
            ..AgentMsg::default()
        }
    }

    #[test]
    fn collects_codex_report_finding_rows() {
        let filed = DigestFindingRef {
            id: "f-1".into(),
            file: "store.go".into(),
            line: 1276,
            end_line: 1286,
            severity: "high".into(),
            message: "activity hook can miss a turn".into(),
            summary: "Hook misses turn".into(),
            status: "open".into(),
            ..DigestFindingRef::default()
        };
        let msgs = [
            me("review this"),
            tool(
                "mcp__recur__report_finding",
                &json!({"file": "store.go", "line": 1276, "end_line": 1286, "severity": "high",
                    "message": "activity hook can miss a turn", "summary": "Hook misses turn"})
                .to_string(),
                &json!({"id": "f-1", "file": "store.go", "line": 1276, "endLine": 1286,
                    "severity": "high", "message": "activity hook can miss a turn",
                    "summary": "Hook misses turn", "status": "open"})
                .to_string(),
            ),
            AgentMsg {
                body: "Filed 1 finding.".into(),
                ..AgentMsg::default()
            },
            end(),
        ];
        assert_eq!(
            still_open_findings_for_turn(&msgs, 3, &[filed]),
            [TurnDigestFinding {
                id: "f-1".into(),
                file: "store.go".into(),
                line: 1276,
                end_line: 1286,
                severity: "high".into(),
                message: "activity hook can miss a turn".into(),
                text: "Hook misses turn".into(),
                notes: vec![],
            }]
        );
    }

    #[test]
    fn update_finding_by_uuid_closes_a_short_id_filing() {
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let filed = DigestFindingRef {
            id: uuid.into(),
            short_id: "find_k7mq2xf9".into(),
            file: "store.go".into(),
            line: 10,
            message: "nil deref".into(),
            status: "open".into(),
            ..DigestFindingRef::default()
        };
        let msgs = [
            me("review this"),
            tool(
                "mcp__recur__report_finding",
                "",
                &json!({"id": uuid, "file": "store.go", "line": 10, "message": "nil deref", "status": "open"})
                    .to_string(),
            ),
            tool(
                "mcp__recur__update_finding",
                &json!({"finding_id": uuid, "status": "resolved"}).to_string(),
                "{}",
            ),
            AgentMsg {
                turn_end: true,
                ..AgentMsg::default()
            },
        ];
        assert!(still_open_findings_for_turn(&msgs, 3, &[filed]).is_empty());
    }

    #[test]
    fn legacy_prose_results_match_the_store_by_input() {
        let store = DigestFindingRef {
            id: "uuid-1".into(),
            short_id: "find_abc".into(),
            file: "a.go".into(),
            line: 3,
            severity: "low".into(),
            message: "the  bug".into(),
            notes: vec![
                TurnDigestNote {
                    body: "  ".into(),
                    ..TurnDigestNote::default()
                },
                TurnDigestNote {
                    id: "n1".into(),
                    body: "seen".into(),
                    ..TurnDigestNote::default()
                },
            ],
            ..DigestFindingRef::default()
        };
        let input =
            json!({"file": "a.go", "line": 3, "message": "the bug", "severity": "low"}).to_string();
        let msgs = [
            me("go"),
            tool("report_finding", &input, "Recorded a low finding in a.go"),
            tool("report_finding", &input, "Recorded again"),
            tool(
                "mcp__recur__report_finding",
                &json!({"file": "b.go", "message": "new"}).to_string(),
                "",
            ),
        ];
        let got = still_open_findings_for_turn(&msgs, 4, std::slice::from_ref(&store));
        assert_eq!(got.len(), 2, "duplicate filings fold by id");
        assert_eq!(
            (got[0].id.as_str(), got[0].message.as_str()),
            ("find_abc", "the bug")
        );
        assert_eq!(got[0].notes.len(), 1);
        assert_eq!(
            (
                got[1].id.as_str(),
                got[1].file.as_str(),
                got[1].text.as_str()
            ),
            ("", "b.go", "new")
        );
        let resolved = DigestFindingRef {
            status: "resolved".into(),
            ..store
        };
        let got = still_open_findings_for_turn(&msgs, 4, &[resolved]);
        assert_eq!(got.len(), 1, "a closed store row drops out");
    }

    #[test]
    fn errors_and_earlier_turns_are_ignored() {
        let input = json!({"file": "a.go", "message": "x"}).to_string();
        let mut failed = tool("report_finding", &input, "");
        failed.result_error = true;
        let msgs = [tool("report_finding", &input, ""), me("next"), failed];
        assert!(still_open_findings_for_turn(&msgs, 3, &[]).is_empty());
        assert_eq!(still_open_findings_for_turn(&msgs, 1, &[]).len(), 1);
    }
}
