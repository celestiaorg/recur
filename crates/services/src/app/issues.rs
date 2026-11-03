//! The issues service: one assigned-issues list merged over
//! the registered trackers, and tracker deep links into desktop apps.

use std::time::Duration;

use recur_issues::IssueQuery;

use super::AppServices;
use crate::api::{AssignedIssue, IssueLabel};

/// The whole listing shares one budget; a tracker that runs out keeps what
/// it has.
const ISSUES_TIMEOUT: Duration = Duration::from_secs(15);

fn assigned_issue(i: recur_issues::AssignedIssue) -> AssignedIssue {
    AssignedIssue {
        source: i.source,
        id: i.id,
        r#ref: i.r#ref,
        title: i.title,
        url: i.url,
        created_at: i.created_at,
        updated_at: i.updated_at,
        group: i.group,
        state: i.state,
        state_color: i.state_color,
        state_position: i.state_position,
        priority: i.priority,
        priority_label: i.priority_label,
        project: i.project,
        labels: i.labels.map(|ls| {
            ls.into_iter()
                .map(|l| IssueLabel {
                    name: l.name,
                    color: l.color,
                })
                .collect()
        }),
        comments: i.comments,
        assignees: i.assignees,
        project_id: i.project_id,
        linear_project: i.linear_project,
        linear_team: i.linear_team,
    }
}

impl AppServices {
    /// Open issues assigned to the user plus unassigned ones in their repos and
    /// teams, newest activity first. A tracker that is unavailable or fails is
    /// skipped.
    pub(crate) async fn list_assigned_issues_at(&self) -> Vec<AssignedIssue> {
        let settings = self.store().settings();
        let projects = self.store().projects().unwrap_or_default();
        let q = IssueQuery {
            settings: &settings,
            projects: &projects,
            deadline: tokio::time::Instant::now() + ISSUES_TIMEOUT,
        };
        recur_issues::list_assigned(&self.engine.trackers().all(), q)
            .await
            .into_iter()
            .map(assigned_issue)
            .collect()
    }

    /// Opens a tracker web link in its desktop app. False means the caller
    /// should open the browser instead.
    // simplify: macOS only (`open` fails when no app handles the scheme). Add
    // an xdg-mime handler check for Linux if Linear ships a desktop build.
    pub(crate) async fn open_linear_in_app_at(&self, raw: &str) -> bool {
        let Some(app_url) = self
            .engine
            .trackers()
            .all()
            .iter()
            .find_map(|t| t.app_url(raw))
        else {
            return false;
        };
        if !cfg!(target_os = "macos") {
            return false;
        }
        tokio::process::Command::new("open")
            .arg(&app_url)
            .status()
            .await
            .is_ok_and(|s| s.success())
    }
}
