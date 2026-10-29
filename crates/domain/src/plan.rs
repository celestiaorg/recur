//! The structured, living plan artifact: written by Plan-mode chats and the
//! loop planner via submit_plan, edited in the Observatory Plan panel, and the
//! source of real run progress once steps exist.

use std::collections::HashMap;
use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::serde_util::is_false;

// Plan step statuses.
pub const STEP_PENDING: &str = "pending";
pub const STEP_DOING: &str = "doing";
pub const STEP_DONE: &str = "done";
pub const STEP_SKIPPED: &str = "skipped";

/// Body is the durable detailed brief other agents execute from; Summary is
/// the short headline for lists and the panel lead-in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Plan {
    pub summary: String,
    /// The full markdown plan. Empty on legacy summary-only records.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<PlanStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PlanAttachment>,
    /// "plan-chat", "loop-planner", or "human" (panel edit).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// The pair-chat tab that filed this plan (PA, PA2, …). Survives a later
    /// human edit so Run as loop can inherit that chat's Simplify chip.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub chat_code: String,
    /// The human's sign-off.
    #[serde(skip_serializing_if = "is_false")]
    pub approved: bool,
}

/// A file that belongs to the plan: an agent artifact or a user-pasted chat
/// image. Path is relative to the managed data root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanAttachment {
    pub id: String,
    /// Artifact kind (html, md, png, …) or image ext.
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub path: String,
}

/// One concrete, ordered action in a Plan. The engine assigns IDs and owns
/// status transitions (agents request them via update_plan_step).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    /// pending | doing | done | skipped.
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Files the step touches (repo-relative). Steps with disjoint files and
    /// satisfied DependsOn may run in parallel as child workspaces.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Step ids that must be done or skipped first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// The child workspace working this step, when fanned out.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace: String,
}

fn settled(status: &str) -> bool {
    status == STEP_DONE || status == STEP_SKIPPED
}

impl Plan {
    fn lead(&self) -> &str {
        match self.body.trim() {
            "" => self.summary.trim(),
            body => body,
        }
    }

    /// The plan as prose for every free-text consumer (DefinitionOfDone
    /// prompts, the plan editor fallback). Prefers Body over Summary and
    /// includes live step status plus attachment paths.
    pub fn text(&self) -> String {
        let mut b = String::from(self.lead());
        if !self.steps.is_empty() {
            b.push_str("\n\nSteps:\n");
            for (i, s) in self.steps.iter().enumerate() {
                let _ = write!(b, "{}. {}", i + 1, s.title);
                if settled(&s.status) {
                    let _ = write!(b, " ({})", s.status);
                }
                b.push('\n');
            }
        }
        if !self.attachments.is_empty() {
            b.push_str("\n\nAttachments:\n");
            for a in &self.attachments {
                let title = a.title.trim();
                let path = a.path.trim();
                if !path.is_empty() && !title.is_empty() && title != path {
                    let _ = write!(b, "- `{path}` - {title}");
                } else if !path.is_empty() {
                    let _ = write!(b, "- `{path}`");
                } else {
                    let _ = write!(b, "- {title}");
                }
                b.push('\n');
            }
        }
        b.trim().to_owned()
    }

    /// The live execution plan for a replan turn: ids, every status
    /// (including doing), files, depends_on, child ownership, and notes.
    pub fn board(&self) -> String {
        let mut b = String::from(self.lead());
        if !self.steps.is_empty() {
            b.push_str("\n\nSteps:\n");
            for s in &self.steps {
                let st = if s.status.is_empty() {
                    STEP_PENDING
                } else {
                    &s.status
                };
                let _ = write!(b, "{} [{}] {}", s.id, st, s.title);
                if !s.files.is_empty() {
                    let _ = write!(b, " files={}", s.files.join(","));
                }
                if !s.depends_on.is_empty() {
                    let _ = write!(b, " after={}", s.depends_on.join(","));
                }
                let ws = s.workspace.trim();
                if !ws.is_empty() {
                    let _ = write!(b, " child={ws}");
                }
                let note = s.note.trim();
                if !note.is_empty() {
                    let _ = write!(b, " note={note}");
                }
                b.push('\n');
            }
        }
        b.trim().to_owned()
    }

    /// The fraction of steps done or skipped; `None` when there are no steps
    /// to measure.
    pub fn progress(&self) -> Option<f64> {
        if self.steps.is_empty() {
            return None;
        }
        let done = self.steps.iter().filter(|s| settled(&s.status)).count();
        Some(done as f64 / self.steps.len() as f64)
    }

