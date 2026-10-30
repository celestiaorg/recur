#![cfg(feature = "fake")]
//! Every service method answered by the fake. The failures are exactly the
//! calls the UI saw reject: a record the fake has no answer for (no default,
//! or a missing seed row) or a default's own error.

use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use recur_services::fake::{Fake, Overrides};

fn note(errs: &mut Vec<String>, name: &str, err: Option<recur_services::Error>) {
    if let Some(e) = err {
        errs.push(format!("{name}: {e}"));
    }
}

#[tokio::test]
async fn every_method_answers() {
    let s = Fake::fresh(Overrides::default()).services();
    let mut errs = Vec::new();
    note(
        &mut errs,
        "GetAgentTrace",
        s.agents.get_agent_trace("LOOP-2231", "PA").await.err(),
    );
    note(
        &mut errs,
        "ListLoopAgents",
        s.agents.list_loop_agents("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "DeleteArtifact",
        s.artifacts.delete_artifact("LOOP-2231", "a1").await.err(),
    );
    note(
        &mut errs,
        "ListArtifacts",
        s.artifacts.list_artifacts("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "PresentArtifact",
        s.artifacts.present_artifact("LOOP-2231", "a1").await.err(),
    );
    note(
        &mut errs,
        "RenameArtifact",
        s.artifacts
            .rename_artifact("LOOP-2231", "a1", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "RevealArtifact",
        s.artifacts.reveal_artifact("LOOP-2231", "a1").await.err(),
    );
    note(
        &mut errs,
        "AppChatItems",
        s.briefing.app_chat_items("PA").await.err(),
    );
    note(
        &mut errs,
        "AppChatSelection",
        s.briefing.app_chat_selection("PA").await.err(),
    );
    note(
        &mut errs,
        "CancelAppMessage",
        s.briefing.cancel_app_message("PA").await.err(),
    );
    note(
        &mut errs,
        "CloseAppChat",
        s.briefing.close_app_chat("PA").await.err(),
    );
    note(
        &mut errs,
        "GetBriefing",
        s.briefing.get_briefing().await.err(),
    );
    note(
        &mut errs,
        "ListAppChats",
        s.briefing.list_app_chats().await.err(),
    );
    note(
        &mut errs,
        "MarkAppChatSeen",
        s.briefing.mark_app_chat_seen("PA").await.err(),
    );
    note(
        &mut errs,
        "MarkWorkspaceViewed",
        s.briefing.mark_workspace_viewed("LOOP-2231", 1).await.err(),
    );
    note(
        &mut errs,
        "NewAppChat",
        s.briefing.new_app_chat().await.err(),
    );
    note(
        &mut errs,
        "RememberAppChatSelection",
        s.briefing
            .remember_app_chat_selection("PA", "x", &BTreeMap::new())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SendAppMessage",
        s.briefing
            .send_app_message("PA", "x", "x", &BTreeMap::new())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetAppChatFocus",
        s.briefing.set_app_chat_focus(false, "PA").await.err(),
    );
    note(
        &mut errs,
        "CompiledCompleteModule",
        s.bundle
            .compiled_complete_module("recur.text.uppercase")
            .await
            .err(),
    );
    note(
        &mut errs,
        "CompiledModule",
        s.bundle.compiled_module("recur.text.uppercase").await.err(),
    );
    note(
        &mut errs,
        "CompiledPromptModule",
        s.bundle
            .compiled_prompt_module("recur.text.uppercase")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ExpandGoal",
        s.bundle.expand_goal("x").await.err(),
    );
    note(
        &mut errs,
        "InvalidateCatalog",
        s.bundle.invalidate_catalog().await.err(),
    );
    note(
        &mut errs,
        "ListBundleSettings",
        s.bundle.list_bundle_settings().await.err(),
    );
    note(
        &mut errs,
        "ListCommands",
        s.bundle.list_commands("x", "src/main.go").await.err(),
    );
    note(
        &mut errs,
        "ListGrammars",
        s.bundle.list_grammars().await.err(),
    );
    note(&mut errs, "ListLoops", s.bundle.list_loops().await.err());
    note(
        &mut errs,
        "ListPolicies",
        s.bundle.list_policies().await.err(),
    );
    note(
        &mut errs,
        "ListPromptActions",
        s.bundle.list_prompt_actions().await.err(),
    );
    note(&mut errs, "ListRoles", s.bundle.list_roles().await.err());
    note(
        &mut errs,
        "ListSkippedBundles",
        s.bundle.list_skipped_bundles().await.err(),
    );
    note(
        &mut errs,
        "ListSnippets",
        s.bundle.list_snippets("x", "src/main.go").await.err(),
    );
    note(
        &mut errs,
        "ListSymbols",
        s.bundle.list_symbols().await.err(),
    );
    note(
        &mut errs,
        "ListTerminalThemes",
        s.bundle.list_terminal_themes().await.err(),
    );
    note(&mut errs, "ListThemes", s.bundle.list_themes().await.err());
    note(
        &mut errs,
        "ReadGrammar",
        s.bundle.read_grammar("x").await.err(),
    );
    note(
        &mut errs,
        "ReadTheme",
        s.bundle.read_theme("recur.text.uppercase").await.err(),
    );
    note(
        &mut errs,
        "RunCommand",
        s.bundle
            .run_command("recur.text.uppercase", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "RunPromptAction",
        s.bundle
            .run_prompt_action("recur.text.uppercase", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "RunPromptComplete",
        s.bundle
            .run_prompt_complete("recur.text.uppercase", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SearchPalette",
        s.bundle.search_palette("x").await.err(),
    );
    note(
        &mut errs,
        "AbsPath",
        s.code
            .abs_path("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "BridgeBenchDone",
        s.code.bridge_bench_done("x").await.err(),
    );
    note(
        &mut errs,
        "BridgeBenchEnabled",
        s.code.bridge_bench_enabled().await.err(),
    );
    note(&mut errs, "BridgeEcho", s.code.bridge_echo("x").await.err());
    note(&mut errs, "BridgePing", s.code.bridge_ping(1).await.err());
    note(
        &mut errs,
        "Commit",
        s.code.commit("storefront", "LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "CommitDiff",
        s.code
            .commit_diff("storefront", "LOOP-2231", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "CommitStaged",
        s.code
            .commit_staged("storefront", "LOOP-2231", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "Copy",
        s.code.copy("storefront", "LOOP-2231", "x", "x").await.err(),
    );
    note(
        &mut errs,
        "CreateFile",
        s.code
            .create_file("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "Delete",
        s.code
            .delete("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "DiffVsBase",
        s.code.diff_vs_base("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "DiffWorking",
        s.code.diff_working("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "DiscardAll",
        s.code.discard_all("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "DiscardFile",
        s.code
            .discard_file("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "FileAtBase",
        s.code
            .file_at_base("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "FileDiff",
        s.code
            .file_diff("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "FindDefs",
        s.code.find_defs("storefront", "LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "FindRefs",
        s.code.find_refs("storefront", "LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "GenerateCommitMessage",
        s.code
            .generate_commit_message("storefront", "LOOP-2231")
            .await
            .err(),
    );
    note(
        &mut errs,
        "GitStatus",
        s.code.git_status("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListAllSkills",
        s.code
            .list_all_skills("storefront", "LOOP-2231")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ListChangedFiles",
        s.code.list_changed_files("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListCommits",
        s.code.list_commits("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListFiles",
        s.code.list_files("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListSkills",
        s.code.list_skills("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListTree",
        s.code
            .list_tree("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ListTrees",
        s.code
            .list_trees("storefront", "LOOP-2231", &[])
            .await
            .err(),
    );
    note(
        &mut errs,
        "Mkdir",
        s.code
            .mkdir("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "OpenInEditor",
        s.code
            .open_in_editor("storefront", "LOOP-2231", "src/main.go", 1)
            .await
            .err(),
    );
    note(
        &mut errs,
        "Outline",
        s.code
            .outline("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ReadFile",
        s.code
            .read_file("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "RefreshFileIndex",
        s.code
            .refresh_file_index("storefront", "LOOP-2231")
            .await
            .err(),
    );
    note(
        &mut errs,
        "Rename",
        s.code
            .rename("storefront", "LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ReplaceInFiles",
        s.code
            .replace_in_files("storefront", "LOOP-2231", "x", "x", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "RevealInFinder",
        s.code
            .reveal_in_finder("storefront", "LOOP-2231", "src/main.go")
            .await
            .err(),
    );
    note(
        &mut errs,
        "Search",
        s.code.search("storefront", "LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "SearchAdvanced",
        s.code
            .search_advanced("storefront", "LOOP-2231", "x", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SearchFiles",
        s.code
            .search_files("storefront", "LOOP-2231", "x", 1, false)
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetFileIndexExclude",
        s.code.set_file_index_exclude(&[]).await.err(),
    );
    note(
        &mut errs,
        "Stage",
        s.code.stage("storefront", "LOOP-2231", &[]).await.err(),
    );
    note(
        &mut errs,
        "StagedFiles",
        s.code.staged_files("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "Unstage",
        s.code.unstage("storefront", "LOOP-2231", &[]).await.err(),
    );
    note(
        &mut errs,
        "WorkingStatus",
        s.code.working_status("storefront", "LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "WriteFile",
        s.code
            .write_file("storefront", "LOOP-2231", "src/main.go", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ClearEditorSession",
        s.editor_session
            .clear_editor_session("LOOP-2231")
            .await
            .err(),
    );
    note(
        &mut errs,
        "GetEditorSession",
        s.editor_session.get_editor_session("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "UpdateEditorSession",
        s.editor_session
            .update_editor_session("LOOP-2231", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "AnswerEscalation",
        s.escalation
            .answer_escalation("LOOP-2231", "x", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "GetEscalation",
        s.escalation.get_escalation("LOOP-2231").await.err(),
    );
    note(&mut errs, "GetInbox", s.inbox.get_inbox().await.err());
    note(
        &mut errs,
        "MarkAllInboxRead",
        s.inbox.mark_all_inbox_read().await.err(),
    );
    note(
        &mut errs,
        "MarkInboxRead",
        s.inbox.mark_inbox_read("ib1").await.err(),
    );
    note(&mut errs, "CloseLSP", s.lsp.close_lsp("l1").await.err());
    note(
        &mut errs,
        "CloseWorkspaceLSP",
        s.lsp.close_workspace_lsp("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "EnsureLSP",
        s.lsp
            .ensure_lsp("storefront", "LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ListLanguageServers",
        s.lsp.list_language_servers().await.err(),
    );
    note(
        &mut errs,
        "WriteLSP",
        s.lsp.write_lsp("l1", "x").await.err(),
    );
    note(
        &mut errs,
        "CancelMaestroMessage",
        s.maestro.cancel_maestro_message("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "GetMaestroChat",
        s.maestro.get_maestro_chat("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "SendMaestroMessage",
        s.maestro
            .send_maestro_message("LOOP-2231", "x", &[])
            .await
            .err(),
    );
    note(
        &mut errs,
        "AddFindingNote",
        s.observatory
            .add_finding_note("LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "CreateFinding",
        s.observatory
            .create_finding("LOOP-2231", "x", 1, "x", 1)
            .await
            .err(),
    );
    note(
        &mut errs,
        "DeleteFindingNote",
        s.observatory
            .delete_finding_note("LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ListFindings",
        s.observatory.list_findings("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "SetFindingStatus",
        s.observatory
            .set_finding_status("LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "AnswerPermission",
        s.permissions
            .answer_permission("p1", false, "x", false)
            .await
            .err(),
    );
    note(
        &mut errs,
        "ListPendingPermissions",
        s.permissions.list_pending_permissions().await.err(),
    );
    note(&mut errs, "GetPolicy", s.policy.get_policy().await.err());
    note(
        &mut errs,
        "SetPolicy",
        s.policy.set_policy(&Default::default()).await.err(),
    );
    note(
        &mut errs,
        "AddProject",
        s.projects.add_project(&Default::default()).await.err(),
    );
    note(
        &mut errs,
        "AddProjectGroup",
        s.projects
            .add_project_group(&Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "DeleteProject",
        s.projects.delete_project("storefront").await.err(),
    );
    note(
        &mut errs,
        "DeleteProjectGroup",
        s.projects.delete_project_group("storefront").await.err(),
    );
    note(
        &mut errs,
        "DeleteScheduledPrompt",
        s.projects.delete_scheduled_prompt("storefront").await.err(),
    );
    note(
        &mut errs,
        "GetProject",
        s.projects.get_project("storefront").await.err(),
    );
    note(
        &mut errs,
        "ListBranches",
        s.projects.list_branches("storefront").await.err(),
    );
    note(
        &mut errs,
        "ListBranchesForPath",
        s.projects.list_branches_for_path("x", "x").await.err(),
    );
    note(
        &mut errs,
        "ListProjectGroups",
        s.projects.list_project_groups().await.err(),
    );
    note(
        &mut errs,
        "ListProjects",
        s.projects.list_projects().await.err(),
    );
    note(
        &mut errs,
        "ListScheduledPrompts",
        s.projects.list_scheduled_prompts("storefront").await.err(),
    );
    note(
        &mut errs,
        "PauseScheduledPrompt",
        s.projects
            .pause_scheduled_prompt("storefront", false)
            .await
            .err(),
    );
    note(
        &mut errs,
        "PreviewCopyFiles",
        s.projects.preview_copy_files("storefront", "x").await.err(),
    );
    note(
        &mut errs,
        "PreviewScheduledPrompt",
        s.projects
            .preview_scheduled_prompt(&Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "ReorderProjectGroups",
        s.projects.reorder_project_groups(&[]).await.err(),
    );
    note(
        &mut errs,
        "ReorderProjects",
        s.projects.reorder_projects(&[]).await.err(),
    );
    note(
        &mut errs,
        "RunScheduledPromptNow",
        s.projects
            .run_scheduled_prompt_now("storefront")
            .await
            .err(),
    );
    note(
        &mut errs,
        "SaveScheduledPrompt",
        s.projects
            .save_scheduled_prompt(&Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetProjectGroup",
        s.projects.set_project_group("storefront", "x").await.err(),
    );
    note(
        &mut errs,
        "SuggestBaseBranch",
        s.projects.suggest_base_branch("x").await.err(),
    );
    note(
        &mut errs,
        "UpdateProjectGroup",
        s.projects
            .update_project_group(&Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "UpdateProjectRunConfig",
        s.projects
            .update_project_run_config(&Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "AddPRComment",
        s.prs.add_pr_comment("482", "x", 1, "x", 1, "x").await.err(),
    );
    note(
        &mut errs,
        "CreateWorkspaceFromPR",
        s.prs.create_workspace_from_pr("482").await.err(),
    );
    note(
        &mut errs,
        "DeletePRComment",
        s.prs.delete_pr_comment("482", "x").await.err(),
    );
    note(
        &mut errs,
        "EnsurePRChatHost",
        s.prs.ensure_pr_chat_host("482").await.err(),
    );
    note(
        &mut errs,
        "GetCommitDiff",
        s.prs.get_commit_diff("482", "x").await.err(),
    );
    note(&mut errs, "GetPR", s.prs.get_pr("482").await.err());
    note(
        &mut errs,
        "GetPRCommits",
        s.prs.get_pr_commits("482").await.err(),
    );
    note(
        &mut errs,
        "GetPRFiles",
        s.prs.get_pr_files("482").await.err(),
    );
    note(
        &mut errs,
        "ListPRFindings",
        s.prs.list_pr_findings("482").await.err(),
    );
    note(&mut errs, "ListPRs", s.prs.list_prs().await.err());
    note(&mut errs, "MergePR", s.prs.merge_pr("482", "x").await.err());
    note(&mut errs, "ReadyPR", s.prs.ready_pr("482").await.err());
    note(
        &mut errs,
        "ReplyPRComment",
        s.prs.reply_pr_comment("482", "x", "x").await.err(),
    );
    note(
        &mut errs,
        "SearchPRMentionableUsers",
        s.prs.search_pr_mentionable_users("482", "x").await.err(),
    );
    note(
        &mut errs,
        "SendBackToLoop",
        s.prs.send_back_to_loop("482", &[]).await.err(),
    );
    note(
        &mut errs,
        "SetPRDraft",
        s.prs.set_pr_draft("482", false).await.err(),
    );
    note(
        &mut errs,
        "SetPRThreadResolved",
        s.prs.set_pr_thread_resolved("482", "x", false).await.err(),
    );
    note(
        &mut errs,
        "SubmitReview",
        s.prs.submit_review("482", "x", "x").await.err(),
    );
    note(
        &mut errs,
        "SyncGitHubPRs",
        s.prs.sync_github_prs().await.err(),
    );
    note(
        &mut errs,
        "TogglePRCommentReaction",
        s.prs
            .toggle_pr_comment_reaction("482", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "UpdatePRComment",
        s.prs.update_pr_comment("482", "x", "x").await.err(),
    );
    note(
        &mut errs,
        "UpdatePRDetails",
        s.prs.update_pr_details("482", "x", "x").await.err(),
    );
    note(
        &mut errs,
        "UpdateWithBase",
        s.prs.update_with_base("482", "x").await.err(),
    );
    note(
        &mut errs,
        "AnswerQuestion",
        s.questions.answer_question("q1", "x", "x", &[]).await.err(),
    );
    note(
        &mut errs,
        "ListPendingQuestions",
        s.questions.list_pending_questions().await.err(),
    );
    note(
        &mut errs,
        "CancelCursorUsageConnect",
        s.settings.cancel_cursor_usage_connect().await.err(),
    );
    note(
        &mut errs,
        "ClearAllData",
        s.settings.clear_all_data().await.err(),
    );
    note(
        &mut errs,
        "CompatibleFlagPreset",
        s.settings.compatible_flag_preset("x").await.err(),
    );
    note(
        &mut errs,
        "ConnectCursorUsage",
        s.settings.connect_cursor_usage().await.err(),
    );
    note(
        &mut errs,
        "DetectAgent",
        s.settings.detect_agent().await.err(),
    );
    note(
        &mut errs,
        "DisconnectCursorUsage",
        s.settings.disconnect_cursor_usage().await.err(),
    );
    note(
        &mut errs,
        "GetClaudeStatus",
        s.settings.get_claude_status().await.err(),
    );
    note(
        &mut errs,
        "GetCodexStatus",
        s.settings.get_codex_status().await.err(),
    );
    note(
        &mut errs,
        "GetCursorStatus",
        s.settings.get_cursor_status().await.err(),
    );
    note(
        &mut errs,
        "GetInstanceLabel",
        s.settings.get_instance_label().await.err(),
    );
    note(
        &mut errs,
        "GetProviderUsage",
        s.settings.get_provider_usage().await.err(),
    );
    note(
        &mut errs,
        "GetResource",
        s.settings.get_resource().await.err(),
    );
    note(
        &mut errs,
        "GetSettings",
        s.settings.get_settings().await.err(),
    );
    note(
        &mut errs,
        "GitHubLogin",
        s.settings.github_login().await.err(),
    );
    note(
        &mut errs,
        "InstallCursorBridge",
        s.settings.install_cursor_bridge().await.err(),
    );
    note(
        &mut errs,
        "ListAssignedIssues",
        s.settings.list_assigned_issues().await.err(),
    );
    note(
        &mut errs,
        "ListModelCatalog",
        s.settings.list_model_catalog().await.err(),
    );
    note(
        &mut errs,
        "ListMonospaceFonts",
        s.settings.list_monospace_fonts().await.err(),
    );
    note(&mut errs, "LogsPath", s.settings.logs_path().await.err());
    note(
        &mut errs,
        "OpenLinearInApp",
        s.settings.open_linear_in_app("x").await.err(),
    );
    note(
        &mut errs,
        "RefreshProviderUsage",
        s.settings.refresh_provider_usage().await.err(),
    );
    note(
        &mut errs,
        "RevealLogs",
        s.settings.reveal_logs().await.err(),
    );
    note(
        &mut errs,
        "SetSettings",
        s.settings.set_settings(&Default::default()).await.err(),
    );
    note(
        &mut errs,
        "TestCodexKey",
        s.settings.test_codex_key("x", false).await.err(),
    );
    note(
        &mut errs,
        "TestCompatibleProvider",
        s.settings
            .test_compatible_provider(&Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "TestCursorKey",
        s.settings.test_cursor_key("x").await.err(),
    );
    note(&mut errs, "Flush", s.shell.flush().await.err());
    note(
        &mut errs,
        "GetExpandedSchedules",
        s.shell.get_expanded_schedules().await.err(),
    );
    note(
        &mut errs,
        "GetIssuesView",
        s.shell.get_issues_view().await.err(),
    );
    note(
        &mut errs,
        "GetShellState",
        s.shell.get_shell_state().await.err(),
    );
    note(
        &mut errs,
        "SetExpandedSchedules",
        s.shell.set_expanded_schedules(&[]).await.err(),
    );
    note(
        &mut errs,
        "SetIssuesView",
        s.shell.set_issues_view(&Default::default()).await.err(),
    );
    note(
        &mut errs,
        "SetShellState",
        s.shell.set_shell_state(&Default::default()).await.err(),
    );
    note(
        &mut errs,
        "AckTerminal",
        s.terminal.ack_terminal("t1", 1).await.err(),
    );
    note(
        &mut errs,
        "AttachTerminal",
        s.terminal.attach_terminal("t1").await.err(),
    );
    note(
        &mut errs,
        "ClearTerminal",
        s.terminal.clear_terminal("t1").await.err(),
    );
    note(
        &mut errs,
        "CloseTerminal",
        s.terminal.close_terminal("t1").await.err(),
    );
    note(
        &mut errs,
        "DetachTerminal",
        s.terminal.detach_terminal("t1").await.err(),
    );
    note(
        &mut errs,
        "InterruptTerminal",
        s.terminal.interrupt_terminal("t1").await.err(),
    );
    note(
        &mut errs,
        "ListTerminalDockChrome",
        s.terminal.list_terminal_dock_chrome().await.err(),
    );
    note(
        &mut errs,
        "ListTerminals",
        s.terminal.list_terminals("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "OpenTerminal",
        s.terminal.open_terminal("LOOP-2231", 1, 1).await.err(),
    );
    note(
        &mut errs,
        "ReorderTerminals",
        s.terminal.reorder_terminals("LOOP-2231", &[]).await.err(),
    );
    note(
        &mut errs,
        "ResizeTerminal",
        s.terminal.resize_terminal("t1", 1, 1).await.err(),
    );
    note(
        &mut errs,
        "RestoreWorkspaceTerminals",
        s.terminal
            .restore_workspace_terminals("LOOP-2231", 1, 1)
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetActiveTerminal",
        s.terminal
            .set_active_terminal("LOOP-2231", "t1")
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetTerminalDockChrome",
        s.terminal
            .set_terminal_dock_chrome("LOOP-2231", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetTerminalTitle",
        s.terminal.set_terminal_title("t1", "x").await.err(),
    );
    note(
        &mut errs,
        "WriteTerminal",
        s.terminal.write_terminal("t1", "x").await.err(),
    );
    note(
        &mut errs,
        "ApplyProvisionalChatTitles",
        s.workspaces
            .apply_provisional_chat_titles("LOOP-2231", "PA", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ApprovePlan",
        s.workspaces.approve_plan("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "AttentionCounts",
        s.workspaces.attention_counts().await.err(),
    );
    note(
        &mut errs,
        "CancelWorkspaceMessage",
        s.workspaces
            .cancel_workspace_message("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ClearWorkspaceIssue",
        s.workspaces.clear_workspace_issue("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "CloseWorkspaceChat",
        s.workspaces
            .close_workspace_chat("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "CreateWorkspace",
        s.workspaces
            .create_workspace("storefront", "x", "x", "agent", "x", "x", None)
            .await
            .err(),
    );
    note(
        &mut errs,
        "CreateWorkspaceChat",
        s.workspaces.create_workspace_chat("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "CreateWorkspaceWith",
        s.workspaces
            .create_workspace_with(
                "storefront",
                "x",
                "x",
                "agent",
                "x",
                "x",
                "x",
                "x",
                None,
                &[],
            )
            .await
            .err(),
    );
    note(
        &mut errs,
        "DeleteClosedWorkspaceChat",
        s.workspaces
            .delete_closed_workspace_chat("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "DeleteWorkspace",
        s.workspaces.delete_workspace("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "EngageLoop",
        s.workspaces.engage_loop("LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "EngageLoopWith",
        s.workspaces
            .engage_loop_with("LOOP-2231", "x", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "FetchWorkingCopyUpstream",
        s.workspaces
            .fetch_working_copy_upstream("LOOP-2231")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ForkWorkspaceChat",
        s.workspaces
            .fork_workspace_chat("LOOP-2231", "PA", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ForkWorkspaceChatToWorkspace",
        s.workspaces
            .fork_workspace_chat_to_workspace("LOOP-2231", "PA", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "GetWorkspace",
        s.workspaces.get_workspace("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "GetWorkspaceFeed",
        s.workspaces.get_workspace_feed("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "LastAssistantText",
        s.workspaces.last_assistant_text("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListClosedWorkspaceChats",
        s.workspaces
            .list_closed_workspace_chats("LOOP-2231")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ListOpenInTargets",
        s.workspaces.list_open_in_targets().await.err(),
    );
    note(
        &mut errs,
        "ListWorkspaceChats",
        s.workspaces.list_workspace_chats("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ListWorkspaces",
        s.workspaces.list_workspaces().await.err(),
    );
    note(
        &mut errs,
        "MarkWorkspaceChatSeen",
        s.workspaces
            .mark_workspace_chat_seen("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "MergeWorkspace",
        s.workspaces.merge_workspace("LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "OpenMainWorkspace",
        s.workspaces.open_main_workspace("storefront").await.err(),
    );
    note(
        &mut errs,
        "OpenPath",
        s.workspaces.open_path("src/main.go").await.err(),
    );
    note(
        &mut errs,
        "OpenWorkspaceIn",
        s.workspaces.open_workspace_in("LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "OpenWorkspacePR",
        s.workspaces
            .open_workspace_pr("LOOP-2231", false)
            .await
            .err(),
    );
    note(
        &mut errs,
        "PauseLoop",
        s.workspaces.pause_loop("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "PullWorkspacePR",
        s.workspaces.pull_workspace_pr("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "PushWorkingCopy",
        s.workspaces.push_working_copy("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "RememberChatLoopPolicy",
        s.workspaces
            .remember_chat_loop_policy("LOOP-2231", "PA", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "RememberWorkspaceChatPromptGroup",
        s.workspaces
            .remember_workspace_chat_prompt_group("LOOP-2231", "PA", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "RememberWorkspaceChatSelection",
        s.workspaces
            .remember_workspace_chat_selection(
                "LOOP-2231",
                "PA",
                "x",
                &BTreeMap::new(),
                "agent",
                None,
            )
            .await
            .err(),
    );
    note(
        &mut errs,
        "RememberWorkspaceChatSimplify",
        s.workspaces
            .remember_workspace_chat_simplify("LOOP-2231", "PA", false)
            .await
            .err(),
    );
    note(
        &mut errs,
        "RemoveWorkspace",
        s.workspaces.remove_workspace("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ReopenWorkspaceChat",
        s.workspaces
            .reopen_workspace_chat("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "ReorderWorkspaceChats",
        s.workspaces
            .reorder_workspace_chats("LOOP-2231", &[])
            .await
            .err(),
    );
    note(
        &mut errs,
        "ResetWorkspaceChat",
        s.workspaces
            .reset_workspace_chat("LOOP-2231", "PA", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "RestartLoop",
        s.workspaces.restart_loop("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "ResumeLoop",
        s.workspaces.resume_loop("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "RetryLoop",
        s.workspaces.retry_loop("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "RetryWorkspaceSetup",
        s.workspaces.retry_workspace_setup("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "SendWorkspaceMessage",
        s.workspaces
            .send_workspace_message(
                "LOOP-2231",
                "PA",
                "x",
                "x",
                &BTreeMap::new(),
                &[],
                "agent",
                "x",
                false,
            )
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetChatUIOccluded",
        s.workspaces.set_chat_ui_occluded(false).await.err(),
    );
    note(
        &mut errs,
        "SetWorkspaceBaseBranch",
        s.workspaces
            .set_workspace_base_branch("LOOP-2231", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetWorkspaceChatFocus",
        s.workspaces
            .set_workspace_chat_focus("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetWorkspaceIssue",
        s.workspaces
            .set_workspace_issue("LOOP-2231", &Default::default())
            .await
            .err(),
    );
    note(
        &mut errs,
        "SetWorkspaceLoopPolicy",
        s.workspaces
            .set_workspace_loop_policy("LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "StartWorkspace",
        s.workspaces
            .start_workspace("storefront", "x", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "StopLoop",
        s.workspaces.stop_loop("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "SubmitPlan",
        s.workspaces
            .submit_plan("LOOP-2231", "x", "x", &[], &[])
            .await
            .err(),
    );
    note(
        &mut errs,
        "UpdatePlan",
        s.workspaces.update_plan("LOOP-2231", "x").await.err(),
    );
    note(
        &mut errs,
        "UpdatePlanStep",
        s.workspaces
            .update_plan_step("LOOP-2231", "x", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "UpdateWorkingCopyFromRemote",
        s.workspaces
            .update_working_copy_from_remote("LOOP-2231", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "UpdateWorkspaceWithBase",
        s.workspaces
            .update_workspace_with_base("LOOP-2231", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "WatchWorkspace",
        s.workspaces.watch_workspace("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "WorkspaceChatItems",
        s.workspaces
            .workspace_chat_items("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "WorkspaceChatModels",
        s.workspaces.workspace_chat_models().await.err(),
    );
    note(
        &mut errs,
        "WorkspaceChatSelection",
        s.workspaces
            .workspace_chat_selection("LOOP-2231", "PA")
            .await
            .err(),
    );
    note(
        &mut errs,
        "WorkspaceChatTurnDiff",
        s.workspaces
            .workspace_chat_turn_diff("LOOP-2231", "PA", "x")
            .await
            .err(),
    );
    note(
        &mut errs,
        "WorkspaceDiscardRisk",
        s.workspaces.workspace_discard_risk("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "WorkspaceFacts",
        s.workspaces.workspace_facts("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "WorkspaceLandState",
        s.workspaces.workspace_land_state("LOOP-2231").await.err(),
    );
    note(
        &mut errs,
        "WorkspaceSessions",
        s.workspaces
            .workspace_sessions("LOOP-2231", "x", "x")
            .await
            .err(),
    );
    // Records without a default answer, missing
    // grammar/theme/project/comment rows, and a reorder that must name every
    // open tab (MA, PA and the PA2/PA3 that create and fork opened).
    let expected = [
        "RenameArtifact: not answered by the fake",
        "ReadGrammar: grammar x not found",
        "ReadTheme: theme recur.text.uppercase not found",
        "SearchPalette: not answered by the fake",
        "EnsureLSP: not answered by the fake",
        "AddFindingNote: not answered by the fake",
        "DeleteFindingNote: not answered by the fake",
        "SaveScheduledPrompt: not answered by the fake",
        "SetProjectGroup: not answered by the fake",
        "ReplyPRComment: not answered by the fake",
        "SendBackToLoop: not answered by the fake",
        "UpdatePRComment: not answered by the fake",
        "TestCodexKey: not answered by the fake",
        "TestCompatibleProvider: not answered by the fake",
        "TestCursorKey: not answered by the fake",
        "GetIssuesView: not answered by the fake",
        "OpenTerminal: not answered by the fake",
        "ClearWorkspaceIssue: not answered by the fake",
        "ReorderWorkspaceChats: reorder chats: got 0 codes, have 4 tabs",
        "SetWorkspaceIssue: not answered by the fake",
    ];
    assert_eq!(errs, expected);
}
