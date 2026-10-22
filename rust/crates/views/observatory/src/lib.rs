//! The workspace's right-hand IDE pane (`Observatory.svelte`): the file explorer, editor tabs,
//! the code / merge editor hosts, file previews, branch review, the tool panels (Source
//! Control, Outline, Problems, Search, References, Findings, Artifacts), Quick Open, the
//! terminal dock and the bundle command prompts.
//!
//! # Embedding
//!
//! Call [`register`] on the shell's registry (actions, key bindings, the keybinding menu
//! overlay). The workspace view owns one [`Observatory`] per hot shell:
//!
//! ```ignore
//! let obs = cx.new(|cx| Observatory::new(
//!     state.clone(),
//!     ObservatoryProps { project_id, workspace_id, loop_scoped: true, shell_active: true, ..Default::default() },
//!     ObservatorySlots { left_panel: Some(chat.into()), left_head: Some(tabs.into()), ..Default::default() },
//!     window, cx,
//! ));
//! cx.subscribe_in(&obs, window, |host, obs, ev: &ObservatoryEvent, window, cx| { /* … */ });
//! ```
//!
//! - **Props** ([`ObservatoryProps`]): push changes with [`Observatory::set_props`]. A new
//!   `project_id` / `workspace_id` switches roots: the outgoing root is snapshotted into the
//!   keep-alive cache and its session persisted; the incoming one restores from the cache
//!   (then revalidates) or loads cold and restores its editor session. `left_open: Some(_)`
//!   hands the left gutter to the host (`onToggleLeft`); `None` keeps it local.
//!   `shell_active: false` parks the Observatory (hot shells): no watcher, menu, keys or focus.
//! - **Slots** ([`ObservatorySlots`]): the Svelte snippets as views the host owns: the agent
//!   chat (`left_panel`), its tab strip (`left_head`), the plan panel and head, and the overlay
//!   shown while `main_blocked`. Replace them with [`Observatory::set_slots`].
//! - **Events** ([`ObservatoryEvent`]): the callbacks (`ToggleLeft`, agent tab commands,
//!   `FixFindings`, `AddEditorMention`, `FocusAgentComposer`) and the snippet arguments
//!   (`MaestroVisible`, `AgentLit`, `SelectedArtifact`); the same values are readable with
//!   [`Observatory::maestro_panel_visible`], [`Observatory::focus_zone`] and
//!   [`Observatory::selected_artifact_id`].
//! - **Imperative calls** (the component's exports): [`Observatory::open_path`],
//!   [`Observatory::open_artifact`], [`Observatory::open_turn_review`],
//!   [`Observatory::select_code_panel`], [`Observatory::show_agent_panel`],
//!   [`Observatory::focused_editor_context`].
//! - **PR detail**: while the PR's agent workspace is this one, the left dock moves into PR
//!   detail; render it there with [`Observatory::render_pr_agent_dock`].
//!
//! The editor's language servers run through [`lsp::client`], an `LspClient` over
//! `LspService` and the `lsp.message` / `lsp.exit` events; diagnostics land in
//! [`lsp::problems::LspProblems`].

pub mod explorer_visible;
pub mod hot_shells;
pub mod keybinding_menu;
pub mod lsp;
pub mod model;
pub mod obs_cache;
pub mod open_external;
pub mod open_in_pref;
pub mod session_host;
pub mod session_tabs;
pub mod snippets;
pub mod workspace_file;

use std::collections::BTreeMap;

use gpui_kit::{App, AppContext as _, Global};
use recur_view_core::{OverlaySlot, ViewContext, ViewRegistry};

pub use model::FocusZone;

/// Unseen artifact ids per workspace, shared across Observatories so a background workspace's
/// badge survives until its shell exists (module `$state` in the Svelte).
#[derive(Default)]
pub struct UnseenArtifacts(pub BTreeMap<String, Vec<String>>);

impl Global for UnseenArtifacts {}

pub fn unseen_artifacts(cx: &App) -> &BTreeMap<String, Vec<String>> {
    static EMPTY: BTreeMap<String, Vec<String>> = BTreeMap::new();
    cx.try_global::<UnseenArtifacts>().map_or(&EMPTY, |u| &u.0)
}

/// The module-level owners the Svelte kept so a parked shell cannot disarm the live one's
/// watcher, command handlers, menu or occluder.
#[derive(Default)]
pub struct Owners {
    pub watch: String,
    pub command: String,
    pub menu: String,
    pub occluder: String,
}

impl Global for Owners {}

/// The app-wide state the Observatory shares across instances.
pub fn init_globals(cx: &mut App) {
    lsp::client::init(cx);
    cx.default_global::<UnseenArtifacts>();
    cx.default_global::<Owners>();
    cx.default_global::<lsp::problems::LspProblems>();
    cx.default_global::<session_tabs::SessionTabParks>();
    cx.default_global::<obs_cache::ObservatoryCache>();
    cx.default_global::<session_host::SessionHost>();
}

/// Registers the Observatory's actions and key bindings and the keybinding menu overlay.
pub fn register(r: &mut ViewRegistry) {
    r.init(|cx| {
        recur_terminal_view::init(cx);
        keybinding_menu::bind(cx);
    });
    r.overlay(
        OverlaySlot::KeybindingMenu,
        |vcx: &ViewContext, window, cx| {
            let state = vcx.state.clone();
            cx.new(|cx| keybinding_menu::KeybindingMenu::new(state, window, cx))
                .into()
        },
    );
}
