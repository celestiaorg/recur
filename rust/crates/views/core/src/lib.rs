//! The state every view shares and the seam views register with the shell through.
//!
//! - [`AppState`]: the app-wide stores of `lib/stores.ts` and the data layer of `lib/api.ts`
//!   (projects, workspaces, PRs, inbox, attention, settings, navigation, overlays, prefs,
//!   terminal dock chrome, run sessions) as one GPUI entity.
//! - [`view`]: how a view crate plugs into the shell (routes, titlebar parts, overlays,
//!   key bindings).
//! - Pure ports the shell, sidebar and views share: [`workspace_nav`], [`shell_state`],
//!   [`modes`], [`models`], [`file_drag`], [`pr`], [`run`], [`commands`], [`chat_route`],
//!   [`format`].
//!
//! A store read or written by two or more view crates lives here; a store one view owns lives
//! in that view's crate.

pub mod chat;
pub mod chat_route;
pub mod command_usage;
pub mod commands;
mod data;
pub mod editor_env;
pub mod file_drag;
pub mod format;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod land;
pub mod local_storage;
pub mod menu;
pub mod models;
pub mod modes;
pub mod nav;
mod navigate;
pub mod open_in_pref;
pub mod pr;
pub mod pr_commits;
pub mod pr_list_filter;
pub mod prefs;
pub mod run;
pub mod shell_state;
pub mod snippets;
pub mod state;
pub mod themes;
pub mod topics;
pub mod view;
pub mod workspace_nav;

pub use data::{AppliedEvent, GITHUB_PR_SYNC_MIN, app_chat_agent_id, is_agent_running_status};
pub use local_storage::{LOCAL_STORAGE_FILE, LocalStorage};
pub use nav::{Crumb, CrumbSeg, CrumbStyle, ViewName};
pub use state::{
    AppState, Attention, ChatRef, NewTaskDraft, ObsJump, Overlays, PrReviewKind,
    PromptActionRequest, ScanKind, ScheduleEditorRequest, ShellEvent, WorkspaceScanEvent,
    WorkspaceSeed, bridge_error_message, call, toast,
};
pub use topics::{ObserveState, StateChanged, Topics, observe_state_app};
pub use view::{OverlaySlot, TitlebarParts, ViewContext, ViewRegistry};
