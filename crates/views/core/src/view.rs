//! The seam between the shell (`recur-app`) and view crates (`crates/views/<area>`).
//!
//! A view crate hands the shell a [`ViewRegistry`] entry: the routes it renders, a
//! builder the shell calls once when the window opens, and optional titlebar parts. The shell
//! renders the entry whose route is current; routes nobody registered render nothing.
//! Overlays (modals, palettes, prompts) register into fixed [`OverlaySlot`]s so their paint
//! order is fixed. Each crate's `init(cx)` binds its actions and keys.
//!
//! Everything a view needs at build time is in [`ViewContext`]: the shared [`AppState`]
//! (observe it, subscribe to its [`crate::ShellEvent`]s, call its methods) and the
//! [`Services`] handle for backend calls and events.

use gpui_kit::{AnyView, App, Entity, Window};
use recur_services::Services;

use crate::nav::ViewName;
use crate::state::AppState;

/// What a view gets when the shell builds it.
#[derive(Clone)]
pub struct ViewContext {
    pub state: Entity<AppState>,
    pub services: Services,
}

pub type Builder = Box<dyn Fn(&ViewContext, &mut Window, &mut App) -> AnyView>;
pub type TitlebarBuilder = Box<dyn Fn(&ViewContext, &mut Window, &mut App) -> TitlebarParts>;

/// Titlebar pieces for a view's routes (InboxTitlebar, PRListTitlebar, the workspace land
/// chrome, …). `left` follows the breadcrumb inside `.tb-left`; `right` leads `.tb-right`.
#[derive(Default)]
pub struct TitlebarParts {
    pub left: Option<AnyView>,
    pub right: Option<AnyView>,
}

/// A main-area view.
pub struct ViewEntry {
    pub routes: &'static [ViewName],
    /// Keep the view laid out while another route shows (`#v-detail`: hidden, not
    /// unmounted, so editors and terminals keep their size).
    pub keep_laid_out: bool,
    pub build: Builder,
    pub titlebar: Option<TitlebarBuilder>,
}

/// Overlay slots. The shell paints the registered overlays in this order, then `Toasts`, then `TitleTooltip` (always last).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlaySlot {
    /// Settings → Providers' Add provider modal (`.nt-bg` inside the Settings view, under the
    /// app-level modals).
    CompatibleProvider,
    NewTask,
    ScheduleEditor,
    PrReviewModal,
    EmojiSuggest,
    UserMentionSuggest,
    NewProject,
    CommandPalette,
    RunCommandPalette,
    OpenInPalette,
    RunPalette,
    StopPalette,
    SyntaxPalette,
    KeybindingMenu,
    ShortcutsHelp,
    PermissionPrompts,
    QuestionPrompts,
}

/// Everything registered with the shell.
#[derive(Default)]
pub struct ViewRegistry {
    pub views: Vec<ViewEntry>,
    pub overlays: Vec<(OverlaySlot, Builder)>,
    pub inits: Vec<fn(&mut App)>,
}

impl ViewRegistry {
    /// Register a main-area view for `routes`.
    pub fn view(
        &mut self,
        routes: &'static [ViewName],
        build: impl Fn(&ViewContext, &mut Window, &mut App) -> AnyView + 'static,
    ) -> &mut ViewEntry {
        self.views.push(ViewEntry {
            routes,
            keep_laid_out: false,
            build: Box::new(build),
            titlebar: None,
        });
        self.views.last_mut().expect("just pushed")
    }

    /// Register an overlay in its slot.
    pub fn overlay(
        &mut self,
        slot: OverlaySlot,
        build: impl Fn(&ViewContext, &mut Window, &mut App) -> AnyView + 'static,
    ) {
        self.overlays.push((slot, Box::new(build)));
        self.overlays.sort_by_key(|(s, _)| *s);
    }

    /// Run once at startup, before any window opens: bind actions and keys.
    pub fn init(&mut self, f: fn(&mut App)) {
        self.inits.push(f);
    }

    /// The entry rendering `view`, if any.
    pub fn entry_for(&self, view: ViewName) -> Option<usize> {
        self.views.iter().position(|e| e.routes.contains(&view))
    }
}

impl ViewEntry {
    pub fn keep_laid_out(&mut self) -> &mut Self {
        self.keep_laid_out = true;
        self
    }

    pub fn titlebar(
        &mut self,
        f: impl Fn(&ViewContext, &mut Window, &mut App) -> TitlebarParts + 'static,
    ) -> &mut Self {
        self.titlebar = Some(Box::new(f));
        self
    }
}
