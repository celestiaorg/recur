//! The PR stores of `lib/stores.ts` the PR views share (the dashboard's filters, sort and
//! sync flag; PR detail's tab, drafts and focus zone) and `lib/pr-fix-checks.ts`'s queue.
//! Like the Svelte stores they live for the session only.

use gpui_kit::{App, AppContext as _, Context, Entity, Global};

use recur_engine::view::pr_fix_checks::PendingFixChecks;

use recur_view_core::pr_list_filter::{ListLife, ListSort};

/// `PRDetailTab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrTab {
    #[default]
    Conv,
    Commits,
    Files,
    Checks,
}

#[derive(Default)]
pub struct PrUi {
    /// `prListSeg`.
    pub seg: ListLife,
    /// `prListFacets`.
    pub facets: Vec<String>,
    /// `prListSort`.
    pub sort: ListSort,
    /// `prListProjectIds` (empty = every git project).
    pub project_ids: Vec<String>,
    /// `prListSyncing`.
    pub syncing: bool,
    /// `prDetailTab`.
    pub tab: PrTab,
    /// `prComposerDraft`.
    pub composer_draft: String,
    /// Bumped when something other than the composer itself rewrote the draft.
    pub composer_rev: u64,
    /// `prReviewDraft`.
    pub review_draft: String,
    pub review_rev: u64,
    /// `prDetailFocus === 'agent'`.
    pub focus_agent: bool,
    /// `pendingFixChecks`.
    pub pending_fix_checks: Option<PendingFixChecks>,
}

impl PrUi {
    pub fn set_composer_draft(&mut self, s: impl Into<String>, cx: &mut Context<Self>) {
        self.composer_draft = s.into();
        self.composer_rev += 1;
        cx.notify();
    }

    pub fn set_review_draft(&mut self, s: impl Into<String>, cx: &mut Context<Self>) {
        self.review_draft = s.into();
        self.review_rev += 1;
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: PrTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    /// `openPR`'s resets: the conversation tab and empty drafts.
    pub fn reset_for_open(&mut self, cx: &mut Context<Self>) {
        self.tab = PrTab::Conv;
        self.set_composer_draft("", cx);
        self.set_review_draft("", cx);
    }
}

struct PrUiGlobal(Entity<PrUi>);

impl Global for PrUiGlobal {}

/// The session's PR stores.
pub fn ui(cx: &mut App) -> Entity<PrUi> {
    if let Some(g) = cx.try_global::<PrUiGlobal>() {
        return g.0.clone();
    }
    let e = cx.new(|_| PrUi::default());
    cx.set_global(PrUiGlobal(e.clone()));
    e
}

/// A fresh session (scenes and tests share one app).
pub fn reset(cx: &mut App) {
    let e = cx.new(|_| PrUi::default());
    cx.set_global(PrUiGlobal(e));
}
