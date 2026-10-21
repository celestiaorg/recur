# Parity inventory

Every surface of the Go/Svelte app that the Rust port must carry over. Nothing here may be dropped. Tick an item only when the Rust implementation matches the original behavior and has a test or a parity screenshot. Each area owner expands their section with the detailed behaviors (edge cases, persistence, keyboard paths) as they port it.

Legend: `[ ]` not started, `[~]` in progress, `[x]` ported and verified.

## Application services (former Wails bindings)

Source: `frontend/bindings/recur/internal/app/*.ts`, implemented in `internal/app`. Rust home: `recur-services`.

### agentsservice (2)

- [ ] `GetAgentTrace`
- [ ] `ListLoopAgents`

### artifactsservice (5)

- [ ] `DeleteArtifact`
- [ ] `ListArtifacts`
- [ ] `PresentArtifact`
- [ ] `RenameArtifact`
- [ ] `RevealArtifact`

### briefingservice (12)

- [ ] `AppChatItems`
- [ ] `AppChatSelection`
- [ ] `CancelAppMessage`
- [ ] `CloseAppChat`
- [ ] `GetBriefing`
- [ ] `ListAppChats`
- [ ] `MarkAppChatSeen`
- [ ] `MarkWorkspaceViewed`
- [ ] `NewAppChat`
- [ ] `RememberAppChatSelection`
- [ ] `SendAppMessage`
- [ ] `SetAppChatFocus`

### bundleservice (23)

- [ ] `CompiledCompleteModule`
- [ ] `CompiledModule`
- [ ] `CompiledPromptModule`
- [ ] `ExpandGoal`
- [ ] `InvalidateCatalog`
- [ ] `ListBundleSettings`
- [ ] `ListCommands`
- [ ] `ListGrammars`
- [ ] `ListLoops`
- [ ] `ListPolicies`
- [ ] `ListPromptActions`
- [ ] `ListRoles`
- [ ] `ListSkippedBundles`
- [ ] `ListSnippets`
- [ ] `ListSymbols`
- [ ] `ListTerminalThemes`
- [ ] `ListThemes`
- [ ] `ReadGrammar`
- [ ] `ReadTheme`
- [ ] `RunCommand`
- [ ] `RunPromptAction`
- [ ] `RunPromptComplete`
- [ ] `SearchPalette`

### codeservice (45)

- [ ] `AbsPath`
- [ ] `BridgeBenchDone`
- [ ] `BridgeBenchEnabled`
- [ ] `BridgeEcho`
- [ ] `BridgePing`
- [ ] `Commit`
- [ ] `CommitDiff`
- [ ] `CommitStaged`
- [ ] `Copy`
- [ ] `CreateFile`
- [ ] `Delete`
- [ ] `DiffVsBase`
- [ ] `DiffWorking`
- [ ] `DiscardAll`
- [ ] `DiscardFile`
- [ ] `FileAtBase`
- [ ] `FileDiff`
- [ ] `FindDefs`
- [ ] `FindRefs`
- [ ] `GenerateCommitMessage`
- [ ] `GitStatus`
- [ ] `ListAllSkills`
- [ ] `ListChangedFiles`
- [ ] `ListCommits`
- [ ] `ListFiles`
- [ ] `ListSkills`
- [ ] `ListTree`
- [ ] `ListTrees`
- [ ] `Mkdir`
- [ ] `OpenInEditor`
- [ ] `Outline`
- [ ] `ReadFile`
- [ ] `RefreshFileIndex`
- [ ] `Rename`
- [ ] `ReplaceInFiles`
- [ ] `RevealInFinder`
- [ ] `Search`
- [ ] `SearchAdvanced`
- [ ] `SearchFiles`
- [ ] `SetFileIndexExclude`
- [ ] `Stage`
- [ ] `StagedFiles`
- [ ] `Unstage`
- [ ] `WorkingStatus`
- [ ] `WriteFile`

### editorsessionservice (3)

- [ ] `ClearEditorSession`
- [ ] `GetEditorSession`
- [ ] `UpdateEditorSession`

### escalationservice (2)

- [ ] `AnswerEscalation`
- [ ] `GetEscalation`

### inboxservice (3)

- [ ] `GetInbox`
- [ ] `MarkAllInboxRead`
- [ ] `MarkInboxRead`

### lspservice (5)

- [ ] `CloseLSP`
- [ ] `CloseWorkspaceLSP`
- [ ] `EnsureLSP`
- [ ] `ListLanguageServers`
- [ ] `WriteLSP`

### maestroservice (3)

- [ ] `CancelMaestroMessage`
- [ ] `GetMaestroChat`
- [ ] `SendMaestroMessage`

### observatoryservice (5)

- [ ] `AddFindingNote`
- [ ] `CreateFinding`
- [ ] `DeleteFindingNote`
- [ ] `ListFindings`
- [ ] `SetFindingStatus`

### permissionsservice (2)

- [ ] `AnswerPermission`
- [ ] `ListPendingPermissions`

### policyservice (2)

- [ ] `GetPolicy`
- [ ] `SetPolicy`

### projectsservice (22)

- [ ] `AddProject`
- [ ] `AddProjectGroup`
- [ ] `DeleteProject`
- [ ] `DeleteProjectGroup`
- [ ] `DeleteScheduledPrompt`
- [ ] `GetProject`
- [ ] `ListBranches`
- [ ] `ListBranchesForPath`
- [ ] `ListProjectGroups`
- [ ] `ListProjects`
- [ ] `ListScheduledPrompts`
- [ ] `PauseScheduledPrompt`
- [ ] `PreviewCopyFiles`
- [ ] `PreviewScheduledPrompt`
- [ ] `ReorderProjectGroups`
- [ ] `ReorderProjects`
- [ ] `RunScheduledPromptNow`
- [ ] `SaveScheduledPrompt`
- [ ] `SetProjectGroup`
- [ ] `SuggestBaseBranch`
- [ ] `UpdateProjectGroup`
- [ ] `UpdateProjectRunConfig`

### prsservice (23)

