//! The list views of the briefing family (`_briefing.scss`): the Inbox (`Briefing.svelte`
//! with its Ask Recur dock), Issues (`Issues.svelte`) and Schedules (`ScheduledPrompts.svelte`
//! with `lib/schedule-expanded.ts`).
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_chat::register`. Project settings embed
//! [`schedules::ScheduledPrompts`] for one project.

pub mod briefing;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod issues;
pub mod schedule_expanded;
pub mod schedules;
pub mod style;

use gpui_kit::AppContext as _;
use recur_view_core::{ViewContext, ViewName, ViewRegistry};

/// Registers the Briefing (Inbox), Issues and Schedules routes.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Briefing], |vcx: &ViewContext, window, cx| {
        briefing::build(vcx, window, cx).into()
    });
    r.view(&[ViewName::Issues], |vcx: &ViewContext, window, cx| {
        cx.new(|cx| issues::IssuesPage::new(vcx, window, cx)).into()
    });
    r.view(&[ViewName::Schedules], |vcx: &ViewContext, window, cx| {
        schedule_expanded::load(&vcx.services, cx);
        cx.new(|cx| schedules::SchedulesPage::new(vcx, window, cx))
            .into()
    });
}
