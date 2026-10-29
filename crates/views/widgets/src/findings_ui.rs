//! The finding list rows the chat shares with the Findings panel: titles, locations,
//! severity icons and tallies, and the
//! `list_findings` / `update_finding` payloads hydrated from the workspace store.

use recur_domain::Finding;
use recur_engine::view::digest::{DigestFindingRef, TurnDigestNote};
use recur_ui::IconName;
use serde_json::{Map, Value};

/// True when `r` is this finding's UUID or public short id (`findingMatchesRef`).
pub fn finding_matches_ref(f: &Finding, r: &str) -> bool {
    !r.is_empty() && (f.id == r || (!f.short_id.is_empty() && f.short_id == r))
}

/// A finding's notes as the digest shows them.
pub fn finding_notes(f: &Finding) -> Vec<TurnDigestNote> {
    f.notes
        .iter()
        .map(|n| TurnDigestNote {
            id: n.id.clone(),
            author: n.author.clone(),
            body: n.body.clone(),
            created_at: n.created_at.clone(),
        })
        .collect()
}

/// The store row as the engine digest reads it.
pub fn digest_ref(f: &Finding) -> DigestFindingRef {
    DigestFindingRef {
        id: f.id.clone(),
        short_id: f.short_id.clone(),
        file: f.file.clone(),
        line: f.line,
        end_line: f.end_line,
        severity: f.severity.as_str().to_string(),
        message: f.message.clone(),
        summary: f.summary.clone(),
        status: f.status.clone(),
        notes: finding_notes(f),
    }
}

