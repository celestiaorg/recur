//! The list views of the briefing family (`_briefing.scss`): the Inbox (`Briefing.svelte`
//! with its Ask Recur dock), Issues (`Issues.svelte`) and Schedules (`ScheduledPrompts.svelte`
//! with `lib/schedule-expanded.ts`).
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_chat::register`. Project settings embed
//! [`schedules::ScheduledPrompts`] for one project.

pub mod schedule_expanded;
pub mod schedules;
pub mod style;

use gpui_kit::AppContext as _;
use recur_view_core::{ViewContext, ViewName, ViewRegistry};

/// Registers the Schedules route.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Schedules], |vcx: &ViewContext, window, cx| {
        schedule_expanded::load(&vcx.services, cx);
        cx.new(|cx| schedules::SchedulesPage::new(vcx, window, cx))
            .into()
    });
}