    /// Reports whether a pending step's dependencies are all done/skipped.
    /// Unknown dependency ids never block (a typo must not wedge the board).
    pub fn ready(&self, s: &PlanStep) -> bool {
        if s.status != STEP_PENDING {
            return false;
        }
        let by_id: HashMap<&str, &PlanStep> =
            self.steps.iter().map(|st| (st.id.as_str(), st)).collect();
        s.depends_on
            .iter()
            .all(|dep| by_id.get(dep.as_str()).is_none_or(|d| settled(&d.status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, title: &str, status: &str) -> PlanStep {
        PlanStep {
            id: id.into(),
            title: title.into(),
            status: status.into(),
            ..Default::default()
        }
    }

    #[test]
    fn text_prefers_body_and_includes_steps_and_attachments() {
        let p = Plan {
            summary: "Headline only".into(),
            body: "Touch `internal/engine/plan.go` and keep the checklist honest.".into(),
            steps: vec![
                step("S1", "Add Body to Plan", STEP_DONE),
                step("S2", "Wire submit_plan", STEP_PENDING),
            ],
            attachments: vec![
                PlanAttachment {
                    id: "a1".into(),
                    kind: "md".into(),
                    title: "Sequence diagram".into(),
                    path: "workspaces/w/artifacts/a1.md".into(),
                },
                PlanAttachment {
                    id: "img".into(),
                    kind: "png".into(),
                    title: String::new(),
                    path: "workspaces/w/attachments/img.png".into(),
                },
            ],
            ..Default::default()
        };
        let got = p.text();
        assert!(got.contains("Touch `internal/engine/plan.go`"), "{got}");
        assert!(!got.contains("Headline only"), "{got}");
        assert!(got.contains("1. Add Body to Plan (done)"), "{got}");
        assert!(got.contains("2. Wire submit_plan"), "{got}");
        assert!(
            got.contains("`workspaces/w/artifacts/a1.md` - Sequence diagram"),
            "{got}"
        );
        assert!(got.contains("`workspaces/w/attachments/img.png`"), "{got}");
        assert_eq!(
            got,
            "Touch `internal/engine/plan.go` and keep the checklist honest.\n\nSteps:\n1. Add Body to Plan (done)\n2. Wire submit_plan\n\n\nAttachments:\n- `workspaces/w/artifacts/a1.md` - Sequence diagram\n- `workspaces/w/attachments/img.png`"
        );
    }

    #[test]
    fn board_includes_doing_files_deps_and_child() {
        let p = Plan {
            body: "Importer".into(),
            steps: vec![
                PlanStep {
                    files: vec!["parser.go".into()],
                    workspace: "child-1".into(),
                    ..step("S1", "Parser", STEP_DOING)
                },
                PlanStep {
                    files: vec!["writer.go".into()],
                    depends_on: vec!["S1".into()],
                    note: "waiting".into(),
                    ..step("S2", "Writer", STEP_PENDING)
                },
            ],
            ..Default::default()
        };
        let got = p.board();
        assert!(
            got.contains("S1 [doing] Parser files=parser.go child=child-1"),
            "{got}"
        );
        assert!(
            got.contains("S2 [pending] Writer files=writer.go after=S1 note=waiting"),
            "{got}"
        );
        let text = p.text();
        assert!(
            !text.contains("files=") && !text.contains("[doing]"),
            "{text}"
        );
    }

    #[test]
    fn board_defaults_blank_status_to_pending() {
        let p = Plan {
            summary: " Sum ".into(),
            steps: vec![step("S1", "T", "")],
            ..Default::default()
        };
        assert_eq!(p.board(), "Sum\n\nSteps:\nS1 [pending] T");
    }

    #[test]
    fn text_falls_back_to_summary() {
        let p = Plan {
            summary: "Add a dark-mode toggle".into(),
            steps: vec![step("", "Add the toggle to Settings", STEP_PENDING)],
            ..Default::default()
        };
        let got = p.text();
        assert!(
            got.contains("Add a dark-mode toggle") && got.contains("1. Add the toggle to Settings"),
            "{got}"
        );
    }

    #[test]
    fn text_blank_line_before_attachments_without_steps() {
        let p = Plan {
            body: "Rerun the tests.".into(),
            attachments: vec![PlanAttachment {
                id: "img".into(),
                kind: "png".into(),
                path: "workspaces/w/attachments/img.png".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            p.text().contains("Rerun the tests.\n\nAttachments:\n"),
            "{}",
            p.text()
        );
    }

    #[test]
    fn text_title_only_attachment() {
        let p = Plan {
            attachments: vec![PlanAttachment {
                title: "Mock".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(p.text(), "Attachments:\n- Mock");
    }

    #[test]
    fn progress_and_ready() {
        assert_eq!(Plan::default().progress(), None);
        let p = Plan {
            steps: vec![
                step("S1", "a", STEP_DONE),
                step("S2", "b", STEP_SKIPPED),
                step("S3", "c", STEP_DOING),
                step("S4", "d", STEP_PENDING),
            ],
            ..Default::default()
        };
        assert_eq!(p.progress(), Some(0.5));

        let waits_on_doing = PlanStep {
            depends_on: vec!["S1".into(), "S3".into()],
            ..step("S5", "e", STEP_PENDING)
        };
        assert!(!p.ready(&waits_on_doing));
        let settled_and_typo = PlanStep {
            depends_on: vec!["S1".into(), "S2".into(), "nope".into()],
            ..step("S6", "f", STEP_PENDING)
        };
        assert!(p.ready(&settled_and_typo));
        assert!(!p.ready(&step("S3", "c", STEP_DOING)));
    }
}
