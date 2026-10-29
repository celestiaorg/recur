use async_trait::async_trait;

use super::{
    BundleSettingInfo, BundleSkipInfo, CommandInfo, GrammarInfo, LoopInfo, PaletteSearchDto,
    PolicyInfo, PromptActionInfo, PromptCompleteRequestDto, PromptCompleteResultDto,
    PromptContextDto, PromptResultDto, RoleInfo, RunContextDto, RunResultDto, SnippetInfo,
    SymbolInfo, TerminalThemeInfo, ThemeInfo,
};
use crate::Result;

/// BundleService discovers Recur Bundles, runs shebang commands, and compiles
/// typescript macros for the frontend Worker (CompiledModule).
///
/// Rust port of Go `app.BundleService` (`frontend/bindings/recur/internal/app/bundleservice.ts`).
#[async_trait]
pub trait BundleService: Send + Sync {
    /// CompiledCompleteModule returns esbuild-compiled CommonJS for a typescript
    /// [[prompt]] complete script. Shebang complete must use RunPromptComplete.
    async fn compiled_complete_module(&self, id: &str) -> Result<String>;

    /// CompiledModule returns esbuild-compiled CommonJS for a typescript command,
    /// using the on-disk BundleCache (rebuilt when source changes). The frontend
    /// runs it in a sandboxed Worker.
    async fn compiled_module(&self, id: &str) -> Result<String>;

    /// CompiledPromptModule returns esbuild-compiled CommonJS for a typescript prompt.
    /// Shebang prompts must use RunPromptAction instead.
    async fn compiled_prompt_module(&self, id: &str) -> Result<String>;

    /// ExpandGoal runs matching [[expand]] shebangs. Unchanged text if none match.
    async fn expand_goal(&self, text: &str) -> Result<PromptResultDto>;

    /// InvalidateCatalog forces the next listing to re-read every bundle, including
    /// script and palette files the bundle.toml stamp does not cover (Reload Bundles).
    async fn invalidate_catalog(&self) -> Result<()>;

    /// ListBundleSettings discovers all [[setting]] entries (grouped by bundle in the UI).
    async fn list_bundle_settings(&self) -> Result<Vec<BundleSettingInfo>>;

    /// ListCommands discovers all bundles and returns commands matching scope/path.
    /// When both scope and path are empty, every command is returned (callers that
    /// need file affinity — e.g. the Run Command palette — re-filter client-side).
    async fn list_commands(&self, scope: &str, path: &str) -> Result<Vec<CommandInfo>>;

    /// ListGrammars returns TextMate grammars from shipped + user bundles.
    async fn list_grammars(&self) -> Result<Vec<GrammarInfo>>;

    /// ListLoops returns discovered [[loop]] items (name order).
    async fn list_loops(&self) -> Result<Vec<LoopInfo>>;

    /// ListPolicies returns discovered [[policy]] items (name order).
    async fn list_policies(&self) -> Result<Vec<PolicyInfo>>;

    /// ListPromptActions discovers all [[prompt]] entries (alphabetical by slash).
    async fn list_prompt_actions(&self) -> Result<Vec<PromptActionInfo>>;

    /// ListRoles returns discovered [[role]] items (name order).
    async fn list_roles(&self) -> Result<Vec<RoleInfo>>;

    /// ListSkippedBundles returns directories under the scan roots that were not loaded
    /// (bad id, parse error, missing manifest, …). Used by Settings → Bundles.
    async fn list_skipped_bundles(&self) -> Result<Vec<BundleSkipInfo>>;

    /// ListSnippets discovers all [[snippet]] entries matching scope/path.
    /// When both scope and path are empty, every snippet is returned.
    async fn list_snippets(&self, scope: &str, path: &str) -> Result<Vec<SnippetInfo>>;

    /// ListSymbols discovers all [[symbol]] showInSymbolList rules.
    async fn list_symbols(&self) -> Result<Vec<SymbolInfo>>;

    /// ListTerminalThemes returns xterm.js ITheme palettes from shipped + user bundles.
    async fn list_terminal_themes(&self) -> Result<Vec<TerminalThemeInfo>>;

    /// ListThemes returns TextMate themes from shipped + user bundles.
    async fn list_themes(&self) -> Result<Vec<ThemeInfo>>;

    /// ReadGrammar returns the raw grammar file contents for scopeName.
    async fn read_grammar(&self, scope_name: &str) -> Result<String>;

    /// ReadTheme returns the raw theme file contents for theme id.
    async fn read_theme(&self, id: &str) -> Result<String>;

    /// RunCommand looks up a shebang command by id and executes it as a subprocess.
    /// TypeScript macros must use CompiledModule + the frontend Worker instead.
    async fn run_command(&self, id: &str, rc: &RunContextDto) -> Result<RunResultDto>;

    /// RunPromptAction looks up a shebang prompt by id and executes it.
    /// TypeScript prompts must use CompiledPromptModule + the frontend Worker.
    async fn run_prompt_action(&self, id: &str, rc: &PromptContextDto) -> Result<PromptResultDto>;

    /// RunPromptComplete runs a shebang [[prompt]] complete script.
    /// TypeScript complete scripts must use CompiledCompleteModule + the frontend Worker.
    async fn run_prompt_complete(
        &self,
        id: &str,
        req: &PromptCompleteRequestDto,
    ) -> Result<PromptCompleteResultDto>;

    /// SearchPalette runs every [[search]] concurrently for the ⌘K query.
    /// Queries shorter than 2 runes, or catalogs with no search items, return empty.
    async fn search_palette(&self, query: &str) -> Result<PaletteSearchDto>;
}
