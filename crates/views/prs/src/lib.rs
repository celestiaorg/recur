//! Pull requests: the dashboard, PR detail, their titlebar parts, the review modal and
//! participant stacks.
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_review::register` and `recur_view_chat::register` (the
//! Files tab's stack, the comment threads and the composers' typeahead come from them). It
//! registers the `pr` route (the dashboard, with its counts in the titlebar) and the
//! `prdetail` route (PR detail, with the review and merge actions leading `.tb-right`).

pub mod actions;
pub mod chrome;
pub mod detail;
pub mod findings;
pub mod list;
pub mod list_titlebar;
pub mod review_modal;
#[cfg(test)]
mod spec;
pub mod style;
pub mod ui;

use gpui_kit::AppContext as _;
use recur_view_core::{OverlaySlot, TitlebarParts, ViewContext, ViewName, ViewRegistry};

use crate::actions::PrTitlebarActions;
use crate::detail::PrDetail;
use crate::list::PrList;
use crate::list_titlebar::PrListTitlebar;
use crate::review_modal::PrReviewModal;

/// Registers the PR routes.
pub fn register(r: &mut ViewRegistry) {
    r.view(&[ViewName::Pr], |vcx: &ViewContext, window, cx| {
        cx.new(|cx| PrList::new(vcx, window, cx)).into()
    })
    .titlebar(|vcx, _, cx| TitlebarParts {
        left: Some(cx.new(|cx| PrListTitlebar::new(vcx, cx)).into()),
        right: None,
    });
    r.view(&[ViewName::PrDetail], |vcx: &ViewContext, window, cx| {
        cx.new(|cx| PrDetail::new(vcx, window, cx)).into()
    })
    .titlebar(|vcx, window, cx| TitlebarParts {
        left: None,
        right: Some(cx.new(|cx| PrTitlebarActions::new(vcx, window, cx)).into()),
    });
    r.overlay(
        OverlaySlot::PrReviewModal,
        |vcx: &ViewContext, window, cx| cx.new(|cx| PrReviewModal::new(vcx, window, cx)).into(),
    );
}
