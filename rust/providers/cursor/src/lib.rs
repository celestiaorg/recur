//! Cursor provider (cursor-sdk bridge).
//!
//! Port of Go `internal/cursor` (bridge install, locate, Connect client,
//! run events, attribution patch, context table, project dirs, reaping,
//! logging), `engine/cursor.go`, `cursor_flags.go`, `cursortools.go`, the
//! Cursor pool and self-heal of `engine/provider.go`,
//! `wrapAttachedImagesForCursor`, `app/cursor_service.go`, and the Cursor
//! branch of `app/model_catalog.go`. See `docs/cursor-provider.md` for why
//! Recur drives Cursor through the bridge.
//!
//! The allowlist the engine sends uses Claude's syntax (R9). Cursor has no
//! per-run allowlist: a list without Edit/Write/NotebookEdit runs the agent
//! in Cursor's plan mode, and `mcp__recur__create_workspace` in the list
//! selects the subagent overlay; nothing else is read.

mod attribution;
mod bridge;
mod catalog;
mod client;
mod connect;
mod context;
mod events;
mod gojson;
mod install;
mod locate;
mod pool;
mod projectdirs;
mod provider;
mod runner;
mod status;
mod steer;
mod tools;
mod turn;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod smoke_tests;
#[cfg(test)]
mod testserver;

pub use attribution::{
    ATTRIBUTION_CONFIG_FILE, PatchError, attribution_config_path, ensure_attribution_patch,
    write_attribution_config,
};
pub use bridge::{Bridge, BridgeConfig, Discovery, ENV_ATTRIBUTION_PATH};
pub use catalog::{cursor_catalog_models, cursor_model_choice};
pub use client::{
    AgentOptions, HttpMcpServer, Image, LocalOptions, McpServer, Model, ModelParameterChoice,
    ModelParameterDefinition, ModelParameterValue, ModelSelection, ModelVariant, ProtoTime,
    RunSnapshot, SETTING_SOURCE_ALL, SETTING_SOURCE_MDM, SETTING_SOURCE_PLUGINS,
    SETTING_SOURCE_PROJECT, SETTING_SOURCE_TEAM, SETTING_SOURCE_USER, SETTING_SOURCES_MINIMAL,
    STEER_COMPLETE_DELIVERED, STEER_REVERT_TO_FOLLOWUP, SandboxOptions, StdioMcpServer, User,
    VersionInfo, run_created_on_this_bridge,
};
pub use connect::{Client, ConnectError, Error, encode_connect_frame};
pub use context::{ContextLimits, VENDOR_ORDER, base_context_window, limits_for, parse_context_flag, resolve_context_window, vendor};
pub use events::*;
pub use install::{
    InstallPhase, InstallProgress, PINNED_SDK_VERSION, check_pin_drift, install_error_payload,
    latest_sdk_version,
};
pub use locate::{BRIDGE_DIR_NAME, ENV_BRIDGE_PATH, bridge_root, launcher_rel_path, locate, owned_bridge_root, resolve_launcher, sdk_version};
pub use projectdirs::{cursor_project_slug, ensure_cursor_project_dirs, remove_cursor_project_dirs};
pub use provider::{CursorConfig, CursorProvider};
pub use runner::wrap_attached_images_for_cursor;
pub use status::{CURSOR_USAGE_AUTH_FILE, CursorStatus};
pub use tools::{canonical_tool, cursor_tool_input, cursor_tool_result_text, display_tool};

/// The pinned wheel's PyPI metadata URL.
pub fn pypi_pinned_project() -> String {
    install::pypi_pinned_project()
}

/// Installs the pinned bridge into `data_dir` without a provider (smoke
/// tests, the CLI).
pub async fn install(
    data_dir: &std::path::Path,
    cancel: &tokio_util::sync::CancellationToken,
    progress: &(dyn Fn(InstallProgress) + Send + Sync),
) -> Result<std::path::PathBuf, String> {
    install::install(data_dir, cancel, progress).await
}
