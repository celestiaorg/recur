//! The service traits over the bridge: each method encodes its arguments as
//! JSON and dispatches by method name.

use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{
    Agent, AgentSkill, Artifact, AttentionCounts, BranchChoice, BriefingSnapshot, ChatAttachment,
    ChatItem, Commit, CompatibleFlagDim, CompatibleProvider, CopyFilesPreview, Escalation,
    FeedItem, FileContent, FileDiff, FileDiffResult, FileSearchResult, Finding, InboxItem,
    IssueRef, OpenInTarget, OpenPathResult, OutlineSymbol, PermissionRequest, Policy, Pr,
    PrComment, PrParticipant, Project, ProjectGroup, ReplaceResult, Resource, ScheduledPrompt,
    SearchHit, SearchOpts, SearchResult, SessionView, Settings, SymbolDef, SymbolRef, TreeEntry,
    UpdateWorkspaceResult, UserQuestion, Workspace, WorkspaceChatTab, WorkspaceLandState,
    WorktreeCommitResult,
};
use recur_term::Info as TermInfo;
use serde::Serialize;
use serde_json::Value;

use super::MockBridge;
use crate::Result;
use crate::api::*;

fn json(v: impl Serialize) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

#[async_trait]
impl AgentsService for MockBridge {
    async fn get_agent_trace(&self, workspace_id: &str, code: &str) -> Result<Agent> {
        self.call("GetAgentTrace", vec![json(workspace_id), json(code)])
            .await
    }

    async fn list_loop_agents(&self, workspace_id: &str) -> Result<Vec<Agent>> {
        self.call("ListLoopAgents", vec![json(workspace_id)]).await
    }
}

#[async_trait]
impl ArtifactsService for MockBridge {
    async fn delete_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
        self.call("DeleteArtifact", vec![json(workspace_id), json(id)])
            .await
    }

    async fn list_artifacts(&self, workspace_id: &str) -> Result<Vec<Artifact>> {
        self.call("ListArtifacts", vec![json(workspace_id)]).await
    }

    async fn present_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
        self.call("PresentArtifact", vec![json(workspace_id), json(id)])
            .await
    }

    async fn rename_artifact(&self, workspace_id: &str, id: &str, title: &str) -> Result<Artifact> {
        self.call(
            "RenameArtifact",
            vec![json(workspace_id), json(id), json(title)],
        )
        .await
    }

    async fn reveal_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
        self.call("RevealArtifact", vec![json(workspace_id), json(id)])
            .await
    }
}

/// `FetchAsset(url)` answers `{contentType, cacheControl, body}` (body
/// base64) or `{status, message}`; without a resolver every asset is 404,
/// as the e2e bridge served no files.
#[async_trait]
impl AssetsService for MockBridge {
    async fn fetch_asset(&self, url: &str) -> std::result::Result<Asset, AssetError> {
        use base64::Engine as _;
        let v: Value = self
            .call("FetchAsset", vec![json(url)])
            .await
            .map_err(|e| AssetError::new(500, e.message))?;
        if v.is_null() {
            return Err(AssetError::not_found());
        }
        let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        if let Some(status) = v.get("status").and_then(Value::as_u64) {
            return Err(AssetError::new(status as u16, text("message")));
        }
        let body = base64::engine::general_purpose::STANDARD
            .decode(text("body"))
            .map_err(|e| AssetError::new(500, e.to_string()))?;
        Ok(Asset {
            content_type: text("contentType"),
            cache_control: text("cacheControl"),
            body,
        })
    }
}

#[async_trait]
impl BriefingService for MockBridge {
    async fn app_chat_items(&self, code: &str) -> Result<Vec<ChatItem>> {
        self.call("AppChatItems", vec![json(code)]).await
    }

    async fn app_chat_selection(&self, code: &str) -> Result<AppChatSelection> {
        self.call("AppChatSelection", vec![json(code)]).await
    }

    async fn cancel_app_message(&self, code: &str) -> Result<()> {
        self.call("CancelAppMessage", vec![json(code)]).await
    }

    async fn close_app_chat(&self, code: &str) -> Result<Vec<WorkspaceChatTab>> {
        self.call("CloseAppChat", vec![json(code)]).await
    }

    async fn get_briefing(&self) -> Result<BriefingSnapshot> {
        self.call("GetBriefing", vec![]).await
    }

    async fn list_app_chats(&self) -> Result<Vec<WorkspaceChatTab>> {
        self.call("ListAppChats", vec![]).await
    }

    async fn mark_app_chat_seen(&self, code: &str) -> Result<()> {
        self.call("MarkAppChatSeen", vec![json(code)]).await
    }

    async fn mark_workspace_viewed(
        &self,
        workspace_id: &str,
        seen_through_activity_at: i64,
    ) -> Result<()> {
        self.call(
            "MarkWorkspaceViewed",
            vec![json(workspace_id), json(seen_through_activity_at)],
        )
        .await
    }

    async fn new_app_chat(&self) -> Result<WorkspaceChatTab> {
        self.call("NewAppChat", vec![]).await
    }

    async fn remember_app_chat_selection(
        &self,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.call(
            "RememberAppChatSelection",
            vec![json(code), json(model), json(flags)],
        )
        .await
    }

    async fn send_app_message(
        &self,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<String> {
        self.call(
            "SendAppMessage",
            vec![json(code), json(text), json(model), json(flags)],
        )
        .await
    }

    async fn set_app_chat_focus(&self, visible: bool, code: &str) -> Result<()> {
        self.call("SetAppChatFocus", vec![json(visible), json(code)])
            .await
    }
}

#[async_trait]
impl BundleService for MockBridge {
    async fn compiled_complete_module(&self, id: &str) -> Result<String> {
        self.call("CompiledCompleteModule", vec![json(id)]).await
    }

    async fn compiled_module(&self, id: &str) -> Result<String> {
        self.call("CompiledModule", vec![json(id)]).await
    }

    async fn compiled_prompt_module(&self, id: &str) -> Result<String> {
        self.call("CompiledPromptModule", vec![json(id)]).await
    }

    async fn expand_goal(&self, text: &str) -> Result<PromptResultDto> {
        self.call("ExpandGoal", vec![json(text)]).await
    }

    async fn invalidate_catalog(&self) -> Result<()> {
        self.call("InvalidateCatalog", vec![]).await
    }

    async fn list_bundle_settings(&self) -> Result<Vec<BundleSettingInfo>> {
        self.call("ListBundleSettings", vec![]).await
    }

    async fn list_commands(&self, scope: &str, path: &str) -> Result<Vec<CommandInfo>> {
        self.call("ListCommands", vec![json(scope), json(path)])
            .await
    }

    async fn list_grammars(&self) -> Result<Vec<GrammarInfo>> {
        self.call("ListGrammars", vec![]).await
    }

