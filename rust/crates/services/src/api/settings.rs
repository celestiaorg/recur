use async_trait::async_trait;
use recur_domain::{CompatibleFlagDim, CompatibleProvider, Resource, Settings};

use super::{
    AssignedIssue, ClaudeStatus, CodexStatus, CompatibleTestResult, CursorStatus, ModelGroup,
};
use super::{CursorUsageConnect, Snapshot};
use crate::Result;

/// Rust port of Go `app.SettingsService` (`frontend/bindings/recur/internal/app/settingsservice.ts`).
#[async_trait]
pub trait SettingsService: Send + Sync {
    /// CancelCursorUsageConnect aborts an in-flight Connect Cursor login.
    async fn cancel_cursor_usage_connect(&self) -> Result<()>;

    /// ClearAllData wipes all projects/loops/PRs/inbox/history (keeping your policy +
    /// settings), returning the app to a clean slate.
    async fn clear_all_data(&self) -> Result<()>;

    /// CompatibleFlagPreset returns stock flag dimensions for the Settings editor.
    async fn compatible_flag_preset(&self, name: &str) -> Result<Vec<CompatibleFlagDim>>;

    /// ConnectCursorUsage starts the unofficial Cursor browser login and returns the URL.
    async fn connect_cursor_usage(&self) -> Result<CursorUsageConnect>;

    /// DetectAgent reports the connected coding agent (PRD §16.13 first-run).
    /// It runs `claude auth status` to surface the logged-in account email/org so
    /// the welcome screen shows who is actually logged in. Returns "" when Claude is
    /// missing, not logged in, or the status call fails — never a fake success string
    /// that would paint Welcome step 1 green before a real login exists.
    async fn detect_agent(&self) -> Result<String>;

    /// DisconnectCursorUsage deletes Recur-owned Cursor Connect tokens and the
    /// minted SDK key. A leftover pasted key is also cleared — Connect is the
    /// only Cursor sign-in.
    async fn disconnect_cursor_usage(&self) -> Result<()>;

    /// GetClaudeStatus probes the local Claude Code CLI for login + version info.
    /// Safe to call from Settings: bounded, never invents a success state.
    ///
    /// `claude auth status` exits non-zero when logged out but still prints JSON —
    /// that must read as "Not logged in", not "not detected". "Claude Code not
    /// detected" is reserved for a missing binary.
    async fn get_claude_status(&self) -> Result<ClaudeStatus>;

    /// GetCodexStatus reports whether Codex is ready without mutating settings.
    async fn get_codex_status(&self) -> Result<CodexStatus>;

    /// GetCursorStatus reports whether the Cursor provider is ready to use.
    /// A leftover pasted key is dropped when Recur has no Connect file. Safe to
    /// call on every Settings render: when no bridge is installed it does no work
    /// beyond a few Stat calls (and that orphan wipe).
    async fn get_cursor_status(&self) -> Result<CursorStatus>;

    /// GetInstanceLabel returns the trimmed RECUR_INSTANCE_LABEL process env.
    /// Empty means the titlebar chip is hidden.
    async fn get_instance_label(&self) -> Result<String>;

    /// GetProviderUsage returns the last quota snapshot, refreshing if it is stale.
    async fn get_provider_usage(&self) -> Result<Snapshot>;

    /// GetResource returns the machine-global status-bar counters (PRD §6.4, D6).
    async fn get_resource(&self) -> Result<Resource>;

    async fn get_settings(&self) -> Result<Settings>;

    /// GitHubLogin returns the authenticated GitHub username from `gh`, or "" when
    /// gh is missing or logged out. Used by Settings → General to fill the prefix
    /// field once; it is not consulted when creating workspaces.
    async fn github_login(&self) -> Result<String>;

    /// InstallCursorBridge downloads and unpacks the Cursor bridge into Recur's data
    /// directory, then records the resolved path in Settings.
    ///
    /// The bridge is ~50MB compressed / ~155MB unpacked (mostly a bundled Node
    /// runtime), so it is fetched on demand rather than shipped with Recur. Progress
    /// is published on the bus as `cursor.install` events; this call blocks until the
    /// install finishes or fails.
    async fn install_cursor_bridge(&self) -> Result<CursorStatus>;

    /// ListAssignedIssues merges open GitHub issues (via `gh`) and Linear issues
    /// (via the configured API key) that are assigned to the user, plus unassigned
    /// ones in the user's project repos and Linear teams, newest first. A source
    /// that is unavailable or fails is skipped; the others still show.
    async fn list_assigned_issues(&self) -> Result<Vec<AssignedIssue>>;

    /// ListModelCatalog returns the grouped model list for the pickers.
    ///
    /// Each provider appears only when it is on in Settings → Providers. Claude Code
    /// needs no Recur-stored key. Cursor appears once Connect has minted a key, and
    /// its models (+ parameters) are fetched live. Codex appears once an API key or
    /// ChatGPT auth is set.
    ///
    /// Fetching Cursor's catalogue starts a bridge, which is slow on a cold start, so
    /// callers should load this once per pane rather than per picker. The result is
    /// remembered so SetSettings can remap Claude-off slots without starting another
    /// throwaway bridge (boot prefetchModelCatalog warms this).
    async fn list_model_catalog(&self) -> Result<Vec<ModelGroup>>;

    /// ListMonospaceFonts returns installed monospace font family names for the
    /// editor font picker. Native enumeration is required because the webview's
    /// canvas font probe often cannot see user-installed faces (e.g. ~/Library/Fonts).
    async fn list_monospace_fonts(&self) -> Result<Vec<String>>;

    /// LogsPath returns the Recur process log path (<dataDir>/logs/recur.log).
    async fn logs_path(&self) -> Result<String>;

    /// OpenLinearInApp opens a linear.app URL in the Linear desktop app. False
    /// means the caller should open it in the browser instead.
    /// simplify: macOS only (open fails when no app handles linear://). Add an
    /// xdg-mime handler check for Linux if Linear ships a desktop build there.
    async fn open_linear_in_app(&self, raw: &str) -> Result<bool>;

    /// RefreshProviderUsage forces a fetch, joining any in-flight collect.
    async fn refresh_provider_usage(&self) -> Result<Snapshot>;

    /// RevealLogs shows recur.log in the OS file manager (parent folder if the
    /// file has not been created yet).
    async fn reveal_logs(&self) -> Result<()>;

    async fn set_settings(&self, set: &Settings) -> Result<Settings>;

    /// TestCodexKey validates an API key (or ChatGPT auth) before save.
    async fn test_codex_key(&self, api_key: &str, chat_gpt_auth: bool) -> Result<CodexStatus>;

    /// TestCompatibleProvider POSTs a tiny /v1/responses ping using the saved
    /// (or draft) provider config. Does not persist settings.
    async fn test_compatible_provider(
        &self,
        p: &CompatibleProvider,
    ) -> Result<CompatibleTestResult>;

    /// TestCursorKey validates an API key that has NOT been saved yet, so the user
    /// gets a verdict before committing it. An empty key falls back to the saved one.
    async fn test_cursor_key(&self, api_key: &str) -> Result<CursorStatus>;
}