- [ ] `AddPRComment`
- [ ] `CreateWorkspaceFromPR`
- [ ] `DeletePRComment`
- [ ] `EnsurePRChatHost`
- [ ] `GetCommitDiff`
- [ ] `GetPR`
- [ ] `GetPRCommits`
- [ ] `GetPRFiles`
- [ ] `ListPRFindings`
- [ ] `ListPRs`
- [ ] `MergePR`
- [ ] `ReadyPR`
- [ ] `ReplyPRComment`
- [ ] `SearchPRMentionableUsers`
- [ ] `SendBackToLoop`
- [ ] `SetPRDraft`
- [ ] `SetPRThreadResolved`
- [ ] `SubmitReview`
- [ ] `SyncGitHubPRs`
- [ ] `TogglePRCommentReaction`
- [ ] `UpdatePRComment`
- [ ] `UpdatePRDetails`
- [ ] `UpdateWithBase`

### questionsservice (2)

- [ ] `AnswerQuestion`
- [ ] `ListPendingQuestions`

### settingsservice (26)

- [ ] `CancelCursorUsageConnect`
- [ ] `ClearAllData`
- [ ] `CompatibleFlagPreset`
- [ ] `ConnectCursorUsage`
- [ ] `DetectAgent`
- [ ] `DisconnectCursorUsage`
- [ ] `GetClaudeStatus`
- [ ] `GetCodexStatus`
- [ ] `GetCursorStatus`
- [ ] `GetInstanceLabel`
- [ ] `GetProviderUsage`
- [ ] `GetResource`
- [ ] `GetSettings`
- [ ] `GitHubLogin`
- [ ] `InstallCursorBridge`
- [ ] `ListAssignedIssues`
- [ ] `ListModelCatalog`
- [ ] `ListMonospaceFonts`
- [ ] `LogsPath`
- [ ] `OpenLinearInApp`
- [ ] `RefreshProviderUsage`
- [ ] `RevealLogs`
- [ ] `SetSettings`
- [ ] `TestCodexKey`
- [ ] `TestCompatibleProvider`
- [ ] `TestCursorKey`

### shellservice (7)

- [ ] `Flush`
- [ ] `GetExpandedSchedules`
- [ ] `GetIssuesView`
- [ ] `GetShellState`
- [ ] `SetExpandedSchedules`
- [ ] `SetIssuesView`
- [ ] `SetShellState`

### terminalservice (16)

- [ ] `AckTerminal`
- [ ] `AttachTerminal`
- [ ] `ClearTerminal`
- [ ] `CloseTerminal`
- [ ] `DetachTerminal`
- [ ] `InterruptTerminal`
- [ ] `ListTerminalDockChrome`
- [ ] `ListTerminals`
- [ ] `OpenTerminal`
- [ ] `ReorderTerminals`
- [ ] `ResizeTerminal`
- [ ] `RestoreWorkspaceTerminals`
- [ ] `SetActiveTerminal`
- [ ] `SetTerminalDockChrome`
- [ ] `SetTerminalTitle`
- [ ] `WriteTerminal`

### workspacesservice (66)

- [ ] `ApplyProvisionalChatTitles`
- [ ] `ApprovePlan`
- [ ] `AttentionCounts`
- [ ] `CancelWorkspaceMessage`
- [ ] `ClearWorkspaceIssue`
- [ ] `CloseWorkspaceChat`
- [ ] `CreateWorkspace`
- [ ] `CreateWorkspaceChat`
- [ ] `CreateWorkspaceWith`
- [ ] `DeleteClosedWorkspaceChat`
- [ ] `DeleteWorkspace`
- [ ] `EngageLoop`
- [ ] `EngageLoopWith`
- [ ] `FetchWorkingCopyUpstream`
- [ ] `ForkWorkspaceChat`
- [ ] `ForkWorkspaceChatToWorkspace`
- [ ] `GetWorkspace`
- [ ] `GetWorkspaceFeed`
- [ ] `LastAssistantText`
- [ ] `ListClosedWorkspaceChats`
- [ ] `ListOpenInTargets`
- [ ] `ListWorkspaceChats`
- [ ] `ListWorkspaces`
- [ ] `MarkWorkspaceChatSeen`
- [ ] `MergeWorkspace`
- [ ] `OpenMainWorkspace`
- [ ] `OpenPath`
- [ ] `OpenWorkspaceIn`
- [ ] `OpenWorkspacePR`
- [ ] `PauseLoop`
- [ ] `PullWorkspacePR`
- [ ] `PushWorkingCopy`
- [ ] `RememberChatLoopPolicy`
- [ ] `RememberWorkspaceChatPromptGroup`
- [ ] `RememberWorkspaceChatSelection`
- [ ] `RememberWorkspaceChatSimplify`
- [ ] `RemoveWorkspace`
- [ ] `ReopenWorkspaceChat`
- [ ] `ReorderWorkspaceChats`
- [ ] `ResetWorkspaceChat`
- [ ] `RestartLoop`
- [ ] `ResumeLoop`
- [ ] `RetryLoop`
- [ ] `RetryWorkspaceSetup`
- [ ] `SendWorkspaceMessage`
- [ ] `SetChatUIOccluded`
- [ ] `SetWorkspaceBaseBranch`
- [ ] `SetWorkspaceChatFocus`
- [ ] `SetWorkspaceIssue`
- [ ] `SetWorkspaceLoopPolicy`
- [ ] `StartWorkspace`
- [ ] `StopLoop`
- [ ] `SubmitPlan`
- [ ] `UpdatePlan`
- [ ] `UpdatePlanStep`
- [ ] `UpdateWorkingCopyFromRemote`
- [ ] `UpdateWorkspaceWithBase`
- [ ] `WatchWorkspace`
- [ ] `WorkspaceChatItems`
- [ ] `WorkspaceChatModels`
- [ ] `WorkspaceChatSelection`
- [ ] `WorkspaceChatTurnDiff`
- [ ] `WorkspaceDiscardRisk`
- [ ] `WorkspaceFacts`
- [ ] `WorkspaceLandState`
- [ ] `WorkspaceSessions`

## App events (backend to UI)

Source: `Events.On(...)` in `frontend/src`. Rust home: `recur_services::AppEvent`.