/// Severity → the dense-row icon (`findingSevIcon`).
pub fn finding_sev_icon(severity: &str) -> IconName {
    match severity.to_lowercase().as_str() {
        "critical" => IconName::SevCritical,
        "high" => IconName::SevHigh,
        "medium" => IconName::SevMedium,
        "info" => IconName::SevInfo,
        _ => IconName::SevLow,
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first sentence or line for the dense list title (`findingTitle`).
pub fn finding_title(msg: &str) -> String {
    let t = collapse_ws(msg);
    if t.is_empty() {
        return "Finding".into();
    }
    let chars: Vec<char> = t.chars().collect();
    let cut = (0..chars.len()).find(|&i| {
        matches!(chars[i], '.' | '!' | '?') && chars.get(i + 1).is_none_or(|c| c.is_whitespace())
    });
    let head: String = match cut {
        Some(c) if c > 0 && utf16_len(&chars[..c]) <= 110 => chars[..=c].iter().collect(),
        _ => t.clone(),
    };
    if head.encode_utf16().count() > 120 {
        let mut out = String::new();
        let mut units = 0;
        for c in head.chars() {
            if units + c.len_utf16() > 117 {
                break;
            }
            units += c.len_utf16();
            out.push(c);
        }
        return format!("{out}…");
    }
    head
}

fn utf16_len(chars: &[char]) -> usize {
    chars.iter().map(|c| c.len_utf16()).sum()
}

/// True when the full message adds detail beyond the title line (`findingBodyNeeded`).
pub fn finding_body_needed(msg: &str) -> bool {
    let title = finding_title(msg);
    if msg.trim().is_empty() || title == "Finding" {
        return !msg.trim().is_empty();
    }
    let flat = collapse_ws(msg);
    flat.encode_utf16().count() > title.encode_utf16().count() + 2 || msg.contains('\n')
}

/// `:12` or `:12–40` (en dash); "" without a line.
pub fn finding_loc_suffix(line: i64, end_line: i64) -> String {
    let start = line.max(0);
    if start == 0 {
        return String::new();
    }
    if end_line > start {
        format!(":{start}–{end_line}")
    } else {
        format!(":{start}")
    }
}

/// `file.go:12–40` for dense rows: the basename only.
pub fn finding_loc_label(file: &str, line: i64, end_line: i64) -> String {
    let name = file.rsplit('/').next().unwrap_or(file);
    let loc = finding_loc_suffix(line, end_line);
    if !name.is_empty() {
        return format!("{name}{loc}");
    }
    loc.trim_start_matches(':').to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeverityTally {
    pub severity: String,
    pub count: usize,
}

const SEV_ORDER: [&str; 5] = ["critical", "high", "medium", "low", "info"];

/// `findingSeverityTally`: known severities in order, then any others.
pub fn finding_severity_tally<'a>(
    severities: impl IntoIterator<Item = &'a str>,
) -> Vec<SeverityTally> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for s in severities {
        let s = if s.is_empty() { "info" } else { s }.to_lowercase();
        match counts.iter_mut().find(|(k, _)| *k == s) {
            Some(c) => c.1 += 1,
            None => counts.push((s, 1)),
        }
    }
    let mut out: Vec<SeverityTally> = SEV_ORDER
        .iter()
        .filter_map(|s| {
            counts
                .iter()
                .find(|(k, _)| k == s)
                .map(|(_, n)| SeverityTally {
                    severity: s.to_string(),
                    count: *n,
                })
        })
        .collect();
    for (k, n) in &counts {
        if !SEV_ORDER.contains(&k.as_str()) {
            out.push(SeverityTally {
                severity: k.clone(),
                count: *n,
            });
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedFindingNote {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// One finding as `list_findings` / `update_finding` JSON (`ListedFinding`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedFinding {
    pub id: String,
    pub file: String,
    pub line: i64,
    pub end_line: i64,
    pub severity: String,
    pub message: String,
    pub summary: String,
    pub status: String,
    pub status_reason: String,
    pub notes: Vec<ListedFindingNote>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedFindings {
    pub status: String,
    pub count: i64,
    pub findings: Vec<ListedFinding>,
}

fn str_of(o: &Map<String, Value>, k: &str) -> String {
    o.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn num_of(o: &Map<String, Value>, k: &str) -> i64 {
    o.get(k).and_then(Value::as_f64).map_or(0, |n| n as i64)
}

fn parse_notes(raw: Option<&Value>) -> Vec<ListedFindingNote> {
    let Some(Value::Array(list)) = raw else {
        return Vec::new();
    };
    list.iter()
        .map(|n| {
            let empty = Map::new();
            let o = n.as_object().unwrap_or(&empty);
            ListedFindingNote {
                id: str_of(o, "id"),
                author: str_of(o, "author"),
                body: str_of(o, "body"),
                created_at: str_of(o, "createdAt"),
            }
        })
        .filter(|n| !n.body.trim().is_empty())
        .collect()
}

/// `parseListedFinding`.
pub fn parse_listed_finding(f: &Value) -> ListedFinding {
    let empty = Map::new();
    let o = f.as_object().unwrap_or(&empty);
    let id = match o.get("id") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(v) => recur_engine::view::js::string_of(Some(v)),
    };
    ListedFinding {
        id,
        file: str_of(o, "file"),
        line: num_of(o, "line"),
        end_line: num_of(o, "endLine"),
        severity: str_of(o, "severity"),
        message: str_of(o, "message"),
        summary: str_of(o, "summary"),
        status: str_of(o, "status"),
        status_reason: str_of(o, "statusReason"),
        notes: parse_notes(o.get("notes")),
    }
}

/// A store row as the list sees it.
pub fn listed_from_store(f: &Finding) -> ListedFinding {
    ListedFinding {
        id: f.id.clone(),
        file: f.file.clone(),
        line: f.line,
        end_line: f.end_line,
        severity: f.severity.as_str().to_string(),
        message: f.message.clone(),
        summary: f.summary.clone(),
        status: f.status.clone(),
        status_reason: f.status_reason.clone(),
        notes: f
            .notes
            .iter()
            .filter(|n| !n.body.trim().is_empty())
            .map(|n| ListedFindingNote {
                id: n.id.clone(),
                author: n.author.clone(),
                body: n.body.clone(),
                created_at: n.created_at.clone(),
            })
            .collect(),
    }
}

/// A JSON object, accepting a prose prefix before the first `{`.
pub fn parse_json_object(text: &str) -> Option<Map<String, Value>> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let try_parse = |s: &str| match serde_json::from_str::<Value>(s) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    };
    if let Some(m) = try_parse(t) {
        return Some(m);
    }
    match t.find('{') {
        Some(i) if i > 0 => try_parse(&t[i..]),
        _ => None,
    }
}

/// `parseListFindingsResult`.
pub fn parse_list_findings_result(text: &str) -> Option<ListedFindings> {
    let raw = parse_json_object(text)?;
    let Some(Value::Array(rows)) = raw.get("findings") else {
        return None;
    };
    let status = str_of(&raw, "status");
    Some(ListedFindings {
        status: if status.is_empty() {
            "open".into()
        } else {
            status
        },
        count: match raw.get("count") {
            Some(Value::Number(n)) => n.as_f64().map_or(0, |f| f as i64),
            _ => rows.len() as i64,
        },
        findings: rows.iter().map(parse_listed_finding).collect(),
    })
}

fn prefer_payload_text(payload: &str, store: &str) -> String {
    let (p, s) = (payload.trim(), store.trim());
    if p.is_empty() {
        return s.to_string();
    }
    if !s.is_empty() && p.ends_with('…') && s.chars().count() > p.chars().count() {
        return s.to_string();
    }
    p.to_string()
}

fn prefer_payload_line(payload: i64, store: i64) -> i64 {
    if payload > 0 { payload } else { store.max(0) }
}

/// `hydrateListedFindings`: fill truncated rows from the store (same ids).
pub fn hydrate_listed_findings(listed: ListedFindings, store: &[Finding]) -> ListedFindings {
    if store.is_empty() {
        return listed;
    }
    ListedFindings {
        findings: listed
            .findings
            .into_iter()
            .map(|f| {
                let Some(s) = (!f.id.is_empty())
                    .then(|| store.iter().find(|x| x.id == f.id))
                    .flatten()
                else {
                    return f;
                };
                let s = listed_from_store(s);
                ListedFinding {
                    file: if f.file.is_empty() { s.file } else { f.file },
                    line: prefer_payload_line(f.line, s.line),
                    end_line: prefer_payload_line(f.end_line, s.end_line),
                    severity: if f.severity.is_empty() {
                        s.severity
                    } else {
                        f.severity
                    },
                    message: prefer_payload_text(&f.message, &s.message),
                    summary: prefer_payload_text(&f.summary, &s.summary),
                    status: if !f.status.is_empty() {
                        f.status
                    } else if !s.status.is_empty() {
                        s.status
                    } else {
                        "open".into()
                    },
                    notes: if f.notes.is_empty() { s.notes } else { f.notes },
                    ..f
                }
            })
            .collect(),
        ..listed
    }
}

/// `resolveListFindings`: the tool payload, hydrated; `None` when unparseable or failed.
pub fn resolve_list_findings(
    result: &str,
    result_error: bool,
    store: &[Finding],
) -> Option<ListedFindings> {
    if result_error {
        return None;
    }
    parse_list_findings_result(result).map(|l| hydrate_listed_findings(l, store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row() -> Value {
        json!({
            "id": "F-1", "file": "docs/plan.md", "line": 10, "endLine": 12,
            "severity": "high", "message": "Deadlock in qa verify.", "status": "open",
        })
    }

    fn store(f: impl FnOnce(&mut Finding)) -> Finding {
        let mut x = Finding {
            id: "F-1".into(),
            file: "docs/plan.md".into(),
            line: 10,
            end_line: 12,
            severity: "high".into(),
            message: "Full store message that the truncated payload lost.".into(),
            status: "open".into(),
            ..Default::default()
        };
        f(&mut x);
        x
    }

    #[test]
    fn loc_labels() {
        assert_eq!(finding_loc_suffix(294, 300), ":294–300");
        assert_eq!(
            finding_loc_label("docs/bundles.md", 294, 300),
            "bundles.md:294–300"
        );
        assert_eq!(
            finding_loc_label("internal/engine/loop.go", 765, 0),
            "loop.go:765"
        );
        assert_eq!(finding_loc_label("", 0, 0), "");
    }

    #[test]
    fn parses_the_envelope() {
        let got = parse_list_findings_result(
            &json!({"status": "all", "count": 1, "findings": [row()]}).to_string(),
        );
        assert_eq!(
            got,
            Some(ListedFindings {
                status: "all".into(),
                count: 1,
                findings: vec![ListedFinding {
                    id: "F-1".into(),
                    file: "docs/plan.md".into(),
                    line: 10,
                    end_line: 12,
                    severity: "high".into(),
                    message: "Deadlock in qa verify.".into(),
                    status: "open".into(),
                    ..Default::default()
                }],
            })
        );
        let wrapped = format!(
            "Listed findings\n{}",
            json!({"status": "open", "findings": [row()]})
        );
        let got = parse_list_findings_result(&wrapped).expect("parsed");
        assert_eq!(got.count, 1);
        assert_eq!(got.findings[0].id, "F-1");
        let raw = json!({"status": "all", "count": 15, "findings": [row(), row()]}).to_string();
        assert_eq!(parse_list_findings_result(&raw[..40]), None);
    }

    #[test]
    fn resolve_hydrates_and_keeps_payload() {
        let got = resolve_list_findings(
            &json!({"status": "all", "count": 1, "findings": [{
                "id": "F-1", "file": "docs/plan.md", "severity": "high",
                "message": "Deadlock…", "status": "open",
            }]})
            .to_string(),
            false,
            &[store(|_| {})],
        )
        .expect("resolved");
        assert_eq!(
            got.findings[0].message,
            "Full store message that the truncated payload lost."
        );
        assert_eq!(got.findings[0].line, 10);
        let got = resolve_list_findings(
            &json!({"status": "open", "count": 1, "findings": [{
                "id": "F-1", "file": "docs/plan.md", "severity": "high",
                "message": "Deadlock in qa verify.", "status": "open",
            }]})
            .to_string(),
            false,
            &[store(|f| {
                f.status = "resolved".into();
                f.severity = "low".into();
            })],
        )
        .expect("resolved");
        assert_eq!(got.findings[0].status, "open");
        assert_eq!(got.findings[0].severity, "high");
        assert_eq!(got.findings[0].message, "Deadlock in qa verify.");
        assert_eq!(
            resolve_list_findings(
                r#"{"status":"all","count":15,"findings":[{"#,
                false,
                &[store(|_| {})]
            ),
            None
        );
        assert_eq!(resolve_list_findings("boom", true, &[store(|_| {})]), None);
    }

    #[test]
    fn severity_tally() {
        assert_eq!(
            finding_severity_tally(["low", "high", "high", "mystery"]),
            vec![
                SeverityTally {
                    severity: "high".into(),
                    count: 2
                },
                SeverityTally {
                    severity: "low".into(),
                    count: 1
                },
                SeverityTally {
                    severity: "mystery".into(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn hydrate_leaves_unknown_ids() {
        let listed = parse_list_findings_result(
            &json!({"status": "open", "findings": [{
                "id": "missing", "file": "a.go", "severity": "low", "message": "x", "status": "open",
            }]})
            .to_string(),
        )
        .expect("parsed");
        assert_eq!(
            hydrate_listed_findings(listed, &[]).findings[0].message,
            "x"
        );
    }

    #[test]
    fn titles() {
        assert_eq!(finding_title(""), "Finding");
        assert_eq!(
            finding_title("Handle the error. Then retry."),
            "Handle the error."
        );
        assert_eq!(finding_title("no stop here"), "no stop here");
        let long = "x".repeat(130);
        assert_eq!(finding_title(&long), format!("{}…", "x".repeat(117)));
        assert!(finding_body_needed(
            "Handle the error. Then retry with backoff."
        ));
        assert!(!finding_body_needed("Handle the error."));
        assert!(finding_body_needed("Handle\nthe error."));
        assert_eq!(finding_sev_icon("HIGH"), IconName::SevHigh);
        assert_eq!(finding_sev_icon(""), IconName::SevLow);
    }
}
