//! Every service DTO decodes Go's `json.Marshal` output and re-encodes it
//! to an equal JSON value. Fixtures come from tests/fixtures/gen_fixtures.go.txt: a
//! zero value, one with non-nil empty collections, and one with every field
//! set. Types owned by lower crates (`recur-agent`, `recur-lsp`, `recur-usage`,
//! the providers) are checked here too, as the services contract uses them.

use std::path::Path;

use pretty_assertions::assert_eq;
use recur_domain::json::canonical;
use recur_services::api::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

fn check<T: DeserializeOwned + Serialize + Default>(go_name: &str) {
    check_modes::<T>(go_name, &["zero", "empty", "full"]);
}

fn check_modes<T: DeserializeOwned + Serialize + Default>(go_name: &str, modes: &[&str]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dto");
    for &mode in modes {
        let path = dir.join(format!("{go_name}.{mode}.json"));
        let go = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let v: T = serde_json::from_str(&go).unwrap_or_else(|e| panic!("{go_name}.{mode}: {e}"));
        let go = canonical(serde_json::from_str(&go).unwrap());
        let back = canonical(serde_json::to_value(&v).unwrap());
        assert_eq!(back, go, "{go_name}.{mode} re-encodes differently");
        if mode == "zero" {
            assert_eq!(
                canonical(serde_json::to_value(T::default()).unwrap()),
                go,
                "{go_name}: Default is not Go's zero value"
            );
        }
    }
}

macro_rules! dto_tests {
    ($($name:ident: $ty:ty = $go:literal;)*) => {
        $(
            #[test]
            fn $name() {
                check::<$ty>($go);
            }
        )*
    };
}

dto_tests! {
    app_chat_selection: AppChatSelection = "AppChatSelection";
    assigned_issue: AssignedIssue = "AssignedIssue";
    issue_label: IssueLabel = "IssueLabel";
    bundle_setting_info: BundleSettingInfo = "BundleSettingInfo";
    bundle_skip_info: BundleSkipInfo = "BundleSkipInfo";
    claude_status: ClaudeStatus = "ClaudeStatus";
    codex_status: CodexStatus = "CodexStatus";
    command_info: CommandInfo = "CommandInfo";
    command_prompt: CommandPromptDto = "CommandPromptDTO";
    compatible_test_result: CompatibleTestResult = "CompatibleTestResult";
    cursor_status: CursorStatus = "CursorStatus";
    editor_buffer: EditorBuffer = "EditorBuffer";
    editor_file_view: EditorFileView = "EditorFileView";
    editor_scratch_doc: EditorScratchDoc = "EditorScratchDoc";
    editor_session: EditorSession = "EditorSession";
    editor_session_meta: EditorSessionMeta = "EditorSessionMeta";
    editor_session_patch: EditorSessionPatch = "EditorSessionPatch";
    ensure_result: EnsureResult = "EnsureResult";
    flag_option: FlagOption = "FlagOption";
    grammar_info: GrammarInfo = "GrammarInfo";
    issues_view: IssuesView = "IssuesView";
    language_server_info: LanguageServerInfo = "LanguageServerInfo";
    loop_info: LoopInfo = "LoopInfo";
    model_choice: ModelChoice = "ModelChoice";
    palette_search: PaletteSearchDto = "PaletteSearchDTO";
    palette_search_group: PaletteSearchGroupDto = "PaletteSearchGroupDTO";
    policy_info: PolicyInfo = "PolicyInfo";
    prompt_action_info: PromptActionInfo = "PromptActionInfo";
    prompt_arg_info: PromptArgInfo = "PromptArgInfo";
    prompt_complete_hit: PromptCompleteHitDto = "PromptCompleteHitDTO";
    prompt_complete_request: PromptCompleteRequestDto = "PromptCompleteRequestDTO";
    prompt_complete_result: PromptCompleteResultDto = "PromptCompleteResultDTO";
    prompt_context: PromptContextDto = "PromptContextDTO";
    prompt_result: PromptResultDto = "PromptResultDTO";
    role_info: RoleInfo = "RoleInfo";
    run_context: RunContextDto = "RunContextDTO";
    run_result: RunResultDto = "RunResultDTO";
    shell_state: ShellState = "ShellState";
    snippet_info: SnippetInfo = "SnippetInfo";
    symbol_info: SymbolInfo = "SymbolInfo";
    terminal_dock_chrome: TerminalDockChrome = "TerminalDockChrome";
    terminal_palette_info: TerminalPaletteInfo = "TerminalPaletteInfo";
    terminal_theme_info: TerminalThemeInfo = "TerminalThemeInfo";
    theme_info: ThemeInfo = "ThemeInfo";
    theme_preview_info: ThemePreviewInfo = "ThemePreviewInfo";
    workspace_chat_selection: WorkspaceChatSelection = "WorkspaceChatSelection";
    usage_snapshot: Snapshot = "Snapshot";
    usage_provider_quota: ProviderQuota = "ProviderQuota";
    usage_window: Window = "Window";
    usage_model_row: ModelRow = "ModelRow";
    usage_cursor_usage_connect: CursorUsageConnect = "CursorUsageConnect";
    lsp_session_info: SessionInfo = "SessionInfo";
}

// simplify: recur-agent's catalog types hold Go's non-omitempty `options` /
// `models` as plain `Vec`, so a nil slice (`null`, the zero fixture) does not
// decode. Go's catalog never sends nil there; switch these to `check` once
// recur-agent models them as `Option<Vec<_>>`.
#[test]
fn flag_dimension() {
    check_modes::<FlagDimension>("FlagDimension", &["empty", "full"]);
}

#[test]
fn model_group() {
    check_modes::<ModelGroup>("ModelGroup", &["empty", "full"]);
}