    async fn list_loops(&self) -> Result<Vec<LoopInfo>> {
        self.call("ListLoops", vec![]).await
    }

    async fn list_policies(&self) -> Result<Vec<PolicyInfo>> {
        self.call("ListPolicies", vec![]).await
    }

    async fn list_prompt_actions(&self) -> Result<Vec<PromptActionInfo>> {
        self.call("ListPromptActions", vec![]).await
    }

    async fn list_roles(&self) -> Result<Vec<RoleInfo>> {
        self.call("ListRoles", vec![]).await
    }

    async fn list_skipped_bundles(&self) -> Result<Vec<BundleSkipInfo>> {
        self.call("ListSkippedBundles", vec![]).await
    }

    async fn list_snippets(&self, scope: &str, path: &str) -> Result<Vec<SnippetInfo>> {
        self.call("ListSnippets", vec![json(scope), json(path)])
            .await
    }

    async fn list_symbols(&self) -> Result<Vec<SymbolInfo>> {
        self.call("ListSymbols", vec![]).await
    }

    async fn list_terminal_themes(&self) -> Result<Vec<TerminalThemeInfo>> {
        self.call("ListTerminalThemes", vec![]).await
    }

    async fn list_themes(&self) -> Result<Vec<ThemeInfo>> {
        self.call("ListThemes", vec![]).await
    }

    async fn read_grammar(&self, scope_name: &str) -> Result<String> {
        self.call("ReadGrammar", vec![json(scope_name)]).await
    }

    async fn read_theme(&self, id: &str) -> Result<String> {
        self.call("ReadTheme", vec![json(id)]).await
    }

    async fn run_command(&self, id: &str, rc: &RunContextDto) -> Result<RunResultDto> {
        self.call("RunCommand", vec![json(id), json(rc)]).await
    }

    async fn run_prompt_action(&self, id: &str, rc: &PromptContextDto) -> Result<PromptResultDto> {
        self.call("RunPromptAction", vec![json(id), json(rc)]).await
    }

    async fn run_prompt_complete(
        &self,
        id: &str,
        req: &PromptCompleteRequestDto,
    ) -> Result<PromptCompleteResultDto> {
        self.call("RunPromptComplete", vec![json(id), json(req)])
            .await
    }

