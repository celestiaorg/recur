//! Structured reviewer findings that drive convergence.

use serde::{Deserialize, Serialize};

use crate::serde_util::is_zero;

string_enum! {
    /// Severity of a reviewer finding.
    Severity {
        Info = "info",
        Low = "low",
        Medium = "medium",
        High = "high",
        Critical = "critical",
    }
}

string_enum! {
    /// The lifecycle of a reviewer finding.
    FindingStatus {
        /// Active; counts toward convergence.
        Open = "open",
        /// Coder (or engine) marked addressed.
        Resolved = "resolved",
        /// Human: out of scope / not worth fixing.
        Ignored = "ignored",
        /// Coder or human: finding is wrong / disputed.
        Invalid = "invalid",
    }
}

// Who last changed a finding's status (attribution).
pub const FINDING_ACTOR_HUMAN: &str = "human";
pub const FINDING_ACTOR_CODER: &str = "coder";
pub const FINDING_ACTOR_REVIEWER: &str = "reviewer";
pub const FINDING_ACTOR_ENGINE: &str = "engine";
pub const FINDING_SOURCE_USER: &str = "user";

/// Reports whether s is a known finding status.
pub fn valid_finding_status(s: &str) -> bool {
    FindingStatus::from(s).is_known()
}

/// An append-only operator comment on a finding (humans write; agents read
/// via list_findings). Independent of statusReason.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FindingNote {
    /// UUID.
    pub id: String,
    /// FINDING_ACTOR_* (human for v1).
    pub author: String,
    pub body: String,
    /// RFC3339.
    pub created_at: String,
}

/// A structured reviewer report (PRD §8.3 report_finding).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Finding {
    /// UUID, the internal storage key.
    pub id: String,
    /// find_… for agents and humans; unique per workspace.
    pub short_id: String,
    pub workspace_id: String,
    pub iteration: i64,
    pub file: String,
    /// 1-based start (0 if not line-specific).
    pub line: i64,
    /// Inclusive end; 0 or == Line means a single line.
    #[serde(skip_serializing_if = "is_zero")]
    pub end_line: i64,
    pub severity: Severity,
    pub message: String,
    /// Optional user-facing digest (markdown).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// open/resolved/ignored/invalid.
    pub status: String,
    /// Required when the coder marks invalid.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status_reason: String,
    /// RFC3339, stamped on first persist.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// human|coder|reviewer|engine.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_by: String,
    /// RFC3339.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<FindingNote>,
    /// "user" for operator notes from the Observatory review surface; empty
    /// means a reviewer/agent finding.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
}

impl Finding {
    /// The identifier agents use (short id), falling back to ID for legacy
    /// rows that have not been backfilled.
    pub fn public_id(&self) -> &str {
        if self.short_id.is_empty() {
            &self.id
        } else {
            &self.short_id
        }
    }

    /// Reports whether ref is this finding's UUID or public short id.
    pub fn matches_ref(&self, r: &str) -> bool {
        !r.is_empty() && (self.id == r || self.short_id == r)
    }

    /// Reports whether a 1-based line falls in this finding's span.
    pub fn covers_line(&self, line_no: i64) -> bool {
        if self.line <= 0 || line_no <= 0 {
            return false;
        }
        if self.end_line > self.line {
            return (self.line..=self.end_line).contains(&line_no);
        }
        line_no == self.line
    }
}

/// Formats file[:line] or file:start-end for display/prompts.
pub fn finding_loc(file: &str, line: i64, end_line: i64) -> String {
    if file.is_empty() {
        return String::new();
    }
    if line <= 0 {
        return file.to_owned();
    }
    if end_line > line {
        return format!("{file}:{line}-{end_line}");
    }
    format!("{file}:{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_validation() {
        for s in ["open", "resolved", "ignored", "invalid"] {
            assert!(valid_finding_status(s), "{s}");
        }
        assert!(!valid_finding_status("") && !valid_finding_status("Open"));
    }

    #[test]
    fn loc_and_covers_line() {
        assert_eq!(finding_loc("", 3, 5), "");
        assert_eq!(finding_loc("a.go", 0, 5), "a.go");
        assert_eq!(finding_loc("a.go", 3, 5), "a.go:3-5");
        assert_eq!(finding_loc("a.go", 3, 3), "a.go:3");
        assert_eq!(finding_loc("a.go", 3, 0), "a.go:3");

        let span = Finding {
            line: 3,
            end_line: 5,
            ..Default::default()
        };
        assert!(span.covers_line(3) && span.covers_line(5));
        assert!(!span.covers_line(6) && !span.covers_line(0));
        let single = Finding {
            line: 3,
            end_line: 2,
            ..Default::default()
        };
        assert!(single.covers_line(3) && !single.covers_line(2));
        assert!(!Finding::default().covers_line(1));
    }
}