- [ ] `agent.step`
- [ ] `agent.updated`
- [ ] `app.focused`
- [ ] `app.open`
- [ ] `app.ready`
- [ ] `artifact.created`
- [ ] `artifact.presented`
- [ ] `attention.updated`
- [ ] `briefing.updated`
- [ ] `chat.mode_changed`
- [ ] `cursor.install`
- [ ] `escalation.raised`
- [ ] `escalation.resolved`
- [ ] `files.dropped`
- [ ] `findings.updated`
- [ ] `github.branch_rules`
- [ ] `inbox.updated`
- [ ] `lsp.exit`
- [ ] `lsp.message`
- [ ] `menu.action`
- [ ] `menu.state`
- [ ] `menu.themeState`
- [ ] `menu.themes`
- [ ] `notification.open`
- [ ] `permission.requested`
- [ ] `permission.resolved`
- [ ] `pr.comment`
- [ ] `pr.removed`
- [ ] `pr.updated`
- [ ] `provider.usage`
- [ ] `prs.synced`
- [ ] `question.answered`
- [ ] `question.asked`
- [ ] `resource.updated`
- [ ] `session.compact`
- [ ] `session.context_usage`
- [ ] `session.delta`
- [ ] `session.fallback`
- [ ] `session.notice`
- [ ] `session.step`
- [ ] `session.thinking`
- [ ] `session.tool_input`
- [ ] `session.tool_result`
- [ ] `session.turn_end`
- [ ] `session.user`
- [ ] `system.theme`
- [x] `term.busy`: `Event::Busy { id, workspace_id }` when the shell loses the foreground process group (idle watcher, 25ms to 1s backoff).
- [x] `term.data`: `Event::Data { id, data }`, raw bytes coalesced 8ms / 64KiB, flow-controlled by `ack` (256KiB high water), routed to the pane by `route_event`.
- [x] `term.exit`: `Event::Exit { id, workspace_id, exit_code }` once after the reader drains; the pane emits `TerminalEvent::Exit`.
- [x] `term.idle`: `Event::Idle { id, workspace_id }` when the shell regains the foreground.
- [ ] `updater.check`
- [ ] `wails:updater:error`
- [ ] `wails:updater:no-update`
- [ ] `wails:updater:update-available`
- [ ] `wails:updater:update-ready`
- [ ] `wails:updater:user:restart`
- [ ] `window.titlebarDblclick`
- [ ] `workspace.chat_tab`
- [ ] `workspace.deleted`
- [ ] `workspace.event`
- [ ] `workspace.file_index`
- [ ] `workspace.fs_changed`
- [ ] `workspace.removed`
- [ ] `workspace.updated`

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

