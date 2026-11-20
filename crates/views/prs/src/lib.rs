//! Pull requests: the dashboard, PR detail, their titlebar parts, the review modal and
//! participant stacks.
//!
//! # Embedding
//!
//! Call [`register`] after `recur_view_review::register` and `recur_view_chat::register` (the
//! Files tab's stack, the comment threads and the composers' typeahead come from them), and
//! after `recur_view_workspace::register` (the workspace titlebar gets the PR's merge button). It
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

use gpui_kit::{App, AppContext as _, Entity, Window};
use recur_view_core::pr_view::{PrViewHost, PrViewParts};
use recur_view_core::{AppState, OverlaySlot, TitlebarParts, ViewContext, ViewName, ViewRegistry};

use crate::actions::{ActionsHost, PrTitlebarActions};
use crate::detail::PrDetail;
use crate::detail::render::PrHead;
use crate::list::PrList;
use crate::list_titlebar::PrListTitlebar;
use crate::review_modal::PrReviewModal;

/// PR detail for a workspace's center (`recur_view_core::pr_view`).
fn mount(
    state: &Entity<AppState>,
    workspace_id: &str,
    window: &mut Window,
    cx: &mut App,
) -> PrViewParts {
    let vcx = ViewContext {
        state: state.clone(),
        services: state.read(cx).services().clone(),
    };
    let detail = cx.new(|cx| PrDetail::hosted(&vcx, workspace_id, window, cx));
    let actions = cx.new(|cx| PrTitlebarActions::new(&vcx, ActionsHost::TabRow, window, cx));
    let head = cx.new(|cx| PrHead::new(detail.clone(), actions, cx));
    PrViewParts {
        head: head.into(),
        body: detail.into(),
    }
}

/// Registers the PR routes and PR detail as a workspace's pull request view.
pub fn register(r: &mut ViewRegistry) {
    r.init(|cx| cx.set_global(PrViewHost { mount }));
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
        right: Some(
            cx.new(|cx| PrTitlebarActions::new(vcx, ActionsHost::Page, window, cx))
                .into(),
        ),
    });
    // The workspace titlebar's merge-state button for the workspace's PR. The detail route is
    // the workspace crate's, registered before this one.
    if let Some(i) = r.entry_for(ViewName::Detail) {
        r.views[i].titlebar(|vcx, window, cx| TitlebarParts {
            left: None,
            right: Some(
                cx.new(|cx| PrTitlebarActions::new(vcx, ActionsHost::Workspace, window, cx))
                    .into(),
            ),
        });
    }
    r.overlay(
        OverlaySlot::PrReviewModal,
        |vcx: &ViewContext, window, cx| cx.new(|cx| PrReviewModal::new(vcx, window, cx)).into(),
    );
}
