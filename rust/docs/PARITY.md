# Parity inventory

Every surface of the Go/Svelte app that the Rust port must carry over. Nothing here may be dropped. Tick an item only when the Rust implementation matches the original behavior and has a test or a parity screenshot. Each area owner expands their section with the detailed behaviors (edge cases, persistence, keyboard paths) as they port it.

Legend: `[ ]` not started, `[~]` in progress, `[x]` ported and verified.

## Open follow-ups

Gaps found while porting that are not yet owned by a slice. Each must be closed before promotion.

- [ ] **Linux: Mermaid and PDF.** `recur-content` runs mermaid.js and pdf.js in a hidden WebKit page on macOS only; Linux builds show an error in their place. Add the WebKitGTK 6.0 path (the Go app already depends on it).
- [ ] **Linux build and CI.** Nothing in `rust/` is built or tested on Linux yet. Add a `rust-linux` CI job (Ubuntu 24.04) and fix what it finds.
- [ ] **`recur_ui::text`: inline code chip at a wrap start** keeps its left padding on the previous line (`file-preview/markdown`, 514 hard pixels).
- [ ] **`overlays/palette`:** keycaps at fractional offsets in a scrolled list and the list thumb's end land one device pixel off (388 hard pixels).
- [ ] **Chat: TypeScript prompt modules.** `PromptCatalog` runs shebang prompts through `BundleService`; TypeScript prompt and completion modules need the macro worker, installed through `PromptCatalog::set_runners`. Owned by the bundles runtime.
- [ ] **Chat: workspace view halves.** `chat.spec.ts` runs against a host double (`spec::harness::Host`). When `WorkspaceView` lands, move its send / queue / steer / Stop / tab routing onto those tests and add the routing halves: prompt actions opening grouped or fresh tabs, the findings panel's Fix button, the explorer and Source Control panel when a transcript chip opens a file, tab persistence, reorder and strip scroll.
- [ ] **Chat: `inboxChatOpen`.** Ask Recur's hide button emits `AppChatEvent::Hide`; `AppState` has no `inboxChatOpen` yet, so the Briefing host must keep it (and pass `visible`).
- [ ] **Fractional line boxes (systemic, schedule before more view ports).** Seen first in chat cards: Chat cards carry one-device-pixel border offsets after 1.45 / 1.35 lines (`chat/question` 11824, `chat/fanout` 8882, `chat/pickers` 3053, `chat/plan` 2508 hard pixels): GPUI rounds each box to device pixels where WebKit snaps at paint.
- [ ] **`recur_ui::text` ellipsis** keeps the space before a truncation cut (`out …` for WebKit's `out…`).
- [ ] **Chat: hover color transitions** snap instead of easing over .12–.15s.
- [ ] **Typeahead hosts.** The PR review modal and the Observatory's commit message must call `suggest::attach_emoji` / `attach_user_mentions` on their fields when they are ported; the Observatory owns `CommandPrompts`.
- [ ] **Editor LSP adapter.** `recur-lsp` is a transport; the app layer needs the adapter that implements `recur_editor::lsp::LspClient` (request ids, `initialize`, document versions, change batching). Owned by the code-editor view port.

## Application services (former Wails bindings)

Source: `frontend/bindings/recur/internal/app/*.ts`, implemented in `internal/app`. Rust home: `recur-services`. The contract is `recur_services::api` (one async trait per service, annotated below with `Trait::method`); `recur_services::mock` answers all of it for views today. Items tick when the real implementation over the engine (slice SV) lands.

### agentsservice (2)

- [ ] `GetAgentTrace`: `AgentsService::get_agent_trace`
- [ ] `ListLoopAgents`: `AgentsService::list_loop_agents`

### artifactsservice (5)

- [ ] `DeleteArtifact`: `ArtifactsService::delete_artifact`
- [ ] `ListArtifacts`: `ArtifactsService::list_artifacts`
- [ ] `PresentArtifact`: `ArtifactsService::present_artifact`
- [ ] `RenameArtifact`: `ArtifactsService::rename_artifact`
- [ ] `RevealArtifact`: `ArtifactsService::reveal_artifact`

### briefingservice (12)

- [ ] `AppChatItems`: `BriefingService::app_chat_items`
- [ ] `AppChatSelection`: `BriefingService::app_chat_selection`
- [ ] `CancelAppMessage`: `BriefingService::cancel_app_message`
- [ ] `CloseAppChat`: `BriefingService::close_app_chat`
- [ ] `GetBriefing`: `BriefingService::get_briefing`
- [ ] `ListAppChats`: `BriefingService::list_app_chats`
- [ ] `MarkAppChatSeen`: `BriefingService::mark_app_chat_seen`
- [ ] `MarkWorkspaceViewed`: `BriefingService::mark_workspace_viewed`
- [ ] `NewAppChat`: `BriefingService::new_app_chat`
- [ ] `RememberAppChatSelection`: `BriefingService::remember_app_chat_selection`
- [ ] `SendAppMessage`: `BriefingService::send_app_message`
- [ ] `SetAppChatFocus`: `BriefingService::set_app_chat_focus`

### bundleservice (23)

- [ ] `CompiledCompleteModule`: `BundleService::compiled_complete_module`
- [ ] `CompiledModule`: `BundleService::compiled_module`
- [ ] `CompiledPromptModule`: `BundleService::compiled_prompt_module`
- [ ] `ExpandGoal`: `BundleService::expand_goal`
- [ ] `InvalidateCatalog`: `BundleService::invalidate_catalog`
- [ ] `ListBundleSettings`: `BundleService::list_bundle_settings`
- [ ] `ListCommands`: `BundleService::list_commands`
- [ ] `ListGrammars`: `BundleService::list_grammars`
- [ ] `ListLoops`: `BundleService::list_loops`
- [ ] `ListPolicies`: `BundleService::list_policies`
- [ ] `ListPromptActions`: `BundleService::list_prompt_actions`
- [ ] `ListRoles`: `BundleService::list_roles`
- [ ] `ListSkippedBundles`: `BundleService::list_skipped_bundles`
- [ ] `ListSnippets`: `BundleService::list_snippets`
- [ ] `ListSymbols`: `BundleService::list_symbols`
- [ ] `ListTerminalThemes`: `BundleService::list_terminal_themes`
- [ ] `ListThemes`: `BundleService::list_themes`
- [ ] `ReadGrammar`: `BundleService::read_grammar`
- [ ] `ReadTheme`: `BundleService::read_theme`
- [ ] `RunCommand`: `BundleService::run_command`
- [ ] `RunPromptAction`: `BundleService::run_prompt_action`
- [ ] `RunPromptComplete`: `BundleService::run_prompt_complete`
- [ ] `SearchPalette`: `BundleService::search_palette`

### codeservice (45)

- [ ] `AbsPath`: `CodeService::abs_path`
- [ ] `BridgeBenchDone`: `CodeService::bridge_bench_done`
- [ ] `BridgeBenchEnabled`: `CodeService::bridge_bench_enabled`
- [ ] `BridgeEcho`: `CodeService::bridge_echo`
- [ ] `BridgePing`: `CodeService::bridge_ping`
- [ ] `Commit`: `CodeService::commit`
- [ ] `CommitDiff`: `CodeService::commit_diff`
- [ ] `CommitStaged`: `CodeService::commit_staged`
- [ ] `Copy`: `CodeService::copy`
- [ ] `CreateFile`: `CodeService::create_file`
- [ ] `Delete`: `CodeService::delete`
- [ ] `DiffVsBase`: `CodeService::diff_vs_base`
- [ ] `DiffWorking`: `CodeService::diff_working`
- [ ] `DiscardAll`: `CodeService::discard_all`
- [ ] `DiscardFile`: `CodeService::discard_file`
- [ ] `FileAtBase`: `CodeService::file_at_base`
- [ ] `FileDiff`: `CodeService::file_diff`
- [ ] `FindDefs`: `CodeService::find_defs`
- [ ] `FindRefs`: `CodeService::find_refs`
- [ ] `GenerateCommitMessage`: `CodeService::generate_commit_message`
- [ ] `GitStatus`: `CodeService::git_status`
- [ ] `ListAllSkills`: `CodeService::list_all_skills`
- [ ] `ListChangedFiles`: `CodeService::list_changed_files`
- [ ] `ListCommits`: `CodeService::list_commits`
- [ ] `ListFiles`: `CodeService::list_files`
- [ ] `ListSkills`: `CodeService::list_skills`
- [ ] `ListTree`: `CodeService::list_tree`
- [ ] `ListTrees`: `CodeService::list_trees`
- [ ] `Mkdir`: `CodeService::mkdir`
- [ ] `OpenInEditor`: `CodeService::open_in_editor`
- [ ] `Outline`: `CodeService::outline`
- [ ] `ReadFile`: `CodeService::read_file`
- [ ] `RefreshFileIndex`: `CodeService::refresh_file_index`
- [ ] `Rename`: `CodeService::rename`
- [ ] `ReplaceInFiles`: `CodeService::replace_in_files`
- [ ] `RevealInFinder`: `CodeService::reveal_in_finder`
- [ ] `Search`: `CodeService::search`
- [ ] `SearchAdvanced`: `CodeService::search_advanced`
- [ ] `SearchFiles`: `CodeService::search_files`
- [ ] `SetFileIndexExclude`: `CodeService::set_file_index_exclude`
- [ ] `Stage`: `CodeService::stage`
- [ ] `StagedFiles`: `CodeService::staged_files`
- [ ] `Unstage`: `CodeService::unstage`
- [ ] `WorkingStatus`: `CodeService::working_status`
- [ ] `WriteFile`: `CodeService::write_file`

### editorsessionservice (3)

- [ ] `ClearEditorSession`: `EditorSessionService::clear_editor_session`
- [ ] `GetEditorSession`: `EditorSessionService::get_editor_session`
- [ ] `UpdateEditorSession`: `EditorSessionService::update_editor_session`

### escalationservice (2)

- [ ] `AnswerEscalation`: `EscalationService::answer_escalation`
- [ ] `GetEscalation`: `EscalationService::get_escalation`

### inboxservice (3)

- [ ] `GetInbox`: `InboxService::get_inbox`
- [ ] `MarkAllInboxRead`: `InboxService::mark_all_inbox_read`
- [ ] `MarkInboxRead`: `InboxService::mark_inbox_read`

### lspservice (5)

- [ ] `CloseLSP`: `LspService::close_lsp`
- [ ] `CloseWorkspaceLSP`: `LspService::close_workspace_lsp`
- [ ] `EnsureLSP`: `LspService::ensure_lsp`
- [ ] `ListLanguageServers`: `LspService::list_language_servers`
- [ ] `WriteLSP`: `LspService::write_lsp`

### maestroservice (3)

- [ ] `CancelMaestroMessage`: `MaestroService::cancel_maestro_message`
- [ ] `GetMaestroChat`: `MaestroService::get_maestro_chat`
- [ ] `SendMaestroMessage`: `MaestroService::send_maestro_message`

### observatoryservice (5)

- [ ] `AddFindingNote`: `ObservatoryService::add_finding_note`
- [ ] `CreateFinding`: `ObservatoryService::create_finding`
- [ ] `DeleteFindingNote`: `ObservatoryService::delete_finding_note`
- [ ] `ListFindings`: `ObservatoryService::list_findings`
- [ ] `SetFindingStatus`: `ObservatoryService::set_finding_status`

### permissionsservice (2)

- [ ] `AnswerPermission`: `PermissionsService::answer_permission`
- [ ] `ListPendingPermissions`: `PermissionsService::list_pending_permissions`

### policyservice (2)

- [ ] `GetPolicy`: `PolicyService::get_policy`
- [ ] `SetPolicy`: `PolicyService::set_policy`

### projectsservice (22)

- [ ] `AddProject`: `ProjectsService::add_project`
- [ ] `AddProjectGroup`: `ProjectsService::add_project_group`
- [ ] `DeleteProject`: `ProjectsService::delete_project`
- [ ] `DeleteProjectGroup`: `ProjectsService::delete_project_group`
- [ ] `DeleteScheduledPrompt`: `ProjectsService::delete_scheduled_prompt`
- [ ] `GetProject`: `ProjectsService::get_project`
- [ ] `ListBranches`: `ProjectsService::list_branches`
- [ ] `ListBranchesForPath`: `ProjectsService::list_branches_for_path`
- [ ] `ListProjectGroups`: `ProjectsService::list_project_groups`
- [ ] `ListProjects`: `ProjectsService::list_projects`
- [ ] `ListScheduledPrompts`: `ProjectsService::list_scheduled_prompts`
- [ ] `PauseScheduledPrompt`: `ProjectsService::pause_scheduled_prompt`
- [ ] `PreviewCopyFiles`: `ProjectsService::preview_copy_files`
- [ ] `PreviewScheduledPrompt`: `ProjectsService::preview_scheduled_prompt`
- [ ] `ReorderProjectGroups`: `ProjectsService::reorder_project_groups`
- [ ] `ReorderProjects`: `ProjectsService::reorder_projects`
- [ ] `RunScheduledPromptNow`: `ProjectsService::run_scheduled_prompt_now`
- [ ] `SaveScheduledPrompt`: `ProjectsService::save_scheduled_prompt`
- [ ] `SetProjectGroup`: `ProjectsService::set_project_group`
- [ ] `SuggestBaseBranch`: `ProjectsService::suggest_base_branch`
- [ ] `UpdateProjectGroup`: `ProjectsService::update_project_group`
- [ ] `UpdateProjectRunConfig`: `ProjectsService::update_project_run_config`

### prsservice (23)

- [ ] `AddPRComment`: `PrsService::add_pr_comment`
- [ ] `CreateWorkspaceFromPR`: `PrsService::create_workspace_from_pr`
- [ ] `DeletePRComment`: `PrsService::delete_pr_comment`
- [ ] `EnsurePRChatHost`: `PrsService::ensure_pr_chat_host`
- [ ] `GetCommitDiff`: `PrsService::get_commit_diff`
- [ ] `GetPR`: `PrsService::get_pr`
- [ ] `GetPRCommits`: `PrsService::get_pr_commits`
- [ ] `GetPRFiles`: `PrsService::get_pr_files`
- [ ] `ListPRFindings`: `PrsService::list_pr_findings`
- [ ] `ListPRs`: `PrsService::list_prs`
- [ ] `MergePR`: `PrsService::merge_pr`
- [ ] `ReadyPR`: `PrsService::ready_pr`
- [ ] `ReplyPRComment`: `PrsService::reply_pr_comment`
- [ ] `SearchPRMentionableUsers`: `PrsService::search_pr_mentionable_users`
- [ ] `SendBackToLoop`: `PrsService::send_back_to_loop`
- [ ] `SetPRDraft`: `PrsService::set_pr_draft`
- [ ] `SetPRThreadResolved`: `PrsService::set_pr_thread_resolved`
- [ ] `SubmitReview`: `PrsService::submit_review`
- [ ] `SyncGitHubPRs`: `PrsService::sync_github_prs`
- [ ] `TogglePRCommentReaction`: `PrsService::toggle_pr_comment_reaction`
- [ ] `UpdatePRComment`: `PrsService::update_pr_comment`
- [ ] `UpdatePRDetails`: `PrsService::update_pr_details`
- [ ] `UpdateWithBase`: `PrsService::update_with_base`

### questionsservice (2)

- [ ] `AnswerQuestion`: `QuestionsService::answer_question`
- [ ] `ListPendingQuestions`: `QuestionsService::list_pending_questions`

### settingsservice (26)

- [ ] `CancelCursorUsageConnect`: `SettingsService::cancel_cursor_usage_connect`
- [ ] `ClearAllData`: `SettingsService::clear_all_data`
- [ ] `CompatibleFlagPreset`: `SettingsService::compatible_flag_preset`
- [ ] `ConnectCursorUsage`: `SettingsService::connect_cursor_usage`
- [ ] `DetectAgent`: `SettingsService::detect_agent`
- [ ] `DisconnectCursorUsage`: `SettingsService::disconnect_cursor_usage`
- [ ] `GetClaudeStatus`: `SettingsService::get_claude_status`
- [ ] `GetCodexStatus`: `SettingsService::get_codex_status`
- [ ] `GetCursorStatus`: `SettingsService::get_cursor_status`
- [ ] `GetInstanceLabel`: `SettingsService::get_instance_label`
- [ ] `GetProviderUsage`: `SettingsService::get_provider_usage`
- [ ] `GetResource`: `SettingsService::get_resource`
- [ ] `GetSettings`: `SettingsService::get_settings`
- [ ] `GitHubLogin`: `SettingsService::github_login`
- [ ] `InstallCursorBridge`: `SettingsService::install_cursor_bridge`
- [ ] `ListAssignedIssues`: `SettingsService::list_assigned_issues`
- [ ] `ListModelCatalog`: `SettingsService::list_model_catalog`
- [ ] `ListMonospaceFonts`: `SettingsService::list_monospace_fonts`
- [ ] `LogsPath`: `SettingsService::logs_path`
- [ ] `OpenLinearInApp`: `SettingsService::open_linear_in_app`
- [ ] `RefreshProviderUsage`: `SettingsService::refresh_provider_usage`
- [ ] `RevealLogs`: `SettingsService::reveal_logs`
- [ ] `SetSettings`: `SettingsService::set_settings`
- [ ] `TestCodexKey`: `SettingsService::test_codex_key`
- [ ] `TestCompatibleProvider`: `SettingsService::test_compatible_provider`
- [ ] `TestCursorKey`: `SettingsService::test_cursor_key`

### shellservice (7)

- [ ] `Flush`: `ShellService::flush`
- [ ] `GetExpandedSchedules`: `ShellService::get_expanded_schedules`
- [ ] `GetIssuesView`: `ShellService::get_issues_view`
- [ ] `GetShellState`: `ShellService::get_shell_state`
- [ ] `SetExpandedSchedules`: `ShellService::set_expanded_schedules`
- [ ] `SetIssuesView`: `ShellService::set_issues_view`
- [ ] `SetShellState`: `ShellService::set_shell_state`

### terminalservice (16)

- [ ] `AckTerminal`: `TerminalService::ack_terminal`
- [ ] `AttachTerminal`: `TerminalService::attach_terminal`
- [ ] `ClearTerminal`: `TerminalService::clear_terminal`
- [ ] `CloseTerminal`: `TerminalService::close_terminal`
- [ ] `DetachTerminal`: `TerminalService::detach_terminal`
- [ ] `InterruptTerminal`: `TerminalService::interrupt_terminal`
- [ ] `ListTerminalDockChrome`: `TerminalService::list_terminal_dock_chrome`
- [ ] `ListTerminals`: `TerminalService::list_terminals`
- [ ] `OpenTerminal`: `TerminalService::open_terminal`
- [ ] `ReorderTerminals`: `TerminalService::reorder_terminals`
- [ ] `ResizeTerminal`: `TerminalService::resize_terminal`
- [ ] `RestoreWorkspaceTerminals`: `TerminalService::restore_workspace_terminals`
- [ ] `SetActiveTerminal`: `TerminalService::set_active_terminal`
- [ ] `SetTerminalDockChrome`: `TerminalService::set_terminal_dock_chrome`
- [ ] `SetTerminalTitle`: `TerminalService::set_terminal_title`
- [ ] `WriteTerminal`: `TerminalService::write_terminal`

### workspacesservice (66)

- [ ] `ApplyProvisionalChatTitles`: `WorkspacesService::apply_provisional_chat_titles`
- [ ] `ApprovePlan`: `WorkspacesService::approve_plan`
- [ ] `AttentionCounts`: `WorkspacesService::attention_counts`
- [ ] `CancelWorkspaceMessage`: `WorkspacesService::cancel_workspace_message`
- [ ] `ClearWorkspaceIssue`: `WorkspacesService::clear_workspace_issue`
- [ ] `CloseWorkspaceChat`: `WorkspacesService::close_workspace_chat`
- [ ] `CreateWorkspace`: `WorkspacesService::create_workspace`
- [ ] `CreateWorkspaceChat`: `WorkspacesService::create_workspace_chat`
- [ ] `CreateWorkspaceWith`: `WorkspacesService::create_workspace_with`
- [ ] `DeleteClosedWorkspaceChat`: `WorkspacesService::delete_closed_workspace_chat`
- [ ] `DeleteWorkspace`: `WorkspacesService::delete_workspace`
- [ ] `EngageLoop`: `WorkspacesService::engage_loop`
- [ ] `EngageLoopWith`: `WorkspacesService::engage_loop_with`
- [ ] `FetchWorkingCopyUpstream`: `WorkspacesService::fetch_working_copy_upstream`
- [ ] `ForkWorkspaceChat`: `WorkspacesService::fork_workspace_chat`
- [ ] `ForkWorkspaceChatToWorkspace`: `WorkspacesService::fork_workspace_chat_to_workspace`
- [ ] `GetWorkspace`: `WorkspacesService::get_workspace`
- [ ] `GetWorkspaceFeed`: `WorkspacesService::get_workspace_feed`
- [ ] `LastAssistantText`: `WorkspacesService::last_assistant_text`
- [ ] `ListClosedWorkspaceChats`: `WorkspacesService::list_closed_workspace_chats`
- [ ] `ListOpenInTargets`: `WorkspacesService::list_open_in_targets`
- [ ] `ListWorkspaceChats`: `WorkspacesService::list_workspace_chats`
- [ ] `ListWorkspaces`: `WorkspacesService::list_workspaces`
- [ ] `MarkWorkspaceChatSeen`: `WorkspacesService::mark_workspace_chat_seen`
- [ ] `MergeWorkspace`: `WorkspacesService::merge_workspace`
- [ ] `OpenMainWorkspace`: `WorkspacesService::open_main_workspace`
- [ ] `OpenPath`: `WorkspacesService::open_path`
- [ ] `OpenWorkspaceIn`: `WorkspacesService::open_workspace_in`
- [ ] `OpenWorkspacePR`: `WorkspacesService::open_workspace_pr`
- [ ] `PauseLoop`: `WorkspacesService::pause_loop`
- [ ] `PullWorkspacePR`: `WorkspacesService::pull_workspace_pr`
- [ ] `PushWorkingCopy`: `WorkspacesService::push_working_copy`
- [ ] `RememberChatLoopPolicy`: `WorkspacesService::remember_chat_loop_policy`
- [ ] `RememberWorkspaceChatPromptGroup`: `WorkspacesService::remember_workspace_chat_prompt_group`
- [ ] `RememberWorkspaceChatSelection`: `WorkspacesService::remember_workspace_chat_selection`
- [ ] `RememberWorkspaceChatSimplify`: `WorkspacesService::remember_workspace_chat_simplify`
- [ ] `RemoveWorkspace`: `WorkspacesService::remove_workspace`
- [ ] `ReopenWorkspaceChat`: `WorkspacesService::reopen_workspace_chat`
- [ ] `ReorderWorkspaceChats`: `WorkspacesService::reorder_workspace_chats`
- [ ] `ResetWorkspaceChat`: `WorkspacesService::reset_workspace_chat`
- [ ] `RestartLoop`: `WorkspacesService::restart_loop`
- [ ] `ResumeLoop`: `WorkspacesService::resume_loop`
- [ ] `RetryLoop`: `WorkspacesService::retry_loop`
- [ ] `RetryWorkspaceSetup`: `WorkspacesService::retry_workspace_setup`
- [ ] `SendWorkspaceMessage`: `WorkspacesService::send_workspace_message`
- [ ] `SetChatUIOccluded`: `WorkspacesService::set_chat_ui_occluded`
- [ ] `SetWorkspaceBaseBranch`: `WorkspacesService::set_workspace_base_branch`
- [ ] `SetWorkspaceChatFocus`: `WorkspacesService::set_workspace_chat_focus`
- [ ] `SetWorkspaceIssue`: `WorkspacesService::set_workspace_issue`
- [ ] `SetWorkspaceLoopPolicy`: `WorkspacesService::set_workspace_loop_policy`
- [ ] `StartWorkspace`: `WorkspacesService::start_workspace`
- [ ] `StopLoop`: `WorkspacesService::stop_loop`
- [ ] `SubmitPlan`: `WorkspacesService::submit_plan`
- [ ] `UpdatePlan`: `WorkspacesService::update_plan`
- [ ] `UpdatePlanStep`: `WorkspacesService::update_plan_step`
- [ ] `UpdateWorkingCopyFromRemote`: `WorkspacesService::update_working_copy_from_remote`
- [ ] `UpdateWorkspaceWithBase`: `WorkspacesService::update_workspace_with_base`
- [ ] `WatchWorkspace`: `WorkspacesService::watch_workspace`
- [ ] `WorkspaceChatItems`: `WorkspacesService::workspace_chat_items`
- [ ] `WorkspaceChatModels`: `WorkspacesService::workspace_chat_models`
- [ ] `WorkspaceChatSelection`: `WorkspacesService::workspace_chat_selection`
- [ ] `WorkspaceChatTurnDiff`: `WorkspacesService::workspace_chat_turn_diff`
- [ ] `WorkspaceDiscardRisk`: `WorkspacesService::workspace_discard_risk`
- [ ] `WorkspaceFacts`: `WorkspacesService::workspace_facts`
- [ ] `WorkspaceLandState`: `WorkspacesService::workspace_land_state`
- [ ] `WorkspaceSessions`: `WorkspacesService::workspace_sessions`

## App events (backend to UI)

Source: `Events.On(...)` in `frontend/src` (and the few the UI sent with `Events.Emit`, marked "UI to backend"). Rust home: `recur_services::AppEvent`, published on `Services::subscribe()`; `name()`, `payload()` and `from_wire()` give the exact Wails event.

- [ ] `agent.step`: `AppEvent::AgentStep(AgentStepEvent)`
- [ ] `agent.updated`: `AppEvent::AgentUpdated(AgentUpdated)`
- [ ] `app.focused`: `AppEvent::AppFocused(AppFocused)`
- [ ] `app.open`: `AppEvent::AppOpen(OpenPathResult)`
- [ ] `app.ready`: `AppEvent::AppReady (UI to backend)`
- [ ] `artifact.created`: `AppEvent::ArtifactCreated(ArtifactEvent)`
- [ ] `artifact.presented`: `AppEvent::ArtifactPresented(ArtifactEvent)`
- [ ] `attention.updated`: `AppEvent::AttentionUpdated(AttentionUpdated)`
- [ ] `briefing.updated`: `AppEvent::BriefingUpdated(BriefingUpdated)`
- [ ] `chat.mode_changed`: `AppEvent::ChatModeChanged(ChatModeChanged)`
- [ ] `cursor.install`: `AppEvent::CursorInstall(CursorInstall)`
- [ ] `escalation.raised`: `AppEvent::EscalationRaised(EscalationRaised)`
- [ ] `escalation.resolved`: `AppEvent::EscalationResolved(EscalationResolved)`
- [ ] `files.dropped`: `AppEvent::FilesDropped(FilesDropped)`
- [ ] `findings.updated`: `AppEvent::FindingsUpdated(WorkspaceRef)`
- [ ] `github.branch_rules`: `AppEvent::GithubBranchRules(GithubBranchRules)`
- [ ] `inbox.updated`: `AppEvent::InboxUpdated(InboxUpdated)`
- [ ] `lsp.exit`: `AppEvent::LspExit(LspExit)`
- [ ] `lsp.message`: `AppEvent::LspMessage(LspMessage)`
- [ ] `menu.action`: `AppEvent::MenuAction(String)`
- [ ] `menu.state`: `AppEvent::MenuState(MenuState) (UI to backend)`
- [ ] `menu.themeState`: `AppEvent::MenuThemeState(MenuThemeState) (UI to backend)`
- [ ] `menu.themes`: `AppEvent::MenuThemes(MenuThemes) (UI to backend)`
- [ ] `notify`: `AppEvent::Notify(Notify)` (desktop shows the OS notification; no UI listener)
- [ ] `notification.open`: `AppEvent::NotificationOpen(String)`
- [ ] `permission.requested`: `AppEvent::PermissionRequested(PermissionRequest)`
- [ ] `permission.resolved`: `AppEvent::PermissionResolved(PermissionResolved)`
- [ ] `pr.comment`: `AppEvent::PrComment(Box<PrCommentEvent>)`
- [ ] `pr.removed`: `AppEvent::PrRemoved(PrRef)`
- [ ] `pr.updated`: `AppEvent::PrUpdated(Box<PrUpdated>)`
- [ ] `provider.usage`: `AppEvent::ProviderUsage(recur_usage::Snapshot)`
- [ ] `prs.synced`: `AppEvent::PrsSynced(PrsSynced)`
- [ ] `question.answered`: `AppEvent::QuestionAnswered(QuestionAnswered)`
- [ ] `question.asked`: `AppEvent::QuestionAsked(QuestionAsked)`
- [ ] `resource.updated`: `AppEvent::ResourceUpdated(Resource)`
- [ ] `session.compact`: `AppEvent::SessionCompact(SessionCompact)`
- [ ] `session.context_usage`: `AppEvent::SessionContextUsage(SessionContextUsage)`
- [ ] `session.delta`: `AppEvent::SessionDelta(SessionDelta)`
- [ ] `session.fallback`: `AppEvent::SessionFallback(SessionFallback)`
- [ ] `session.notice`: `AppEvent::SessionNotice(SessionText)`
- [ ] `session.step`: `AppEvent::SessionStep(SessionStep)`
- [ ] `session.thinking`: `AppEvent::SessionThinking(SessionThinking)`
- [ ] `session.tool_input`: `AppEvent::SessionToolInput(SessionToolInput)`
- [ ] `session.tool_result`: `AppEvent::SessionToolResult(SessionToolResult)`
- [ ] `session.turn_end`: `AppEvent::SessionTurnEnd(SessionTurnEnd)`
- [ ] `session.user`: `AppEvent::SessionUser(SessionText)`
- [ ] `system.theme`: `AppEvent::SystemTheme(bool)`
- [x] `term.busy`: `Event::Busy { id, workspace_id }` when the shell loses the foreground process group (idle watcher, 25ms to 1s backoff). Services: `AppEvent::Term(recur_term::Event)`.
- [x] `term.data`: `Event::Data { id, data }`, raw bytes coalesced 8ms / 64KiB, flow-controlled by `ack` (256KiB high water), routed to the pane by `route_event`. Services: `AppEvent::Term(recur_term::Event)`.
- [x] `term.exit`: `Event::Exit { id, workspace_id, exit_code }` once after the reader drains; the pane emits `TerminalEvent::Exit`. Services: `AppEvent::Term(recur_term::Event)`.
- [x] `term.idle`: `Event::Idle { id, workspace_id }` when the shell regains the foreground. Services: `AppEvent::Term(recur_term::Event)`.
- [ ] `updater.check`: `AppEvent::UpdaterCheck (UI to backend)`
- [ ] `wails:updater:error`: `AppEvent::Updater(UpdaterEvent::Error)`
- [ ] `wails:updater:no-update`: `AppEvent::Updater(UpdaterEvent::NoUpdate)`
- [ ] `wails:updater:update-available`: `AppEvent::Updater(UpdaterEvent::UpdateAvailable)`
- [ ] `wails:updater:update-ready`: `AppEvent::Updater(UpdaterEvent::UpdateReady)`
- [ ] `wails:updater:user:restart`: `AppEvent::UpdaterUserRestart (UI to backend)`
- [ ] `window.titlebarDblclick`: `AppEvent::WindowTitlebarDblclick (UI to backend)`
- [ ] `workspace.chat_tab`: `AppEvent::WorkspaceChatTab(WorkspaceChatTabEvent)`
- [ ] `workspace.deleted`: `AppEvent::WorkspaceDeleted(WorkspaceRef)`
- [ ] `workspace.event`: `AppEvent::WorkspaceEvent(WorkspaceEvent)`
- [ ] `workspace.file_index`: `AppEvent::WorkspaceFileIndex(WorkspaceFileIndex)`
- [ ] `workspace.fs_changed`: `AppEvent::WorkspaceFsChanged(WorkspaceFsChanged)`
- [ ] `workspace.removed`: `AppEvent::WorkspaceRemoved(WorkspaceRef)`
- [ ] `workspace.updated`: `AppEvent::WorkspaceUpdated(Box<WorkspaceUpdated>)`

## Views and components

Source: `frontend/src`. Rust home: `recur-app` (views), `recur-ui` (shared widgets), `recur-editor`, `recur-terminal-view`. Each must match the web UI pixel for pixel (see `docs/ui-parity.md`).

### views

- [ ] `Briefing.svelte` (247 lines)
- [ ] `NewProject.svelte` (239 lines)
- [ ] `NewTask.svelte` (642 lines)
- [ ] `PRDetail.svelte` (2329 lines)
- [ ] `PRList.svelte` (226 lines)
- [ ] `Project.svelte` (1302 lines)
- [ ] `Settings.svelte` (3571 lines)
- [ ] `Welcome.svelte` (300 lines)
- [ ] `WorkspaceDetail.svelte` (190 lines)
- [ ] `WorkspaceView.svelte` (3652 lines)

### components

- [x] `AgentChat.harness.svelte` (12 lines) → the SessionCard renderer in `recur-view-chat`'s `spec::agent_chat` tests
- [x] `AgentChat.svelte` (6396 lines) → `recur_view_chat::chat::AgentChat` (scenes `chat/*`)
  - [x] Transcript: blocks and step groups (thinking, compaction, fallback, narration, Subagent notices folded), every tool card body, user bubbles with inline chips and images, queued bubbles, error card with Retry, stopped row, turn footer (copy, fork menu, turn diff, reset, presented workspace brief, findings digest with status, notes and compose), facts (PR completion, inline HTML, loop wrap, fan-out and plan cards), specialist sessions through a host `SessionCardRenderer`, inline permission / question / escalation cards
  - [x] Scrolling: bottom pin, scroll restore per `resetKey` (tab switch, unmount and remount), windowed transcripts over 48 blocks with stubs and measured heights, edge fades, turn clock
  - [x] Composer: a native editor over `ComposerDoc` (chips as U+FFFC atoms in a custom inline flow with `vertical-align: middle`), IME, undo bursts, `@` file / folder / terminal / artifact mentions with the 200ms search, `/` prompts and skills, prompt chips with arg slots and completions, paste and Finder / explorer drops, chip clicks, Shift+Enter, ⏎ / ⌘⏎ steer and queue, ⌘. mode cycle, ⌘⇧⌫ stop, Escape denies the oldest permission
  - [x] Props, hooks and events as the Svelte callbacks (`ChatProps`, `ChatHooks`, `ChatEvent`); the embedding API is documented in the crate root
- [x] `AppChat.svelte` (374 lines) → `recur_view_chat::app_chat::AppChat`: tabs (list, new, close, select, unseen and busy chips, lit underline while focused), the ask chips, model pickers with the remembered selection, live session events for the `""` workspace with turn-end resync, focus reporting, the active tab in local storage; the host owns visibility and handles `AppChatEvent::Hide` (scenes `chat/app`, `chat/app-paper`; `spec::dock` tests)
- [x] `ArmedButton.svelte` (51 lines) → `recur_ui::widgets::armed_button` (scene `armed-button/default`)
  - [x] Arm on click, confirm on the next pointer-up (handled in capture so it precedes GPUI's click), swallow the click echo, timeout reset (3s default), disabled resets
  - [x] `.tbtn.danger` / `.armed` looks at base and titlebar sizes, icon slot, title tooltip, `on_arm`
- [ ] `ArtifactsPanel.svelte` (320 lines)
- [x] `AttachmentLightbox.svelte` (155 lines) → `recur_content::AttachmentLightbox` (scene `lightbox/image`)
  - [x] Fit inside the padded viewport at WebKit's fractional geometry, actual-size toggle when scaled, Escape / ⌘C, copy and close buttons with titles, backdrop click closes, context menu, deferred overlay at the lightbox's z-order; `fit` test ported
- [x] `CodeEditor.svelte` (1405 lines) → `recur_editor::view::EditorView` (`EditorKind::Code`); scenes `editor/code`, `editor/paper`, `editor/find`, `editor/vim`
  - [x] Base extensions in order: history, indent on input, Enter between pairs, close brackets, completion (bundle snippets + buffer words), selection-match highlights, pointer selection, find panel, the code keymap, Mod-click / F12 definitions
  - [x] Pref compartments (theme, chrome, wrap, indent, features, keymap, diff gutter, read-only) through `set_prefs` / `set_theme`; app chrome tokens follow `recur_ui::Theme`
  - [x] Gutters (findings, lint severity, line numbers, fold, SCM bars), TextMate language and theme, LSP attach, view state restore, dirty / saved baseline, save, events (`EditorEvent`)
  - [x] vim (`@replit/codemirror-vim` 6.3.0) and emacs (`@replit/codemirror-emacs` 6.1.0) after the editor keymap, as the keymap compartment orders them
  - [ ] Tab parking across switches (`caches/cm-session-tabs.ts`) is the host's: keep the `EditorView` entity
- [ ] `CommandPalette.svelte` (269 lines)
- [x] `CommandPrompts.svelte` (142 lines) → `recur_view_chat::palettes::CommandPrompts` (text, password and choice fields; Enter resolves, Escape and the scrim cancel; `spec::shell` test). The Observatory's bundle commands own it
- [ ] `CommentReactions.svelte` (114 lines)
- [ ] `CommitRow.svelte` (80 lines)
- [ ] `CommitsView.svelte` (80 lines)
- [ ] `CompatibleProviderCard.svelte` (468 lines)
- [ ] `CompatibleProviderModal.svelte` (215 lines)
- [x] `ContextUsageMeter.svelte` (278 lines) → `recur_view_chat::meter::ContextMeter` (ring, popover, compacting pulse; values test)
- [x] `DecodeText.svelte` (107 lines) → `recur_ui::widgets::decode_text`
  - [x] Left-to-right scramble at 28ms ticks with 2-tick holds, spaces kept, `.decoding` at .85 opacity, `source_key` snaps on identity change, `animate(false)`, reduced motion snaps
  - [x] `decode_frame` ported with its cases
- [ ] `DiffFile.svelte` (898 lines)
- [x] `EmojiSuggest.svelte` (90 lines) → `recur_view_chat::suggest` (`attach_emoji`, `SuggestOverlay` in the `EmojiSuggest` slot; `spec::shell` test)
- [x] `FanOutCard.svelte` (234 lines) → `recur_view_chat::cards::fanout` (rows from the live workspace store, flash, child transcripts with specialist cards; scene `chat/fanout`)
- [x] `FilePreview.svelte` (364 lines) → `recur_content::FilePreview` (scenes `file-preview/markdown`, `file-preview/svg`)
  - [x] Markdown (`.md-doc`), SVG (`<img>` raster with its ring), HTML (`HtmlPreview`: an embedded WKWebView over the srcdoc shell and CSP, find through its page script, no sibling-file loading) and Mermaid surfaces, empty states, scroll restore and save through the preview scroll store, parked shells
  - [x] Find bar (count, previous / next, case / regexp / word toggles), ⌘F / Escape / ⌘G / F3 while active, none for SVG, recollect when the rendered text changes, highlights only on the active preview; `FilePreview.test.ts` ported (`tests/file_preview.rs`)
- [ ] `FilesChangedView.svelte` (712 lines)
- [ ] `FindingsPanel.svelte` (506 lines)
- [x] `Icon.svelte` (51 lines) → `recur_ui::icon` (`Icon`, `icon`, `svg_markup`)
  - [x] Every `lib/icons.ts` name as an SVG asset (`IconName`), same viewBoxes and stroke widths, `stroke_width` override, rotation, per-pixel CSS mask, polychrome markup with `currentColor`
  - [x] Rasterized with CoreGraphics on macOS at the exact CSS size from a snapped origin (WebKit's SVG painting); scenes `icons/app`, `project-pic/default`
- [x] `IconPicker.svelte` (237 lines) → `recur_ui::widgets::IconPicker` (scenes `icon-picker/closed`, `icon-picker/open`)
  - [x] `.ico-pick.sel.sm` trigger (hover, open, `:focus-visible` ring and outline, disabled fade), searchable 8-column grid with the themed scrollbar, current icon first, 80-hit search, arrow/Home/End/Enter/Escape keys, hover gate, outside-press close, `IconPicked` event
  - [x] `shown_icons` ported with its cases
- [x] `ImageContextMenu.svelte` (84 lines) → `recur_content::image_menu` (position clamp, copy image as PNG, open externally)
- [ ] `InboxTitlebar.svelte` (24 lines)
- [ ] `Issues.svelte` (512 lines)
- [x] `Kbd.svelte` (13 lines) → `recur_ui::kbd` (`kbd`, `shortcut_parts`); scene `kbd/default` with `.pk`, `.pal-foot .k` and `.esc` keycaps
- [ ] `KeybindingMenu.svelte` (180 lines)
- [ ] `LineNoteThread.svelte` (375 lines)
- [x] `LoopPolicyPickers.svelte` (93 lines) → `recur_view_chat::pickers::LoopPolicyPickers` (scene `chat/mode-loop`)
- [x] `LucideIcon.svelte` (26 lines) → `recur_ui::lucide_icon` (aliases, unknown names draw nothing); scene `icons/lucide`
- [x] `MarkdownHtml.svelte` (87 lines) → `recur_content::MarkdownView` (scenes `markdown/chat`, `markdown/paper`, `mermaid/sequence`)
  - [x] Sanitized HTML laid out natively in `.chat-canvas` / `.md-doc`: headings, lists with WebKit's markers, task checkboxes, tables, fences with codehl and copy buttons, blockquotes, links (hover, open externally), images (data and `/gh-image` through `ImageLoader`, lightbox), Mermaid hydration, find highlights through the text mirror
- [x] `MergeEditor.svelte` (703 lines) → `EditorView` with `EditorKind::Merge` (`merge.rs`: unified merge view, collapsed regions, full-line marks, change gutter, revert chip and `revertHunk` with Mod-Alt-z)
- [x] `MermaidPreview.svelte` (329 lines) → `recur_content::MermaidPreview` (scene `mermaid/flowchart`)
  - [x] Fit, zoom (toolbar, wheel, double-click), pan, camera persisted per path through the preview scroll store, toolbar, pending / error / empty states, find over the labels with highlights and pan-to-match
- [ ] `MiniScriptEditor.svelte` (295 lines)
- [x] `ModelFlagsSelect.svelte` (305 lines) → `recur_view_chat::pickers::ModelFlagsSelect` (scenes `chat/mode-*`)
- [x] `ModelPickers.svelte` (79 lines) → `recur_view_chat::pickers::ModelPickers` (scenes `chat/mode-*`, `chat/pickers`)
- [ ] `Observatory.svelte` (8371 lines)
- [ ] `OpenInPalette.svelte` (235 lines)
- [ ] `PRListTitlebar.svelte` (15 lines)
- [ ] `PRPeopleStack.svelte` (64 lines)
- [ ] `PRReviewModal.svelte` (180 lines)
- [ ] `PRTitlebarActions.svelte` (520 lines)
- [x] `PdfPreview.svelte` (289 lines) → `recur_content::PdfPreview` on pdf.js 6.3.289 in a hidden WebKit page (scene `pdf/page`; decision in `ARCHITECTURE.md`)
  - [x] Fit-to-width on load, zoom (toolbar, ⌘+ / ⌘-) within 0.1-8, page field (Enter / blur commit) and previous / next, page follows the scroll, pages render while they intersect the viewport and are discarded when they leave it, loading and error states
- [x] `PermissionCard.svelte` (116 lines) → `recur_view_chat::cards::permission` (inline and floating; scene `chat/permission`)
- [x] `PermissionPrompts.svelte` (63 lines) → `recur_view_chat::prompts::PermissionPrompts` in the `PermissionPrompts` slot (oldest request no focused chat shows, Escape denies; `spec::shell` test)
- [x] `PlanCard.svelte` (115 lines) → `recur_view_chat::cards::plan` (scene `chat/plan`)
- [ ] `PlanPanel.svelte` (609 lines)
- [x] `ProjectPic.svelte` (49 lines) → `recur_ui::widgets::project_pic` (`.nt-pic` and palette boxes, letter, lucide and image pictures, readable ink on any color); scene `project-pic/default`
- [x] `ProviderIcon.svelte` (52 lines) → `recur_ui::provider_icon` (provider slug mapping, polychrome marks); scene `icons/provider`
- [x] `ProviderUsageMeter.svelte` (453 lines) → `recur_view_sidebar::usage` (`UsageMeter`: ring chips with tweens, hover open / close delays, click toggle with prefetch or refresh, popover with every window, resets, Cursor model buckets and the Connect Cursor link; `lib/providerUsage.ts` rules tested)
- [x] `QuestionCard.svelte` (239 lines) → `recur_view_chat::cards::question` (scene `chat/question`)
- [x] `QuestionPrompts.svelte` (67 lines) → `recur_view_chat::prompts::QuestionPrompts` (one ask toast per off-tab question; `spec::shell` test)
- [ ] `QuickOpen.svelte` (273 lines)
- [x] `ResizeHandle.svelte` (126 lines) → `recur_ui::widgets::resize_handle` (scenes `resize-handle/hover`, `resize-handle/focus`)
  - [x] `.vsplit` / `.hsplit` (`_terminal.scss`) at either edge, hover/focus/drag wash, painted over the panel border (z-index 8), title tooltip
  - [x] Pointer drag with live and commit callbacks, layout ceiling, arrow keys (8px, 24px with Shift); `drag_size` and `key_step` with cases. Persisting widths (`panelWidths`) is the caller's
- [ ] `ReviewFileList.svelte` (280 lines)
- [ ] `ReviewSurface.svelte` (229 lines)
- [x] `Ring.svelte` (30 lines) → `recur_ui::widgets::ring` (progress arc and iteration count); scene `ring/default`
- [ ] `RunCommandPalette.svelte` (250 lines)
- [ ] `RunPalette.svelte` (171 lines)
- [ ] `ScheduleEditor.svelte` (334 lines)
- [ ] `ScheduledPrompts.svelte` (792 lines)
- [ ] `SchedulesTitlebar.svelte` (20 lines)
- [x] `Select.svelte` (466 lines) → `recur_ui::widgets::Select` (scenes `select/closed`, `select/closed-paper`, `select/open`, `select/searchable`, `select/multiple`, `select/focus`)
  - [x] md, sm and ghost triggers, provider and app icons with tints, placeholder, multiple values, disabled fade, `TriggerStyle` for contexts like `.nt-pickers`
  - [x] Cascade details: hover fill on an open trigger keeps its amber border, `:focus-visible` rounds to 4px with the 2px outline, sm uses `chevrons-up-down` without rotation, md and ghost rotate `chevron-down`
  - [x] Menu: placement from the trigger's border box (`menu-space`), up/down, min/max widths per variant, groups with provider headings, hints, checkmarks, themed scrollbar, search header (placeholder drawn by recur-ui text), fuzzy filter, keyboard navigation, hover gate, outside-press close, `SelectEvent`
- [x] `SessionCard.svelte` (88 lines) → `recur_view_chat::cards::session` (`spec::agent_chat` tests)
- [x] `SettingsGroup.svelte` (65 lines) → `recur_ui::widgets::settings_group` (scenes `settings-group/default`, `settings-group/paper`)
  - [x] Title and blurb (rich spans with `blurb_code`), override toggle collapsing the body, disabled override, `.set-subsec-nested` with the real cascade (its own background and radius lose to `.set-subsec`)
- [x] `SettingsGroups.svelte` (15 lines) → `recur_ui::widgets::settings_groups`
- [ ] `ShortcutsHelp.svelte` (128 lines)
- [x] `Sidebar.svelte` (1848 lines) → `recur-view-sidebar` (scenes `shell/*`; `crates/app/tests/shell.rs`)
  - [x] Nav (Inbox / Issues / Pull requests / Schedules with chips, PR-host running dot and unseen chip, New schedule), Projects header (show hidden, add group, add project), footer (Settings, Get started, usage meter, project / running / needs-you stats), rail resize
  - [x] Tree: groups, projects and rows with collapse seeded from `open` and persisted, Scheduled subgroup, child connectors and step badges, every status / PR icon, unread chips, working copy row with branch and diff, hover actions (View PR, two-click Remove with discard risk), project actions, sticky group and project headers with hierarchical under-fades, bottom fade, overlay scrollbar
  - [x] Drag reorder of projects (within and across groups, transient group opens) and groups, with placeholder, FLIP, settle and persistence; right-click Hide / Move to group menu; group add, rename and delete
  - [x] Keyboard-cycle scan (peek, commit, persist), reveal scheduled rows, scroll the selected row into view below sticky headers
  - [x] Collapsed-rail flyout: edge hover / tap, show and hide delays, hold while a menu, arm or drag is active, slide and shadow, focus parked in the content column when it closes
- [x] `StopPalette.svelte` (164 lines) → `recur_view_chat::palettes::StopPalette` in the `StopPalette` slot (filter, arrows, 1–9, Enter, Escape; `spec::shell` test)
- [x] `StreamingMarkdown.svelte` (85 lines) → `recur_content::StreamingMarkdown` (scene `markdown/streaming`; `StreamingMarkdown.test.ts` ported)
- [ ] `SyntaxPalette.svelte` (317 lines)
- [x] `TabChip.svelte` (293 lines) → `recur_ui::widgets::tab_chip` (scenes `tab-strip/default`, `tab-strip/paper`, `tab-strip/hover`)
  - [x] Active, lit, dirty, unseen, badge, busy wash (per-pixel mask over icon and label), muted, decoded label, app and file icons, close button with tooltip, reorder cursors, floating chip with eased lift shadow
  - [x] Click, close, key, press and context-menu callbacks; click guard after drags
- [x] `TabStrip.svelte` (290 lines) → `recur_ui::widgets::tab_strip` (scenes above and `tab-strip/overflow`)
  - [x] Chrome strip, leading and trailing slots, `.tab-strip-act` actions, active reveal chase, keep-alive scroll restore, reveal nonce, user pinning
  - [x] `scrollbar-width: thin` as WebKit's macOS overlay scroller (no layout space, shown after wheel scrolling)
- [ ] `TerminalDock.svelte` (856 lines)
- [~] `TerminalPane.svelte` (581 lines): `recur_terminal_view::TerminalView` + `TerminalElement`
  - [x] Attach on mount, paint replay, queue live output until the replay lands, ack in 32KiB batches (`tests/view.rs`)
  - [x] Fit: `termFitSize` over the padded content box (6/0/6/8), grid pinned to the bottom, 100ms debounced `resize` (`font.rs`, `tests/view.rs`)
  - [x] xterm options: font stack with ligatures off, size, line height, cell metrics from WebGL `_updateDimensions`, 10000 scrollback, `macOptionIsMeta`, `macOptionClickForcesSelection`, `rightClickSelectsWord` on mac, bar cursor with blink, `minimumContrastRatio` 1 (`font.rs`, `screen/render.rs`)
  - [x] WebGL colour pipeline: bold-as-bright, dim, inverse, hidden, selection at 30% with inactive variant, block cursor override, underline styles (single/double/curly/dotted/dashed) and colours, strikethrough, custom box/block/powerline glyphs (`screen/render.rs`, `custom_glyphs.rs`)
  - [x] WebGL glyph rendering on macOS: `TextureAtlas` rasters through CoreText into a device canvas (synthetic oblique, decorations, bounding-box crop, texel round trips), rows composited with the WebGL blend and WebKit's premultiplied page compositing, cached as content-keyed tiles; overhang clipped at background changes (`atlas.rs`, `tiles.rs`, `ui-parity.md`)
  - [ ] Custom glyphs are painted procedurally over the tiles rather than through the atlas canvas, and no parity scene covers them yet
  - [x] Cursor styles (DECSCUSR), blink manager (600ms, idle stop at 5 min, pause on blur) (`blink.rs`)
  - [x] Keyboard: xterm `Keyboard.ts` evaluation, custom key handler (⌘ chords to the app, Ctrl+` to the dock, third-level chars as text), app cursor keys, PageUp/PageDown scroll (`input.rs`, `tests/view.rs`)
  - [x] IME: marked text drawn at the cursor, commit writes once, candidate window at the cursor cell (`tests/view.rs`)
  - [x] Copy (⌘C with selection), paste (⌘V, bracketed paste, `\n` to `\r`), Linux primary selection and middle-click paste
  - [x] Mouse: selection (click, double word, triple line, shift extend, alt column, drag autoscroll), alt-click moves the prompt cursor, mouse reporting X10/1000/1002/1003 in default/SGR/SGR-pixels, wheel reporting and alt-screen arrow keys (`tests/view.rs`, `screen/tests.rs`)
  - [x] Scrollbar: VS Code 14px slider, 50px wheel units, trackpad accumulation, track click and drag, 500ms hide with fades (`scrollbar.rs`, `tests/view.rs`)
  - [x] Search addon: regex/case/whole word, xterm's highlight limit, active match, result counts, incremental refresh (`screen/search.rs`, `tests/view.rs`)
  - [x] Find bar UI (`TerminalFindBar`): toggles, count label, Enter/Shift+Enter, Escape, ⌥⌘C/R/W; text, icons, tooltips and focus rings through recur-ui
  - [x] Web links addon: URL regex, hover underline and pointer, ⌘/click opens http(s) in the browser; OSC 8 hyperlinks underline but do not open (the webview never showed xterm's confirm) (`screen/links.rs`)
  - [x] Focus: dock focus routing (`termDockOwnsFocus`), DECSET 1004 reports, window activation; hidden panes take no pointer events (`tests/view.rs`)
  - [x] Theme and font changes repaint without remount; frosted (transparent) cells when maximized on dark palettes (`theme.rs`, `tests/view.rs`)
  - [x] Explorer drops type quoted paths; title and exit events reach the dock; clear wipes screen and replay; dispose detaches (`tests/view.rs`)
  - [x] Query replies answered in the emulator (DA1/DA2, XTVERSION, DSR/CPR, DECRQM, DECRQSS, OSC 4/10/11/12, CSI t) (`screen/scanner.rs`)
  - [x] SGR 53 / 55 overline: a spare alacritty cell flag set on the cursor template through an in-band carrier the wrapped handler applies in stream order (synchronized output included); survives scrollback, reflow (overlined blanks keep a marker foreground), the alternate screen, DECSC/DECRC; erase, RIS and SGR 0 drop it. Drawn as the atlas does: the top of the character box in the glyph colour, after the underline, before the strikethrough (`screen/overline.rs`, `screen/tests.rs`, scene `terminal/decorations`)
  - [x] Pixel parity against the web pane: scenes `terminal/prompt`, `paper`, `selection`, `search`, `scrollbar`, `decorations`, `solarized-light`, `solarized-dark` (`ui-parity.md`)
  - [x] Replay lands on the fitted grid (TerminalPane refits before attaching) (`tests/view.rs`)
  - [ ] SGR 5 text blink: xterm.js 6.1's WebGL renderer blinks text (`TextBlinkStateManager`); not ported
  - [ ] SGR 21: xterm.js reads it as double underline, vte as "not bold" (and 221 / 222, kitty's not bold / not faint, are dropped)
- [x] `TitleTooltip.svelte` (309 lines) → `recur_ui::tooltip` (`titled`, `TitleTooltip`; scenes `tooltip/below`, `tooltip/above`, `tooltip/paper`)
  - [x] Hover and keyboard-focus candidates, show delay and re-show window, cursor anchoring on wide targets, below/above placement with viewport clamping, dismissal on press, scroll, Escape, window exit and blur, dismissed-text memory
  - [x] `.ui-tip` card with themed shadow and ring, 90ms entrance, wrapping body, placed at its exact fractional position
- [x] `Toasts.svelte` (55 lines) → `recur_ui::widgets::{Toasts, ToastStore, push_toast}` (scenes `toasts/default`, `toasts/paper`)
  - [x] Store from `lib/stores.ts`: kinds, default TTLs (3.5s, 8s errors, 30s asks, 0 sticky), replace keys, `on_dismiss` unless the action ran, editor-theme toast, ask toasts; cases ported
  - [x] Plain toasts (dot, wrapping text, action, close) and ask toasts (165deg gradient, accent rail and draining TTL bar clipped to the rounded card, kicker, where line, two-line question, CTA), entrance fade
- [x] `UserMentionSuggest.svelte` (105 lines) → `recur_view_chat::suggest` (`attach_user_mentions`, the `UserMentionSuggest` slot; `spec::shell` tests)
- [ ] `WorkspaceBaseSelect.svelte` (93 lines)
- [~] `WorkspaceBriefCard.svelte` (134 lines) → `recur_view_chat::cards::brief` (the presented-workspace card on turn footers); no scene or test yet
- [ ] `WorkspaceIssueButton.svelte` (50 lines)

### App shell

- [~] `App.svelte` (1289 lines) → `recur-app` (`shell`, `titlebar`, `land`, `keys`, `actions`, `menu`, `new_workspace`, `bundles`) over `recur-view-core`
  - [x] Routing: one view entry per route from `ViewRegistry`, `#v-detail` kept laid out, unported routes empty; boot (`loadInitial`, shell-state restore, Welcome when there are no projects, `app.ready`), shell-state persistence
  - [x] Titlebar: traffic-light clear, rail toggle, back buttons, every breadcrumb (decoded workspace names, branch chip), inbox and schedule counts, base-branch select, Mark all read, Open In / Copy path, run and stop splits, land chrome (update, merge, push, pull, force push, PR open/create/view, issue link and picker, require-PR rules, land cache), Remove with discard risk, the `dev` instance chip, view titlebar parts
  - [x] Global keys (`onKey`): ⌘K, ⌘⇧P, ⌘⇧/, ⌘N, ⇧⌘N, ⌥⌘O, ⌘O, ⌘B, ⌘,, ⌥⌘↑↓, ⌥⌘[ ], ⌘R / ⌘⇧R, ⌃` / ⌃⇧`, Escape-to-back (skipped in text entry and under overlays, closes the rail flyout first)
  - [x] Events: `app.ready`, `app.open`, `app.focused`, `system.theme` (window appearance), `menu.action`, `notification.open`, `files.dropped`, updater toasts; overlays host in App.svelte's order, toasts, title tooltip
  - [ ] The issue split's chevron half keeps its left corners square (`.issue-split .sel-btn`): needs a per-corner radius in `recur_ui::widgets::TriggerStyle`

### Frontend logic modules

Non-visual TypeScript that holds real behavior (stores, keymaps, composer, editor integrations). Port the logic, not the DOM plumbing.

- [~] `lib/agentchat.ts` (2226 lines) → `recur_engine::view::{chat, mentions, digest}` (mention regexes with JS lookaround and whitespace semantics, chips/labels/tokens/strips, caret queries, `mapChatItems`, `mergePersistedWithLive`, `retainOptimisticUsers`, every live reducer, turn-clock gates, error-card title/detail, ask_user/Coder display collapse, findings digest, steer merges); `agentchat.test.ts` ported. clipboard, drop and image drafts are `recur-view-chat`'s composer
- [~] `lib/api.ts` (1461 lines) → `recur_view_core::data` and `state`: `call`, `loadInitial`, `wireEvents` and the event reducers, workspace / project / PR upserts, `removeWorkspace`, provider usage, briefing refresh; view-specific helpers move with their views
- [x] `lib/armed-confirm.ts` (52 lines) → `recur_ui::widgets::armed_button` (`ArmedConfirm`, `armed_click`, `armed_pointer_up`, `take_click_echo`; cases ported)
- [x] `lib/artifacts.ts` (198 lines) → `recur_engine::view::artifacts` (event payloads, unseen ids, icons, list_artifacts parsing, scratch paths, kind tally, relative age); `artifacts.test.ts` ported
- [ ] `lib/autosize-textarea.ts` (29 lines)
- [ ] `lib/bridge-bench.ts` (98 lines)
- [x] `lib/briefingtools.ts` (345 lines) → `recur_agent_tools::display::briefing_tools` (detectors, brief/context/present parsing incl. legacy `Opening`/`Presented` strings, hydration, labels; `briefingtools.test.ts` ported)
- [ ] `lib/bundle-reload.ts` (25 lines)
- [ ] `lib/caches/byte-lru.ts` (189 lines)
- [x] `lib/caches/chat-heap-cache.ts` (211 lines) → `recur_view_chat::heap` (cases ported)
- [x] `lib/caches/chat-strip-cache.ts` (61 lines) → `recur_view_chat::strip` (cases ported)
- [x] `lib/caches/chat-transcript-ui.ts` (324 lines) → `recur_view_chat::transcript_ui` (cases ported)
- [ ] `lib/caches/cm-session-tabs.ts` (112 lines)
- [ ] `lib/caches/observatory-cache.ts` (150 lines)
- [x] `lib/caches/term-park.ts` (123 lines): `recur_terminal_view::Park` + `TerminalPark` global
  - [x] Stash/take, 1MiB per entry, 48MiB budget with LRU eviction returning evicted views to detach (`park.rs`)
  - [x] Discard flag for closed tabs, `should_dispose` consumed on unmount, prune by live workspace ids (`park.rs`)
  - [x] Parked panes stay attached and keep updating (`tests/view.rs`)
  - Pane hooks (`bindTermPaneHooks`) are the view entity itself: the dock calls its methods directly.
- [x] `lib/caches/workspace-land-cache.ts` (90 lines) → `recur_app::land::LandCache` (byte-bounded LRU, prune to live workspaces)
- [x] `lib/changed-tree.ts` (118 lines) → `recur_code::view::changed_tree` (`build_tree_from_paths` with `localeCompare` order via ICU root collation, type-change leaf kept beside children, `flatten_visible_tree`, `cap_visible_tree` borrowing the input when nothing is cut, `paths_under_dir`, `order_changed_paths`, `sort_files_by_changed_tree`); test file ported, plus a node-recorded golden (`tests/golden/ts_view.json`)
- [x] `lib/chat-glance.ts` (48 lines) → `recur_view_chat::glance` (cases ported)
- [x] `lib/chat-occlusion.ts` (38 lines) → `AppState::set_chat_occluder`, `Overlays::{chat_ui_occluded, chat_overlay_open}`; the shell reports `SetChatUIOccluded`
- [x] `lib/chat-route.ts` (28 lines) → `recur_view_core::chat_route` (cases ported)
- [x] `lib/chat-surface.ts` (77 lines) → `recur_engine::view::chat_surface` (the wire marker the UI stamps, never the bubble; the engine side is `chat::surface`); `chat-surface.test.ts` ported
- [ ] `lib/chat-tab-reorder.ts` (74 lines)
- [x] `lib/chat-tab-session.ts` (84 lines) → `recur_engine::view::chat_tab_session` (`ActiveChatTabs` over the exact stored JSON, `resolve_active_chat_code`); `chat-tab-session.test.ts` ported
- [x] `lib/chat-window.ts` (36 lines) → `recur_view_chat::window` (cases ported)
- [x] `lib/check-duration.ts` (90 lines) → `recur_forge::view` (`format_check_seconds`, `parse_check_duration`, `live_check_detail`, `PendingClock`, `prune_pending_clocks`, `check_clock_key`); test file ported
- [ ] `lib/chrome-focus.ts` (89 lines)
- [ ] `lib/clipboard.ts` (69 lines)
- [x] `lib/cm-diff-gutter.ts` (207 lines) → `recur_editor::gutters` (SCM bars, idle recompute, `diffMarks`)
- [x] `lib/cm-diff-lines.ts` (48 lines) → `recur_editor::prefs::{diff_line_kind, wants_diff_line_chrome}` and the diff-line washes
- [x] `lib/cm-editor-history.ts` (214 lines) → `recur_editor::history` + `history_cache` (capture / restore within the 20 MiB byte budget)
- [x] `lib/cm-editor-prefs.ts` (232 lines) → `recur_editor::prefs` + `EditorView::set_prefs` (features, rulers, drop cursor, blink, keymap modes)
- [x] `lib/cm-findings-gutter.ts` (253 lines) → `recur_editor::gutters` findings column (severity icons via `recur_ui::icon`, click opens)
- [x] `lib/cm-goto-line.ts` (125 lines) → `recur_editor::search::{goto_line_initial, goto_line_submit}` + the go-to-line dialog in `panels.rs`
- [x] `lib/cm-keymap.ts` (18 lines) → `recur_editor::bindings` (`recur_default`)
- [x] `lib/cm-lang-data.ts` (311 lines) → `recur_editor::indent` / `close_brackets` / `commands` over `recur_textmate::lang_data` (bundle indent rules, brackets, comments, Enter between pairs)
- [x] `lib/cm-lang.ts` (234 lines) → `recur_editor::view::language_config` + `prefs` language ids
- [x] `lib/cm-line-diff.ts` (230 lines) → `recur_editor::diff`
- [x] `lib/cm-lint-gutter.ts` (215 lines) → `recur_editor::lint` severity gutter
- [x] `lib/cm-mention-sel.ts` (71 lines) → `recur_editor::view_state` mention span
- [x] `lib/cm-merge-full-line-marks.ts` (90 lines) → `recur_editor::merge`
- [x] `lib/cm-merge-revert.ts` (299 lines) → `recur_editor::merge` revert chip + `revert_hunk`
- [x] `lib/cm-mini-script-highlight.ts` (177 lines) → `recur_editor::highlight` chrome-paired engine
- [x] `lib/cm-pointer-pos.ts` (277 lines) → `recur_editor::view_input` (`pos_at_pointer`, click kinds, drag selection, rectangular select)
- [x] `lib/cm-search-panel.ts` (380 lines) → `recur_editor::search::SearchPanel` + `panels.rs` (recur-ui chrome; scene `editor/find`)
- [x] `lib/cm-textmate.ts` (371 lines) → `recur_editor::highlight` over `recur_textmate`
- [x] `lib/cm-themes.ts` (255 lines) → `recur_editor::theme` (`EditorTheme::resolve`, `app_chrome_editor_theme`, `TextMate::activate_theme`)
- [x] `lib/cm-tm-deco.ts` (151 lines) → `recur_textmate::paint` + `recur_editor::highlight` spans
- [x] `lib/cm-tm-delta.ts` (84 lines) → `recur_textmate::delta`
- [x] `lib/cm-tm-doc.ts` (58 lines) → `recur_editor::highlight` document keys
- [x] `lib/cm-tm-fold.ts` (97 lines) → `recur_textmate::fold` + `recur_editor::fold` (and `foldGutter`'s marker rebuild triggers)
- [x] `lib/cm-tm-paint.ts` (15 lines) → `recur_textmate::paint`
- [x] `lib/cm-tm-stacks.ts` (153 lines) → `recur_textmate::stacks`
- [x] `lib/codehl.ts` (279 lines) → `recur_textmate::codehl` (LSP documentation code tint)
- [~] `lib/colors.ts` (159 lines): PR rules (`prLifecycle`, `keepMergedPr`, `prIconName`, `prForWorkspace`) → `recur_view_core::pr`; the rest moves with the views that use it
- [ ] `lib/command-usage.ts` (99 lines)
- [x] `lib/commands.ts` (489 lines) → `recur_view_core::commands` (registry with its cache, scope and glob matching, triggers and key equivalents, input resolution, macro runner / editor command / editor focus hooks on `AppState`; cases ported)
- [ ] `lib/comment-header.ts` (22 lines)
- [ ] `lib/comment-hunk.ts` (107 lines)
- [x] `lib/comment-permalink.ts` (91 lines) → `recur_forge::github::{comment_permalink, commit_url, blob_url, ui_node_database_id}`, reached through `Forge::{comment_url, commit_url, blob_url}`; test file ported (UI node-id decoding kept separate from the backend's: BigInt-sized ids, printable legacy ids)
- [~] `lib/comment-reactions.ts` (137 lines) → pure rules in `recur_forge::view` (`REACTIONS` picker table, `reaction_icon`, `reaction_label`, `visible_reactions`, `apply_comment_reactions`, `apply_reaction_holds`; normalize/toggle/mine are `recur_domain::reaction`); test file ported. Left for `recur-app`: the optimistic toggle with its 120 s hold map and store painting
- [ ] `lib/comment-thread.ts` (372 lines)
- [ ] `lib/commit-meta.ts` (12 lines)
- [ ] `lib/compatible-providers.ts` (81 lines)
- [x] `lib/composerDom.ts` (1582 lines) → `recur_engine::view::composer` (wire ↔ `Segment` lines: hydrate with chip restore and boundary repair, serialize, chip boundaries, prompt chips with quoted multi-arg wire, `promptChipArgsWithTrailing`); text cases of `composerDom.test.ts` ported. the editor itself is `recur_view_chat::{composer_doc, composer, dock}` (a native composer: caret, selection, chips, IME, keys), with the behavior cases ported as `spec::composer` and `spec::chat_spec` tests
- [x] `lib/composerUndo.ts` (102 lines) → `recur_view_chat::composer_undo` (cases ported)
- [x] `lib/contextUsage.ts` (90 lines) → `recur_agent::context_usage` (ContextUsage used/ready/after_compact, from_turn, from_chat_items, format_tokens with JS `toFixed`/`Math.round` rounding; `contextUsage.test.ts` ported)
- [ ] `lib/dialog-focus.ts` (84 lines)
- [x] `lib/editor-dirty.ts` (60 lines) → `recur_editor::view_state`
- [x] `lib/editor-session.ts` (870 lines) → `recur_editor::session` (shape, caps, pruning, wire format, patch gate; debounced writes are the host's)
- [x] `lib/editor-view-state.ts` (443 lines) → `recur_editor::view_state` + `EditorView::apply_view_state`
- [x] `lib/editor-view.ts` (136 lines) → `recur_editor::view_state` (view-mode chip, SCM base rules)
- [x] `lib/emoji.ts` (197 lines) → `recur_content::emoji` (shortcode expansion outside code and at the caret; the shortcode table is read from `frontend/src/lib/emoji-shortcodes.json` in file order); the typeahead is `recur_view_chat::suggest` (cases ported)
- [ ] `lib/explorer-visible.ts` (33 lines)
- [ ] `lib/file-drag.ts` (332 lines)
- [ ] `lib/file-icons/icon-tables.ts` (52 lines)
- [ ] `lib/file-icons/icon.ts` (31 lines)
- [ ] `lib/file-icons/index.ts` (41 lines)
- [ ] `lib/file-icons/types.ts` (26 lines)
- [~] `lib/file-index.ts` (162 lines) → `recur_code::view::file_index::map_file_search_hits` (dir flag, trailing slash, parent-of-another-hit); test file ported. The readiness map, listeners, in-flight search cancel and `SetFileIndexExclude` sync are `recur-app` / services wiring over `recur_code::IndexHub`
- [x] `lib/finding-paths.ts` (112 lines) → `recur_code::view::finding_paths` (`match_finding_path`, `findings_for_path` over caller-ordered buckets, `right_diff_line_set` / `right_diff_has_range` with JS `parseInt`, `finding_covers_line`, `findings_on_line` / `_covering_line` / `_ending_line`, `extra_html_without_finding_flags`); test file ported
- [~] `lib/findings-ui.ts` (396 lines) → the chat half in `recur_view_chat::findings_ui` (titles, locations, severity tallies, `list_findings` parsing and store hydration, digest refs; cases ported). Left for the PR detail and Observatory ports: review anchors, pending / submitted comment matching and explorer flags
- [~] `lib/format.ts` (51 lines): `formatCompactCount` → `recur_view_core::format`
- [x] `lib/fuzzy.ts` (145 lines) → `recur_code::fuzzy` (`fuzzy_score`, `prepare_fuzzy`, `fuzzy_filter_prepared`, `fuzzy_filter`): UTF-16 scoring and the `a`-`z`/digit/other mask like JS, ties in `Intl.Collator` order (ICU root collation). The scorer is shared with Go's Quick Open ranking (`rank_files`, on bytes). Node-recorded golden over 17 queries × 31 labels (punctuation, case, ß, ü, emoji)
- [x] `lib/glob.ts` (64 lines) → `recur_code::glob` (`compile_excludes`, `is_excluded_by`; JavaScript regex semantics through `regress` on UTF-16, per-pattern cache). The glob → regex translation is shared with the file index and copy-files
- [x] `lib/file-icons/icon-tables.ts` (52 lines) → `recur_ui::file_icons` (the shipped `icondb.js` tables parsed at startup, regexes via `fancy-regex`)
- [x] `lib/file-icons/icon.ts` (31 lines) → `recur_ui::file_icons::FileIconClass` (light/dark color slots)
- [x] `lib/file-icons/index.ts` (41 lines) → `recur_ui::file_icons::{file_icon_class, file_icon}` (dotenv rule, basename matching, fallback; glyphs from the icon fonts through CoreGraphics with smoothing off); scenes `icons/file`, `icons/file-light`
- [x] `lib/file-icons/types.ts` (26 lines) → `recur_ui::file_icons`
- [ ] `lib/file-index.ts` (162 lines)
- [ ] `lib/finding-paths.ts` (112 lines)
- [ ] `lib/findings-ui.ts` (396 lines)
- [ ] `lib/format.ts` (51 lines)
- [x] `lib/fuzzy.ts` (145 lines) → `recur_ui::logic::fuzzy` (`fuzzy_score`, `prepare_fuzzy`, `fuzzy_filter_prepared`, `fuzzy_filter`, CLDR punctuation collation; cases ported)
- [ ] `lib/glob.ts` (64 lines)
- [x] `lib/hoist-chat-keys.ts` (286 lines) → `recur_view_chat::hoist` (cases ported)
- [ ] `lib/hot-shells.ts` (51 lines)
- [x] `lib/hoverGate.ts` (50 lines) → `recur_ui::logic::hover_gate::HoverGate` (cases ported)
- [x] `lib/icons.ts` (1003 lines) → `recur_ui::IconName` and `assets/icons/app/*.svg`, generated by `assets/icons/generate.mjs` from the frontend sources
- [~] `lib/inbox-read.ts` (28 lines) → `recur_engine::view::{workspace_unread_ids, mark_workspace_read, mark_item_read, mark_all_read}` (the optimistic inbox rules; tested). Left for `recur-app`: calling `InboxService.MarkInboxRead` / `MarkAllInboxRead` and updating the inbox store
- [~] `lib/issue-link.ts` (36 lines) → `recur_issues::view::{issue_key, issue_url}` (tracker fallback via `IssueTracker::issue_url`); tested. Left for `recur-app`: the `trackerIssues` cache keyed on `issuesStale` and `openIssueUrl` (services `OpenLinearInApp` uses `IssueTracker::app_url`)
- [ ] `lib/keybinding-menu.ts` (27 lines)
- [x] `lib/keyequiv.ts` (285 lines) → `recur_editor::keyequiv`
- [x] `lib/lineartools.ts` (320 lines) → `recur_issues::linear::tools`; test file ported
- [x] `lib/loop-cockpit.ts` (432 lines) → `recur_engine::view::loop_cockpit` (`merge_loop_cockpit`, `split_session_turns`, live codes, kickoff/wrap facts, PR-fact parsing and completion, loop-wrap strip; `loop-cockpit.test.ts` ported). Rendering is `recur-app`'s
- [~] `lib/loops.ts` (183 lines) → `recur_engine::view::loops` (shipped ids, enable/default rules, workspace pick, select options, `autonomy_from_policy_id`, fallback catalog checked against `bundles/Loop`). Left for `recur-app`: the `ListLoops` / `CreateWorkspaceWith` / `RememberChatLoopPolicy` / `EngageLoopWith` calls
- [ ] `lib/lsp-lang.ts` (86 lines)
- [ ] `lib/lsp-problems.ts` (116 lines)
- [~] `lib/lsp.ts` (1520 lines) → editor UI half in `recur_editor::lsp` (`LspClient` trait, hover, signature help, formatting, rename, definitions, completion source, diagnostics mapped through unsynced edits, `content_changes_for`); transport rules in `recur_lsp::protocol`
  - [ ] The `LspClient` adapter over `recur_lsp::Manager` (request ids, `initialize`, `didOpen` / `didChange` versions) lives in `recur-app`
- [x] `lib/lucide-icons.ts` (82 lines) → `recur_ui::assets` (`lucide_icons`, `lucide_suggested`, `lucide_entry`, `has_lucide_icon`, aliases; generated `lucide.json`)
- [ ] `lib/macro-worker.ts` (479 lines)
- [x] `lib/markdown-host.ts` (262 lines) → `recur_content::markdown::host` (byte-budget LRU, per-entry cap, keyed streaming renders, worker threads, 2s watchdog with strikes)
- [x] `lib/markdown-parse.ts` (191 lines) → `recur_content::markdown::parse` over a verbatim `marked` 18 port (`marked/`, 820-case golden)
- [x] `lib/markdown-worker.ts` (20 lines) → `MarkdownHost` worker threads
- [x] `lib/markdown.ts` (51 lines) → `recur_content::markdown` with a DOMPurify 3 port (`purify/`, golden); `markdown.test.ts` ported
- [x] `lib/mcp.ts` (69 lines) → `recur_agent::mcp` over `recur_domain::McpServer` (untrimmed id rule, slug, global off-list, Linear upsert, custom list, enable/remove/update keeping the id; `mcp.test.ts` ported)
- [ ] `lib/mdcopy.ts` (88 lines)
- [x] `lib/menu-space.ts` (248 lines) → `recur_ui::logic::menu_space` (`menu_viewport_space`, `menu_prefer_up`, `menu_fixed_box`, `clamp_menu_left`, `popover_top`, `more_menu_fixed_box`, `rects_overlap`, `MenuVariant` sizes; cases ported). Window resize/scroll watching belongs to the overlay owner
- [x] `lib/menu.ts` (152 lines) → `recur_view_core::menu` (`MenuState` merge, idle state, `terminalIdFromMenu`, theme lists and state) and `recur_app::menu`
- [x] `lib/mermaid.ts` (137 lines) → `recur_content::mermaid` (config and theme variables, ids, hydration, `unclipMermaidSvg` in the page script); mermaid 11.16.1 in a hidden WebKit page, drawn natively (decision in `ARCHITECTURE.md`)
- [ ] `lib/models.ts` (179 lines)
- [x] `lib/modes.ts` (419 lines): shell and sidebar predicates (`wsRunActive`, `workspaceIsRunning`, `parkedNeedsYouByWorkspace`, `sidebarUnreadTitle`, `workspaceShowsRemove`, `workspaceCanRemove`, `wsCreating`, `wsMainBlocked`) → `recur_view_core::modes`; the chat-mode half (dock mode list, cycling, per-mode Settings default model, picker rule after a switch, create/setup predicates) → `recur_engine::view::modes`; `modes.test.ts` ported; the chat's mode labels, icons and tints → `recur_view_core::modes::ChatMode`
- [ ] `lib/mono-fonts.ts` (236 lines)
- [~] `lib/new-workspace.ts` (278 lines): `createBlankWorkspace` (⇧⌘N, File > New Blank Workspace) → `recur_app::new_workspace`; the pure launch rules (eager-create input, the eager handle and its abandon, opening chat seed, parked-opening failures) → `recur_engine::view::new_workspace` with `new-workspace.test.ts`; the modal's flows move with NewTask
- [ ] `lib/open-external.ts` (81 lines)
- [ ] `lib/open-in-pref.ts` (34 lines)
- [x] `lib/open-path.ts` (48 lines) → `Shell::pick_and_open_path`, `AppState::apply_open_path_result`
- [x] `lib/open-pr-tool.ts` (70 lines) → `recur_agent_tools::display::open_pr_tool` (`open-pr-tool.test.ts` ported)
- [x] `lib/overlay-scrollbar.ts` (141 lines) → `recur_ui::widgets::scroll_area(..).overlay()` (`overlay_thumb`, `scroll_from_track` with cases; show on scroll, hover and drag, 700ms hide, .12s/.45s fades, track press and drag); scenes `scrollbar/default`, `scrollbar/paper`
- [x] `lib/pdf-preview.ts` (85 lines) → `recur_content::pdf::logic`; `discardPageCanvas` is the slot drop, `pdfjsAssetDirs` the `recur-pdf:` asset routes (`pdf/engine.rs` test)
- [x] `lib/permissionTools.ts` (60 lines) → `recur_agent_tools::display::permission_tools::permission_tool_label`
- [x] `lib/plantools.ts` (120 lines) → `recur_agent_tools::display::plan_tools` (step/attachment/body parsing, labels, ack title recovery)
- [ ] `lib/pr-attention.ts` (98 lines)
- [ ] `lib/pr-checks.ts` (139 lines)
- [ ] `lib/permissionTools.ts` (60 lines)
- [ ] `lib/plantools.ts` (120 lines)
- [x] `lib/pr-attention.ts` (98 lines) → `recur_forge::view` (`pr_attention_codes`, `pr_attention_labels`, `pr_has_review_request`, `pr_has_check_status`, `pr_checks_rollup`, `pr_checks_passing`); test file ported
- [x] `lib/pr-checks.ts` (139 lines) → `recur_forge::view` (`is_pending_check`, `is_skipped_check`, `group_checks`, `checks_rollup` → `RollupKind`, `checks_heading` with the forge label, `check_group_collapsed`, `checks_box_byline`) and `Forge::{checks_url, check_run_url}`; test file ported
- [ ] `lib/pr-commits.ts` (23 lines)
- [ ] `lib/pr-detail-pending.ts` (48 lines)
- [ ] `lib/pr-fix-checks.ts` (53 lines)
- [ ] `lib/pr-list-filter.ts` (190 lines)
- [ ] `lib/pr-merge-action.ts` (276 lines)
- [x] `lib/pr-participants.ts` (165 lines) → `recur_forge::view` (`PrPerson`, `PR_FACE_CAP`, `pr_participants`, `pr_people_with_reviews`, `reviewer_summary`, `review_state_label`, `participant_label`, `participant_initials`, `participant_avatar_url` via `Forge::avatar_url(.., 40)`, `pr_face_slots`, `person_key`); test file ported
- [ ] `lib/pr-thread-resolved.ts` (43 lines)
- [~] `lib/pr-workspace.ts` (107 lines): `prAgentWorkspaceId`, `prChatHostRunningCount`, `prChatHostUnseenCount`, `prIdForChatWorkspace` → `recur_view_core::pr`; the rest moves with PR detail
- [x] `lib/preview-find.ts` (260 lines) → `recur_content::preview::find` over rendered text blocks (the DOM walk's script / style / hidden skips have no native counterpart: those nodes never render)
- [x] `lib/preview-scroll.ts` (237 lines) → `recur_content::preview::scroll`; `preview-scroll.test.ts` ported
- [x] `lib/preview.ts` (139 lines) → `recur_content::preview` (kinds, find gating, CSP, srcdoc shell and scripts byte-identical); the iframe find-reveal case needs a live webview and is not ported as a unit test
- [x] `lib/project-pic.ts` (120 lines) → `recur_ui::widgets::project_pic` (`parse_project_pic`, `format_lucide_pic`, `is_letter_pic`, `project_pic_from_name`, `letter_from_name`, `PROJECT_COLORS`, `fg_on_background`; cases ported)
- [~] `lib/promptActions.ts` (438 lines) → `recur_engine::view::prompts` (`pick_prompt_tab`, disabled prompts, slash lookup, arg slots, `params_from_args`, `/` menu gates, committed lines, args query, prompt context, error wording) over `recur_bundles::PromptAction`; `promptActions.test.ts` ported. The list cache and the run and complete calls are `recur_view_chat::prompt_actions` (`PromptCatalog`; TypeScript modules run through its runner hooks, see the follow-up). Left for `recur-app`: the titlebar request store
- [x] `lib/providerUsage.ts` (141 lines) → `recur_view_sidebar::usage` (cases ported)
- [ ] `lib/providers.ts` (76 lines)
- [x] `lib/rail-peek.ts` (106 lines) → `recur_view_sidebar::peek` and `AppState::{open_rail_peek, close_rail_peek, set_rail_peek_locked}`; focus parking in the sidebar (cases ported)
- [x] `lib/reenter-phase-tool.ts` (32 lines) → `recur_agent_tools::display::reenter_phase_tool` (`reenter-phase-tool.test.ts` ported)
- [~] `lib/remove-workspace.ts` (72 lines) → `AppState::remove_workspace` (optimistic hide, leave detail, restore on failure) and `recur_engine::view::{mark_removed, removed_toast, unique_remove_ids}` plus the engine's `workspace_discard_risk` copy; `remove-workspace.test.ts` pure cases ported. Left for `recur-app`: the optimistic hide, editor-session flush, leaving the detail view and the ListWorkspaces restore on failure
- [ ] `lib/review-diff.ts` (131 lines)
- [ ] `lib/review-scope.ts` (47 lines)
- [ ] `lib/review-scroll.ts` (18 lines)
- [ ] `lib/review-window.ts` (263 lines)
- [x] `lib/run-commands.ts` (69 lines) → `recur_view_core::run` (`project_runs`, `project_default_run`, `run_label`, `filter_runs`, `order_runs`; cases ported)
- [~] `lib/run-dev.ts` (426 lines): the run store (sessions, tabs, starting / launched / busy bookkeeping), run and stop requests and menus, `workspaceRun` / `workspaceDevCmd` → `recur_view_core::run` and `AppState`; launching into the terminal dock moves with it
- [x] `lib/sanitize-lsp.ts` (20 lines) → `recur_content::sanitize_lsp` (recur-editor lays LSP docs out from markdown events and needs no HTML sanitizing)
- [ ] `lib/schedule-expanded.ts` (34 lines)
- [ ] `lib/scroll-fade.ts` (132 lines)
- [x] `lib/session-feed.svelte.ts` (314 lines) → `recur_view_chat::session_feed::SessionFeed` (cases ported)
- [x] `lib/session-live.ts` (57 lines) → `recur_engine::view::live::SessionLiveFlags`; `session-live.test.ts` ported
- [x] `lib/shell-state.ts` (177 lines) → `recur_view_core::shell_state` (snapshot, restore resolution, coalesced writer; cases ported)
- [ ] `lib/shhl.ts` (99 lines)
- [x] `lib/shortcut.ts` (67 lines) → `recur_ui::kbd::shortcut_parts` (modifier runs; cases ported)
- [ ] `lib/shortcuts.ts` (96 lines)
- [x] `lib/simplify.ts` (13 lines) → `recur_engine::simplify::text::{simplify_enabled, simplify_from_selection}`; test file ported
- [x] `lib/skills.ts` (176 lines) → `recur_code::view::skills` (`SkillRow`, `SkillKind`, `skill_kind`, `is_recur_skill_path`, `infer_home_from_skill_path`, `skill_location`, `map_skill_rows`, `effective_skill_enabled` with the UI's untrimmed names, `toggle_disabled_name`, `project_skill_override_lists`, `skill_override_hint`, `skill_kind_override_count`, `skill_kind_has_overrides`); unit tests plus node golden
- [x] `lib/snippets.ts` (319 lines) → `recur_editor::snippet`
- [ ] `lib/split-diff.ts` (107 lines)
- [ ] `lib/spy-pick.ts` (115 lines)
- [~] `lib/stores.ts` (1873 lines) → `recur_view_core::AppState`: data stores, navigation (`go`, `openWorkspace`, `openPR`, `openProject`, crumbs), overlays, rail and peek, panel sizes, themes and system sync, terminal prefs, dock chrome, keyboard cycling; view-local stores move with their views
- [ ] `lib/stream-reveal.ts` (8 lines)
- [x] `lib/subagent.ts` (79 lines) → `recur_engine::view::subagent` (Agent/Task and SendMessage display); `subagent.test.ts` ported
- [ ] `lib/syntax-mode.ts` (213 lines)
- [x] `lib/tab-strip-drag.svelte.ts` (392 lines) → `recur_ui::widgets::tab_strip` (4px threshold, floating chip, dashed placeholder, insert index from pointer x, edge autoscroll, FLIP 180ms, 200ms settle, Alt+arrow moves, order restore; `reorder_by_insert`, `insert_index_from_pointer_x`, `restore_strip_order` with cases)
- [x] `lib/tab-strip-scroll.ts` (174 lines) → `recur_ui::widgets::tab_strip` (`scroll_delta_to_reveal`, `next_strip_scroll_left`, `strip_scroll_is_programmatic`, 280ms ease-out chase on frozen-aware motion; cases ported)
- [x] `lib/tasktools.ts` (340 lines) → `recur_agent::display::tasks` (args, TaskCreate id, TaskList JSON/line parse with `[blocked by]`/`(owner)` suffixes, TaskGet JSON/multiline parse, labels, meta, chip status, has-body, ack rules; regex semantics hand-ported; `tasktools.test.ts` ported)
- [ ] `lib/term-fit.ts` (35 lines)
- [ ] `lib/term-focus.ts` (15 lines)
- [ ] `lib/terminal.ts` (206 lines)
- [ ] `lib/tasktools.ts` (340 lines)
- [x] `lib/term-fit.ts` (35 lines): `term_fit_size`, `TERM_FIT_*` constants; the content box comes from the element's padded bounds (`font.rs`)
- [x] `lib/term-focus.ts` (15 lines): `TerminalOptions::dock_focus` + `TerminalView::try_focus_if_dock_owns_keys` / `set_dock_focus`: the dock passes its focus handle and panes only take focus while it is focused
- [~] `lib/terminal.ts` (206 lines)
  - [x] `wireTerminalEvents` / `onTerminalData` / `onTerminalExit`: `Router` global + `route_event`; bytes stay raw, so `decodeTermBytes` has no counterpart
  - [x] `ackWriter`: acks after the emulator parses, batched at `ACK_THRESHOLD` (`tests/view.rs`)
  - [x] `terminalTheme`, `resolveTerminalTheme`, `terminalSurfaceBg`, `terminalFrosts`: `theme.rs` (`ThemeContext`, ported tests)
  - [ ] `terminalThemeCatalog` / `ensureTerminalThemes`: the app loads `ListTerminalThemes` and passes the catalog in `ThemeContext`
- [ ] `lib/textarea-caret.ts` (58 lines)
- [x] `lib/title-tooltip.ts` (248 lines) → `recur_ui::tooltip` (`place_tooltip`, `show_delay_ms`, `should_reshow_after_dismiss`, `TipCore` with `apply_hide`/`apply_consider`/`apply_timer`; cases ported)
- [x] `lib/titlebar.ts` (30 lines) → `recur_app::chrome` (zoom targets: the titlebar minus its controls)
- [~] `lib/tm-bootstrap.ts` (79 lines) → `TextMate::activate_theme` and the grammar registration in `recur_editor::gallery::shipped_textmate`; the app lists grammars and themes through services
- [x] `lib/tm-engine.ts` (624 lines) → `recur_textmate::engine`
- [x] `lib/tm-fill.ts` (103 lines) → `recur_textmate::fill`
- [x] `lib/tm-registry.ts` (628 lines) → `recur_textmate::catalog` / `registry`
- [x] `lib/tm-scope-selector.ts` (121 lines) → `recur_textmate::scope_selector`
- [x] `lib/tm-symbols.ts` (297 lines) → `recur_textmate::symbols`
- [x] `lib/tm-theme.ts` (590 lines) → `recur_textmate::tm_theme`
- [x] `lib/todos.ts` (38 lines) → `recur_agent::display::todos` (parse, counts, latest over `ChatItem`s; `todos.test.ts` ported)
- [ ] `lib/tree-sticky.ts` (137 lines)
- [x] `lib/updater.ts` (42 lines) → `Shell::{check_for_updates, updater_event}` (silent background checks, Restart toast)
- [x] `lib/user-mention.ts` (200 lines) → pure rules in `recur_forge::view` (`mention_login`, `mention_people`, `suggest_user_mentions` taking the fuzzy ranker from `lib/fuzzy.ts`'s port, `merge_mention_hits`); test cases ported. The textarea typeahead (caret tracking, the 150 ms remote search, keys) is `recur_view_chat::suggest` (cases ported)
- [ ] `lib/uuid.ts` (16 lines)
- [~] `lib/wait-for-worktree.ts` (38 lines) → `recur_engine::view::{worktree_wait, ws_creating}` (`WorktreeWait::{Ready, Gone, Pending}`); `wait-for-worktree.test.ts` ported. Left for `recur-app`: the store subscription and abort signal
- [x] `lib/webtools.ts` (87 lines) → `recur_agent::display::web` (WebFetch URL/prompt, WebSearch query, `compact_url` on the WHATWG `url` crate, JSON sniff; `webtools.test.ts` ported)
- [ ] `lib/workspace-file.ts` (96 lines)
- [x] `lib/workspace-nav.ts` (350 lines) → `recur_view_core::workspace_nav` (nesting, scheduled split, sidebar order, cycling, scan peek / commit / forget, reveal delta; test file ported)
- [x] `lib/workspace-path.ts` (31 lines) → `recur_app::titlebar::copy_selected_workspace_path`

### Styles

- [~] `styles/_base.scss` (the generic parts; view-specific rules belong to their views)
  - [x] `body` and `window_root` (fonts, colors, line height, WebKit half-pixel ties), `.tbtn` and `.tb-right` buttons, `.tb-icon-btn`, `.tb-rail` (scenes `buttons/*`)
  - [x] `::-webkit-scrollbar` 11px themed scrollbar and `.overlay-sb` (`recur_ui::widgets::scroll_area`)
  - [x] `.kbd` / `.kbd-mod` keycaps (system-ui through CoreText with the weight trait and variation)
- [ ] `styles/_briefing.scss`
- [x] `styles/_chat.scss` → `recur-view-chat` (the `chat/*` scenes)
- [x] `styles/_codemirror.scss` → `recur_editor` (`panels.rs`, `tooltip_ui.rs`, `markdown.rs`, `vim/prompt.rs`, `element.rs`)
  - [x] Panels, find / replace / go-to-line, completion list and info, hover, signature help, `.cm-lsp-documentation` (UA block margins collapsed, inline code boxes), title tooltips through `recur_ui::tooltip`
  - [ ] Link `border-bottom` drawn as an underline and the signature's hanging indent (recur-ui text has no per-side inline border or text-indent)
- [~] `styles/_forms.scss` (the generic parts)
  - [x] `.sel*` select and menus, `.tog` toggles, `.prog`, `.hint`, `.nt-go`, `.nt-link`, `.nt-bg` / `.nt-modal`, `.nt-tip-pop` (scenes `select/*`, `toggles/*`, `buttons/*`, `overlays/*`)
- [ ] `styles/_loop.scss`
- [ ] `styles/_main.scss`
- [ ] `styles/_observatory.scss`
- [~] `styles/_overlays.scss` (the generic parts)
  - [x] Palette: `.palette-bg`, `.palette`, `.cmd-palette`, `.pal-in` (with WebKit's input padding), `.pal-g`, `.pal-row` with `.pic`/`.pl`/`.psub`/`.pk`/`.dot`, `.pal-empty`, `.pal-foot` (scene `overlays/palette`)
  - [x] `.toasts`, `.toast*` (scenes `toasts/*`), `.emoji-suggest` panel, `.skel` with its shimmer and `.skelrow`, global `:focus-visible` ring (scenes `overlays/misc*`, `select/focus`)
  - [x] Box shadows as WebKit's ShadowBlur, diagonal gradients, backgrounds under opaque borders (`recur_ui::shadow`, `style::css_gradient`, `style::inset_background`)
- [ ] `styles/_prs.scss`
- [ ] `styles/_settings.scss`
- [ ] `styles/_sidebar.scss`
- [~] `styles/_terminal.scss`
  - [x] Pane padding, bottom-pinned grid, inactive panes hidden and absolutely positioned (`view.rs`)
  - [x] Scrollbar slider colours from the palette, hover/active states (`element.rs`)
  - [x] Find bar (`cm-recur-search` look): field, toggles, count, buttons, focus rings and title tips on recur-ui (`find_bar.rs`, scene `terminal/search`)
  - [x] Terminal tokens from `recur_ui::Theme`; panes observe it, so runtime theme switches repaint (`theme.rs`, `tests/view.rs`)
  - [ ] Dock chrome (tab strip, maximize, beside layout) belongs to the `TerminalDock.svelte` port
  - [x] Pixel parity: the pane and find bar (scenes `terminal/*`); `.term-dock .xterm` overrides mirrored by the scene's dock body
- [x] `styles/_tokens.scss` → `recur_ui::theme` (scenes `tokens/*` exact in every theme)
  - [x] Every token of every `data-theme` (Charcoal default, Paper, Slate, Cool Glow, Catppuccin Mocha and Latte, Nord) with exact `color-mix` results, theme aliases, light/dark, `--coral-line`, scrollbar and git tokens, `--on-amber`
  - [x] Runtime switching (`Theme::set`, windows refresh, gpui-component theme synced) and persisted appearance with system sync (`Appearance`, the `shell.json` shape)
  - [x] Light-theme overrides used by shared widgets (`.skel::after`, tooltip shadows, file-icon color slots)
- [ ] `styles/_welcome.scss`
- [ ] `styles/app.scss`

## Go packages

Source: `internal/`, root `*.go`, `cmd/`. Each Go test file is a behavior spec: port its cases alongside the code.

- [x] `internal/agenttools`: 52 files, 3802 lines, 3 test files → `recur-agent-tools` (all three Go test files ported)
  - [x] `Host` trait (engine seam) with Go `noopReporter` as default bodies (`NoopHost`), all request types, `HostError`; `Tool`/`Permitter`/`Call`/`Registry`, `advertised_name`, `bare_name`, `HALT_NAMES`, `halt_after`; `mcp__recur__*` and bare tool-name constants (`names`, `names::mcp`)
  - [x] All 42 host tools (`host_tools()` in `tools/list` order): names, descriptions and schemas byte-identical to Go (`tests/catalog.rs`), and 623 recorded calls replayed for identical result text, error text and host calls (`tests/host_golden.rs`, fixtures from `tests/golden/golden_gen_test.go.txt`)
  - [x] Go `encoding/json` decode semantics for tool args (`json`): scanner syntax errors, `Decoder` vs `Unmarshal`, case-insensitive and Kelvin/long-s field folding, first error wins, decode into existing values on duplicate keys, `UnmarshalTypeError` text with Go type names, `json.RawMessage`, `DisallowUnknownFields`; `%q` (`strconv.Quote`) and Go's per-byte U+FFFD handling (`gostr`)
  - [x] Worktree sandbox (`path`): `code.SafeJoin`, `filepath` Clean/Join/Rel/Abs/Dir/Base, `EvalSymlinks`, nearest-existing-ancestor resolution, `MkdirAll`, `*PathError` text
  - [x] Read, Edit, Write, Bash, TodoWrite + `TodoStore`: 75 recorded Go cases replayed (`tests/coding_golden.rs`); Bash keeps process-group kill on cancel/timeout, `WaitDelay` (2 s), capped capture, `exit: …` text with Go signal names per platform
  - [x] Glob and Grep always built (the `code` feature is gone): ripgrep through `recur_code::rg::locate`, Go-walk fallback through `recur_code::search_opts`, managed binary under `recur_store::paths::data_dir`; all 82 recorded coding cases replay (`tests/coding_golden.rs`)
- [ ] `internal/app`: 25 files, 8921 lines, 27 test files
  - [x] `instance.go` + test: `recur_platform::identity::{instance_label, window_title}` (title falls back to `Recur Next` side by side)
  - `open_path.go` stays with `recur-services`: it is WorkspacesService logic (store, engine, git), not OS path handling. Platform takes its result through `open_boot::attach_open_ipc` / `apply_cold_opens`.
  - [x] `model_catalog.go` catalog types → `recur_agent::catalog` (FlagOption, FlagDimension, ModelChoice, ModelGroup serde-identical; `defaultFlagsFromDims`). Per-provider lists, merge and fallbacks belong to P1/P2/P3 and SV
  - [x] `gh_image.go` → `recur_forge::github` behind `Forge::fetch_image` (allow-list, S3-only redirects with credentials dropped off-host, 5-redirect cap, 20 s bound, 8 MiB cap, content-type and sniffing, cached `gh auth token`); `gh_image_test.go` ported with a fake `ImageTransport`. The `/gh-image` HTTP handler (method check, missing `url` → 400, `Cache-Control`) is services (`fileserver`)
  - [x] `issues.go` trackers → `recur_issues::{github::GitHubIssues, linear::Linear}` (gh search batches of 5 repos, remote → project map, Linear assigned + paged unassigned to 300, claim, `linear://` links, GraphQL error handling) and `recur_issues::{list_assigned, claim}`; `issues_test.go`, the claim half of `workspace_issue_test.go`, and Go-generated golden fixtures (`tests/golden/linear.json`) ported. `ListAssignedIssues` / `OpenLinearInApp` service wrappers are SV
  - [x] `model_catalog.go` Claude branch → `recur_provider_claude::flags::catalog` (pinned ids, effort + 200K/1M context dims, context windows); Codex branch → `recur_provider_codex::catalog` (hidden/id-less dropped, friendly model and effort labels, effort/272K-1M/Fast dims with the real boost tier id). Both behind `Provider::{models, cached_models}`; Codex remembers the last good `model/list` (from `models` and `status`). Go-matched in `go_claude_app.json` / `go_codex_app.json`. Merge, fallbacks and remap stay SV
  - [x] `claude_status.go` + test → `recur_provider_claude::status` (`--version` and `auth status` each 5 s, logged-out JSON vs missing binary vs timeout, provider/login labels, `detect_agent` for `DetectAgent`) behind `Provider::status`. The `GetClaudeStatus` / `DetectAgent` service methods are SV
  - [x] `model_catalog.go` Cursor branch → `recur_provider_cursor::catalog` (drops `default`, house models first, ListModels params to dims with variant defaults and On/Off booleans, context windows from the table) behind `Provider::{models, cached_models}`; the status probe also warms the cache
  - [x] `cursor_service.go` + test → `recur_provider_cursor::status` and `CursorProvider::{cursor_status, test_cursor_key, install}` (`CursorStatus` JSON-identical, throwaway probe bridges, Connect-file gate, pin/update rules) behind `Provider::{status, test_credentials}`. The orphan-key drop, `cursor.install` publishing and `CursorBridgePath` save in `GetCursorStatus` / `InstallCursorBridge` are SV
  - [x] `codex_service.go` → `recur_provider_codex::status` (throwaway app-server probe with 60 s budget, account/read + model/list, login method, plan names, ChatGPT-auth precedence, `test_key` draft fallback to saved key) behind `Provider::{status, test_credentials, models}`. The `GetCodexStatus` / `TestCodexKey` service methods are SV
  - [x] `compatible_service.go` + test → `recur_provider_compatible::status` (`test_compatible_provider` with normalization, first model, 20 s; `compatible_flag_preset`) behind `Provider::{status, test_credentials}`. The `TestCompatibleProvider` / `CompatibleFlagPreset` service methods are SV
  - [x] `model_catalog.go` compatible branch → `recur_provider_compatible::catalog` (`compatible_catalog_models`, `compatible_flag_dims`) behind `Provider::{models, cached_models}`; Go-matched in `go_catalog.json`, including the enabled+configured groups. Merge and fallbacks stay SV
  - [x] `provider_usage.go` + test → `recur_usage::UsageService` (see `internal/usage`); `lsp_service.go` → `recur_lsp::service` (see `internal/lsp`)
- [ ] `internal/bundles`: 19 files, 6525 lines, 20 test files
- [ ] `internal/cli`: 7 files, 830 lines, 2 test files
- [x] `internal/bundles`: 19 files, 6525 lines, 20 test files → `recur-bundles` (all 148 Go test functions ported: 14 unit, 131 in `tests/suite` mirroring the Go files, 5 env-mutating in `tests/env_serial`, plus 2 compile-parity tests)
  - [x] Manifest schema (`manifest.rs`): `[bundle]`, `[[command]]` (input/fallbackInput/output/interpreter/triggers/keyEquivalent/language/openInPreview/prompts, inline `body` XOR `script`), `[[prompt]]` (slash default from name, mode/loop/fresh_session/tab_group/timeoutMs/takesArgs/argPlaceholder/`[[prompt.arg]]`/complete/icon), `[[search]]`, `[[expand]]` (RE2 `match`), `[[languageServer]]`; every validation message byte-identical (Go `%q`, `*fs.PathError` text)
  - [x] TOML decoding with go-toml v2 semantics: unknown keys ignored, case-insensitive key match (`tomlx.rs`)
  - [x] Go semantics the messages depend on (`go.rs`): `strconv.Quote`, `filepath.Clean/Join/Rel/Base/Dir/Ext/Abs/Match` (byte-level, `ErrBadPattern`), `os.Getwd` (`$PWD`), `strconv.Atoi`, `strings.ToLower/ToUpper` simple case mapping, RE2 regexps (ASCII `\d \s \w \b`, `\Q…\E`, literal `[ & ~` in classes)
  - [x] Registry (`registry.rs`): default root then user root, user wins by id for every kind; grammar and language-data scopeName collapse (user wins); prompt slash collisions (user > default, then bundle id, then prompt id) with skip reasons; role-code collisions; skip reasons for missing/unreadable/invalid manifests; every Go sort order
  - [x] Seeding (`seed.rs`): `seed_defaults` stamp/sync (`.recur-seed` = SHA-256 of `path\0len\0bytes\0`, whole-dir replace on change, loose root files overwritten on change, dotfiles and `tests/` skipped, `.sh` and `bin/` executable) over embedded (`SHIPPED_BUNDLES`, `SHIPPED_SKILLS` via `include_dir!` from repo `bundles/` and `recur-skills/`), on-disk (`DirSource`) or in-memory (`MapSource`) trees; embedded stamps equal the repo-tree stamps
  - [x] TypeScript compile (`compile.rs`): swc in-process instead of the esbuild Go library. TS strip with esbuild defaults (unused/type-only imports dropped, define semantics for class fields), ES2021/2022 lowered for the ES2020 target, ESM → CommonJS, `@recur/sdk` (embedded `macro_sdk.ts`) compiled once and inlined where required; only `@recur/sdk` resolves (imports, `require`, `require.resolve`, dynamic `import()`), "No matching export" for missing SDK names, ES2020 top-level-await error, esbuild's `file:line:col` locations (cwd-relative path); same cache file names and pruning; `tests/compile_parity.rs` runs all 72 `.ts` files under `bundles/` through both compilers (reference: `tests/esbuild_ref`, compile.go's exact esbuild build) across 3320 node executions with identical results. Only syntax-error wording differs from esbuild
  - [x] Shebang execution (`exec.rs`, `env.rs`): process group, stdin input, `RECUR_*` env (stale `RECUR_*`/`PORT` stripped, 64 KiB blob cap with Go byte truncation, settings env + JSON), 30s default deadline, SIGTERM → 500ms → SIGKILL, partial output kept on timeout; prompt/complete/search/expand runners and Go JSON decoding (case-insensitive fields, Go type-error text); `collect_search_groups` concurrent, `expand_matching` skip chain; `RunCtx` is Go's deadline + cancel context
  - [x] Prompt actions (`prompt_run.rs`): TypeScript prompts under node with the Worker boot, shebang otherwise
  - [x] Bundle tests (`testcase.rs`, `testrun.rs`): `tests/*.toml` with inline or `*File` payloads, command XOR snippet, TypeScript macros under node (WORKER_BOOT twin), checks and report text/colors identical
  - [x] Lint (`lint.rs`): schema, grammar/theme/terminal-theme conventions and JSON shape, language data, snippet and command keyEquivalents (parse errors, reserved app chords), shared chords (info), Oniguruma symbol transforms, loop conductor/verifier/role-mode/findings checks, openInPreview, TS compile, shebang `#!`, prompt slash/mode/loop/tab_group/timeout rules, cross-bundle slash duplicates; report text identical
  - [x] Loops/roles/policies (`loops.rs`), grammars/themes/terminal themes (`grammar.rs`), language data (`langdata.rs`), snippets (`snippet.rs`), symbols (`symbol.rs`), settings and secrets (`setting.rs`), scopes and file globs (`scope.rs`), key equivalents (`keyequiv.rs`), language server declarations
  - [x] `bundles test` / `bundles lint` over repo `bundles/` are byte-identical to `bin/recur-cli` (258 passed; 0 errors, 7 info)
- [x] `internal/cli`: 7 files, 830 lines, 2 test files → `recur-cli` (both Go test files ported: `tests/cli.rs`, `tests/env_serial.rs`; `tests/golden.rs` compares against `go build ./cmd/recur`)
  - [x] Cobra/pflag behavior the Go CLI exposed (`cobra.rs`): `Find`/`stripFlags` with lazily defined help/version flags, legacy and explicit arg validators and their messages, pflag long/short/grouped parsing, `--`, `=` values, bool `ParseBool`, every flag error text, help/usage/version rendering, `help [command]`, silenced errors printed red by `exitCode`
  - [x] `completion bash|zsh|fish|powershell [--no-descriptions]` (cobra's templates verbatim) and the hidden `__complete` / `__completeNoDesc` protocol (subcommand, flag-name and help-topic completion, directives, `COBRA_*`/`RECUR_*` env, `BASH_COMP_DEBUG_FILE`)
  - [x] Viper config (`config.rs`): `--bundles-dir` > `$RECUR_BUNDLES_DIR` > first `~/.recur/config.{json,toml,yaml,…}` or `config` parsed as YAML > `~/.recur/bundles`
  - [x] `resolve.rs`, `create.rs` (embedded `internal/cli/scaffold`, Go replacer and modes), `open.rs` (recur-platform `openipc` client, retry on shutdown, launch via `$RECUR_APP` / `open -b build.recur.next` / `open -a "Recur Next"` / desktop binary, Go `net` error text), `color.rs` (`NO_COLOR`, `FORCE_COLOR`, char-device check)
  - Data-dir paths (`paths.rs`: `RecurNext`, `$RECUR_DATA_DIR`, socket, bundle roots) and `CanonPath` live in the CLI until recur-store / recur-git land
- [x] `internal/cliinstall`: 3 files, 568 lines, 3 test files (`recur_platform::cliinstall`, 25 tests)
  - [x] `bundled_cli`: `.app/Contents/MacOS/recur-cli` on macOS, sibling `recur-cli` on Linux
  - [x] `is_dev_bundle`: `*.dev.app`, Go checkout `bin/` (`go.mod`), plus a cargo `target/` dir in a workspace checkout
  - [x] `is_bundled_cli_path`, `ensure_link` (atomic `.tmp` + rename, same-file no-op, refuses regular files and foreign links)
  - [x] `destinations`: writable `/usr/local/bin` first, then `~/.local/bin`; link name `recur-next` side by side
  - [x] `install_from_app` / `install_bundled_cli`: no-op for dev builds, unbundled exes and `/opt/recur-next` package installs; Linux desktop entry (`build.recur.next.desktop`, Exec quoting, unchanged-file skip)
  - [x] `ensure_path`: login-shell PATH (marker-wrapped probe, fish syntax, 5s timeout with process-group kill, `$SHELL` → `id -P` → `/etc/passwd` → default shell), common user/system bin dirs, path_helper fallback, merge order and dedup
- [x] `internal/code`: 18 files, 4497 lines, 7 test files → `recur-code` (all 7 Go test files ported; 165 unit tests, plus `tests/golden/go_code.json` recorded by `gen_go_code.go.txt`: outline over 25 files in 20 languages, search with regex/glob/paging, defs/refs, Quick Open ranking, exclude matchers and rg globs, `filepath.Match` globs, SKILL.md frontmatter, explorer tree, file listing)
  - [x] Roots and sandbox (`resolve_root`, `resolve_root_info`, `safe_join`, `rel_slash`), `Error::{NotFound, WorktreeMissing}` keeping the `not found: <rel>` / `worktree is not a directory` texts the UI sniffs, Go `*PathError` text
  - [x] Files (`read_file` inline cap, binary probe with split-rune trim, image/PDF/video MIME and caps, `prepare_serve`, `prepare_external`, `prepare_chat_image` with magic bytes, `rewind_binary_check`, atomic mode-keeping `write_file`, `image_at_base` through recur-git), dropped-path allowlist with persistence and LRU eviction (`drop_allow`), language via `recur_chroma::lexer_name`
  - [x] Tree (`list_tree`, `list_trees` sharing one status and check-ignore pass, deleted-path injection, symlinked dirs), `git_status`, `file_diff`, `file_at_base`; ops (create, mkdir, rename, remove, copy, `abs_path`)
  - [x] Search and replace (Go RE2 classes on the regex crate, `(?i)`, byte columns, 256 KiB skip count, symlink escape check, offset paging, `filepath.Match` globs, Go's case-fold replace), outline (all language rules), symbol index (`find_defs`, `index_symbols` one walk, `find_refs`, `changed_symbols`, `changed_defs`, `rank_refs_in_package`, `extract_idents`), `list_files` / `match_files`
  - [x] File index (`FileIndex`, `IndexHub`): byte budget with LRU eviction and a pinned workspace, coalesced and superseding builds on tokio tasks, partial batches, dirty-sequence race guard, file-system splices, 300 ms debounced publishes, `files.exclude` fast matchers and rg globs, `rank_files` / `rank_files_and_dirs` / `rank_hits`
  - [x] Skills discovery (`list_skills`, `list_skills_for_menu`, `skill_by_name`, `read_skill_body`, `skill_disables_model_invocation`, `list_skill_resources`)
  - [x] Open (`open_in_editor`, `reveal_in_finder`, `list_open_in_targets` with 64 px NSWorkspace icons as PNG data URLs via objc2, `open_in`)
- [x] `internal/codex`: 6 files, 1173 lines, 5 test files → `recur-provider-codex` (all 5 test files ported)
  - [x] `types.go` → `types` (wire structs with Go field order and `omitempty`, `format_plan`)
  - [x] `rpc.go` → `rpc` (newline JSON-RPC without `jsonrpc`, ordered reader task, sync notification handler, server requests on tasks with `-32000` errors, pending calls fail `-32001` on EOF, `null` results decode as defaults, `Ctx` = Go context cancel + deadline). After EOF new calls fail fast instead of waiting on their context
  - [x] `client.go` → `client` (account/read, model/list, thread start/resume, turn start/steer/interrupt). `DecodeNotification` (unused in Go) not ported
  - [x] `server.go` → `server` (Recur `CODEX_HOME`, seeded `auth.json` only when newer, trusted-project entry, `app-server -c …` in its own process group, env scrub + `CODEX_HOME`/`CODEX_API_KEY`, 45 s handshake, 80-line stderr ring, auto-approve failsafe, SIGTERM → 2 s → SIGKILL close)
  - [x] `locate.go` → `locate` (`$RECUR_CODEX`, Settings path, PATH, Homebrew cask dirs; Mach-O/ELF magic rejects the npm shim; `home_dir`)
  - [x] `mcp.go` → `mcp` (`-c mcp_servers…` overrides, TOML escaping, automation defaults incl. skills/multi-agent off)
- [x] `internal/copyfiles`: 1 files, 414 lines, 1 test files → `recur_code::copyfiles` (`parse_patterns`, `matches`, `preview`, `copy`; cancel token plus deadline for Go's context; test file ported, plus `tests/golden/go_copyfiles.json`)
- [x] `internal/cursor`: 12 files, 3691 lines, 17 test files → `recur-provider-cursor` (all 17 test files ported; smoke files env-gated as in Go)
  - [x] `connect.go` → `connect` (Connect/JSON unary with 60 s bound, SteerRun unbounded, 5-byte framed server streams, end-stream errors, 64 MiB caps, Connect faults logged to the bridge log, `ConnectError` predicates, `bridge_unreachable` / `active_run_conflict` on the wrapped error)
  - [x] `client.go` → `client` (every sdk.v1 call, wire shapes byte-identical to Go's `json.Marshal`: oneof MCP wrappers, repeated `cwd`, explicit `enabled:false`, nested SdkImage, `enableDeltas`; `ProtoTime`; `run_created_on_this_bridge` with the 1 s slack)
  - [x] `events.go` → `events` (nested/flat payloads, snake/camel keys, `flexInt`, call-id fallbacks, MCP labels, result error rules, interaction types; Go's typed-decode strictness: a mistyped frame is skipped, `null` is zero, structs reject arrays)
  - [x] `bridge.go`, `bridge_log.go`, `bridge_reap.go` → `bridge` (process group, single reaper, SIGTERM → 2 s → SIGKILL, ready-line discovery with validation, auth token file, env scrub + SHELL pinning, project dirs, redacted 40-line ring + rotating `cursor-bridge.log`, pidfile, orphan reaping via `ps` that never touches probes or live siblings)
  - [x] `attribution.go` → `attribution` (preload, entrypoint import, SteerRun server hook, createLocalExecutor anchors for 1.0.26–1.0.38, async skills stub, summary onDelta rewrite with marker, atomic writes); anchors, preload and pin compared byte for byte with Go in `golden_tests` (R13)
  - [x] `install.go` → `install` (pinned PyPI wheel, filename matching, sha256, zip-slip guard, mode-preserving extract, staged swap, `cursor.install` payloads; `check_pin_drift` for `check:cursor-sdk-pin`)
  - [x] `locate.go`, `manifest.go`, `projectdirs.go`, `context.go` → `locate`, `projectdirs`, `context`
- [x] `internal/domain`: 15 files, 4945 lines, 16 test files → `recur-domain` (all 16 Go test files ported as unit tests)
  - [x] Serde parity: every one of the 107 exported structs decodes Go `json.Marshal` output (zero and fully populated) and re-encodes byte for byte (`tests/go_parity.rs`, `tests/fixtures/go_types.json`); the e2e `seed.json` round-trips (`tests/seed_round_trip.rs`)
  - [x] `omitempty` → `skip_serializing_if`; non-`omitempty` slices/maps are `Option<…>` so Go's nil still encodes as `null`; `json::to_string` matches `json.Marshal` (HTML-safe escaping, compacted raw JSON, integral floats without `.0`)
  - [x] String enums with an `Other(String)` escape hatch (empty Go zero value is `Default`): AutonomyLevel, WorkspaceState, Group, AgentRole, Severity, FindingStatus, MergeMethod, UpdateBaseMethod, SquashCommitMessage, MergeCommitMessage, PrSource, ArtifactKind, BranchPrefixMode, IdType
  - [x] Plain string const groups kept as `&str` consts: chat modes, plan step statuses, feed kinds/cards, finding actors, attention reasons, review states, update-workspace statuses, workspace type markers, providers, stream statuses, block types, context modes, compact statuses, briefing buckets/reasons/actions, MCP ids/transports/URLs, permission modes, child-workspace-model values, Linear setting ids
  - [x] `workspace`: Workspace (HasRun, GoalLabel, ReplaceGoal, IsPRChatHost), WorkspaceType, RunActive, IssueRef, ChildFanOut, Checkpoint, ConvPoint, DiffStat, WorkspaceLandState, UpdateWorkspaceResult, Resource
  - [x] `plan`: Plan Text/Board/Progress/Ready, PlanStep, PlanAttachment
  - [x] `project`: Project, ProjectGroup, BranchChoice, CopyFilesPreview, OpenPathResult, FilesProjectID, RunCommand with EffectiveRuns/DefaultRun/VerifyDevCmd/RunsKey (name uniquifying)
  - [x] `merge`: Normalize/Effective for merge method, update-base method, squash and merge-commit message formats; MergeMethodPermitted; labels
  - [x] `agent`, `finding`, `escalation`, `artifact`, `policy`: Agent, AgentStep, FeedItem, StoredEvent, Finding (PublicID, MatchesRef, CoversLine), FindingLoc, ValidFindingStatus, FindingNote, Escalation (legacy `applied` decode), UserQuestion(+Answer), PermissionRequest/Decision, Artifact, Rail, Policy, InboxItem
  - [x] `pr` + `reaction`: PR permission rules (edit details, approve, edit/delete/react comment, resolve thread, conversation lock), PRComment shapes, Commit, FileDiff(+Result), DiffLine, PendingLineComment, participants, reviewers, VerifyStep, Check; reaction normalize/GraphQL mapping/toggle/sort/mine/equal
  - [x] `session`: Session, SessionContext, Message, ContentBlock constructors and typed decoders (Go null-tolerant), ParentToolUseID, WithParent (sorted keys like Go), payload types, chat modes, WorkspaceChatTab, AttentionCounts, SessionView, ChatItem, ChatAttachment, ChatImageRef
  - [x] `transcript`: TranscriptXML for messages and transcripts (tool-result folding, assistant merge, CDATA rules, omit thinking, clip tools, max-bytes oldest-first dropping, image root via `filepath.Join` clean)
  - [x] `settings` + `providers` + `compatible` + `mcp` + `skills`: every Settings field and method (models/flags per mode, loop role aliases, bundle value migration, turn timeout clamp, governor caps, permission mode, branch prefix incl. legacy modes, provider enable/offer/normalize/remap/catalog fallback), compatible providers (slug rules, `net/url.Parse`-exact base URL validation checked against Go in `tests/go_rules.rs`, header/model/flag normalization, presets, wire allowlist), MCP catalog/validation/normalization/Linear URL, skill roots and override rules
  - [x] `schedule`: ScheduledPrompt(+Run), OnStartup, NextScheduledTime with a robfig/cron v3 port (seconds-optional fields, names, steps, descriptors, `@every`, `CRON_TZ=`/`TZ=`, five-year limit) and Go `time.Date` DST resolution on embedded tzdata (chrono-tz); 2,844 Go-computed cases in `tests/go_rules.rs`
  - [x] `briefing`, `observatory`, `model`, `ids`: briefing snapshot types and codes, observatory/search/metrics types, ModelRef (ParseModelRef, String, IsZero, NewModelRef), ProviderLabel, BuiltinProvider, NewID/NewUniqueID/ValidID
- [ ] `internal/engine`: 88 files, 47557 lines, 106 test files
  - [x] Slice A (agent contracts) → `recur-agent` + `recur_provider_claude::{stream, testing}`; Go-generated golden fixtures in `crates/agent/tests/golden/go_agent.json` and `providers/claude/tests/golden/go_stream.json` (generators committed as `.go.txt`)
    - [x] `runner.go` → `recur_agent::{turn, event}` (TurnRequest, SessionRoute, McpAttach, `standing_in_user_prompt`, AgentEvent for every ParsedKind, TurnResult with Go JSON tags, McpServerStatus); `TurnMirror` and `Inject*` become `TurnContext` sink/steer
    - [x] `toolview.go` → `Provider::{display_tool, display_tool_result}` + `recur_agent::display::{display_tool_result, summarize_display_input, canonical_claude_tool}`; the Codex and Cursor mappings are P1/P2 impls
    - [x] `agenterr.go` classification → `recur_agent::error` (transient markers, `classifyAgentError` incl. the provider-set transient kind, `agentFailureReason`, `knownAgentError`, `tidyAgentError`, CLI log prefix, `firstLine`, `term.StripVT` byte for byte, Codex login sentence). `turnError` / `transientFailure` are E3 (`has_transient_marker` is their scan)
    - [x] `attachments.go` markers → `recur_agent::attach` (marker, parse/strip/ensure, UserImage, `userContentBlocks` incl. foreign markers and the image-first hidden sentinel, marker order, ordinals, id sanitizing, MIME table). Disk I/O is E2; Claude/Cursor instructions are P1/P2
    - [x] `permission.go` labels → `recur_agent::permission` (prompt tool name, Recur tool labels, label, summary). Broker is E2; Claude flags are P1
    - [x] `streamjson.go` → parser in `recur_provider_claude::stream`; display helpers, limits and listing compactors in `recur_agent::display` (with `createPlanBody`/`isCreatePlanTool` from `cursortools.go`, which both need)
    - [x] `scripted.go` → `recur_agent::testing::ScriptedProvider` + `recur_provider_claude::testing::{NdjsonScript, fixture_lines}` (`testing` features)
    - [x] `mirror.go` `contextUsageTracker` / `preferContextUsage` / `contextUsedTokens` → `ContextUsageTracker`, `TokenUsage::{prefer_context, context_used}`
    - [x] `loop.go` steer text used by providers → `recur_agent::Steer` + `turn::{format_steer_user_message, follow_up_text}`
    - [x] Tests: `streamjson_test.go`, `standing_test.go`, `context_usage_test.go` (parser + tracker cases), `permission_test.go` (label/summary cases), `attachments_test.go` (marker/content-block cases)
  - [x] Slice F (forge): `recur-forge` with the `Forge` trait, `github::GitHub` and `testing::FakeForge` (replaces `listGitHubPRsFn`, `githubLoginFn`, `githubPRViewStateFn`, `ghCLIAvailableFn`, `probeGHCLIFn`, `getGitHubPRSnapshot`, `getGitHubPRChecks`, `postGitHubPRCommentFn`, `submitGitHubPRReviewFn`, `editGitHubPRFn`, `editGitHubPRCommentFn`, `deleteGitHubPRCommentFn`, `replyGitHubPRCommentFn`, `setGitHubReviewThreadFn`, `setGitHubReactionFn`, `githubViewerFn`, `setGitHubPRDraftFn`, `mergeGitHubPR`, `fetchGitHubMergeFieldsFn`, `fetchGitHubBranchRequiresPRFn`)
    - [x] `github.go` forge half: gh availability (30 s cache), login, create (title sanitize 256 → retry 80 on "title is too long", push error kept, head lookup by `pr list`), push head (fork URL, lease against `refs/recur/pr/N`), merge (REST with mergeability retries, GraphQL, CLI, merge queue / `enqueuePullRequest` fallback), failure notes and diagnostics, checks (GraphQL rollup with rulesets/protection/queued suites, CLI fallback, Checks-tab links, durations), branch rules probe, owner/repo parsing, retarget base, merge fields
    - [x] `github_prs.go` forge half: `gh pr list` (open, merged by update time), rollup checks, participants, reviewers, state probe; forge-neutral rules in `recur_forge::model` (attention reasons, state/group, merge-field stickiness, behind, check reconciliation, compact ages)
    - [x] `github_thread.go` forge half: view/inline/timeline parsing, node ids and permalinks, avatars, review nesting and reply-only hiding, review threads, commit rollup, mentionable users, every mutation, comment merging (`merge_comments`); conversation state/attention, `comments_equal`, `commits_equal`, commit enrichment in `model`
    - [x] `github_snapshot.go`, `github_reaction.go` forge half, `pr_participants.go`, `pr_reviewers.go`
    - [x] Linear half of `user_mcp.go` → `IssueTracker::{mcp_preset, agent_mcp, agent_hint, secret_env}`; `linearIssueMeta` → `recur_issues::goal_meta`
    - [x] Tests: forge halves of `github_test.go`, `github_thread_test.go`, `github_reaction_test.go`; `github_snapshot_test.go`, `github_mentions_test.go`, `pr_participants_test.go`, `pr_reviewers_test.go`, Linear cases of `user_mcp_test.go`; `github_snapshot_live_test.go` as `#[ignore]` env-gated tests; fake-`gh` runs of every command path; Go-generated golden fixtures (`crates/forge/tests/golden/github.json`: queries, a full snapshot + timeline, checks, list rows, merge messages)
  - [x] Slice P1 (Claude + Codex providers) → `recur-provider-claude`, `recur-provider-codex`; Go-generated golden fixtures in `providers/{claude,codex}/tests/golden/` (generators committed as `.go.txt`)
    - [x] `claude.go` → `ClaudeProvider` (`runner`, `args`, `input`): argv incl. stream-json input for steers/images/large prompts, `--append-system-prompt-file` spill over 64 KiB, inline `--mcp-config` + `--strict-mcp-config`, env scrub, process group with single reaper and SIGTERM → 2 s → SIGKILL, 8 KiB stderr tail, Go error wording for no-result exits, steers flushed at the next `tool_result` (plain steers ack after the write, conductor steers on receipt, pending plain steers flushed at the result or nacked on unwind), stdin closed on the result, background-task result hold with the 10-minute ceiling
    - [x] `claude_flags.go` → `flags` (`[1m]` rules, context windows, `has_context_flag`, labels, `--effort`)
    - [x] `permission.go` Claude flags → `args` (`permission_args` with the `dontAsk` fallback, `disallowed_edit_tools`, `disallowed_tools`); allowlist translation documented on the module
    - [x] `attachments.go` Claude half → `input` (content blocks at markers, attachment-id instruction)
    - [x] `codex.go` → `CodexProvider` (`runner`, `items`, `approvals`, `display`): per-turn app-server, thread start/resume fallback, developer instructions vs in-band append notes, notifications to events behind a closing emit gate, `turn/steer` (30 s), interrupt on cancel, usage meter (last request vs thread-total delta), wall-clock duration floored at 1 ms, error results without turn-end duration, sandbox/approval policy, approvals routed to `PermissionAsker` (elicitations auto-accept, `dontAsk` declines), file-change enrichment, Codex display boundary
    - [x] Tests: `claude_flags_test.go`, `claude_inject_test.go` (fake `claude` scripts, plus steer delivery, group kill, no-result errors), `codex_test.go`, `codex_smoke_test.go` (`RECUR_CODEX_E2E`-gated), Claude cases of `permission_test.go` and `models_test.go`, `mcpConfigJSON` cases of `user_mcp_test.go`, the Claude content case of `attachments_test.go`, the spill case of `planbrief_test.go`; Codex turns, probes, approvals and crashes against fake app-server scripts. `permission_wiring_test.go` asserts an engine invariant over real loop turns (E2/E3); `TestRealClaudeSmoke` and `permission_e2e_test.go` drive the engine and MCP server (E1/E2)
    - [ ] `Provider::usage_probe` for Claude and Codex: deferred edge until `recur-usage` (P3) lands
  - [x] Slice P2 (Cursor provider) → `recur-provider-cursor`; Go-generated golden fixtures in `providers/cursor/tests/golden/` (generators committed as `.go.txt`), including full frame-fold replays
    - [x] `cursor.go` → `runner` + `turn` + `steer`: create/resume with fresh-agent fallback, synthetic init, role instructions in-band, subagent overlays, plan mode for read-only allowlists, SteerRun first then CancelRun + one joined follow-up Send with the CANCELLED result suppressed (R6), Stop's once-only CancelRun, active-run recovery (observe-to-idle on offsets, cancel + retry), frame folds (one step/result per call, unreported calls closed at the next model boundary, chunked text and reasoning), usage meter scaling and compaction, deferred turn end so live steers precede it, error hints, CloseAgent on every exit
    - [x] `cursor_flags.go`, `cursortools.go`, `wrapAttachedImagesForCursor` → `context`, `tools`, `runner`; createPlan also emits `AgentEvent::PlanProposal`
    - [x] `provider.go` Cursor half → `pool` (per-worktree bridges, LRU cap 8 sparing reserved runners, key/binary change teardown, cold-start patch + attribution config, capability check, deferred close) and `CursorProvider::run_turn` (pre-stream self-heal retry once, "Cursor is turned off" wrap)
    - [x] Tests: `cursor_test.go`, `cursor_flags_test.go`, `cursor_steer_test.go`, `cursor_active_run_test.go`, `cursortools_test.go` (mapping, display and frame cases), pool and Cursor router cases of `provider_test.go`, `cursor_smoke_test.go` (`RECUR_CURSOR_BRIDGE` + `CURSOR_API_KEY`). The transcript cases of `cursortools_test.go` (`flattenTranscript`, `sessionBroadcast`) belong to E2
    - [ ] `Provider::usage_probe` for Cursor: deferred edge until `recur-usage` (P3) lands
  - [x] Slice P3 (compatible provider) → `recur-provider-compatible`; Go-generated golden fixtures in `providers/compatible/tests/golden/` (generators as `.go.txt`)
    - [x] `harness_prompt.go` → `prompt::RECUR_CODING_PROMPT`, byte-identical (golden)
    - [x] `harness_history.go` → `history` (instructions: prompt, environment, AGENTS.md/CLAUDE.md regular files only with the 32 KiB cap, overlays; transcript replay after the fresh_session cutoff, unfinished rows and the current user row skipped except for hidden continuations, provider call ids kept, stored images reloaded from the data dir, the 8 MiB cap dropping whole call groups); Go-matched input JSON for every history case
    - [x] `harness.go` → `CompatibleProvider` (`harness`, `tools`, the full coding toolbox incl. Glob and Grep): rounds with flag wiring onto the sorted Go-encoded body (request bytes match Go exactly), streamed vs final text per item, reasoning to thinking, terminal-status checks, read-only calls in parallel (8) then the rest in order, halting tools, `max_turns`, steers between rounds, usage and context window on the result, cancellation with the partial result. `HarnessTools` is the in-process `ToolHost` (allowlist, `permission_request` hidden, Bash gated through `Host::request_permission`)
    - [x] Tests: `harness_test.go` (engine-only cases noted below), `app/compatible_service_test.go`, plus a full two-round run replayed against Go's recorded wire bodies and events
    - `TestRegisterMCPSkipsCompatible`, `TestResolveRunnerCompatible`, `TestMirroringRunner*` and `TestHarnessAskUserHalts`'s `PendingAskUsers` assertion exercise the engine (E2/E4); the harness half of each is covered here
  - [x] Slice E1 (engine core) → `recur_engine::{core, bus, governor, chatlife, loop_handle, loop_registry, publish, attention, workspace, fswatch, roles, guard, host, registry, terminals, view, testing}`; Go-generated golden fixture `crates/engine/tests/golden/go_e1.json` (generator `gen_e1_golden.go.txt`): every event payload, prompts, tool tables, kitchen words, synthetic guards. Calls into E2–E5 go through the named hooks in `deferred.rs` (listed in the plan, E1 "As built")
    - [x] `bus.go` → `bus` (`EngineEvent` for every event of plan §3 with Go-identical JSON, `Bus::{subscribe, subscribe_fn, publish}`: never drops, per-publisher order)
    - [x] `governor.go`, `keepawake.go` → `governor` (loop/agent caps, wake one on release / all on cap change, cancellable agent wait, iters) and `AgentPermit` (keep-awake sync, provider `on_idle` at zero)
    - [x] `chat_cancel.go` → `chatlife` (turn registry with swap/if-idle/unregister, FIFO tab slots + stop generation, ref-counted async life locks, continue generations, park/stop/timeout flags, armed deadline, steer channel with acked delivery, delete/hoist tombstones, `cancel_chat`)
    - [x] `chat_inbox.go` → `attention::chat_inbox` (rows, `parse_chat_inbox_id`, Mark all read, boot heal, Loop finished); engine focus/unseen/attention publishers and `flagChatUnseen` → `attention`
    - [x] `diff_sync.go`, `branch_sync.go`, `workspace_id.go`, `ws_hooks.go` → `workspace::{diff_sync, branch_sync, ids, hooks}`
    - [x] `fswatch.go`, `fswatch_darwin.go`, `fswatch_fsnotify.go` → `fswatch` (+ `backend` on `notify`: recursive FSEvents on macOS, per-directory inotify with tree add/remove elsewhere, one latency batcher)
    - [x] `roles.go`, `role_tools.go`, `policy.go` → `roles`, `roles::tools`, `guard` (prompts byte-identical, golden-checked)
    - [x] `engine.go` E1 half → `core` (state, construction, MCP start, git alert, caps), `publish`, `lifecycle` (Pause, Stop, Shutdown, Recover incl. its loop half), `workspace::{teardown, main_copy}`
    - [x] `loop.go` control plane → `loop_handle::LoopHandle` (commands, pause/stop/escalation waits, steers, grants, done summaries, findings, evidence; branch under the lock for R18)
    - [x] `reporter.go` loop registry + E1 Host methods (`report_finding`, `done`, `list_findings`, `update_finding`, `report_result`, `escalate`, `attach_evidence`) → `loop_registry`, `host::EngineHost`
    - [x] `workspace_create.go` E1 half, `workspace_remote.go` E1 half → `workspace::{create, remote}`
    - [x] Tests: `governor_test.go`, `keepawake_test.go`, `chat_inbox_test.go` and `attention_test.go` (minus E2/E4 cases), `chat_cancel_test.go` kernel cases, `diff_sync_test.go`, `fswatch_test.go`, `branch_sync_test.go` (minus Open PR), `workspace_create_test.go` allocate cases, `workspace_id_test.go` (minus dispatch), `setup_teardown_test.go`, `policy_test.go`, `roles_test.go`, `role_tools_test.go`, the hand-workspace halves of `lifecycle_test.go` and `engine_test.go`; `fixture_test.go` scaffolding → `testing`. Loop-driven cases move to E3 (see the plan)
    - [~] Deferred hooks filled by E2–E5 (`deferred.rs`): E2's `ask_user_pending_for` and `clear_ask_users_for_workspace` are real (`ask_user.rs`); E3's `drain_queued`, `emit_settled_wrap_up`, `retire_open_escalations` and `maybe_dispatch_after_allocate` are real (`loops`); every E4 hook, including those E2 and E3 added, is real (`chat`, `app_chat`, `briefing`, `feed`); E5 pending (plan, E4 "As built")
  - [x] Slice E2 (turn pipeline and sessions) → `recur_engine::{turn, composer, permissions, ask_user, attachments, artifacts, plan, simplify, renames, mcp_attach, host::{backfill, chat}, view::{chat, mentions, composer, prompts, artifacts, live, digest}}`; Go-generated golden fixture `crates/engine/tests/golden/go_e2.json` (generator `gen_e2_golden.go.txt`): constants and sentinels, one-shot prompts, composer wraps and mention collectors, formatters, the plan card. As-built API, deviations and moved tests are in the plan (E2 "As built")
    - [x] Router half of `provider.go` → `turn` (`run_agent`, `resolve_provider` with no fallback, the pin, Settings → MCP, partials, tracker secret scrub, `register_mcp` / `unregister_mcp` with the in-process turn token, `ensure_mcp` retried after a failed start); R7 guard `tests/r7_guard.rs`
    - [x] `mirror.go` → `turn::{mirror, broadcast}` (`SessionMirror` record_user/event/finish, step dedupe, steer resets, fallback/retract/supersede, compaction context, `SessionBroadcast` thinking seq and nested buffers, context usage) and `TurnSink`; `PermissionAsker`/`HistorySource`/`ToolHost` per the provider's caps (`turn::tools::InProcessTools`)
    - [x] `workspace_chat.go` transcript + session-config half → `turn::{sessions, transcript, stream}` (`ensure_agent_session`, routed and owned sessions, chat selection/mode/loop helpers, pair and Maestro agents, `flatten_transcript`, `workspace_chat_items`, `session_chat_items`, last/first chat prompt, loop goal images, `ChatStream` with the Plan harvest)
    - [x] `chat_errors.go` → `turn::finalize` (sentinels, `format_chat_agent_error`, `finalize_chat_reply`, `persist_chat_failure`, failure duration)
    - [x] `checkpoint.go` → `turn::checkpoint` (take/mark checkpoints, `chat_turn`, `workspace_chat_turn_diff`, `reset_workspace_chat`, `review_checkpoint`, `edited_after`)
    - [x] `commit_message.go` → `turn::oneshot` (commit message and PR description prompts and parsing, diff caps, `loop_commit_message`, one-shot deadline)
    - [x] `composer.go`, `raw_regions.go`, `file_mentions.go`, `skill_mentions.go`, `terminal_mentions.go`, `artifact_mentions.go` → `composer::{raw, files, skills, terminals, artifacts}` (`expand_composer`, prompt actions, wraps; skill guidance and comment hygiene)
    - [x] `artifacts.go`, disk half of `attachments.go` → `artifacts`, `attachments` (MarshalIndent metadata, Go base64, hoist with rollback; image storage and lookup)
    - [x] Brokers of `ask_user.go` and `permission.go` → `ask_user`, `permissions` (park/answer/resume claim, 10-minute permission timeout, inbox rows for loop surfaces); the resume send is the E4 hook `send_ask_user_resume`
    - [x] `plan.go`, `simplify.go`, `rename_session.go`, generic `user_mcp.go`, `mcp_tool_backfill.go`, `reporter.go` E2 methods → `plan`, `simplify`, `renames`, `mcp_attach`, `host::{backfill, chat}`, `EngineHost`
    - [x] Tests: `mirror_test.go`, `thinking_broadcast_test.go`, mirror cases of `context_usage_test.go`, `chat_errors_test.go`, `checkpoint_test.go`, `composer_test.go`, `raw_regions_test.go`, `file_mentions_test.go`, `skill_mentions_test.go` (HOME cases in `tests/env_serial.rs`), `skills_settings_test.go`, `terminal_mentions_test.go`, `artifact_mentions_test.go`, `artifacts_test.go`, `attachments_test.go` disk cases, broker cases of `ask_user_test.go` / `permission_test.go`, `permission_e2e_test.go` (`RECUR_E2E=1`), `mcp_required_test.go` (minus loop/send cases), `mcp_tool_backfill_test.go`, `commit_message_test.go`, `plan_test.go`, `simplify_test.go` (flag rules), `user_mcp_test.go` generic cases, `workspace_chat_items_test.go`, `TestExpandComposerFreshSession`, router cases of `provider_test.go`, `feedkind_test.go`, the rename cases of `workspace_chat_test.go`; send- and loop-driven cases move to E3/E4/E5 (plan)
  - [x] Slice E3 (loops) → `recur_engine::{loops, chat_shared, view::{loops, loop_cockpit}}` and the E3 `EngineHost` methods (`plan`, `edit_plan`, `rerun_review`, `dispatch_specialist`, `extend_iterations`, `extend_rounds`, `reenter_phase`, `run_phase`, `resume_loop`); Go-generated golden fixture `crates/engine/tests/golden/go_e3.json` (generator `gen_e3_golden.go.txt`): loop errors and prompts, briefings, PR fallback text, wrap-up, title rules, child step plan, and escalations plus Maestro feed lines from real walks. As-built API, hooks, deviations and moved tests are in the plan (E3 "As built")
    - [x] E3-M1: `queue_start_workspace`, `start_run_on_workspace(_with)`, `dispatch_additional_chat_run`, `resume` / `resume_with_owner`, `send_maestro_message(_to)`, `send_loop_chat_message`
    - [x] `walker.go`, `loop.go` data plane, `loop_prompts.go`, `agenterr.go` `turnError` / `transientFailure` → `loops::{walker, converge, specialists, prompts, failure}` and `loops::Walk` (every verb, retries and `on_fail`, the converge rounds with caps and grants, specialists over `run_agent` with the real MCP, commit, plan gate, steers, checkpoints, per-turn policy/phase timeouts)
    - [x] `escalation.go`, `fanout.go`, `board.go`, `handoff.go`, `verify.go` (boot via `runconfig`, QA evidence), `rails.go`, `resume.go` (resume points, Restart, Retry, AutoResume) → `loops::{escalation, fanout, board, handoff, verify, rails, resume}`
    - [x] `maestro.go`, `maestro_hands.go` → `loops::{maestro, hands}`; loop engage half of `workspace_chat.go` → `loops::engage` and `chat_shared`; E3 halves of `engine.go` and `workspace_create.go` (start, dispatch, resume, drain, `loop_exited` wiring, `BeginStartWorkspace*`, `maybeDispatchAfterAllocate`) → `loops::lifecycle`; `Loop.openPR`, wrap-up, PR-created feed and inbox of `pr.go` → `loops::{open_pr, wrap_up}`
    - [x] Tests (262, two env-gated): `walker_test.go`, `default_walk_test.go`, `converge_test.go`, `findings_checklist_test.go`, `escalation_test.go`, `fanout_test.go`, `handoff_test.go`, `planbrief_test.go`, `loop_prompts_test.go`, `maestro_test.go`, `maestro_hands_test.go`, `loop_engage_test.go`, `resume_test.go`, `restart_test.go`, `verify_test.go`, `rails_test.go`, `m8_test.go`, `acceptance_test.go` and `real_e2e_test.go` (loop halves; `RECUR_E2E=1`), and the cases E1/E2 moved here (`engine_test.go`, `lifecycle_test.go`, `harden_test.go` loop halves, dispatch cases of `attention_test.go` / `workspace_id_test.go`, `TestStartRunOn*`, `TestBeginStartWorkspace*`, `TestPlanApprovalGate`, `TestLoopSpecialistsAttachLinear`, `TestDeadMCPFailsTheLoopInsteadOfConverging`, simplify loop cases, `pr_test.go` wrap-up cases). Cases through E4's send/close/list and E5's merge/send-back are theirs (plan)
  - [x] Slice E4 (chat and coordination) → `recur_engine::{chat::{send, tabs, fork, modes, surface, switch_mode, recover, children, create_tool, ask_resume}, app_chat, schedules, briefing, feed, view::{modes, chat_surface, chat_tab_session, subagent, new_workspace}}` and the E4 `EngineHost` methods (`switch_mode`, `create_workspace`, `merge_child`, `read_child`, `archive_child`, `message_child`, `read_child_artifact`, `list_workspace_briefs`, `workspace_context`, `present_workspace`); Go-generated golden fixture `crates/engine/tests/golden/go_e4.json` (generator `gen_e4_golden.go.txt`): mode shapes, the switch_mode matrix and prompts, surface peel and subject prompts, child text, the child rule, Ask Recur's prompt and ids, briefing text and full briefs, the fork preamble. As-built API, hooks, deviations and moved tests are in the plan (E4 "As built")
    - [x] E4-M1: `send_workspace_message`, `send_workspace_turn` (`fresh_session`), `create_workspace_chat`, `remember_workspace_chat_selection`, `ensure_pair_agent`
    - [x] Send pipeline and tabs of `workspace_chat.go` → `chat::{send, tabs}` (FIFO ticket, steer with the life lock released (R6), run-lock, retry sentinel, Loop/Maestro routing, standing notes, tab list/reorder/close/reopen/delete, prompt groups, provider remap)
    - [x] `chat_fork.go`, `chat_recover.go`, `chat_surface.go`, `switch_mode.go`, `modes.go` → `chat::{fork, recover, surface, switch_mode, modes}` (fork inject and compaction watermark, Recover/AutoResume of PA*/MA tabs and zombie Stop (R14), the looking-at marker with forge subject prompts (R8), park + remount with the cap, byte-identical mode prompts)
    - [x] `chat_child.go`, `create_workspace_tool.go` → `chat::{children, create_tool}` (watchers, one report turn, wave cards, read/message/merge/archive_child, restart re-arm, the child-or-standalone rule)
    - [x] `schedules.go`, `app_chat.go`, `briefing.go`, `feedproject.go`, resume half of `ask_user.go` → `schedules`, `app_chat`, `briefing`, `feed`, `chat::ask_resume`
    - [x] Tests (286): `workspace_chat_test.go`, `chat_child_test.go`, `chat_fork_test.go`, `chat_recover_test.go`, `chat_surface_test.go`, `switch_mode_test.go`, `schedules_test.go`, `app_chat_test.go`, `briefing_test.go`, `feedproject_test.go`, the ask_user resume case, the five view test files, and the send-driven cases E1–E3 moved here (`chat_cancel_test.go`, `mcp_required_test.go`, `workspace_chat_fresh_session_test.go`, `skill_mentions_test.go`, `simplify_test.go`, `user_mcp_test.go`, `chat_errors_test.go`, `checkpoint_test.go`, `loop_engage_test.go`, `chat_inbox_test.go`, `attention_test.go`, `harness_test.go` remount, feed reconstruction in `engine_test.go` / `acceptance_test.go`). PR-host send cases of `github_prs_test.go` are E5's
- [ ] `internal/git`: 12 files, 4392 lines, 14 test files
- [x] `internal/git`: 12 files, 4392 lines, 14 test files → `recur-git` and `recur-chroma` (all 14 Go test files ported: 159 unit tests across both crates, `ExpandHome`'s in `recur-project`, 3 environment-isolated tests in `crates/git/tests/env_isolated.rs`, 2 Go-parity fixture tests in `crates/chroma/tests/highlight_parity.rs`)
  - [x] Command runner (`command.go`): every git process runs with `GIT_OPTIONAL_LOCKS=0` (inherited and extra values stripped), leads its own process group, gets SIGTERM to the group on cancel (future dropped or `deadline`) and SIGKILL to the leader after the 2s grace, and treats inherited pipes still open after a clean exit (ssh ControlMaster) as success; error text matches Go (`exit status N`, `signal: terminated`, `exec: "git": executable file not found in $PATH`); `output` carries stderr like `ExitError.Stderr`, `combined_output` interleaves through one pipe
  - [x] Version (`version.go`): `parse_version` (Apple/windows suffixes, `2.45.GIT`), `MIN_VERSION` 2.31, `probe` once per process with a 3s timeout, `message_if_unsupported`, `unsupported_alert` titles, `--diff-merges=first-parent` gating
  - [x] Worktrees (`worktree.go`): `Manager` with branch prefix (default `recur`, explicit none), `add` (unique `prefix/slug[-N]` from one `branch --list`, D/F-conflict refusal, stale registration and moved/orphan directory repair, unborn-repo initial commit as the git identity, remote base fetch with 15s sub-timeout and stale-tip fallback), `add_at_ref` (fast-forward ancestor, fallback on diverged or checked-out), `restore` (recorded branch, historical `recur/<slug>` default), `try_set_upstream`, `remove` (managed-root guard with symlink-resolved prefixes, locked worktrees, nested project dir pruning), `delete_branch_if_at_base`, `delete_local_branch`, `list`, `commit` (MERGE_HEAD-only commits), `changed_diff`, `merge` (merge/squash/rebase onto the base checkout, remote-tracking bases without DWIM, `DirtyBaseError` with overwritten paths, rebase cleanup that never resets away dirty work), update-with-base begin/finish/abort/autostash/stash-tip/in-progress, `merge_into` (conflicts left in progress), discard risk, `is_repo`/`repo_root`/`canon_path`/`path_has_prefix`/`same_path`/`align_under_root` (macOS case-folded inode checks), workspace id → path, branch sanitizing with `check-ref-format`, `current_branch`, `rename_branch`
  - [x] Branches and remotes (`branches.go`): remotes, remote branches, longest-prefix remote-tracking parse, `compare_ref`, `fetch_compare_ref`, `behind_base`, fetch with prompts disabled, PR heads into `refs/recur/pr/N` (force-updated), upstream fetch/push (HEAD to the upstream branch), `push_branch`, `suggest_base_branch` (+ for any repo path)
  - [x] SCM (`scm.go`): identity from `git var GIT_AUTHOR_IDENT` with no fallback, discard file/all (fresh status, never a coalesced one), stage/unstage with old-git fallbacks, `commit_staged`, staged paths/diff for non-git folders, recent subjects
  - [x] Status (`status.go`, `status_coalesce.go`): porcelain letters with rename/copy and quoting rules; in-flight coalescing of `status_map` and `change_status` per directory (and base), detached 15s work that survives a cancelled caller, no caching after landing
  - [x] Ignore (`ignore.go`): `check-ignore -z --stdin` for any byte in names
  - [x] Diffs (`diff.go`, `working_diff.go`): diffstat vs base with untracked text lines (file/byte caps, binary sniffing), PR diff, working diff vs base (unborn HEAD → empty tree, untracked synthetic adds with path/body/byte caps, omitted rows, truncation flag), commit log with one `git log --numstat` (role/code/iteration/color, author initials with Go's Unicode letter/number/upper tables, relative time), commit diffs with and without `--first-parent`, unified diff parser (header vs content dashes, binary, renames, GitHub-style hunk labels), single-file working diff with staged-only fallback, change letters vs merge-base, file size/stream/contents at a revision (raw bytes)
  - [x] Checkpoints (`checkpoint.go`): private-index snapshots under `refs/recur/checkpoints/`, list (commit and tree oids), delete, diff, restore with untracked cleanup
  - [x] Highlighting (`highlight.go`): `highlight_files` (500-line per-file and 4,000-line per-patch budgets, already-highlighted lines skipped), `lexer_name`, token palette mapping. Go used chroma, so `recur-chroma` ports chroma's lexer engine (registry order and priorities, filename globs via Go `filepath.Match`, include/combined/push/pop, ByGroups/ByGroupNames/Using/UsingSelf/UsingByGroup, coalescing, delegating and remapping lexers, Markdown frontmatter, HTTP bodies, rst code blocks, Haxe preprocessor, the Raku lexer's runtime rule replacement) and the regexp2 .NET regex engine it runs on (parser, writer and backtracking interpreter with variable-length lookbehind, Go Unicode 15 tables, 250ms match timeout). chroma's 297 lexers are vendored under `crates/chroma/data` with `regenerate.sh`; `recur-git` re-exports `highlight_files` and `lexer_name`; all 336 chroma lexer test files match token for token, and Go `highlightLine` output matches byte for byte on 1,779 fixture lines
  - [x] `ExpandHome` is `recur_project::home::expand_home`, re-exported as `recur_git::expand_home`
- [x] `internal/lsp`: 5 files, 916 lines, 4 test files → `recur-lsp` (all 6 Go test files ported: `frame_test.go`, `uri_test.go`, `servers_test.go`, `session_test.go`, `managed/managed_test.go`, `managed/assets_test.go`; Go-generated golden fixtures in `crates/lsp/tests/golden/` with generators as `.go.txt`; env-override cases in `tests/env_serial.rs`)
  - [x] `frame.go` → `frame` (`Content-Length` framing, case-insensitive header, Go error texts); byte-identical frames
  - [x] `uri.go` → `uri` (`canon_path` via symlink resolution, `path_to_uri` with Go `url.URL` path escaping, strict `uri_to_path`), checked against Go on escaping-heavy paths
  - [x] `servers.go` → `servers` (the 17 builtins in order, bundle overrides by key, disabled set, `lookup`/`lookup_all` with file globs and the empty-language glob rule, Vue first, PATH `available`). Go's package-level registry is a `ServerRegistry` owned by the `Manager`
  - [x] `session.go` → `session` (own process group, `KEY=VAL` env extras, stderr to debug log, events in stdout order behind a 64-slot channel, one `lsp.exit` after the last message, `write` rejected after close, `close` SIGTERMs the group without waiting on a blocked write)
  - [x] `manager.go` → `manager` (`ensure`/`ensure_all` with managed installs and soft-degrade reasons, session reuse per workspace+server, TypeScript restart when the Vue plugin appears, Vue install for TS pairing, exit bookkeeping, `write`, `close`, `close_workspace`, `close_servers`, `close_all`); `LspEvent` carries the `lsp.message {id, message}` / `lsp.exit {id}` payloads
  - [x] `managed/*` → `managed` (pins, release URLs and SHA-256 tables Go-matched; env overrides, pinned-version reuse, install then PATH fallback, serialized installs, verified downloads via `.partial`, gunzip and tar extraction, `go install` for gopls, `npm install --prefix` with the shared vscode-langservers prefix and symlinked shims, `vue_plugin_path`). The network gopls install test is `#[ignore]`
  - [x] `app/lsp_service.go` → `recur_lsp::service` (`refresh_registry`, `ensure_lsp` with language fallback, extra sessions and file URIs through `code.SafeJoin` rules, `list_language_servers`, `write_lsp`; `EnsureResult`/`SessionInfo`/`LanguageServerInfo` serde-identical). Root resolution (`resolveRoot`) and the `LSPService` methods stay SV
  - [x] Transport rules of `frontend/src/lib/lsp.ts` → `recur_lsp::protocol` (server-request auto-replies, `initialize` workspace folders and Vue plugin rewrites, the Vue `tsserver/request` bridge messages; the vue tsserver cases of `lsp.test.ts` ported). The editor UI half of `lib/lsp.ts` stays with `recur-editor`
- [x] `internal/mcp`: 6 files, 839 lines, 3 test files → `recur-mcp` (`server_test.go`, `tools_briefing_test.go`, `wrap_test.go` ported in full)
  - [x] `McpServer` on hyper HTTP/1.1: `POST /<token>/mcp`, stable token per agent reused across turns, extra tokens for overlapping registrations, 404 when inactive or unknown, 405 + `Allow: POST`, Go `ServeMux` path cleaning (307), `http.Error` bodies and headers, 202 for notifications, 32 MiB body cap, 10 s header timeout, request cancellation on client disconnect, 3 s graceful close
  - [x] JSON-RPC framing (`protocol`): id echoed verbatim (compacted), protocol version echo, error codes, `isError` tool results; 79 HTTP exchanges recorded from the Go server replayed byte for byte, including the `tools/list` body snapshot (`tests/http_golden.rs`)
  - [x] `names_from_allowed_tools` (never null), `append_permission_request`, 128-bit hex tokens
- [x] `internal/observe`: 5 files, 1030 lines, 1 test files → `recur_code::observe` (`collect_metrics` with FNV clone windows, `compute_impact`, `run_checks` / `run_light_checks` with combined output, 50 s per command and Go's nil-context bounds, `project_findings_onto_diff`, `finding_flag_html`, `analyze`, `analyze_root`); test file ported, metrics and impact JSON byte-identical to Go
- [x] `internal/openipc`: 1 files, 286 lines, 1 test files (`recur_platform::openipc`, 10 tests)
  - [x] Wire format: one JSON request / response, Go tags (`projectId`, `workspaceId`, omitempty), `null` request tolerated, `invalid request` reply
  - [x] `Instance::claim(data_dir)`: live-socket probe, exclusive flock on `instance.lock`, bind `open.sock` 0600; `Held` when another process owns the dir; close unlinks before releasing the flock
  - [x] `serve` never steals a live socket; `serve_listener` runs handlers off the async runtime, unlinks on stop and drains in-flight handlers
  - [x] `call`: 400ms dial timeout, 60s absolute call deadline, dial errors distinguishable (`is_dial`), `retryable` for `shutting down`
- [x] `internal/osfonts`: 4 files, 382 lines, 1 test files (`recur_platform::osfonts`, 7 tests)
  - [x] fontconfig `fc-list :spacing=mono family` (PATH, Homebrew, Cellar glob; 3s timeout) filtered by junk list and mono-name regex
  - [x] Font-dir scan: sfnt header, `name` table family ranking, `post.isFixedPitch` at offset 12, OS/2 PANOSE proportion 9
  - [x] Case-insensitive dedup, first-localized-name cleanup, dot-prefixed system faces dropped, sorted output
- [x] `internal/power`: 1 files, 173 lines, 1 test files (`recur_platform::power::Keeper`, 3 tests)
  - [x] `caffeinate -i -w <pid>` in its own process group, idempotent `set`, `stop`, terminal `close`; no-op off macOS
  - [x] Reaped-state tracking (no signals to recycled pids), SIGTERM then SIGKILL after 2s off the caller's thread
- [ ] `internal/projfile`: 2 files, 466 lines, 1 test files
- [ ] `internal/openipc`: 1 files, 286 lines, 1 test files
- [ ] `internal/osfonts`: 4 files, 382 lines, 1 test files
- [ ] `internal/power`: 1 files, 173 lines, 1 test files
- [x] `internal/projfile`: 2 files, 466 lines, 1 test files → `recur_project::projfile` (test file ported, plus golden cases recorded from Go)
  - [x] Path/Exists/Overlay/Apply: run-config comes only from `.recur/project.toml`; missing file clears it, malformed file sets `ProjectFileError` (`parse .recur/project.toml: toml: …`) and is logged once per mtime; mtime+size parse cache; `~` repo paths
  - [x] Write: repo-path checks with Go's messages, `.recur/` creation, atomic temp-file + rename, fresh files with the standard header encoded exactly like go-toml (literal-first strings, `multiline` fields, `[[run]]` blocks)
  - [x] Comment-preserving merge into an existing file: `recur_project::tomledit` ports go-toml `unstable/edit` (span index, attached comments, insertion points, deletes with trailing-blank absorption, CRLF, inline arrays/tables, validation after every edit) on `toml_parser`, the parser under toml_edit; `[run.N]` healing, `dev_cmd` removal, malformed-file rewrite. 21 edit scenarios and 14 Write outputs replay byte for byte against Go (`tests/fixtures/gen_projfile.go.txt`)
- [x] `internal/responses`: 4 files, 795 lines, 2 test files → `recur_provider_compatible::{client, sse, flags, types}` (`client_test.go`, `flags_test.go` ported; SSE, JSON-fallback and flag cases Go-matched in `providers/compatible/tests/golden/go_harness.json`)
  - [x] Client: stream with JSON fallback and SSE sniffing, 1 MiB error / 32 MiB body caps, 800-byte error clip, bearer key then extra headers, no overall stream timeout and never a compressed stream, ping (16 tokens, `incomplete` accepted, failed/queued rejected, 30 s)
  - [x] SSE: dispatch on a blank line or as soon as the data is valid JSON (LM Studio / llama.cpp), `[DONE]`, Go typed-decode semantics (null = zero, mismatched types drop the event), deltas as string or `{text}`, function calls from items and completed outputs, usage with cached-token fallback
  - [x] `DecodeResponse`: envelope and `{response}` wrap overlay, Responses-object detection, terminal statuses, error events
  - [x] Flags: wire paths, temperature/max tokens coercion with Go float formatting, `reasoning.summary=auto`, `text.format`
- [x] `internal/rg`: 2 files, 403 lines, 2 test files → `recur_code::rg` (`locate`, `files_args`, `files`, `ensure` with the pinned 15.2.0 release, SHA-256 check and tar.gz extraction); both test files ported (`tests/env_serial.rs` for the `$RECUR_RG` cases)
- [x] `internal/runconfig`: 1 files, 333 lines, 1 test files → `recur_project::runconfig` (test file ported)
  - [x] Boot: free or persisted port, `$RECUR_PORT`/`${PORT}` substitution, own process group, single reaper, probe until any HTTP response (2s per request, redirects, no proxy for loopback), exits/timeouts/cancel tear down; the server outlives the boot token and `Booted::stop` is idempotent (SIGTERM, 2s grace, SIGKILL)
  - [x] RunCmd/RunCmdOutput: parent-owned pipe for stdout+stderr, 4 KiB tail, 100ms drain wait without SIGPIPE-ing background children, cancel vs deadline errors (Go's `context canceled` / `context deadline exceeded`), Go `exit status N` / `signal: NAME` text
  - [x] FreePort, Go `Duration.String` for messages
- [x] `internal/shortid`: 1 files, 111 lines, 1 test files → `recur_domain::shortid` (test file ported)
  - [x] New (prefix/length validation, letter-first suffix, unbiased rejection sampling over OS randomness), NewUnique (attempt budget, Collision error), IsValid / VALID_PATTERN, MinLength/MaxLength/DefaultAttempts
- [x] `internal/simplify`: 1 files, 56 lines, 1 test files → `recur_engine::simplify::text` (`NOTICE` embedded from the repo, `applies_to_chat`, `prompt` byte-identical to Go's `Instructions`); test file ported
- [ ] `internal/store`: 9 files, 6229 lines, 6 test files
- [x] `internal/term`: 7 files, 1797 lines, 6 test files → `recur-term`
  - [x] `term.go`: login shell spawn with env/cwd/size, `TERM`/`COLORTERM`/`TERM_PROGRAM`, write, resize clamp, list/reorder/active, titles, detach vs close, close_all, tombstones (max 4), restore trailer, replay seeding (`manager.rs`, `manager_tests.rs`)
  - [x] Coalescing (8ms / 64KiB), flow control with ack and 256KiB high water counted at ingest
  - [x] Busy/idle watcher with backoff; foreground group detection (`process.rs`)
  - [x] `interrupt.go`: SIGINT, then SIGTERM, then SIGKILL escalation over the process tree (`process.rs`)
  - [x] `ring.go`: 1MiB replay ring (`ring.rs`)
  - [x] `cwd.go`: lsof / `/proc` cwd with timeout (`cwd.rs`)
  - [x] `osc7.go`: OSC 7 cwd tracking with local host check (`osc7.rs`)
  - [x] `plain.go`: VT stripping and plain tail for `read_terminal` (`plain.rs`)
  - [x] `queries.go`: OSC 10/11/12 and CPR answered in-process for detached sessions (`queries.rs`)
  - [x] Emulator state (`screen/`): alacritty_terminal behind xterm.js semantics, UI-free and tested
- [x] `internal/usage`: 12 files, 2911 lines, 3 test files → `recur-usage` (`fetch_test.go`, `parse_test.go`, `poller_test.go` and `app/provider_usage_test.go` ported; parsers, merge, local rows and the Connect auth file Go-matched in `crates/usage/tests/golden/go_usage.json`)
  - [x] `types.go` → `recur_agent::quota` (Snapshot, ProviderQuota, Window, ModelRow, CursorUsageConnect serde-identical, provider/source/window/bucket constants, EmptySnapshot, setSource, clampPct) plus the `UsageProbe` trait
  - [x] `http.go`, `parse.go` → `http`, `parse` (15 s client, 1 MiB reads, Go `url.Error`-shaped transport errors, `HTTP N: <180 bytes>`, loose number/string/bool readers, resets as seconds/ms/RFC 3339, JWT claims with Go base64 leniency, email discovery)
  - [x] `claude.go`, `claude_creds.go` → `claude` (OAuth usage endpoint and headers, top-level/`limits`/model-scoped/extra windows with the inactive-then-active replacement rule, scoped ids and labels; Keychain via `security` and credential files incl. `CLAUDE_CONFIG_DIR`, newest-expiry pick, `claude auth status` nudge on expiry and one 401 retry, step deadlines that reserve 4 s for HTTP, account email from `~/.claude.json`)
  - [x] `codex.go` → `codex` (auth.json from Recur's `codex-home` then `~/.codex`, API-key-only row, rate-limit windows by duration, individual/spend-control limits, monthly spend-controls fallback with the usage-payload account id, credits, plan names from Go `codex.FormatPlan` as `format_codex_plan`)
  - [x] `cursor.go`, `cursor_oauth.go`, `cursor_session.go`, `connect.go` → `cursor`, `connect` (period pools and model rows scaled per pool, email via JWT / dashboard cookie / REST / GetMe; PKCE login and 1.5 s poll for 3 minutes, session refresh a week before expiry, user API key mint/rotate/revoke with masked-key lookup and the `on_minted_api_key` hook, 0600 `cursor-usage-auth.json` written byte for byte like Go, local-only Connect rows)
  - [x] `fetcher.go`, `poller.go` → `Fetcher` (hook fields as in Go, `want` per collect, Claude 12 s budget beside other providers, connecting and stub rows) and `Poller` (lazy 3-minute ticker, 60 s freshness, single-flight refresh that survives a panicking collect, rate-limit/reachability stale merge, carried rows, Connect marks, disconnect with key revoke). `provider.usage` goes to the injected publisher; SV puts it on the bus
  - [x] `ClaudeProbe`, `CodexProbe`, `CursorProbe` for `Provider::usage_probe` (P1/P2 wire them)
  - [x] `app/provider_usage.go` → `recur_usage::UsageService` (start/stop, wants and Cursor stub from Settings, get/refresh snapshot, Connect/cancel/disconnect, clearing a leftover pasted key, `drop_orphan_cursor_key`, minted key into Settings through the injected save). The `SettingsService` methods stay SV
- [ ] `internal/wsenv`: 2 files, 325 lines, 2 test files
- [x] `internal/store`: 9 files, 6229 lines, 6 test files → `recur-store` (all 6 Go test files ported; 148 unit tests + seeddump integration test)
  - [x] SQLite via rusqlite (bundled), one connection behind a mutex like Go's single pooled connection; busy_timeout 5s, WAL, foreign_keys, TRUNCATE checkpoint on open and close
  - [x] Migrations embedded from `internal/store/migrations/` with Go's `__migrations` bookkeeping (one transaction per migration); schema guard refuses a DB that recorded unknown migrations (`Error::NewerSchema`)
  - [x] Open sequence: migrate, finding identity backfill, feed-sort seed, seed defaults on a DB without settings, else load (with PR time backfill)
  - [x] Every Store method: project groups, projects (project.toml overlay, `write_project_file`, pinned-run validation, extra_json), reorder, cascading deletes, clear-all, workspaces, agents (incl. move/merge), feed (prepend/upsert/patch off the store lock), events, findings (UUIDv7 + per-workspace short ids with UNIQUE retry, notes, actor rules), analysis, PRs (list rows without diffs, deep copies with Go nil semantics, open/linked PR rules, Local number sequence), escalations (chat-keyed lookup), inbox, attention counts, policy/settings/resource, GitHub branch rules
  - [x] Sessions: create/get/lookups by kind/agent/owner, headers, streaming recovery, reassign with code remap, config json_set/json_patch patches, messages and blocks with idx allocation under the session lock, truncate/drop/delete with fork-aware orphan cleanup, delete closed chat, forks with context copy, compaction contexts, tool_use/fallback patches, transcript load, first prompt / last assistant text
  - [x] Schedules: ISO-millis columns, flags, latest run, due and active queries (partial index), claim, link, update
  - [x] Paths: `<UserConfigDir>/RecurNext` and `~/.recur/worktrees-next` defaults, `RECUR_DATA_DIR`/`RECUR_WORKTREES_DIR` overrides as in Go, legacy worktree migration, per-workspace dirs; `codex.DefaultSeedAuthPath` lives here as `paths::codex_default_seed_auth_path`
  - [x] App log: rotating `logs/recur.log` (2 MiB, also mid-session), tee writer for `tracing` with Go's log line format, panics written to the log
  - [x] Seed defaults and the demo dataset (`seed.go`, `seed_prs.go`)
  - [x] Go compatibility (`tests/fixtures/gen_store.go.txt`): a Go-populated DB reads back exactly as Go reads it, its JSON columns re-encode byte for byte, rewriting every row leaves every table unchanged; a recur-store-written DB passes the Go verifier and Go reads it exactly as recur-store does; a fresh DB matches Go's schema dump and bookkeeping
- [ ] `internal/term`: 7 files, 1797 lines, 6 test files
- [x] `internal/wsenv`: 2 files, 325 lines, 2 test files → `recur_project::wsenv` (both test files ported)
  - [x] Vars/FromWorkspace/RecurMap/Environ: host → Recur vars → project env, two-pass expansion that never re-expands substituted values, protected keys stripped for project.toml env and dotenv
  - [x] Dotenv parsing and loading (`.env`, `.env.local`, `.recur/.env`, `.recur/.env.local`, repo then worktree, 256 KiB cap), Go `os.Expand` syntax, `expand_known`
  - [x] `git.ExpandHome` as `recur_project::home::expand_home`, with Go's `os.UserHomeDir` / `os.UserConfigDir` and `filepath.Clean`/`Join`/`Abs`
- [ ] `cmd/recur`: 1 files, 22 lines, 0 test files
- [ ] `cmd/bundles`: 1 files, 15 lines, 0 test files
- [x] `cmd/seeddump`: 1 files, 86 lines, 0 test files → `recur_store::seeddump` + bin `seeddump` (`cargo run -p recur-store --bin seeddump`); output equals `frontend/e2e/fixtures/seed.json` byte for byte (`tests/seeddump.rs`)
- [x] `cmd/recur`: 1 files, 22 lines, 0 test files → `recur-cli` binary (`src/main.rs`)
- [x] `cmd/bundles`: 1 files, 15 lines, 0 test files → deprecated `bundles` forwarder binary (`src/bin/bundles.rs`, same stderr note)
- [ ] `cmd/seeddump`: 1 files, 86 lines, 0 test files

### Root desktop entry (`*.go` at repo root)

- [x] `app_menu.go` → `recur_app::menu` (see Native app menu)
- [x] `debug_devtools.go` → `recur_app::INSPECTOR_AVAILABLE` (dev builds) and View > Open Developer Tools toggling GPUI's inspector
- [x] `debug_devtools_off.go` → release builds without the `devtools` feature leave the item out
- [x] `main.go` → `recur-desktop` (`crates/desktop/src/main.rs`) on the OS helpers below
  - [x] Boot order: updater helper mode, `--open` peeling, claim or hand off before the store, `recur.log` tee (`GoLogFormat`), `ensure_path` before any thread, store open, external-allow reload, bundles and recur-skills seeding, tokio runtime, open IPC, services, IPC handler and cold opens, updater (release bundles), GPUI app
  - [x] Backend events the desktop owns: `app.ready` → `OpenHub::ui_ready_now`, `notify` → OS notification off the event loop, `inbox.updated` / `attention.updated` → dock badge from `attention_counts().dock` once live (500ms after launch, after notification and badge authorization in a bundle), `updater.check` / `wails:updater:user:restart` → the updater
  - [x] Open hub hooks: `app.open` emit, focus posted to the main thread (activate, show the window) before the IPC reply; Dock reopen brings the window forward; notification clicks focus and emit `notification.open`
  - [x] Window close: revoke open UI, wait out focus, flush shell state; last window closed quits; quit revokes UI and stops IPC before the store closes; dropped files are allowed for `/external-file` (`recur_code::allow_external_path`)
  - [x] Services: the mock bridge behind the default `mock` feature; `services()` is the one line that changes for the engine-backed implementation, which then also owns `Recover`, provider usage, git and cursor checks, scheduled prompts, auto-resume, the PR sweep and `SetAppFocused` (`ShellOptions::on_window_active`)
  - [x] `runningInAppBundle` → `recur_platform::running_in_app_bundle`
  - [x] `installBundledCLI` → `cliinstall::install_bundled_cli`
  - [x] `sendOSNotification`: native `UNUserNotificationCenter` from a bundle (`recur-<nanos>` id, `target` userInfo, default sound, 5s), else `osascript` with Go `%q` quoting or `notify-send --app-name=Recur Next` (3s, logged) → `notify::send_os_notification`
  - [x] Notification delegate (banner/list/sound while frontmost, click responses with `DEFAULT_ACTION` mapping and userInfo), authorization check (15s) and request with badge (180s), bundle-id guard → `notify::{start, on_response, check_authorization, request_authorization}`
  - [x] Dock badge `SetBadge` / `RemoveBadge` (regular activation policy only, `●` for empty, main-queue dispatch, no-op on Linux), `dockBadgeCount`, `syncDockBadge` → `dock::{set_badge, remove_badge, badge_count, sync_badge}`
  - `serveWorkspaceData` and the other asset middleware are webview HTTP routes; GPUI reads files directly, so they move with `recur-app` / `recur-services`
- [x] `open_boot.go` + test (`recur_platform::open_boot`, 25 tests)
  - [x] `peel_open_args` (`--open PATH`, `--open=PATH`, dangling flag dropped)
  - [x] `handoff_to_running_instance`: focus-only empty path, every path must be accepted, shutting-down and connected drops are not a handoff
  - [x] `claim_or_handoff` with `CLAIM_RETRY_FOR` (call timeout + 8s) and 50ms retry
  - [x] `IpcGate`: parks requests until `set`, authoritative `stop`
  - [x] `OpenHub`: replay last open once the UI is ready, `bring_forward` waits for the posted show to apply, `drop_ui` / `wait_focus` semantics, `revoke_open_ui`
  - [x] `start_open_ipc` (stop revokes, cancels and joins), `attach_open_ipc`, `apply_cold_opens`
- [x] `updater.go` + test, and Wails `pkg/updater` in headless (`WindowNone`) mode (`recur_platform::updater`, 82 tests)
  - [x] `RecurUpdater::install`: release builds only (version + manifest URL + app bundle), pinned `build/updater.key.pub`, 30s connect and response-header timeouts without a body timeout, check after 1 minute then every 6 hours, `check_now` only from idle / up-to-date / error
  - [x] `SignedOnly` rejects manifests without a signature
  - [x] Updater state machine, provider fallback chain, skip version, event names and JSON payloads (`wails:updater:*`), user actions only inside a check-and-install session, state replay with `meta`
  - [x] Download into `wails-update-*` with streaming sha256/sha512 (sha512 for ed25519ph), throttled progress, cleanup on every failure, filename neutralising
  - [x] Verification: constant-time digest, ed25519, ed25519ph, ecdsa-p256 (raw or DER, trailing data rejected), raw or PKIX/PEM keys, fail closed without algo or key
  - [x] `.zip` / `.tar.gz` extraction: single top-level entry, zip-slip and symlink escape checks, symlinks last, entry and size caps
  - [x] Restart: helper re-exec in a new session with `WAILS_UPDATER_HELPER*` env, readiness file with timeout, quit only after ready; `handle_helper_mode` swap with backup, exec-bit restore, 20 retries, restore paths and exit codes, helper env scrubbed from relaunches, `open -n` for bundles
  - [x] Endpoint provider: placeholders and sorted query params, Accept and configured headers (same-host, no downgrade; reqwest strips Authorization on cross-host redirects), 204/404 up to date, schemaVersion, channel, semver (`x/mod/semver` rules), artifact aliases, relative URLs, base64 raw/std
  - Not ported (unused by Recur): built-in and BYO update windows, `CheckInterval` polling, GitHub / AppCast / keygen providers, Windows helper paths
- [x] `window_chrome_linux.go`: GTK's prefer-dark follows the app theme so WebKitGTK's native widgets match; GPUI draws every widget and its client-side decorations from the app theme, so there is no GTK surface left to recolor
- [x] `window_chrome_stub.go`
- [x] `window_state.go` + test (`recur_platform::window_state`, 6 tests)
  - [x] `window.json` format (camelCase, two-space indent), missing/malformed fallback, Go zero-value decoding, undersized clamp
  - [x] `WindowOptions::from_state`: size, min size, position or centred, background, titlebar height, file drop, instance-label title
  - [x] `GeometryTracker`: records only normal geometry, keeps maximised flag, zoom once on first show; 250ms save debounce constant for the window layer
- [x] `window_titlebar.go` + test → `recur_app::chrome` (`parse_titlebar_double_click_action`, `apply_titlebar_double_click`: zoom / minimize / none, skipped in full screen; drag regions and no-drag controls)
- [x] `window_titlebar_darwin.go` → `AppleActionOnDoubleClick` (and the legacy `AppleMiniaturizeOnDoubleClick`) read from `NSUserDefaults`; inset traffic lights at `TRAFFIC_LIGHT_POSITION` in a transparent titlebar the app drags
- [x] `window_titlebar_stub.go`

## Native app menu

Source: `app_menu.go`. Rust home: `recur-app::menu`. Every item, accelerator, enabled-state rule and dynamic submenu (themes, Open In) must match.

All in `recur_app::menu` (`app_menus`, `bind_menu_keys`, `menu_action_name`): roles (About panel, Hide / Hide Others ⌥⌘H / Show All, Quit, Edit roles with Paste and Match Style, Window: Minimize ⌘M, Zoom, Bring All to Front, Services), accelerators shown and bound exactly where `nativeMenuAccelerators` binds them, enabled rules from `menu.state` (`AppState::set_menu_state`), and the App / Code Editor / Terminal Theme submenus built from the theme catalogs ("Loading…" until they load, checked current). The shell rebuilds the menubar when the state or catalogs change. Linux has no native menubar under GPUI; there the same keys reach the shell's key handler, as in Wails' Linux build.

- [x] Recur
- [x] Check for Updates…
- [x] Settings…
- [x] File
- [x] Open…
- [x] New Workspace
- [x] New Blank Workspace
- [x] New Tab
- [x] Save
- [x] Open In…
- [x] Copy Path
- [x] Close Tab
- [x] Quit
- [x] View
- [x] Appearance
- [x] App Theme
- [x] Code Editor Theme
- [x] Dark
- [x] Light
- [x] Terminal Theme
- [x] Previous Editor Theme
- [x] Next Editor Theme
- [x] Toggle Sidebar
- [x] Toggle Left Panel
- [x] Toggle Right Panel
- [x] Toggle Editor
- [x] Toggle Terminal
- [x] Maximize Terminal
- [x] Run…
- [x] Stop Run
- [x] Navigate
- [x] Go to…
- [x] Run Command…
- [x] Keyboard Shortcuts…
- [x] Reload Bundles
- [x] Quick Open…
- [x] Go to Symbol…
- [x] Focus Agent Panel
- [x] Focus Editor
- [x] Focus Terminal
- [x] Previous Tab
- [x] Next Tab
- [x] Previous Workspace
- [x] Next Workspace

## Agent host tools (MCP)

Source: `internal/agenttools`. Rust home: `recur-agent-tools`, served by `recur-mcp`. Names, schemas and descriptions must be byte-identical so prompts and bundles keep working.

Checked against Go-generated fixtures: catalog text (`crates/agent-tools/tests/catalog.rs`), every tool's validation, result and host-call behavior (`tests/host_golden.rs`), and the served `tools/list` body (`crates/mcp/tests/http_golden.rs`).

- [x] `activate_skill`
- [x] `ask_user`
- [x] `attach_evidence`
- [x] `list_workspace_briefs`
- [x] `get_workspace_context`
- [x] `present_workspace`
- [x] `create_artifact`
- [x] `create_workspace`
- [x] `dispatch_specialist`
- [x] `done`
- [x] `edit_plan`
- [x] `escalate`
- [x] `extend_iterations`
- [x] `extend_rounds`
- [x] `list_artifacts`
- [x] `list_findings`
- [x] `list_skills`
- [x] `list_terminals`
- [x] `merge_child`
- [x] `open_pr`
- [x] `permission_request`
- [x] `plan`
- [x] `present_artifact`
- [x] `read_artifact`
- [x] `read_child`
- [x] `archive_child`
- [x] `message_child`
- [x] `read_terminal`
- [x] `reenter_phase`
- [x] `rename_branch`
- [x] `rename_session`
- [x] `rename_workspace`
- [x] `report_finding`
- [x] `report_result`
- [x] `rerun_review`
- [x] `resume_loop`
- [x] `run_phase`
- [x] `send_back`
- [x] `submit_plan`
- [x] `switch_mode`
- [x] `update_finding`
- [x] `update_plan_step`

## Behavior specs to port

Playwright e2e specs (`frontend/e2e/*.spec.ts`) and the seeded fixture (`frontend/e2e/fixtures/seed.json`) describe UI behavior. Port each spec's scenarios to GPUI `TestAppContext` tests in `recur-app` and drive the same seed.

- [~] `a11y-keys.spec.ts` (8 tests) → `crates/app/tests/shell.rs`: Esc from detail and PR detail, Esc in text entry, Add project not focused after launch, ⇧⌘N blank workspace. Icon-button names are `aria_label`s (not queryable from tests); the New workspace modal and select menus move with NewTask
- [ ] `activity.spec.ts` (17 tests)
- [ ] `briefing.spec.ts` (18 tests)
- [ ] `bundles.spec.ts` (7 tests)
- [x] `chat.spec.ts` (68 tests) → `recur-view-chat`'s `spec::chat_spec` (69 tests: the queue / steer scenario runs once per setting). The workspace view is not ported, so the tests drive the chat through `spec::harness::Host`, a double of its send, follow-up queue, steer, Stop, Retry, live-turn and tab contract; the halves that live in the workspace view (prompt routing to tabs, the findings panel, the explorer's selection, the tab strip) assert the chat's events and are listed in the follow-ups
- [ ] `compatible-provider.spec.ts` (1 tests)
- [ ] `editor-hotpath.spec.ts` (27 tests)
- [ ] `editor-syntax.spec.ts` (2 tests)
- [ ] `explorer-scope.spec.ts` (8 tests)
- [x] `loading-states.spec.ts` (1 tests) → `spec::dock` (the session feed's delayed fetch into a session card)
- [x] `loop-lifecycle.spec.ts` (4 tests) → `spec::shell` (the workspace titlebar's Remove in the shell with the chat's overlays registered)
- [x] `maestro-dock.spec.ts` (3 tests) → `spec::dock` (send, unmount and remount keep the transcript; the right-hand placement is `PanelLayout`'s rule). The dock's panel itself moves with the workspace view
- [ ] `observatory.spec.ts` (47 tests)
- [ ] `pr-review.spec.ts` (77 tests)
- [ ] `scheduled-prompts.spec.ts` (4 tests)
- [ ] `settings-newloop.spec.ts` (34 tests)
- [~] `shell.spec.ts` (38 tests) → `crates/app/tests/shell.rs` (20 scenarios): boot, needs-you flags, hidden PR host in a collapsed group, restart restore (workspace and briefing), selection cleared on Inbox / PRs, hover actions, two-click Remove and its disabled state, expand / collapse persistence, hide and reveal, hidden projects at boot, flyout and its Escape, ⌥⌘ cycling and peek-until-dwell, drag into a group, group rename. Icons, chips, tree lines and scrollbar stacking are covered by the `shell/*` parity scenes. Palettes, the workspace view, escalation chat, project settings and Open In move with their views
- [ ] `terminal-dock.spec.ts` (34 tests)
- [~] `toasts.spec.ts` (3 tests): toast lifetimes are tested in `recur_ui::widgets::toasts` and through the shell (`a_failed_save_shows_an_error_toast_that_auto_dismisses`); the PR approve / merge flows move with PR detail
- [ ] `welcome.spec.ts` (14 tests)
- [ ] `workspace-base.spec.ts` (37 tests)

Frontend unit tests (`vitest`, 170 files) are logic specs for the matching `lib/` modules; port their cases with the module.

## Shipped content (unchanged, embedded by the Rust app)

- [x] `bundles/` seeded into the data dir exactly as `bundles.SeedDefaults` does (stamp/sync semantics): `recur_bundles::seed_defaults(dir, &SHIPPED_BUNDLES, ".")`; the boot call belongs with the `main.go` port
- [x] `recur-skills/` seeded into `~/.recur/recur-skills`: `seed_defaults(dir, &SHIPPED_SKILLS, ".")`
- [ ] `internal/store/migrations/*.sql` applied by `recur-store` (same files, same order, same `schema_migrations` bookkeeping)
- [x] `frontend/src/lib/file-icons` icon font and mapping (embedded fonts, `icondb.js`, `icons.css`, `dotenv-fix.css`, `atom-octicons.css`, `colors.css`; WOFF/WOFF2 decoded with `wuff`)
- [x] lucide icon set used by `lib/icons.ts` (`assets/icons/lucide/*.svg`, 1776 icons with `lucide.json`)
- [ ] `build/` app icons, Info.plist keys, URL schemes, Linux `.desktop` entry

## CLI

Source: `cmd/recur`, `internal/cli`. Rust home: `crates/cli`. Output and exit codes are byte-identical to the Go CLI (`tests/golden.rs`).

- [x] `recur [path]`: no path focuses the app, a path-like arg opens it, anything else is `unknown command` (exit 1); `--bundles-dir`, `-h/--help`, `-v/--version`
- [x] `recur open [path]`: resolve (exit 2), send over open IPC, launch and retry for 30s (exit 1)
- [x] `recur bundles`: help (also for unknown subcommands)
- [x] `recur bundles test [name|path]...`: target resolution (exit 2), run tests, report, exit 1 on failure
- [x] `recur bundles lint [name|path]...`: target resolution (exit 2), lint report, exit 1 on errors
- [x] `recur bundles create <Name> [--dir] [--force]`: scaffold, messages, exit codes
- [x] `recur help [command]`
- [x] `recur completion bash|zsh|fish|powershell [--no-descriptions]` and hidden `recur __complete` / `__completeNoDesc`