    async fn search_palette(&self, query: &str) -> Result<PaletteSearchDto> {
        self.call("SearchPalette", vec![json(query)]).await
    }
}

#[async_trait]
impl CodeService for MockBridge {
    async fn abs_path(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<String> {
        self.call(
            "AbsPath",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn bridge_bench_done(&self, stats_json: &str) -> Result<()> {
        self.call("BridgeBenchDone", vec![json(stats_json)]).await
    }

    async fn bridge_bench_enabled(&self) -> Result<bool> {
        self.call("BridgeBenchEnabled", vec![]).await
    }

    async fn bridge_echo(&self, payload: &str) -> Result<i64> {
        self.call("BridgeEcho", vec![json(payload)]).await
    }

    async fn bridge_ping(&self, n: i64) -> Result<i64> {
        self.call("BridgePing", vec![json(n)]).await
    }

    async fn commit(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
    ) -> Result<WorktreeCommitResult> {
        self.call(
            "Commit",
            vec![json(project_id), json(workspace_id), json(message)],
        )
        .await
    }

    async fn commit_diff(
        &self,
        project_id: &str,
        workspace_id: &str,
        hash: &str,
    ) -> Result<Vec<FileDiff>> {
        self.call(
            "CommitDiff",
            vec![json(project_id), json(workspace_id), json(hash)],
        )
        .await
    }

    async fn commit_staged(
        &self,
        project_id: &str,
        workspace_id: &str,
        message: &str,
    ) -> Result<WorktreeCommitResult> {
        self.call(
            "CommitStaged",
            vec![json(project_id), json(workspace_id), json(message)],
        )
        .await
    }

    async fn copy(&self, project_id: &str, workspace_id: &str, from: &str, to: &str) -> Result<()> {
        self.call(
            "Copy",
            vec![json(project_id), json(workspace_id), json(from), json(to)],
        )
        .await
    }

    async fn create_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        self.call(
            "CreateFile",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn delete(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        self.call(
            "Delete",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn diff_vs_base(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult> {
        self.call("DiffVsBase", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn diff_working(&self, project_id: &str, workspace_id: &str) -> Result<FileDiffResult> {
        self.call("DiffWorking", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn discard_all(&self, project_id: &str, workspace_id: &str) -> Result<()> {
        self.call("DiscardAll", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn discard_file(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        self.call(
            "DiscardFile",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn file_at_base(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<String> {
        self.call(
            "FileAtBase",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn file_diff(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileDiff> {
        self.call(
            "FileDiff",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn find_defs(
        &self,
        project_id: &str,
        workspace_id: &str,
        name: &str,
    ) -> Result<Vec<SymbolDef>> {
        self.call(
            "FindDefs",
            vec![json(project_id), json(workspace_id), json(name)],
        )
        .await
    }

    async fn find_refs(
        &self,
        project_id: &str,
        workspace_id: &str,
        name: &str,
    ) -> Result<Vec<SymbolRef>> {
        self.call(
            "FindRefs",
            vec![json(project_id), json(workspace_id), json(name)],
        )
        .await
    }

    async fn generate_commit_message(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<String> {
        self.call(
            "GenerateCommitMessage",
            vec![json(project_id), json(workspace_id)],
        )
        .await
    }

    async fn git_status(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        self.call("GitStatus", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn list_all_skills(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<AgentSkill>> {
        self.call("ListAllSkills", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn list_changed_files(&self, workspace_id: &str) -> Result<Vec<String>> {
        self.call("ListChangedFiles", vec![json(workspace_id)])
            .await
    }

    async fn list_commits(&self, project_id: &str, workspace_id: &str) -> Result<Vec<Commit>> {
        self.call("ListCommits", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn list_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        self.call("ListFiles", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn list_skills(&self, project_id: &str, workspace_id: &str) -> Result<Vec<AgentSkill>> {
        self.call("ListSkills", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn list_tree(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<Vec<TreeEntry>> {
        self.call(
            "ListTree",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn list_trees(
        &self,
        project_id: &str,
        workspace_id: &str,
        dirs: &[String],
    ) -> Result<BTreeMap<String, Vec<TreeEntry>>> {
        self.call(
            "ListTrees",
            vec![json(project_id), json(workspace_id), json(dirs)],
        )
        .await
    }

    async fn mkdir(&self, project_id: &str, workspace_id: &str, path: &str) -> Result<()> {
        self.call(
            "Mkdir",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn open_in_editor(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        line: i64,
    ) -> Result<()> {
        self.call(
            "OpenInEditor",
            vec![json(project_id), json(workspace_id), json(path), json(line)],
        )
        .await
    }

    async fn outline(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<Vec<OutlineSymbol>> {
        self.call(
            "Outline",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn read_file(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<FileContent> {
        self.call(
            "ReadFile",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn refresh_file_index(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<String>> {
        self.call(
            "RefreshFileIndex",
            vec![json(project_id), json(workspace_id)],
        )
        .await
    }

    async fn rename(
        &self,
        project_id: &str,
        workspace_id: &str,
        from: &str,
        to: &str,
    ) -> Result<()> {
        self.call(
            "Rename",
            vec![json(project_id), json(workspace_id), json(from), json(to)],
        )
        .await
    }

    async fn replace_in_files(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        replacement: &str,
        opts: &SearchOpts,
    ) -> Result<ReplaceResult> {
        self.call(
            "ReplaceInFiles",
            vec![
                json(project_id),
                json(workspace_id),
                json(query),
                json(replacement),
                json(opts),
            ],
        )
        .await
    }

    async fn reveal_in_finder(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
    ) -> Result<()> {
        self.call(
            "RevealInFinder",
            vec![json(project_id), json(workspace_id), json(path)],
        )
        .await
    }

    async fn search(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
    ) -> Result<Vec<SearchHit>> {
        self.call(
            "Search",
            vec![json(project_id), json(workspace_id), json(query)],
        )
        .await
    }

    async fn search_advanced(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        opts: &SearchOpts,
    ) -> Result<SearchResult> {
        self.call(
            "SearchAdvanced",
            vec![
                json(project_id),
                json(workspace_id),
                json(query),
                json(opts),
            ],
        )
        .await
    }

    async fn search_files(
        &self,
        project_id: &str,
        workspace_id: &str,
        query: &str,
        limit: i64,
        files_only: bool,
    ) -> Result<FileSearchResult> {
        self.call(
            "SearchFiles",
            vec![
                json(project_id),
                json(workspace_id),
                json(query),
                json(limit),
                json(files_only),
            ],
        )
        .await
    }

    async fn set_file_index_exclude(&self, patterns: &[String]) -> Result<()> {
        self.call("SetFileIndexExclude", vec![json(patterns)]).await
    }

    async fn stage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()> {
        self.call(
            "Stage",
            vec![json(project_id), json(workspace_id), json(paths)],
        )
        .await
    }

    async fn staged_files(&self, project_id: &str, workspace_id: &str) -> Result<Vec<String>> {
        self.call("StagedFiles", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn unstage(&self, project_id: &str, workspace_id: &str, paths: &[String]) -> Result<()> {
        self.call(
            "Unstage",
            vec![json(project_id), json(workspace_id), json(paths)],
        )
        .await
    }

    async fn working_status(
        &self,
        project_id: &str,
        workspace_id: &str,
    ) -> Result<BTreeMap<String, String>> {
        self.call("WorkingStatus", vec![json(project_id), json(workspace_id)])
            .await
    }

    async fn write_file(
        &self,
        project_id: &str,
        workspace_id: &str,
        path: &str,
        content: &str,
    ) -> Result<()> {
        self.call(
            "WriteFile",
            vec![
                json(project_id),
                json(workspace_id),
                json(path),
                json(content),
            ],
        )
        .await
    }
}

#[async_trait]
impl EditorSessionService for MockBridge {
    async fn clear_editor_session(&self, workspace_id: &str) -> Result<()> {
        self.call("ClearEditorSession", vec![json(workspace_id)])
            .await
    }

    async fn get_editor_session(&self, workspace_id: &str) -> Result<Option<EditorSession>> {
        self.call("GetEditorSession", vec![json(workspace_id)])
            .await
    }

    async fn update_editor_session(
        &self,
        workspace_id: &str,
        patch: &EditorSessionPatch,
    ) -> Result<()> {
        self.call("UpdateEditorSession", vec![json(workspace_id), json(patch)])
            .await
    }
}

#[async_trait]
impl EscalationService for MockBridge {
    async fn answer_escalation(
        &self,
        workspace_id: &str,
        choice: &str,
        free_text: &str,
        agent_code: &str,
    ) -> Result<Escalation> {
        self.call(
            "AnswerEscalation",
            vec![
                json(workspace_id),
                json(choice),
                json(free_text),
                json(agent_code),
            ],
        )
        .await
    }

    async fn get_escalation(&self, workspace_id: &str) -> Result<Escalation> {
        self.call("GetEscalation", vec![json(workspace_id)]).await
    }
}

#[async_trait]
impl InboxService for MockBridge {
    async fn get_inbox(&self) -> Result<Vec<InboxItem>> {
        self.call("GetInbox", vec![]).await
    }

    async fn mark_all_inbox_read(&self) -> Result<i64> {
        self.call("MarkAllInboxRead", vec![]).await
    }

    async fn mark_inbox_read(&self, id: &str) -> Result<i64> {
        self.call("MarkInboxRead", vec![json(id)]).await
    }
}

#[async_trait]
impl LspService for MockBridge {
    async fn close_lsp(&self, id: &str) -> Result<()> {
        self.call("CloseLSP", vec![json(id)]).await
    }

    async fn close_workspace_lsp(&self, workspace_id: &str) -> Result<()> {
        self.call("CloseWorkspaceLSP", vec![json(workspace_id)])
            .await
    }

    async fn ensure_lsp(
        &self,
        project_id: &str,
        workspace_id: &str,
        language_id: &str,
        rel_path: &str,
    ) -> Result<EnsureResult> {
        self.call(
            "EnsureLSP",
            vec![
                json(project_id),
                json(workspace_id),
                json(language_id),
                json(rel_path),
            ],
        )
        .await
    }

    async fn list_language_servers(&self) -> Result<Vec<LanguageServerInfo>> {
        self.call("ListLanguageServers", vec![]).await
    }

    async fn write_lsp(&self, id: &str, message: &str) -> Result<()> {
        self.call("WriteLSP", vec![json(id), json(message)]).await
    }
}

#[async_trait]
impl MaestroService for MockBridge {
    async fn cancel_maestro_message(&self, workspace_id: &str) -> Result<()> {
        self.call("CancelMaestroMessage", vec![json(workspace_id)])
            .await
    }

    async fn get_maestro_chat(&self, workspace_id: &str) -> Result<Vec<ChatItem>> {
        self.call("GetMaestroChat", vec![json(workspace_id)]).await
    }

    async fn send_maestro_message(
        &self,
        workspace_id: &str,
        text: &str,
        attachments: &[ChatAttachment],
    ) -> Result<String> {
        self.call(
            "SendMaestroMessage",
            vec![json(workspace_id), json(text), json(attachments)],
        )
        .await
    }
}

#[async_trait]
impl ObservatoryService for MockBridge {
    async fn add_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        body: &str,
    ) -> Result<Finding> {
        self.call(
            "AddFindingNote",
            vec![json(workspace_id), json(finding_id), json(body)],
        )
        .await
    }

    async fn create_finding(
        &self,
        workspace_id: &str,
        file: &str,
        line: i64,
        message: &str,
        end_line: i64,
    ) -> Result<Finding> {
        self.call(
            "CreateFinding",
            vec![
                json(workspace_id),
                json(file),
                json(line),
                json(message),
                json(end_line),
            ],
        )
        .await
    }

    async fn delete_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        note_id: &str,
    ) -> Result<Finding> {
        self.call(
            "DeleteFindingNote",
            vec![json(workspace_id), json(finding_id), json(note_id)],
        )
        .await
    }

    async fn list_findings(&self, workspace_id: &str) -> Result<Vec<Finding>> {
        self.call("ListFindings", vec![json(workspace_id)]).await
    }

    async fn set_finding_status(
        &self,
        workspace_id: &str,
        finding_id: &str,
        status: &str,
    ) -> Result<Finding> {
        self.call(
            "SetFindingStatus",
            vec![json(workspace_id), json(finding_id), json(status)],
        )
        .await
    }
}

#[async_trait]
impl PermissionsService for MockBridge {
    async fn answer_permission(
        &self,
        id: &str,
        allow: bool,
        reason: &str,
        remember: bool,
    ) -> Result<()> {
        self.call(
            "AnswerPermission",
            vec![json(id), json(allow), json(reason), json(remember)],
        )
        .await
    }

    async fn list_pending_permissions(&self) -> Result<Vec<PermissionRequest>> {
        self.call("ListPendingPermissions", vec![]).await
    }
}

#[async_trait]
impl PolicyService for MockBridge {
    async fn get_policy(&self) -> Result<Policy> {
        self.call("GetPolicy", vec![]).await
    }

    async fn set_policy(&self, p: &Policy) -> Result<Policy> {
        self.call("SetPolicy", vec![json(p)]).await
    }
}

#[async_trait]
impl ProjectsService for MockBridge {
    async fn add_project(&self, p: &Project) -> Result<Project> {
        self.call("AddProject", vec![json(p)]).await
    }

    async fn add_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup> {
        self.call("AddProjectGroup", vec![json(g)]).await
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        self.call("DeleteProject", vec![json(id)]).await
    }

    async fn delete_project_group(&self, id: &str) -> Result<()> {
        self.call("DeleteProjectGroup", vec![json(id)]).await
    }

    async fn delete_scheduled_prompt(&self, id: &str) -> Result<()> {
        self.call("DeleteScheduledPrompt", vec![json(id)]).await
    }

    async fn get_project(&self, id: &str) -> Result<Project> {
        self.call("GetProject", vec![json(id)]).await
    }

    async fn list_branches(&self, project_id: &str) -> Result<Vec<BranchChoice>> {
        self.call("ListBranches", vec![json(project_id)]).await
    }

    async fn list_branches_for_path(
        &self,
        repo_path: &str,
        preferred: &str,
    ) -> Result<Vec<BranchChoice>> {
        self.call(
            "ListBranchesForPath",
            vec![json(repo_path), json(preferred)],
        )
        .await
    }

    async fn list_project_groups(&self) -> Result<Vec<ProjectGroup>> {
        self.call("ListProjectGroups", vec![]).await
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        self.call("ListProjects", vec![]).await
    }

    async fn list_scheduled_prompts(&self, project_id: &str) -> Result<Vec<ScheduledPrompt>> {
        self.call("ListScheduledPrompts", vec![json(project_id)])
            .await
    }

    async fn pause_scheduled_prompt(&self, id: &str, paused: bool) -> Result<()> {
        self.call("PauseScheduledPrompt", vec![json(id), json(paused)])
            .await
    }

    async fn preview_copy_files(
        &self,
        project_id: &str,
        patterns: &str,
    ) -> Result<CopyFilesPreview> {
        self.call("PreviewCopyFiles", vec![json(project_id), json(patterns)])
            .await
    }

    async fn preview_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<Vec<i64>> {
        self.call("PreviewScheduledPrompt", vec![json(p)]).await
    }

    async fn reorder_project_groups(&self, ids: &[String]) -> Result<()> {
        self.call("ReorderProjectGroups", vec![json(ids)]).await
    }

    async fn reorder_projects(&self, ids: &[String]) -> Result<()> {
        self.call("ReorderProjects", vec![json(ids)]).await
    }

    async fn run_scheduled_prompt_now(&self, id: &str) -> Result<String> {
        self.call("RunScheduledPromptNow", vec![json(id)]).await
    }

    async fn save_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<ScheduledPrompt> {
        self.call("SaveScheduledPrompt", vec![json(p)]).await
    }

    async fn set_project_group(&self, project_id: &str, group_id: &str) -> Result<Project> {
        self.call("SetProjectGroup", vec![json(project_id), json(group_id)])
            .await
    }

    async fn suggest_base_branch(&self, repo_path: &str) -> Result<String> {
        self.call("SuggestBaseBranch", vec![json(repo_path)]).await
    }

    async fn update_project_group(&self, g: &ProjectGroup) -> Result<ProjectGroup> {
        self.call("UpdateProjectGroup", vec![json(g)]).await
    }

    async fn update_project_run_config(&self, p: &Project) -> Result<Project> {
        self.call("UpdateProjectRunConfig", vec![json(p)]).await
    }
}

#[async_trait]
impl PrsService for MockBridge {
    async fn add_pr_comment(
        &self,
        pr_id: &str,
        file: &str,
        line: i64,
        body: &str,
        start_line: i64,
        finding_id: &str,
    ) -> Result<PrComment> {
        self.call(
            "AddPRComment",
            vec![
                json(pr_id),
                json(file),
                json(line),
                json(body),
                json(start_line),
                json(finding_id),
            ],
        )
        .await
    }

    async fn create_workspace_from_pr(&self, pr_id: &str) -> Result<Workspace> {
        self.call("CreateWorkspaceFromPR", vec![json(pr_id)]).await
    }

    async fn delete_pr_comment(&self, pr_id: &str, comment_id: &str) -> Result<Pr> {
        self.call("DeletePRComment", vec![json(pr_id), json(comment_id)])
            .await
    }

    async fn ensure_pr_chat_host(&self, pr_id: &str) -> Result<Workspace> {
        self.call("EnsurePRChatHost", vec![json(pr_id)]).await
    }

    async fn get_commit_diff(&self, id: &str, hash: &str) -> Result<Commit> {
        self.call("GetCommitDiff", vec![json(id), json(hash)]).await
    }

    async fn get_pr(&self, id: &str) -> Result<Pr> {
        self.call("GetPR", vec![json(id)]).await
    }

    async fn get_pr_commits(&self, id: &str) -> Result<Vec<Commit>> {
        self.call("GetPRCommits", vec![json(id)]).await
    }

    async fn get_pr_files(&self, id: &str) -> Result<Vec<FileDiff>> {
        self.call("GetPRFiles", vec![json(id)]).await
    }

    async fn list_pr_findings(&self, pr_id: &str) -> Result<Vec<Finding>> {
        self.call("ListPRFindings", vec![json(pr_id)]).await
    }

    async fn list_prs(&self) -> Result<Vec<Pr>> {
        self.call("ListPRs", vec![]).await
    }

    async fn merge_pr(&self, pr_id: &str, method: &str) -> Result<Pr> {
        self.call("MergePR", vec![json(pr_id), json(method)]).await
    }

    async fn ready_pr(&self, pr_id: &str) -> Result<Pr> {
        self.call("ReadyPR", vec![json(pr_id)]).await
    }

    async fn reply_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<PrComment> {
        self.call(
            "ReplyPRComment",
            vec![json(pr_id), json(comment_id), json(body)],
        )
        .await
    }

    async fn search_pr_mentionable_users(
        &self,
        pr_id: &str,
        query: &str,
    ) -> Result<Vec<PrParticipant>> {
        self.call("SearchPRMentionableUsers", vec![json(pr_id), json(query)])
            .await
    }

    async fn send_back_to_loop(&self, pr_id: &str, comments: &[String]) -> Result<Pr> {
        self.call("SendBackToLoop", vec![json(pr_id), json(comments)])
            .await
    }

    async fn set_pr_draft(&self, pr_id: &str, draft: bool) -> Result<Pr> {
        self.call("SetPRDraft", vec![json(pr_id), json(draft)])
            .await
    }

    async fn set_pr_thread_resolved(
        &self,
        pr_id: &str,
        thread_id: &str,
        resolved: bool,
    ) -> Result<Pr> {
        self.call(
            "SetPRThreadResolved",
            vec![json(pr_id), json(thread_id), json(resolved)],
        )
        .await
    }

    async fn submit_review(&self, pr_id: &str, kind: &str, body: &str) -> Result<Pr> {
        self.call("SubmitReview", vec![json(pr_id), json(kind), json(body)])
            .await
    }

    async fn sync_github_prs(&self) -> Result<Vec<Pr>> {
        self.call("SyncGitHubPRs", vec![]).await
    }

    async fn toggle_pr_comment_reaction(
        &self,
        pr_id: &str,
        comment_id: &str,
        content: &str,
    ) -> Result<Pr> {
        self.call(
            "TogglePRCommentReaction",
            vec![json(pr_id), json(comment_id), json(content)],
        )
        .await
    }

    async fn update_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<PrComment> {
        self.call(
            "UpdatePRComment",
            vec![json(pr_id), json(comment_id), json(body)],
        )
        .await
    }

    async fn update_pr_details(&self, pr_id: &str, title: &str, body: &str) -> Result<Pr> {
        self.call(
            "UpdatePRDetails",
            vec![json(pr_id), json(title), json(body)],
        )
        .await
    }

    async fn update_with_base(&self, pr_id: &str, method: &str) -> Result<UpdateWorkspaceResult> {
        self.call("UpdateWithBase", vec![json(pr_id), json(method)])
            .await
    }
}

#[async_trait]
impl QuestionsService for MockBridge {
    async fn answer_question(
        &self,
        id: &str,
        choice: &str,
        free_text: &str,
        attachments: &[ChatAttachment],
    ) -> Result<()> {
        self.call(
            "AnswerQuestion",
            vec![json(id), json(choice), json(free_text), json(attachments)],
        )
        .await
    }

    async fn list_pending_questions(&self) -> Result<Vec<UserQuestion>> {
        self.call("ListPendingQuestions", vec![]).await
    }
}

#[async_trait]
impl SettingsService for MockBridge {
    async fn cancel_chat_gpt_login(&self, login_id: &str) -> Result<()> {
        self.call("CancelChatGPTLogin", vec![json(login_id)]).await
    }

    async fn cancel_codex_login(&self, login_id: &str) -> Result<()> {
        self.call("CancelCodexLogin", vec![json(login_id)]).await
    }

    async fn disconnect_chat_gpt(&self) -> Result<ChatGPTStatus> {
        self.call("DisconnectChatGPT", vec![]).await
    }

    async fn get_chat_gpt_status(&self) -> Result<ChatGPTStatus> {
        self.call("GetChatGPTStatus", vec![]).await
    }

    async fn refresh_chat_gpt_models(&self) -> Result<ChatGPTStatus> {
        self.call("RefreshChatGPTModels", vec![]).await
    }

    async fn start_chat_gpt_login(&self, account_id: &str, method: &str) -> Result<ChatGPTLogin> {
        self.call("StartChatGPTLogin", vec![json(account_id), json(method)])
            .await
    }

    async fn start_codex_login(&self) -> Result<CodexLogin> {
        self.call("StartCodexLogin", vec![]).await
    }

    async fn wait_chat_gpt_login(&self, login_id: &str) -> Result<ChatGPTStatus> {
        self.call("WaitChatGPTLogin", vec![json(login_id)]).await
    }

    async fn wait_codex_login(&self, login_id: &str) -> Result<CodexStatus> {
        self.call("WaitCodexLogin", vec![json(login_id)]).await
    }

    async fn cancel_cursor_usage_connect(&self) -> Result<()> {
        self.call("CancelCursorUsageConnect", vec![]).await
    }

    async fn clear_all_data(&self) -> Result<()> {
        self.call("ClearAllData", vec![]).await
    }

    async fn compatible_flag_preset(&self, name: &str) -> Result<Vec<CompatibleFlagDim>> {
        self.call("CompatibleFlagPreset", vec![json(name)]).await
    }

    async fn connect_cursor_usage(&self) -> Result<CursorUsageConnect> {
        self.call("ConnectCursorUsage", vec![]).await
    }

    async fn detect_agent(&self) -> Result<String> {
        self.call("DetectAgent", vec![]).await
    }

    async fn disconnect_cursor_usage(&self) -> Result<()> {
        self.call("DisconnectCursorUsage", vec![]).await
    }

    async fn get_claude_status(&self) -> Result<ClaudeStatus> {
        self.call("GetClaudeStatus", vec![]).await
    }

    async fn get_codex_status(&self) -> Result<CodexStatus> {
        self.call("GetCodexStatus", vec![]).await
    }

    async fn get_cursor_status(&self) -> Result<CursorStatus> {
        self.call("GetCursorStatus", vec![]).await
    }

    async fn get_instance_label(&self) -> Result<String> {
        self.call("GetInstanceLabel", vec![]).await
    }

    async fn get_provider_usage(&self) -> Result<Snapshot> {
        self.call("GetProviderUsage", vec![]).await
    }

    async fn get_resource(&self) -> Result<Resource> {
        self.call("GetResource", vec![]).await
    }

    async fn get_settings(&self) -> Result<Settings> {
        self.call("GetSettings", vec![]).await
    }

    async fn github_login(&self) -> Result<String> {
        self.call("GitHubLogin", vec![]).await
    }

    async fn install_cursor_bridge(&self) -> Result<CursorStatus> {
        self.call("InstallCursorBridge", vec![]).await
    }

    async fn list_assigned_issues(&self) -> Result<Vec<AssignedIssue>> {
        self.call("ListAssignedIssues", vec![]).await
    }

    async fn list_model_catalog(&self) -> Result<Vec<ModelGroup>> {
        self.call("ListModelCatalog", vec![]).await
    }

    async fn list_monospace_fonts(&self) -> Result<Vec<String>> {
        self.call("ListMonospaceFonts", vec![]).await
    }

    async fn logs_path(&self) -> Result<String> {
        self.call("LogsPath", vec![]).await
    }

    async fn open_linear_in_app(&self, raw: &str) -> Result<bool> {
        self.call("OpenLinearInApp", vec![json(raw)]).await
    }

    async fn refresh_provider_usage(&self) -> Result<Snapshot> {
        self.call("RefreshProviderUsage", vec![]).await
    }

    async fn reveal_logs(&self) -> Result<()> {
        self.call("RevealLogs", vec![]).await
    }

    async fn set_settings(&self, set: &Settings) -> Result<Settings> {
        self.call("SetSettings", vec![json(set)]).await
    }

    async fn test_codex_key(&self, api_key: &str, chat_gpt_auth: bool) -> Result<CodexStatus> {
        self.call("TestCodexKey", vec![json(api_key), json(chat_gpt_auth)])
            .await
    }

    async fn test_compatible_provider(
        &self,
        p: &CompatibleProvider,
    ) -> Result<CompatibleTestResult> {
        self.call("TestCompatibleProvider", vec![json(p)]).await
    }

    async fn test_cursor_key(&self, api_key: &str) -> Result<CursorStatus> {
        self.call("TestCursorKey", vec![json(api_key)]).await
    }
}

#[async_trait]
impl ShellService for MockBridge {
    async fn flush(&self) -> Result<()> {
        self.call("Flush", vec![]).await
    }

    async fn get_expanded_schedules(&self) -> Result<Vec<String>> {
        self.call("GetExpandedSchedules", vec![]).await
    }

    async fn get_issues_view(&self) -> Result<IssuesView> {
        self.call("GetIssuesView", vec![]).await
    }

    async fn get_shell_state(&self) -> Result<ShellState> {
        self.call("GetShellState", vec![]).await
    }

    async fn set_expanded_schedules(&self, ids: &[String]) -> Result<()> {
        self.call("SetExpandedSchedules", vec![json(ids)]).await
    }

    async fn set_issues_view(&self, v: &IssuesView) -> Result<()> {
        self.call("SetIssuesView", vec![json(v)]).await
    }

    async fn set_shell_state(&self, st: &ShellState) -> Result<()> {
        self.call("SetShellState", vec![json(st)]).await
    }
}

#[async_trait]
impl TerminalService for MockBridge {
    async fn ack_terminal(&self, id: &str, n: i64) -> Result<()> {
        self.call("AckTerminal", vec![json(id), json(n)]).await
    }

    async fn attach_terminal(&self, id: &str) -> Result<String> {
        self.call("AttachTerminal", vec![json(id)]).await
    }

    async fn clear_terminal(&self, id: &str) -> Result<()> {
        self.call("ClearTerminal", vec![json(id)]).await
    }

    async fn close_terminal(&self, id: &str) -> Result<()> {
        self.call("CloseTerminal", vec![json(id)]).await
    }

    async fn detach_terminal(&self, id: &str) -> Result<()> {
        self.call("DetachTerminal", vec![json(id)]).await
    }

    async fn interrupt_terminal(&self, id: &str) -> Result<()> {
        self.call("InterruptTerminal", vec![json(id)]).await
    }

    async fn list_terminal_dock_chrome(&self) -> Result<Vec<TerminalDockChrome>> {
        self.call("ListTerminalDockChrome", vec![]).await
    }

    async fn list_terminals(&self, workspace_id: &str) -> Result<Vec<TermInfo>> {
        self.call("ListTerminals", vec![json(workspace_id)]).await
    }

    async fn open_terminal(&self, workspace_id: &str, cols: i64, rows: i64) -> Result<TermInfo> {
        self.call(
            "OpenTerminal",
            vec![json(workspace_id), json(cols), json(rows)],
        )
        .await
    }

    async fn reorder_terminals(&self, workspace_id: &str, ids: &[String]) -> Result<()> {
        self.call("ReorderTerminals", vec![json(workspace_id), json(ids)])
            .await
    }

    async fn resize_terminal(&self, id: &str, cols: i64, rows: i64) -> Result<()> {
        self.call("ResizeTerminal", vec![json(id), json(cols), json(rows)])
            .await
    }

    async fn restore_workspace_terminals(
        &self,
        workspace_id: &str,
        cols: i64,
        rows: i64,
    ) -> Result<Vec<TermInfo>> {
        self.call(
            "RestoreWorkspaceTerminals",
            vec![json(workspace_id), json(cols), json(rows)],
        )
        .await
    }

    async fn set_active_terminal(&self, workspace_id: &str, id: &str) -> Result<()> {
        self.call("SetActiveTerminal", vec![json(workspace_id), json(id)])
            .await
    }

    async fn set_terminal_dock_chrome(
        &self,
        workspace_id: &str,
        chrome: &TerminalDockChrome,
    ) -> Result<()> {
        self.call(
            "SetTerminalDockChrome",
            vec![json(workspace_id), json(chrome)],
        )
        .await
    }

    async fn set_terminal_title(&self, id: &str, title: &str) -> Result<()> {
        self.call("SetTerminalTitle", vec![json(id), json(title)])
            .await
    }

    async fn write_terminal(&self, id: &str, data: &str) -> Result<()> {
        self.call("WriteTerminal", vec![json(id), json(data)]).await
    }
}

#[async_trait]
#[allow(clippy::too_many_arguments)]
impl WorkspacesService for MockBridge {
    async fn apply_provisional_chat_titles(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
    ) -> Result<()> {
        self.call(
            "ApplyProvisionalChatTitles",
            vec![json(workspace_id), json(code), json(text)],
        )
        .await
    }

    async fn approve_plan(&self, workspace_id: &str) -> Result<String> {
        self.call("ApprovePlan", vec![json(workspace_id)]).await
    }

    async fn attention_counts(&self) -> Result<AttentionCounts> {
        self.call("AttentionCounts", vec![]).await
    }

    async fn cancel_workspace_message(&self, workspace_id: &str, code: &str) -> Result<()> {
        self.call(
            "CancelWorkspaceMessage",
            vec![json(workspace_id), json(code)],
        )
        .await
    }

    async fn clear_workspace_issue(&self, workspace_id: &str) -> Result<Workspace> {
        self.call("ClearWorkspaceIssue", vec![json(workspace_id)])
            .await
    }

    async fn close_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        self.call("CloseWorkspaceChat", vec![json(workspace_id), json(code)])
            .await
    }

    async fn create_workspace(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        simplify: Option<bool>,
    ) -> Result<Workspace> {
        self.call(
            "CreateWorkspace",
            vec![
                json(project_id),
                json(name),
                json(base_branch),
                json(mode),
                json(prompt),
                json(autonomy),
                json(simplify),
            ],
        )
        .await
    }

    async fn create_workspace_chat(&self, workspace_id: &str) -> Result<WorkspaceChatTab> {
        self.call("CreateWorkspaceChat", vec![json(workspace_id)])
            .await
    }

    async fn create_workspace_with(
        &self,
        project_id: &str,
        name: &str,
        base_branch: &str,
        mode: &str,
        prompt: &str,
        autonomy: &str,
        loop_id: &str,
        policy_id: &str,
        simplify: Option<bool>,
        attachments: &[ChatAttachment],
    ) -> Result<Workspace> {
        self.call(
            "CreateWorkspaceWith",
            vec![
                json(project_id),
                json(name),
                json(base_branch),
                json(mode),
                json(prompt),
                json(autonomy),
                json(loop_id),
                json(policy_id),
                json(simplify),
                json(attachments),
            ],
        )
        .await
    }

    async fn delete_closed_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        self.call(
            "DeleteClosedWorkspaceChat",
            vec![json(workspace_id), json(code)],
        )
        .await
    }

    async fn delete_workspace(&self, id: &str) -> Result<()> {
        self.call("DeleteWorkspace", vec![json(id)]).await
    }

    async fn engage_loop(&self, workspace_id: &str, goal: &str) -> Result<String> {
        self.call("EngageLoop", vec![json(workspace_id), json(goal)])
            .await
    }

    async fn engage_loop_with(
        &self,
        workspace_id: &str,
        goal: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<String> {
        self.call(
            "EngageLoopWith",
            vec![
                json(workspace_id),
                json(goal),
                json(loop_id),
                json(policy_id),
            ],
        )
        .await
    }

    async fn fetch_working_copy_upstream(&self, id: &str) -> Result<WorkspaceLandState> {
        self.call("FetchWorkingCopyUpstream", vec![json(id)]).await
    }

    async fn fork_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<WorkspaceChatTab> {
        self.call(
            "ForkWorkspaceChat",
            vec![json(workspace_id), json(code), json(message_id)],
        )
        .await
    }

    async fn fork_workspace_chat_to_workspace(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Workspace> {
        self.call(
            "ForkWorkspaceChatToWorkspace",
            vec![json(workspace_id), json(code), json(message_id)],
        )
        .await
    }

    async fn get_workspace(&self, id: &str) -> Result<Workspace> {
        self.call("GetWorkspace", vec![json(id)]).await
    }

    async fn get_workspace_feed(&self, workspace_id: &str) -> Result<Vec<FeedItem>> {
        self.call("GetWorkspaceFeed", vec![json(workspace_id)])
            .await
    }

    async fn last_assistant_text(&self, workspace_id: &str) -> Result<String> {
        self.call("LastAssistantText", vec![json(workspace_id)])
            .await
    }

    async fn list_closed_workspace_chats(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        self.call("ListClosedWorkspaceChats", vec![json(workspace_id)])
            .await
    }

    async fn list_open_in_targets(&self) -> Result<Vec<OpenInTarget>> {
        self.call("ListOpenInTargets", vec![]).await
    }

    async fn list_workspace_chats(&self, workspace_id: &str) -> Result<Vec<WorkspaceChatTab>> {
        self.call("ListWorkspaceChats", vec![json(workspace_id)])
            .await
    }

    async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        self.call("ListWorkspaces", vec![]).await
    }

    async fn mark_workspace_chat_seen(&self, workspace_id: &str, code: &str) -> Result<()> {
        self.call(
            "MarkWorkspaceChatSeen",
            vec![json(workspace_id), json(code)],
        )
        .await
    }

    async fn merge_workspace(&self, workspace_id: &str, method: &str) -> Result<Workspace> {
        self.call("MergeWorkspace", vec![json(workspace_id), json(method)])
            .await
    }

    async fn open_main_workspace(&self, project_id: &str) -> Result<Workspace> {
        self.call("OpenMainWorkspace", vec![json(project_id)]).await
    }

    async fn open_path(&self, path: &str) -> Result<OpenPathResult> {
        self.call("OpenPath", vec![json(path)]).await
    }

    async fn open_workspace_in(&self, workspace_id: &str, target_id: &str) -> Result<()> {
        self.call("OpenWorkspaceIn", vec![json(workspace_id), json(target_id)])
            .await
    }

    async fn open_workspace_pr(&self, workspace_id: &str, force: bool) -> Result<String> {
        self.call("OpenWorkspacePR", vec![json(workspace_id), json(force)])
            .await
    }

    async fn pause_loop(&self, id: &str) -> Result<()> {
        self.call("PauseLoop", vec![json(id)]).await
    }

    async fn pull_workspace_pr(&self, workspace_id: &str) -> Result<UpdateWorkspaceResult> {
        self.call("PullWorkspacePR", vec![json(workspace_id)]).await
    }

    async fn push_working_copy(&self, workspace_id: &str) -> Result<Workspace> {
        self.call("PushWorkingCopy", vec![json(workspace_id)]).await
    }

    async fn remember_chat_loop_policy(
        &self,
        workspace_id: &str,
        code: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<()> {
        self.call(
            "RememberChatLoopPolicy",
            vec![
                json(workspace_id),
                json(code),
                json(loop_id),
                json(policy_id),
            ],
        )
        .await
    }

    async fn remember_workspace_chat_prompt_group(
        &self,
        workspace_id: &str,
        code: &str,
        group: &str,
    ) -> Result<()> {
        self.call(
            "RememberWorkspaceChatPromptGroup",
            vec![json(workspace_id), json(code), json(group)],
        )
        .await
    }

    async fn remember_workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        mode: &str,
        simplify: Option<bool>,
    ) -> Result<()> {
        self.call(
            "RememberWorkspaceChatSelection",
            vec![
                json(workspace_id),
                json(code),
                json(model),
                json(flags),
                json(mode),
                json(simplify),
            ],
        )
        .await
    }

    async fn remember_workspace_chat_simplify(
        &self,
        workspace_id: &str,
        code: &str,
        on: bool,
    ) -> Result<()> {
        self.call(
            "RememberWorkspaceChatSimplify",
            vec![json(workspace_id), json(code), json(on)],
        )
        .await
    }

    async fn remove_workspace(&self, id: &str) -> Result<Workspace> {
        self.call("RemoveWorkspace", vec![json(id)]).await
    }

    async fn reopen_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<Vec<WorkspaceChatTab>> {
        self.call("ReopenWorkspaceChat", vec![json(workspace_id), json(code)])
            .await
    }

    async fn reorder_workspace_chats(&self, workspace_id: &str, codes: &[String]) -> Result<()> {
        self.call(
            "ReorderWorkspaceChats",
            vec![json(workspace_id), json(codes)],
        )
        .await
    }

    async fn reset_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<()> {
        self.call(
            "ResetWorkspaceChat",
            vec![json(workspace_id), json(code), json(message_id)],
        )
        .await
    }

    async fn restart_loop(&self, id: &str) -> Result<String> {
        self.call("RestartLoop", vec![json(id)]).await
    }

    async fn resume_loop(&self, id: &str) -> Result<()> {
        self.call("ResumeLoop", vec![json(id)]).await
    }

    async fn retry_loop(&self, id: &str) -> Result<()> {
        self.call("RetryLoop", vec![json(id)]).await
    }

    async fn retry_workspace_setup(&self, id: &str) -> Result<()> {
        self.call("RetryWorkspaceSetup", vec![json(id)]).await
    }

    async fn send_workspace_message(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        attachments: &[ChatAttachment],
        mode: &str,
        display_text: &str,
        fresh_session: bool,
    ) -> Result<String> {
        self.call(
            "SendWorkspaceMessage",
            vec![
                json(workspace_id),
                json(code),
                json(text),
                json(model),
                json(flags),
                json(attachments),
                json(mode),
                json(display_text),
                json(fresh_session),
            ],
        )
        .await
    }

    async fn set_chat_ui_occluded(&self, occluded: bool) -> Result<()> {
        self.call("SetChatUIOccluded", vec![json(occluded)]).await
    }

    async fn set_workspace_base_branch(
        &self,
        workspace_id: &str,
        base_branch: &str,
    ) -> Result<Workspace> {
        self.call(
            "SetWorkspaceBaseBranch",
            vec![json(workspace_id), json(base_branch)],
        )
        .await
    }

    async fn set_workspace_chat_focus(&self, workspace_id: &str, code: &str) -> Result<()> {
        self.call(
            "SetWorkspaceChatFocus",
            vec![json(workspace_id), json(code)],
        )
        .await
    }

    async fn set_workspace_issue(&self, workspace_id: &str, issue: &IssueRef) -> Result<Workspace> {
        self.call("SetWorkspaceIssue", vec![json(workspace_id), json(issue)])
            .await
    }

    async fn set_workspace_loop_policy(
        &self,
        workspace_id: &str,
        loop_id: &str,
        policy_id: &str,
    ) -> Result<Workspace> {
        self.call(
            "SetWorkspaceLoopPolicy",
            vec![json(workspace_id), json(loop_id), json(policy_id)],
        )
        .await
    }

    async fn start_workspace(
        &self,
        project_id: &str,
        task: &str,
        base_branch: &str,
        autonomy: &str,
    ) -> Result<String> {
        self.call(
            "StartWorkspace",
            vec![
                json(project_id),
                json(task),
                json(base_branch),
                json(autonomy),
            ],
        )
        .await
    }

    async fn stop_loop(&self, id: &str) -> Result<()> {
        self.call("StopLoop", vec![json(id)]).await
    }

    async fn submit_plan(
        &self,
        workspace_id: &str,
        summary: &str,
        body: &str,
        steps: &[String],
        step_ids: &[String],
    ) -> Result<String> {
        self.call(
            "SubmitPlan",
            vec![
                json(workspace_id),
                json(summary),
                json(body),
                json(steps),
                json(step_ids),
            ],
        )
        .await
    }

    async fn update_plan(&self, workspace_id: &str, plan: &str) -> Result<String> {
        self.call("UpdatePlan", vec![json(workspace_id), json(plan)])
            .await
    }

    async fn update_plan_step(
        &self,
        workspace_id: &str,
        step_id: &str,
        status: &str,
    ) -> Result<String> {
        self.call(
            "UpdatePlanStep",
            vec![json(workspace_id), json(step_id), json(status)],
        )
        .await
    }

    async fn update_working_copy_from_remote(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult> {
        self.call(
            "UpdateWorkingCopyFromRemote",
            vec![json(workspace_id), json(method)],
        )
        .await
    }

    async fn update_workspace_with_base(
        &self,
        workspace_id: &str,
        method: &str,
    ) -> Result<UpdateWorkspaceResult> {
        self.call(
            "UpdateWorkspaceWithBase",
            vec![json(workspace_id), json(method)],
        )
        .await
    }

    async fn watch_workspace(&self, workspace_id: &str) -> Result<()> {
        self.call("WatchWorkspace", vec![json(workspace_id)]).await
    }

    async fn workspace_chat_items(&self, workspace_id: &str, code: &str) -> Result<Vec<ChatItem>> {
        self.call("WorkspaceChatItems", vec![json(workspace_id), json(code)])
            .await
    }

    async fn workspace_chat_models(&self) -> Result<Vec<String>> {
        self.call("WorkspaceChatModels", vec![]).await
    }

    async fn workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Result<WorkspaceChatSelection> {
        self.call(
            "WorkspaceChatSelection",
            vec![json(workspace_id), json(code)],
        )
        .await
    }

    async fn workspace_chat_turn_diff(
        &self,
        workspace_id: &str,
        code: &str,
        message_id: &str,
    ) -> Result<Commit> {
        self.call(
            "WorkspaceChatTurnDiff",
            vec![json(workspace_id), json(code), json(message_id)],
        )
        .await
    }

    async fn workspace_discard_risk(&self, id: &str) -> Result<String> {
        self.call("WorkspaceDiscardRisk", vec![json(id)]).await
    }

    async fn workspace_facts(&self, workspace_id: &str) -> Result<Vec<FeedItem>> {
        self.call("WorkspaceFacts", vec![json(workspace_id)]).await
    }

    async fn workspace_land_state(&self, id: &str) -> Result<WorkspaceLandState> {
        self.call("WorkspaceLandState", vec![json(id)]).await
    }

    async fn workspace_sessions(
        &self,
        workspace_id: &str,
        owner_code: &str,
        agent_code: &str,
    ) -> Result<Vec<SessionView>> {
        self.call(
            "WorkspaceSessions",
            vec![json(workspace_id), json(owner_code), json(agent_code)],
        )
        .await
    }
}
