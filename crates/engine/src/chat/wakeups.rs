//! Agents' "check back later": `schedule_wakeup` keeps one pending wakeup per
//! chat in recur.db, and the schedule ticker claims due ones (overdue ones on
//! launch too) and sends each chat a hidden notice turn. Closing the chat or
//! archiving the workspace cancels it.

use chrono::{DateTime, Utc};
use recur_agent_tools::HostError;
use recur_agent_tools::display::schedule_tools::delay_label;
use recur_store::ChatWakeup;

use crate::Engine;
use crate::text::truncate;

/// The display label every wakeup notice starts with (the transcript's
/// "Wakeup" row).
pub const WAKEUP_NOTICE_PREFIX: &str = "Wakeup";
const NOTICE_REASON_MAX: usize = 80;

/// The due time in a schedule_wakeup result, so the chat can show the
/// pending wakeup without another service call.
pub fn wakeup_due(result: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let rest = &result[result.find(", due ")? + ", due ".len()..];
    chrono::DateTime::parse_from_rfc3339(&rest[..rest.find(')')?]).ok()
}

impl Engine {
    /// Host `schedule_wakeup`: `delay_secs` is already clamped.
    pub fn schedule_wakeup_from_chat(
        &self,
        workspace_id: &str,
        chat: &str,
        delay_secs: i64,
        reason: &str,
    ) -> Result<String, HostError> {
        if self
            .store
            .workspace(workspace_id)
            .is_none_or(|w| w.archived)
        {
            return Err(HostError::new("workspace not found"));
        }
        let due = self.now() + chrono::Duration::seconds(delay_secs);
        self.store
            .save_chat_wakeup(&ChatWakeup {
                workspace_id: workspace_id.into(),
                chat: chat.into(),
                due_at: due.timestamp_millis(),
                reason: reason.into(),
            })
            .map_err(|e| HostError(e.to_string()))?;
        Ok(format!(
            "Wakeup set: this chat gets a new turn in {} (at {}, due {}), replacing any earlier pending wakeup.",
            delay_label(delay_secs),
            due.format("%-I:%M %p"),
            due.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        ))
    }

    /// Claims the wakeups due by `now` and sends each its notice turn.
    pub(crate) fn fire_due_wakeups(&self, now: DateTime<Utc>) {
        let due = match self.store.claim_due_chat_wakeups(now) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("chat wakeups: {e}");
                return;
            }
        };
        for w in due {
            let e = self.arc();
            tokio::spawn(async move {
                let label = format!(
                    "{WAKEUP_NOTICE_PREFIX}: {}",
                    truncate(&w.reason, NOTICE_REASON_MAX)
                );
                e.send_hidden_notice(&w.workspace_id, &w.chat, &wakeup_notice(&w), &label)
                    .await;
            });
        }
    }
}

fn wakeup_notice(w: &ChatWakeup) -> String {
    format!(
        "{WAKEUP_NOTICE_PREFIX}: the check-back you scheduled with schedule_wakeup is due.\nReason: {}\n\nCheck on it now. Call schedule_wakeup again to keep checking.",
        w.reason
    )
}