- [ ] `AgentChat.harness.svelte` (12 lines)
- [ ] `AgentChat.svelte` (6396 lines)
- [ ] `AppChat.svelte` (374 lines)
- [ ] `ArmedButton.svelte` (51 lines)
- [ ] `ArtifactsPanel.svelte` (320 lines)
- [ ] `AttachmentLightbox.svelte` (155 lines)
- [ ] `CodeEditor.svelte` (1405 lines)
- [ ] `CommandPalette.svelte` (269 lines)
- [ ] `CommandPrompts.svelte` (142 lines)
- [ ] `CommentReactions.svelte` (114 lines)
- [ ] `CommitRow.svelte` (80 lines)
- [ ] `CommitsView.svelte` (80 lines)
- [ ] `CompatibleProviderCard.svelte` (468 lines)
- [ ] `CompatibleProviderModal.svelte` (215 lines)
- [ ] `ContextUsageMeter.svelte` (278 lines)
- [ ] `DecodeText.svelte` (107 lines)
- [ ] `DiffFile.svelte` (898 lines)
- [ ] `EmojiSuggest.svelte` (90 lines)
- [ ] `FanOutCard.svelte` (234 lines)
- [ ] `FilePreview.svelte` (364 lines)
- [ ] `FilesChangedView.svelte` (712 lines)
- [ ] `FindingsPanel.svelte` (506 lines)
- [ ] `Icon.svelte` (51 lines)
- [ ] `IconPicker.svelte` (237 lines)
- [ ] `ImageContextMenu.svelte` (84 lines)
- [ ] `InboxTitlebar.svelte` (24 lines)
- [ ] `Issues.svelte` (512 lines)
- [ ] `Kbd.svelte` (13 lines)
- [ ] `KeybindingMenu.svelte` (180 lines)
- [ ] `LineNoteThread.svelte` (375 lines)
- [ ] `LoopPolicyPickers.svelte` (93 lines)
- [ ] `LucideIcon.svelte` (26 lines)
- [ ] `MarkdownHtml.svelte` (87 lines)
- [ ] `MergeEditor.svelte` (703 lines)
- [ ] `MermaidPreview.svelte` (329 lines)
- [ ] `MiniScriptEditor.svelte` (295 lines)
- [ ] `ModelFlagsSelect.svelte` (305 lines)
- [ ] `ModelPickers.svelte` (79 lines)
- [ ] `Observatory.svelte` (8371 lines)
- [ ] `OpenInPalette.svelte` (235 lines)
- [ ] `PRListTitlebar.svelte` (15 lines)
- [ ] `PRPeopleStack.svelte` (64 lines)
- [ ] `PRReviewModal.svelte` (180 lines)
- [ ] `PRTitlebarActions.svelte` (520 lines)
- [ ] `PdfPreview.svelte` (289 lines)
- [ ] `PermissionCard.svelte` (116 lines)
- [ ] `PermissionPrompts.svelte` (63 lines)
- [ ] `PlanCard.svelte` (115 lines)
- [ ] `PlanPanel.svelte` (609 lines)
- [ ] `ProjectPic.svelte` (49 lines)
- [ ] `ProviderIcon.svelte` (52 lines)
- [ ] `ProviderUsageMeter.svelte` (453 lines)
- [ ] `QuestionCard.svelte` (239 lines)
- [ ] `QuestionPrompts.svelte` (67 lines)
- [ ] `QuickOpen.svelte` (273 lines)
- [ ] `ResizeHandle.svelte` (126 lines)
- [ ] `ReviewFileList.svelte` (280 lines)
- [ ] `ReviewSurface.svelte` (229 lines)
- [ ] `Ring.svelte` (30 lines)
- [ ] `RunCommandPalette.svelte` (250 lines)
- [ ] `RunPalette.svelte` (171 lines)
- [ ] `ScheduleEditor.svelte` (334 lines)
- [ ] `ScheduledPrompts.svelte` (792 lines)
- [ ] `SchedulesTitlebar.svelte` (20 lines)
- [ ] `Select.svelte` (466 lines)
- [ ] `SessionCard.svelte` (88 lines)
- [ ] `SettingsGroup.svelte` (65 lines)
- [ ] `SettingsGroups.svelte` (15 lines)
- [ ] `ShortcutsHelp.svelte` (128 lines)
- [ ] `Sidebar.svelte` (1848 lines)
- [ ] `StopPalette.svelte` (164 lines)
- [ ] `StreamingMarkdown.svelte` (85 lines)
- [ ] `SyntaxPalette.svelte` (317 lines)
- [ ] `TabChip.svelte` (293 lines)
- [ ] `TabStrip.svelte` (290 lines)
- [ ] `TerminalDock.svelte` (856 lines)
- [~] `TerminalPane.svelte` (581 lines): `recur_terminal_view::TerminalView` + `TerminalElement`
  - [x] Attach on mount, paint replay, queue live output until the replay lands, ack in 32KiB batches (`tests/view.rs`)
  - [x] Fit: `termFitSize` over the padded content box (6/0/6/8), grid pinned to the bottom, 100ms debounced `resize` (`font.rs`, `tests/view.rs`)
  - [x] xterm options: font stack with ligatures off, size, line height, cell metrics from WebGL `_updateDimensions`, 10000 scrollback, `macOptionIsMeta`, `macOptionClickForcesSelection`, `rightClickSelectsWord` on mac, bar cursor with blink, `minimumContrastRatio` 1 (`font.rs`, `screen/render.rs`)
  - [x] WebGL colour pipeline: bold-as-bright, dim, inverse, hidden, selection at 30% with inactive variant, block cursor override, underline styles (single/double/curly/dotted/dashed) and colours, strikethrough, custom box/block/powerline glyphs (`screen/render.rs`, `custom_glyphs.rs`)
  - [x] Cursor styles (DECSCUSR), blink manager (600ms, idle stop at 5 min, pause on blur) (`blink.rs`)
  - [x] Keyboard: xterm `Keyboard.ts` evaluation, custom key handler (⌘ chords to the app, Ctrl+` to the dock, third-level chars as text), app cursor keys, PageUp/PageDown scroll (`input.rs`, `tests/view.rs`)
  - [x] IME: marked text drawn at the cursor, commit writes once, candidate window at the cursor cell (`tests/view.rs`)
  - [x] Copy (⌘C with selection), paste (⌘V, bracketed paste, `\n` to `\r`), Linux primary selection and middle-click paste
  - [x] Mouse: selection (click, double word, triple line, shift extend, alt column, drag autoscroll), alt-click moves the prompt cursor, mouse reporting X10/1000/1002/1003 in default/SGR/SGR-pixels, wheel reporting and alt-screen arrow keys (`tests/view.rs`, `screen/tests.rs`)
  - [x] Scrollbar: VS Code 14px slider, 50px wheel units, trackpad accumulation, track click and drag, 500ms hide with fades (`scrollbar.rs`, `tests/view.rs`)
  - [x] Search addon: regex/case/whole word, xterm's highlight limit, active match, result counts, incremental refresh (`screen/search.rs`, `tests/view.rs`)
  - [x] Find bar UI (`TerminalFindBar`): toggles, count label, Enter/Shift+Enter, Escape, ⌥⌘C/R/W
  - [x] Web links addon: URL regex, hover underline and pointer, ⌘/click opens http(s) in the browser; OSC 8 hyperlinks underline but do not open (the webview never showed xterm's confirm) (`screen/links.rs`)
  - [x] Focus: dock focus routing (`termDockOwnsFocus`), DECSET 1004 reports, window activation; hidden panes take no pointer events (`tests/view.rs`)
  - [x] Theme and font changes repaint without remount; frosted (transparent) cells when maximized on dark palettes (`theme.rs`, `tests/view.rs`)
  - [x] Explorer drops type quoted paths; title and exit events reach the dock; clear wipes screen and replay; dispose detaches (`tests/view.rs`)
  - [x] Query replies answered in the emulator (DA1/DA2, XTVERSION, DSR/CPR, DECRQM, DECRQSS, OSC 4/10/11/12, CSI t) (`screen/scanner.rs`)
  - [ ] SGR 53 overline: alacritty_terminal cells have no overline flag, so it is parsed and dropped
  - [ ] Pixel parity screenshot against the web pane (needs the app shell and screen recording permission)
- [ ] `TitleTooltip.svelte` (309 lines)
- [ ] `Toasts.svelte` (55 lines)
- [ ] `UserMentionSuggest.svelte` (105 lines)
- [ ] `WorkspaceBaseSelect.svelte` (93 lines)
- [ ] `WorkspaceBriefCard.svelte` (134 lines)
- [ ] `WorkspaceIssueButton.svelte` (50 lines)

### App shell

- [ ] `App.svelte` (1289 lines)

### Frontend logic modules

Non-visual TypeScript that holds real behavior (stores, keymaps, composer, editor integrations). Port the logic, not the DOM plumbing.

- [ ] `lib/agentchat.ts` (2226 lines)
- [ ] `lib/api.ts` (1461 lines)
- [ ] `lib/armed-confirm.ts` (52 lines)
- [ ] `lib/artifacts.ts` (198 lines)
- [ ] `lib/autosize-textarea.ts` (29 lines)
- [ ] `lib/bridge-bench.ts` (98 lines)
- [x] `lib/briefingtools.ts` (345 lines) → `recur_agent_tools::display::briefing_tools` (detectors, brief/context/present parsing incl. legacy `Opening`/`Presented` strings, hydration, labels; `briefingtools.test.ts` ported)
- [ ] `lib/bundle-reload.ts` (25 lines)
- [ ] `lib/caches/byte-lru.ts` (189 lines)
- [ ] `lib/caches/chat-heap-cache.ts` (211 lines)
- [ ] `lib/caches/chat-strip-cache.ts` (61 lines)
- [ ] `lib/caches/chat-transcript-ui.ts` (324 lines)
- [ ] `lib/caches/cm-session-tabs.ts` (112 lines)
- [ ] `lib/caches/observatory-cache.ts` (150 lines)
- [x] `lib/caches/term-park.ts` (123 lines): `recur_terminal_view::Park` + `TerminalPark` global
  - [x] Stash/take, 1MiB per entry, 48MiB budget with LRU eviction returning evicted views to detach (`park.rs`)
  - [x] Discard flag for closed tabs, `should_dispose` consumed on unmount, prune by live workspace ids (`park.rs`)
  - [x] Parked panes stay attached and keep updating (`tests/view.rs`)
  - Pane hooks (`bindTermPaneHooks`) are the view entity itself: the dock calls its methods directly.
- [ ] `lib/caches/workspace-land-cache.ts` (90 lines)
- [ ] `lib/changed-tree.ts` (118 lines)
- [ ] `lib/chat-glance.ts` (48 lines)
- [ ] `lib/chat-occlusion.ts` (38 lines)
- [ ] `lib/chat-route.ts` (28 lines)
- [ ] `lib/chat-surface.ts` (77 lines)
- [ ] `lib/chat-tab-reorder.ts` (74 lines)
- [ ] `lib/chat-tab-session.ts` (84 lines)
- [ ] `lib/chat-window.ts` (36 lines)
- [ ] `lib/check-duration.ts` (90 lines)
- [ ] `lib/chrome-focus.ts` (89 lines)
- [ ] `lib/clipboard.ts` (69 lines)
- [ ] `lib/cm-diff-gutter.ts` (207 lines)
- [ ] `lib/cm-diff-lines.ts` (48 lines)
- [ ] `lib/cm-editor-history.ts` (214 lines)
- [ ] `lib/cm-editor-prefs.ts` (232 lines)
- [ ] `lib/cm-findings-gutter.ts` (253 lines)
- [ ] `lib/cm-goto-line.ts` (125 lines)
- [ ] `lib/cm-keymap.ts` (18 lines)
- [ ] `lib/cm-lang-data.ts` (311 lines)
- [ ] `lib/cm-lang.ts` (234 lines)
- [ ] `lib/cm-line-diff.ts` (230 lines)
- [ ] `lib/cm-lint-gutter.ts` (215 lines)
- [ ] `lib/cm-mention-sel.ts` (71 lines)
- [ ] `lib/cm-merge-full-line-marks.ts` (90 lines)
- [ ] `lib/cm-merge-revert.ts` (299 lines)
- [ ] `lib/cm-mini-script-highlight.ts` (177 lines)
- [ ] `lib/cm-pointer-pos.ts` (277 lines)
- [ ] `lib/cm-search-panel.ts` (380 lines)
- [ ] `lib/cm-textmate.ts` (371 lines)
- [ ] `lib/cm-themes.ts` (255 lines)
- [ ] `lib/cm-tm-deco.ts` (151 lines)
- [ ] `lib/cm-tm-delta.ts` (84 lines)
- [ ] `lib/cm-tm-doc.ts` (58 lines)
- [ ] `lib/cm-tm-fold.ts` (97 lines)
- [ ] `lib/cm-tm-paint.ts` (15 lines)
- [ ] `lib/cm-tm-stacks.ts` (153 lines)
- [ ] `lib/codehl.ts` (279 lines)
- [ ] `lib/colors.ts` (159 lines)
- [ ] `lib/command-usage.ts` (99 lines)
- [ ] `lib/commands.ts` (489 lines)
- [ ] `lib/comment-header.ts` (22 lines)
- [ ] `lib/comment-hunk.ts` (107 lines)
- [ ] `lib/comment-permalink.ts` (91 lines)
- [ ] `lib/comment-reactions.ts` (137 lines)
- [ ] `lib/comment-thread.ts` (372 lines)
- [ ] `lib/commit-meta.ts` (12 lines)
- [ ] `lib/compatible-providers.ts` (81 lines)
- [ ] `lib/composerDom.ts` (1582 lines)
- [ ] `lib/composerUndo.ts` (102 lines)
- [x] `lib/contextUsage.ts` (90 lines) → `recur_agent::context_usage` (ContextUsage used/ready/after_compact, from_turn, from_chat_items, format_tokens with JS `toFixed`/`Math.round` rounding; `contextUsage.test.ts` ported)
- [ ] `lib/dialog-focus.ts` (84 lines)
- [ ] `lib/editor-dirty.ts` (60 lines)
- [ ] `lib/editor-session.ts` (870 lines)
- [ ] `lib/editor-view-state.ts` (443 lines)
- [ ] `lib/editor-view.ts` (136 lines)
- [ ] `lib/emoji.ts` (197 lines)
- [ ] `lib/explorer-visible.ts` (33 lines)
- [ ] `lib/file-drag.ts` (332 lines)
- [ ] `lib/file-icons/icon-tables.ts` (52 lines)
- [ ] `lib/file-icons/icon.ts` (31 lines)
- [ ] `lib/file-icons/index.ts` (41 lines)
- [ ] `lib/file-icons/types.ts` (26 lines)
- [ ] `lib/file-index.ts` (162 lines)
- [ ] `lib/finding-paths.ts` (112 lines)
- [ ] `lib/findings-ui.ts` (396 lines)
- [ ] `lib/format.ts` (51 lines)
- [ ] `lib/fuzzy.ts` (145 lines)
- [ ] `lib/glob.ts` (64 lines)
- [ ] `lib/hoist-chat-keys.ts` (286 lines)
- [ ] `lib/hot-shells.ts` (51 lines)
- [ ] `lib/hoverGate.ts` (50 lines)
- [ ] `lib/icons.ts` (1003 lines)
- [ ] `lib/inbox-read.ts` (28 lines)
- [ ] `lib/issue-link.ts` (36 lines)
- [ ] `lib/keybinding-menu.ts` (27 lines)
- [ ] `lib/keyequiv.ts` (285 lines)
- [ ] `lib/lineartools.ts` (320 lines)
- [ ] `lib/loop-cockpit.ts` (432 lines)
- [ ] `lib/loops.ts` (183 lines)
- [ ] `lib/lsp-lang.ts` (86 lines)
- [ ] `lib/lsp-problems.ts` (116 lines)
- [ ] `lib/lsp.ts` (1520 lines)
- [ ] `lib/lucide-icons.ts` (82 lines)
- [ ] `lib/macro-worker.ts` (479 lines)
- [ ] `lib/markdown-host.ts` (262 lines)
- [ ] `lib/markdown-parse.ts` (191 lines)
- [ ] `lib/markdown-worker.ts` (20 lines)
- [ ] `lib/markdown.ts` (51 lines)
- [x] `lib/mcp.ts` (69 lines) → `recur_agent::mcp` over `recur_domain::McpServer` (untrimmed id rule, slug, global off-list, Linear upsert, custom list, enable/remove/update keeping the id; `mcp.test.ts` ported)
- [ ] `lib/mdcopy.ts` (88 lines)
- [ ] `lib/menu-space.ts` (248 lines)
- [ ] `lib/menu.ts` (152 lines)
- [ ] `lib/mermaid.ts` (137 lines)
- [ ] `lib/models.ts` (179 lines)
- [ ] `lib/modes.ts` (419 lines)
- [ ] `lib/mono-fonts.ts` (236 lines)
- [ ] `lib/new-workspace.ts` (278 lines)
- [ ] `lib/open-external.ts` (81 lines)
- [ ] `lib/open-in-pref.ts` (34 lines)
- [ ] `lib/open-path.ts` (48 lines)
- [x] `lib/open-pr-tool.ts` (70 lines) → `recur_agent_tools::display::open_pr_tool` (`open-pr-tool.test.ts` ported)
- [ ] `lib/overlay-scrollbar.ts` (141 lines)
- [ ] `lib/pdf-preview.ts` (85 lines)
- [x] `lib/permissionTools.ts` (60 lines) → `recur_agent_tools::display::permission_tools::permission_tool_label`
- [x] `lib/plantools.ts` (120 lines) → `recur_agent_tools::display::plan_tools` (step/attachment/body parsing, labels, ack title recovery)
- [ ] `lib/pr-attention.ts` (98 lines)
- [ ] `lib/pr-checks.ts` (139 lines)
- [ ] `lib/pr-commits.ts` (23 lines)
- [ ] `lib/pr-detail-pending.ts` (48 lines)
- [ ] `lib/pr-fix-checks.ts` (53 lines)
- [ ] `lib/pr-list-filter.ts` (190 lines)
- [ ] `lib/pr-merge-action.ts` (276 lines)
- [ ] `lib/pr-participants.ts` (165 lines)
- [ ] `lib/pr-thread-resolved.ts` (43 lines)
- [ ] `lib/pr-workspace.ts` (107 lines)
- [ ] `lib/preview-find.ts` (260 lines)
- [ ] `lib/preview-scroll.ts` (237 lines)
- [ ] `lib/preview.ts` (139 lines)
- [ ] `lib/project-pic.ts` (120 lines)
- [ ] `lib/promptActions.ts` (438 lines)
- [ ] `lib/providerUsage.ts` (141 lines)
- [ ] `lib/providers.ts` (76 lines)
- [ ] `lib/rail-peek.ts` (106 lines)
- [x] `lib/reenter-phase-tool.ts` (32 lines) → `recur_agent_tools::display::reenter_phase_tool` (`reenter-phase-tool.test.ts` ported)
- [ ] `lib/remove-workspace.ts` (72 lines)
- [ ] `lib/review-diff.ts` (131 lines)
- [ ] `lib/review-scope.ts` (47 lines)
- [ ] `lib/review-scroll.ts` (18 lines)
- [ ] `lib/review-window.ts` (263 lines)
- [ ] `lib/run-commands.ts` (69 lines)
- [ ] `lib/run-dev.ts` (426 lines)
- [ ] `lib/sanitize-lsp.ts` (20 lines)
- [ ] `lib/schedule-expanded.ts` (34 lines)
- [ ] `lib/scroll-fade.ts` (132 lines)
- [ ] `lib/session-feed.svelte.ts` (314 lines)
- [ ] `lib/session-live.ts` (57 lines)
- [ ] `lib/shell-state.ts` (177 lines)
- [ ] `lib/shhl.ts` (99 lines)
- [ ] `lib/shortcut.ts` (67 lines)
- [ ] `lib/shortcuts.ts` (96 lines)
- [ ] `lib/simplify.ts` (13 lines)
- [ ] `lib/skills.ts` (176 lines)
- [ ] `lib/snippets.ts` (319 lines)
- [ ] `lib/split-diff.ts` (107 lines)
- [ ] `lib/spy-pick.ts` (115 lines)
- [ ] `lib/stores.ts` (1873 lines)
- [ ] `lib/stream-reveal.ts` (8 lines)
- [ ] `lib/subagent.ts` (79 lines)
- [ ] `lib/syntax-mode.ts` (213 lines)
- [ ] `lib/tab-strip-drag.svelte.ts` (392 lines)
- [ ] `lib/tab-strip-scroll.ts` (174 lines)
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
- [ ] `lib/title-tooltip.ts` (248 lines)
- [ ] `lib/titlebar.ts` (30 lines)
- [ ] `lib/tm-bootstrap.ts` (79 lines)
- [ ] `lib/tm-engine.ts` (624 lines)
- [ ] `lib/tm-fill.ts` (103 lines)
- [ ] `lib/tm-registry.ts` (628 lines)
- [ ] `lib/tm-scope-selector.ts` (121 lines)
- [ ] `lib/tm-symbols.ts` (297 lines)
- [ ] `lib/tm-theme.ts` (590 lines)
- [x] `lib/todos.ts` (38 lines) → `recur_agent::display::todos` (parse, counts, latest over `ChatItem`s; `todos.test.ts` ported)
- [ ] `lib/tree-sticky.ts` (137 lines)
- [ ] `lib/updater.ts` (42 lines)
- [ ] `lib/user-mention.ts` (200 lines)
- [ ] `lib/uuid.ts` (16 lines)
- [ ] `lib/wait-for-worktree.ts` (38 lines)
- [x] `lib/webtools.ts` (87 lines) → `recur_agent::display::web` (WebFetch URL/prompt, WebSearch query, `compact_url` on the WHATWG `url` crate, JSON sniff; `webtools.test.ts` ported)
- [ ] `lib/workspace-file.ts` (96 lines)
- [ ] `lib/workspace-nav.ts` (350 lines)
- [ ] `lib/workspace-path.ts` (31 lines)

### Styles

- [ ] `styles/_base.scss`
- [ ] `styles/_briefing.scss`
- [ ] `styles/_chat.scss`
- [ ] `styles/_codemirror.scss`
- [ ] `styles/_forms.scss`
- [ ] `styles/_loop.scss`
- [ ] `styles/_main.scss`
- [ ] `styles/_observatory.scss`
- [ ] `styles/_overlays.scss`
- [ ] `styles/_prs.scss`
- [ ] `styles/_settings.scss`
- [ ] `styles/_sidebar.scss`
- [~] `styles/_terminal.scss`
  - [x] Pane padding, bottom-pinned grid, inactive panes hidden and absolutely positioned (`view.rs`)
  - [x] Scrollbar slider colours from the palette, hover/active states (`element.rs`)
  - [x] Find bar (`cm-recur-search` look): field, toggles, count, buttons (`find_bar.rs`)
  - [x] Terminal tokens read from a local `_tokens.scss` table until `recur-ui` exposes them (`tokens.rs`)
  - [ ] Dock chrome (tab strip, maximize, beside layout) belongs to the `TerminalDock.svelte` port
  - [ ] Pixel parity screenshot
- [ ] `styles/_tokens.scss`
- [ ] `styles/_welcome.scss`
- [ ] `styles/app.scss`

## Go packages

Source: `internal/`, root `*.go`, `cmd/`. Each Go test file is a behavior spec: port its cases alongside the code.

- [ ] `internal/agenttools`: 52 files, 3802 lines, 3 test files → `recur-agent-tools` (all three Go test files ported)
  - [x] `Host` trait (engine seam) with Go `noopReporter` as default bodies (`NoopHost`), all request types, `HostError`; `Tool`/`Permitter`/`Call`/`Registry`, `advertised_name`, `bare_name`, `HALT_NAMES`, `halt_after`; `mcp__recur__*` and bare tool-name constants (`names`, `names::mcp`)
  - [x] All 42 host tools (`host_tools()` in `tools/list` order): names, descriptions and schemas byte-identical to Go (`tests/catalog.rs`), and 623 recorded calls replayed for identical result text, error text and host calls (`tests/host_golden.rs`, fixtures from `tests/golden/golden_gen_test.go.txt`)
  - [x] Go `encoding/json` decode semantics for tool args (`json`): scanner syntax errors, `Decoder` vs `Unmarshal`, case-insensitive and Kelvin/long-s field folding, first error wins, decode into existing values on duplicate keys, `UnmarshalTypeError` text with Go type names, `json.RawMessage`, `DisallowUnknownFields`; `%q` (`strconv.Quote`) and Go's per-byte U+FFFD handling (`gostr`)
  - [x] Worktree sandbox (`path`): `code.SafeJoin`, `filepath` Clean/Join/Rel/Abs/Dir/Base, `EvalSymlinks`, nearest-existing-ancestor resolution, `MkdirAll`, `*PathError` text
  - [x] Read, Edit, Write, Bash, TodoWrite + `TodoStore`: 75 recorded Go cases replayed (`tests/coding_golden.rs`); Bash keeps process-group kill on cancel/timeout, `WaitDelay` (2 s), capped capture, `exit: …` text with Go signal names per platform
  - [ ] Glob and Grep enabled by default: complete and tested (ripgrep and Go-walk fallback, `filepath.Match`), but behind the `code` feature until `recur-code` exports `rg::locate`/`search_opts` and the store port exports `data_dir` (slice C / store; see engine-port-plan 5.3 T)
- [ ] `internal/app`: 25 files, 8921 lines, 27 test files
  - [x] `instance.go` + test: `recur_platform::identity::{instance_label, window_title}` (title falls back to `Recur Next` side by side)
  - `open_path.go` stays with `recur-services`: it is WorkspacesService logic (store, engine, git), not OS path handling. Platform takes its result through `open_boot::attach_open_ipc` / `apply_cold_opens`.
  - [x] `model_catalog.go` catalog types → `recur_agent::catalog` (FlagOption, FlagDimension, ModelChoice, ModelGroup serde-identical; `defaultFlagsFromDims`). Per-provider lists, merge and fallbacks belong to P1/P2/P3 and SV
- [ ] `internal/bundles`: 19 files, 6525 lines, 20 test files
- [ ] `internal/cli`: 7 files, 830 lines, 2 test files
- [x] `internal/cliinstall`: 3 files, 568 lines, 3 test files (`recur_platform::cliinstall`, 25 tests)
  - [x] `bundled_cli`: `.app/Contents/MacOS/recur-cli` on macOS, sibling `recur-cli` on Linux
  - [x] `is_dev_bundle`: `*.dev.app`, Go checkout `bin/` (`go.mod`), plus a cargo `target/` dir in a workspace checkout
  - [x] `is_bundled_cli_path`, `ensure_link` (atomic `.tmp` + rename, same-file no-op, refuses regular files and foreign links)
  - [x] `destinations`: writable `/usr/local/bin` first, then `~/.local/bin`; link name `recur-next` side by side
  - [x] `install_from_app` / `install_bundled_cli`: no-op for dev builds, unbundled exes and `/opt/recur-next` package installs; Linux desktop entry (`build.recur.next.desktop`, Exec quoting, unchanged-file skip)
  - [x] `ensure_path`: login-shell PATH (marker-wrapped probe, fish syntax, 5s timeout with process-group kill, `$SHELL` → `id -P` → `/etc/passwd` → default shell), common user/system bin dirs, path_helper fallback, merge order and dedup
- [ ] `internal/code`: 18 files, 4497 lines, 7 test files
- [ ] `internal/codex`: 6 files, 1173 lines, 5 test files
- [ ] `internal/copyfiles`: 1 files, 414 lines, 1 test files
- [ ] `internal/cursor`: 12 files, 3691 lines, 17 test files
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
- [ ] `internal/git`: 12 files, 4392 lines, 14 test files
- [ ] `internal/lsp`: 5 files, 916 lines, 4 test files
- [x] `internal/mcp`: 6 files, 839 lines, 3 test files → `recur-mcp` (`server_test.go`, `tools_briefing_test.go`, `wrap_test.go` ported in full)
  - [x] `McpServer` on hyper HTTP/1.1: `POST /<token>/mcp`, stable token per agent reused across turns, extra tokens for overlapping registrations, 404 when inactive or unknown, 405 + `Allow: POST`, Go `ServeMux` path cleaning (307), `http.Error` bodies and headers, 202 for notifications, 32 MiB body cap, 10 s header timeout, request cancellation on client disconnect, 3 s graceful close
  - [x] JSON-RPC framing (`protocol`): id echoed verbatim (compacted), protocol version echo, error codes, `isError` tool results; 79 HTTP exchanges recorded from the Go server replayed byte for byte, including the `tools/list` body snapshot (`tests/http_golden.rs`)
  - [x] `names_from_allowed_tools` (never null), `append_permission_request`, 128-bit hex tokens
- [ ] `internal/observe`: 5 files, 1030 lines, 1 test files
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
- [ ] `internal/responses`: 4 files, 795 lines, 2 test files
- [ ] `internal/rg`: 2 files, 403 lines, 2 test files
- [ ] `internal/runconfig`: 1 files, 333 lines, 1 test files
- [x] `internal/shortid`: 1 files, 111 lines, 1 test files → `recur_domain::shortid` (test file ported)
  - [x] New (prefix/length validation, letter-first suffix, unbiased rejection sampling over OS randomness), NewUnique (attempt budget, Collision error), IsValid / VALID_PATTERN, MinLength/MaxLength/DefaultAttempts
- [ ] `internal/simplify`: 1 files, 56 lines, 1 test files
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
- [ ] `internal/usage`: 12 files, 2911 lines, 3 test files
  - [x] `types.go` → `recur_agent::quota` (Snapshot, ProviderQuota, Window, ModelRow, CursorUsageConnect serde-identical, provider/source/window/bucket constants, EmptySnapshot, setSource, clampPct) plus the `UsageProbe` trait
- [ ] `internal/wsenv`: 2 files, 325 lines, 2 test files
- [ ] `cmd/recur`: 1 files, 22 lines, 0 test files
- [ ] `cmd/bundles`: 1 files, 15 lines, 0 test files
- [ ] `cmd/seeddump`: 1 files, 86 lines, 0 test files

### Root desktop entry (`*.go` at repo root)

- [ ] `app_menu.go`
- [ ] `debug_devtools.go`
- [ ] `debug_devtools_off.go`
- [~] `main.go` (OS helpers ported; window, menu, service and webview wiring belong to `recur-desktop` / `recur-app`)
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
- [ ] `window_chrome_linux.go`
- [ ] `window_chrome_stub.go`
- [x] `window_state.go` + test (`recur_platform::window_state`, 6 tests)
  - [x] `window.json` format (camelCase, two-space indent), missing/malformed fallback, Go zero-value decoding, undersized clamp
  - [x] `WindowOptions::from_state`: size, min size, position or centred, background, titlebar height, file drop, instance-label title
  - [x] `GeometryTracker`: records only normal geometry, keeps maximised flag, zoom once on first show; 250ms save debounce constant for the window layer
- [ ] `window_titlebar.go`
- [ ] `window_titlebar_darwin.go`
- [ ] `window_titlebar_stub.go`

## Native app menu

Source: `app_menu.go`. Rust home: `recur-app::menu`. Every item, accelerator, enabled-state rule and dynamic submenu (themes, Open In) must match.

- [ ] Recur
- [ ] Check for Updates…
- [ ] Settings…
- [ ] File
- [ ] Open…
- [ ] New Workspace
- [ ] New Blank Workspace
- [ ] New Tab
- [ ] Save
- [ ] Open In…
- [ ] Copy Path
- [ ] Close Tab
- [ ] Quit
- [ ] View
- [ ] Appearance
- [ ] App Theme
- [ ] Code Editor Theme
- [ ] Dark
- [ ] Light
- [ ] Terminal Theme
- [ ] Previous Editor Theme
- [ ] Next Editor Theme
- [ ] Toggle Sidebar
- [ ] Toggle Left Panel
- [ ] Toggle Right Panel
- [ ] Toggle Editor
- [ ] Toggle Terminal
- [ ] Maximize Terminal
- [ ] Run…
- [ ] Stop Run
- [ ] Navigate
- [ ] Go to…
- [ ] Run Command…
- [ ] Keyboard Shortcuts…
- [ ] Reload Bundles
- [ ] Quick Open…
- [ ] Go to Symbol…
- [ ] Focus Agent Panel
- [ ] Focus Editor
- [ ] Focus Terminal
- [ ] Previous Tab
- [ ] Next Tab
- [ ] Previous Workspace
- [ ] Next Workspace

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

- [ ] `a11y-keys.spec.ts` (8 tests)
- [ ] `activity.spec.ts` (17 tests)
- [ ] `briefing.spec.ts` (18 tests)
- [ ] `bundles.spec.ts` (7 tests)
- [ ] `chat.spec.ts` (68 tests)
- [ ] `compatible-provider.spec.ts` (1 tests)
- [ ] `editor-hotpath.spec.ts` (27 tests)
- [ ] `editor-syntax.spec.ts` (2 tests)
- [ ] `explorer-scope.spec.ts` (8 tests)
- [ ] `loading-states.spec.ts` (1 tests)
- [ ] `loop-lifecycle.spec.ts` (4 tests)
- [ ] `maestro-dock.spec.ts` (3 tests)
- [ ] `observatory.spec.ts` (47 tests)
- [ ] `pr-review.spec.ts` (77 tests)
- [ ] `scheduled-prompts.spec.ts` (4 tests)
- [ ] `settings-newloop.spec.ts` (34 tests)
- [ ] `shell.spec.ts` (38 tests)
- [ ] `terminal-dock.spec.ts` (34 tests)
- [ ] `toasts.spec.ts` (3 tests)
- [ ] `welcome.spec.ts` (14 tests)
- [ ] `workspace-base.spec.ts` (37 tests)

Frontend unit tests (`vitest`, 170 files) are logic specs for the matching `lib/` modules; port their cases with the module.

## Shipped content (unchanged, embedded by the Rust app)

- [ ] `bundles/` seeded into the data dir exactly as `bundles.SeedDefaults` does (stamp/sync semantics)
- [ ] `recur-skills/` seeded into `~/.recur/recur-skills`
- [ ] `internal/store/migrations/*.sql` applied by `recur-store` (same files, same order, same `schema_migrations` bookkeeping)
- [ ] `frontend/src/lib/file-icons` icon font and mapping
- [ ] lucide icon set used by `lib/icons.ts`
- [ ] `build/` app icons, Info.plist keys, URL schemes, Linux `.desktop` entry

## CLI

Source: `cmd/recur`, `internal/cli`. Rust home: `crates/cli`.

- [ ] `recur bundles`
- [ ] `recur test [name|path]...`
- [ ] `recur lint [name|path]...`
- [ ] `recur create <Name>`
- [ ] `recur open [path]`
- [ ] `recur recur [path]`

