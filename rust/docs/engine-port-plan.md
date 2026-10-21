# Engine port plan

This is the work plan for porting Recur's orchestration core to Rust: `internal/engine`, its providers, the GitHub/PR code, the issue code, the agent host tools, the MCP server, the services layer, and the support packages they lean on. Several agents work from it in parallel, one slice each. Read `ARCHITECTURE.md` and `PARITY.md` first; this document assumes both.

It has seven parts:

1. Subsystem map: every Go file in scope, the Rust crate and module it lands in, and what it does.
2. Trait designs for the three extension points: `recur_agent::Provider`, `recur_forge::Forge`, `recur_issues::IssueTracker` (plus the agent-tools `Host` seam).
3. The engine event bus: every event, its payload, and its UI name.
4. The concurrency model: every goroutine, lock, channel and context, mapped to tokio, with the invariants that must survive.
5. Work slices and waves.
6. Risks and hidden coupling.
7. What slice owners need from `recur-domain` and `recur-store` (ported in parallel).

Sizes below are Go source lines without tests. "Go" always means the current implementation under `internal/`.

## How to read ownership

- A file listed under one slice is owned by that slice. When a Go file mixes concerns it is marked **MIXED** in the map and the split is spelled out; each half has one owner.
- `recur-engine` is one crate split across six slices. Its modules call each other in cycles (Go is one package). Two rules keep parallel work sane:
  1. **A function lives with its deepest dependency.** If Go function `f` in file X calls into code owned by a later slice, the later slice owns `f` (or the call site inside `f`). Section 5 lists these as *deferred call sites*: the earlier slice ships the function without that edge and the later slice adds the edge when it lands. Nothing ships a stub; a function either has its full behavior or is not merged yet.
  2. **Milestones unblock same-wave work.** Where a slice in wave N needs a few functions from another wave-N slice, the provider of those functions lands them first as a named milestone (for example E3-M1). Dependents may start their other work before the milestone lands.
- Shared files with per-method ownership: `crates/engine/src/host.rs` (the `recur_agent_tools::Host` implementation). E2 creates it; each later engine slice adds the methods it owns (listed per slice). Unported methods keep the trait's default bodies, which are exactly Go's `noopReporter` behavior (see 2.4).

## Module naming

Rust module paths in the map use these crate names:

| Crate dir | Package name | Notes |
|---|---|---|
| `crates/agent` | `recur-agent` | Provider trait, turn types, events, display vocabulary, error classes |
| `crates/agent-tools` | `recur-agent-tools` | Host + coding tools, Host trait |
| `crates/mcp` | `recur-mcp` | MCP HTTP server |
| `crates/forge` | `recur-forge` | Forge trait + GitHub implementation |
| `crates/issues` | `recur-issues` | IssueTracker trait + GitHub issues + Linear |
| `crates/code` | `recur-code` | worktree files, search, rg, copyfiles, observe |
| `crates/lsp` | `recur-lsp` | language-server client + managed installs |
| `crates/usage` | `recur-usage` | quota poller and snapshot |
| `crates/engine` | `recur-engine` | orchestration |
| `crates/services` | `recur-services` | UI-facing API, AppEvent, composition root |
| `providers/claude` | `recur-provider-claude` | Claude Code CLI |
| `providers/codex` | `recur-provider-codex` | Codex app-server |
| `providers/cursor` | `recur-provider-cursor` | Cursor SDK bridge |
| `providers/compatible` | `recur-provider-compatible` | OpenAI-compatible Responses harness |
| `crates/store` | `recur-store` | gains `projfile` (see C slice) |

## 1. Subsystem map

Slice codes (defined in section 5): **A** agent contracts, **T** agent tools + MCP, **C** code + support libs, **P1** Claude + Codex providers, **P2** Cursor provider, **P3** compatible provider + usage + LSP, **F** forge + issue trackers, **E1** engine core, **E2** turn pipeline + sessions, **E3** loops, **E4** chat + coordination, **E5** pull requests, **SV** services.

### 1.1 `internal/engine` (88 source files)

| Go file | Lines | Rust home | Slice | Responsibility |
|---|---:|---|---|---|
| `agenterr.go` | 267 | `recur_agent::error` (classification), `recur_engine::loops::failure` (`turnError`) | A, E3 | **MIXED**. Transient-vs-genuine failure classification (D56): `transientMarkers`, `classifyAgentError`, `agentFailureReason`, `knownAgentError`, `tidyAgentError`, Codex login-expired sentence. The `cursor.Error.Transient()` check becomes `ProviderError::transient` set by the provider. `turnError`/`newTurnError`/`transientFailure` (+ `errMCPUnavailable`) move to E3. |
| `app_chat.go` | 585 | `recur_engine::app_chat` | E4 | Ask Recur: app-scoped chat tabs (APP, AP2..), app sessions, turn send with briefing tools, focus/unseen for app tabs, `PresentWorkspace` target validation and brief card. |
| `artifact_mentions.go` | 370 | `recur_engine::composer::artifacts` | E2 | `@artifact[:x]` and loose `@Title` mentions; inlines artifact bodies into the wire prompt. |
| `artifacts.go` | 387 | `recur_engine::artifacts` | E2 | Workspace artifacts on disk (`workspaces/<id>/artifacts/<uuidv7>.<ext>` + `.meta.json`): create/list/read/present/rename/delete/abs path, hoist between workspaces. Publishes `artifact.created` / `artifact.presented`. |
| `ask_user.go` | 363 | `recur_engine::ask_user` (broker, publish), `recur_engine::chat::ask_resume` (resume turn) | E2, E4 | **MIXED**. Park-and-return `ask_user` broker, inbox row `ib-ask-<id>`, `question.asked`/`question.answered`. `resumeAfterAskUser` + `AnswerAskUser`'s resume tail send a chat turn, so they belong to E4. |
| `attachments.go` | 489 | `recur_agent::attach` (markers, `UserImage`, `userContentBlocks`, ordering), `recur_engine::attachments` (disk I/O), providers | A, E2, P1, P2 | **MIXED**. Marker grammar `⟦attachment:<id>⟧`, `displayHiddenSentinel`, image validation/storage under `workspaces/<id>/attachments/`, transcript content blocks. `claudeAttachmentIDInstruction`/`prependClaudeAttachmentIDs` go to P1; `wrapAttachedImagesForCursor` goes to P2. |
| `board.go` | 184 | `recur_engine::loops::board` | E3 | Structured-plan board for the first coding pass of a converge entry: fan-out rounds, replans on handoff, conflict-resolve turns, fallback parent turn, event cap. |
| `branch_sync.go` | 61 | `recur_engine::workspace::branch_sync` | E1 | Sync cached `Workspace.Branch` from the live checkout, propagate to running loop and open Local PR. Deferred call site: `link_workspace_to_matching_prs` (E5). |
| `briefing.go` | 1144 | `recur_engine::briefing` | E4 | Briefing projection (`BriefingSnapshot`, `WorkspaceContext`), activity watermarks (`MarkWorkspaceViewed`, `touchWorkspaceActivity`), `briefing.updated`. Mostly pure ranking/text helpers. |
| `bus.go` | 46 | `recur_engine::bus` | E1 | Synchronous fan-out bus. Becomes the typed `EngineEvent` bus (section 3). |
| `chat_cancel.go` | 766 | `recur_engine::chatlife` | E1 | The chat concurrency kernel: `chatTurn` registry, FIFO `chatTabSlot`, ref-counted per-tab `lifeLock`, continue generations, `CancelChat`, park/stop/timeout flags, inject channel, delete/hoist tombstones (`reserveDeleting`, `chatLifeReserved`). Deferred call sites: `maybe_archive_linked_pr_chat_host` on unlock/unregister (E5), `idle_walk_by_chat` in `CancelChat` (E1 owns it via `LoopHandle`), `heal_zombie_chat` (E4). |
| `chat_child.go` | 666 | `recur_engine::chat::children` | E4 | Subagents created by Agent chats (`create_workspace` children): watcher tasks, one report turn to the parent tab, wave cards, `read_child`/`message_child`/`merge_child`/`archive_child`, restart re-arm. |
| `chat_errors.go` | 277 | `recur_engine::turn::finalize` | E2 | User-visible turn outcome: "Stopped by user", interrupted/resume sentinels, retry/inject sentinels (U+2060), `finalizeChatReply`, `persistChatFailure`, `chatSessionMeta`. |
| `chat_fork.go` | 508 | `recur_engine::chat::fork` | E4 | Fork a chat at a turn (same workspace or new sibling workspace), fork-inject transcript preamble, compaction watermark replay, foreign resume-handle sanitizing, dirty-tree overlay copy. |
| `chat_inbox.go` | 254 | `recur_engine::attention::chat_inbox` | E1 | Unseen-chat inbox rows `ib-chat-<ws>-<code>`, `MarkAllChatsSeen`, boot heal `SyncChatInbox`, loop-finished unseen flag. |
| `chat_recover.go` | 330 | `recur_engine::chat::recover` | E4 | Restart healing of live-but-dead chat agents, closing streaming rows, synthetic tool results, AutoResume of interrupted PA*/MA tabs, zombie Stop. |
| `chat_surface.go` | 238 | `recur_engine::chat::surface` | E4 | "Looking-at" surface marker `⟦recur-surface⟧{json}⟦/recur-surface⟧`, PR subject prompts. The GitHub-specific sentence comes from `Forge::agent_subject_hint` (2.2). |
| `checkpoint.go` | 328 | `recur_engine::turn::checkpoint` | E2 | Per-turn worktree checkpoints `refs/recur/checkpoints/<ws>/<msg>/{start,end}`, turn diff, reset-to-turn, review checkpoints, prune. |
| `claude.go` | 721 | `recur_provider_claude::runner`, `::args` | P1 | `claude -p` runner: argv, stream-json stdin for steers/images/large prompts, process-group kill, stderr tail, background-task result hold, MCP config JSON. |
| `claude_flags.go` | 112 | `recur_provider_claude::flags` | P1 | `[1m]` suffix rules, context window, model labels, `--effort`. |
| `codex.go` | 1224 | `recur_provider_codex::runner`, `::approvals` | P1 | Codex app-server turn: thread start/resume, notifications to events, steer, interrupt, token usage, sandbox/approval policy, approval requests routed to the permission asker. |
| `commit_message.go` | 462 | `recur_engine::turn::oneshot` | E2 | One-shot title-model turns: commit message, PR title/description, loop commit subject (`loopCommitMessage`). Cursor branch becomes `ProviderCaps::can_disable_tools` + `oneshot_flags`. `linearIssueMeta` reads the `Identifier:`/`Title:` header convention (keep as generic helper). |
| `composer.go` | 137 | `recur_engine::composer` | E2 | `ExpandComposer`: leading `/slash` prompt action via bundles, then `wrapComposer`. |
| `create_workspace_tool.go` | 160 | `recur_engine::chat::create_tool` | E4 | `create_workspace` MCP tool: child vs standalone decision, model choice, idempotent title, launch through `launchPromptWorkspace`. |
| `cursor.go` | 1982 | `recur_provider_cursor::runner`, `::turn_state`, `::steer` | P2 | Cursor turn over the bridge: create/resume agent, Send stream, SteerRun or CancelRun + follow-up Send, active-run recovery, frame folding, usage meter scaling, compaction, error hints. |
| `cursor_flags.go` | 63 | `recur_provider_cursor::flags` | P2 | Cursor context window from flags and model table. |
| `cursortools.go` | 353 | `recur_provider_cursor::tools` | P2 | Cursor tool vocabulary to canonical names/inputs/results (`shell` to `Bash`, ...), `createPlan` body extraction. |
| `diff_sync.go` | 195 | `recur_engine::workspace::diff_sync` | E1 | Debounced per-workspace `+N/-M` and behind-base refresh, single-flight refresh-all, generation guard. |
| `engine.go` | 2982 | `recur_engine::{core,publish,attention,workspace::teardown,workspace::main_copy}` (E1), `recur_engine::loops::lifecycle` (E3) | E1, E3 | **MIXED**. Engine struct + state, MCP token registration, OpenMainWorkspace, Pause/Stop, Remove/Delete/teardown, Shutdown, ClearAll, state/feed/agent/attention/resource publishers, ids. Lines covering StartWorkspace*, StartRunOnWorkspace*, dispatch*, Resume, drainQueued, loopExited, and the loop half of Recover move to E3 `loops::lifecycle`. |
| `escalation.go` | 675 | `recur_engine::loops::escalation` | E3 | Raise/block/answer escalations, re-dispatch after answer, post-restart answer application, kinds, legacy heuristics, precision counters. |
| `fanout.go` | 821 | `recur_engine::loops::fanout` | E3 | Plan steps to child workspaces (grouped by file overlap), wave cards, merge-back, conflict hand-off, child resume after restart. |
| `feedproject.go` | 228 | `recur_engine::feed` | E4 | Reload-path activity feed projection from sessions + events table. |
| `file_mentions.go` | 335 | `recur_engine::composer::files` | E2 | `@path[:line[:col][-line[:col]]]` mentions to the attached-files system instruction. Regex shared with the UI. |
| `fswatch.go` | 325 | `recur_engine::fswatch` | E1 | One watched worktree: FS event batches + git fingerprint backstop, `workspace.fs_changed`, `workspace.watched`. |
| `fswatch_darwin.go` | 60 | `recur_engine::fswatch::backend` (`notify` FSEvents backend) | E1 | Recursive FSEvents stream, MustScanSubDirs to rescan. |
| `fswatch_fsnotify.go` | 127 | `recur_engine::fswatch::backend` (`notify` inotify backend) | E1 | Per-directory watches, latency batching, ENOSPC fallback. |
| `github.go` | 1794 | `recur_forge::github::{cli,create,push,merge,checks,branch_rules,repo}` | F | **MIXED** (forge side only). `gh` availability, create/push/merge/enqueue, checks (CLI + GraphQL rollup), branch PR rules + cache, owner/repo parsing. Engine glue (`mergeGitHub`, `githubPRChecks` callers) goes to E5. |
| `github_prs.go` | 2127 | `recur_forge::github::list` (F), `recur_engine::prs::{sync,linking,from_pr}` and `recur_engine::hoist` (E5) | F, E5 | **MIXED**. List parsing/attention/checks reconciliation go to F. Sweep orchestration, apply listed rows, reconcile missing, workspace linking, CreateWorkspaceFromPR, EnsurePRChatHost, diff hydration and PR chat host hoisting (`archivePRChatHosts` .. `maybeArchiveLinkedPRChatHost`, lines ~1413-1940) go to E5. |
| `github_reaction.go` | 309 | `recur_forge::github::reactions` (F), hold logic `recur_engine::prs::holds` (E5) | F, E5 | **MIXED**. Reaction groups fetch/apply/set; `commentReactionHold` stays engine-side. |
| `github_snapshot.go` | 472 | `recur_forge::github::snapshot` | F | One GraphQL snapshot + REST timeline into a forge-neutral `PrSnapshot`. |
| `github_thread.go` | 2314 | `recur_forge::github::{thread,timeline,mutations}` (F), `recur_engine::prs::{conversation,holds}` (E5) | F, E5 | **MIXED**. Parsers and every mutation (comment, inline, review, edit, delete, reply, resolve, draft, viewer, mentions) go to F. `mergeGitHubComments`, dropped/resolved holds, `ScheduleRefreshPRConversation`, `RefreshPRConversation` go to E5. |
| `governor.go` | 166 | `recur_engine::governor` | E1 | Machine-global loop and agent slot caps, iteration counter. |
| `handoff.go` | 233 | `recur_engine::loops::handoff` | E3 | Implementer done-notes, reviewer briefing section, bounded replan turn. |
| `harness.go` | 561 | `recur_provider_compatible::harness` | P3 | In-process tool loop over `POST /v1/responses`: rounds, parallel read-only tools, halting tools, steers between rounds. |
| `harness_history.go` | 297 | `recur_provider_compatible::history` | P3 | Rebuild Responses input from the Recur transcript, repo rules inlining, 8 MiB history cap. |
| `harness_prompt.go` | 43 | `recur_provider_compatible::prompt` | P3 | `recurCodingPrompt` (byte-identical). |
| `keepawake.go` | 33 | `recur_engine::governor::keep_awake` | E1 | Agent-slot acquire wraps caffeinate sync and the Cursor close-when-idle hook. |
| `loop.go` | 2229 | `recur_engine::loop_handle` (control plane, E1), `recur_engine::loops::{kernel,converge,specialists,commit}` (E3) | E1, E3 | **MIXED**. Command/flag/steer/escalation/finding/evidence/done-summary state shared with MCP callbacks goes to E1 as `LoopHandle`. Walk data-plane (converge, cap handling, planner/coder/reviewer turns, commit, plan approval gate, panic recovery) is E3. |
| `loop_prompts.go` | 155 | `recur_engine::loops::prompts` | E3 | Phase slots/roles, bundle role prompts, phase prompt expansion, findings threshold briefing. |
| `maestro.go` | 440 | `recur_engine::loops::maestro` | E3 | Loop conductor chat turn (`sendMaestroMessage`), live loop context, steer application. |
| `maestro_hands.go` | 390 | `recur_engine::loops::hands` | E3 | Conductor actions: edit plan, dispatch specialist, extend iterations/rounds, reenter phase, on-demand phase, scoped walks. |
| `mcp_tool_backfill.go` | 84 | `recur_engine::host::backfill` | E2 | Backfill empty transcript tool inputs from the real `tools/call` args; `session.tool_input`. |
| `mirror.go` | 1224 | `recur_engine::turn::{mirror,broadcast,sessions}` | E2 | Wraps every provider turn: lossless transcript projection, `session.*` live events with monotonic seq, `agentConfig` session config, `ensureAgentSession`. `contextUsageTracker` goes to A. |
| `modes.go` | 256 | `recur_engine::chat::modes` | E4 | Chat-mode tool shapes and append prompts (byte-identical constants). |
| `permission.go` | 397 | `recur_engine::permissions` (broker, E2), `recur_agent::permission` (labels, summary, A), `recur_provider_claude::args` (CLI flags, P1) | A, E2, P1 | **MIXED**. Human permission broker with per-turn remember set and 10-minute timeout; tool labels/summary shared with Codex approvals and the UI; Claude `--disallowedTools` / `--permission-mode` rendering. |
| `plan.go` | 531 | `recur_engine::plan` | E2 | Structured plans: submit, merge steps by id/title, renumber, attachments, step updates, prose adoption, human edits, approval, plan cards. |
| `policy.go` | 313 | `recur_engine::guard` | E1 | Resolve `[[loop]]`/`[[policy]]` guards (bundle catalog or synthetic defaults), child loop derivation, findings bar math. |
| `pr.go` | 2331 | `recur_engine::prs::{ops,threads,merge,refresh,publish}` (E5), `recur_engine::loops::{open_pr,wrap_up}` (E3) | E5, E3 | **MIXED**. Local/remote PR record operations (comments, reviews, edits, reactions, threads, draft, merge, message formatting, refresh, publish). `Loop.openPR`, `emitFeedPRCreated`, `emitPRReadyInbox`, wrap-up turn go to E3. |
| `pr_participants.go` | 152 | `recur_forge::github::participants` | F | Merge participants from list rows, snapshot, comments, commits. |
| `pr_reviewers.go` | 187 | `recur_forge::github::reviewers` | F | Reviewer state ranking and merge. |
| `provider.go` | 573 | `recur_provider_cursor::pool` (P2), `recur_engine::turn::router` (E2) | P2, E2 | **MIXED**. Cursor bridge pool (per worktree, LRU, close-when-idle) goes to P2 behind the Provider trait. `runAgent`/`runnerFor`/`resolveRunner` become the provider router; the Cursor self-heal becomes a generic "transport failed before stream" retry. |
| `rails.go` | 34 | `recur_engine::loops::rails` | E3 | `CanOpenPR` policy bar, `plural`. |
| `raw_regions.go` | 141 | `recur_engine::composer::raw` | E2 | `⟦raw[:nonce]⟧` masking and the `wrapComposer` pipeline order. |
| `rename_session.go` | 207 | `recur_engine::renames` | E2 | `rename_session`/`rename_workspace`/`rename_branch` host tools; `open_pr` result text (E5 owns `OpenPR` host method). |
| `reporter.go` | 600 | `recur_engine::host` (E2 base + per-slice methods), `recur_engine::loop_registry` (E1) | E2, E1 | **MIXED**. `engineReporter` = the Host implementation. Loop registry lookups (`loopMapKey`, `forEachRunningLoop`, `runningLoopBy*`, `stopLoopByChat`, `idleWalkByChat`) go to E1. |
| `resume.go` | 317 | `recur_engine::loops::resume` | E3 | Checkpoint decode, failure recording, Retry/Restart/idle walks, AutoResume of loops. Deferred call site: launching interrupted chats (E4). |
| `rework.go` | 287 | `recur_engine::prs::{send_back,update_base}` | E5 | PR Send Back to the conductor, Update with base for PR worktrees. |
| `role_tools.go` | 201 | `recur_engine::roles::tools` | E1 | Mode tool sets, role allowlists from bundle roles, model slot mapping. |
| `roles.go` | 628 | `recur_engine::roles` | E1 | Role table, `mcp__recur__*` names, all specialist and Maestro prompts (byte-identical). |
| `runner.go` | 364 | `recur_agent::{turn,event}` | A | `AgentRunner`/`AgentSpec`/`ParsedEvent`/`Result` contract, standing-notes rule. Replaced by the trait in 2.1. |
| `schedules.go` | 562 | `recur_engine::schedules` | E4 | Scheduled prompts: save/validate/pause/run-now, 1s ticker, missed/skipped rules, workspace launch, pruning. |
| `scripted.go` | 104 | `recur_agent::testing::ScriptedProvider` + `recur_provider_claude::testing::NdjsonScript` | A | Test double (event scripts, scripted edits, `OnRun`, one steer drained per tool result). The NDJSON half replays through the Claude parser. |
| `simplify.go` | 144 | `recur_engine::simplify` | E2 | Per-session Simplify flag, freeze-on-first-write, inheritance into loops. |
| `skill_mentions.go` | 262 | `recur_engine::composer::skills` | E2 | `/skill` host activation, `<skill_content>` blocks, skill guidance and comment-hygiene overlays, `joinAppendPrompt`. |
| `streamjson.go` | 1519 | `recur_provider_claude::stream` (parser, ~700) and `recur_agent::display` (~800) | A | **MIXED**. Claude NDJSON parser (ported by A as milestone A-M2 because engine tests replay NDJSON, see R3) vs. provider-neutral display helpers (`chatToolInput`, `chatToolResult`, compactors, `summarizeToolInput`, `truncate`, limits). |
| `switch_mode.go` | 414 | `recur_engine::chat::switch_mode` | E4 | Privilege-safe `switch_mode`, park + remount continuation, remount cap, unseen flag on dead ends. |
| `terminal_mentions.go` | 223 | `recur_engine::composer::terminals` | E2 | `@terminal[:x]` snapshots via a terminal source trait. |
| `toolview.go` | 158 | `recur_agent::display` + `Provider::display_tool` | A (trait), P1/P2 (impls) | Display boundary. The provider `switch` becomes a trait method. |
| `user_mcp.go` | 134 | `recur_engine::mcp_attach` (E2), Linear parts `recur_issues::linear::mcp` (F) | E2, F | **MIXED**. Settings → MCP servers onto a turn; the Linear preset (URL by mode, API key, team hint, env scrub) moves behind `IssueTracker::agent_mcp`. |
| `verify.go` | 261 | `recur_engine::loops::verify` | E3 | Boot dev server, QA/browser turn, evidence, pass/fail. |
| `walker.go` | 418 | `recur_engine::loops::walker` | E3 | Graph walk over `[[loop]]` phases, verbs, retries, on_fail, on-demand phases, step cap. |
| `workspace_base.go` | 125 | `recur_engine::prs::base_branch` | E5 | Change merge base; retarget open PR (forge `retarget_base` for remote PRs). |
| `workspace_chat.go` | 2650 | `recur_engine::chat::{send,tabs}` (E4), `recur_engine::turn::{transcript,sessions}` (E2), `recur_engine::loops::engage` (E3) | E4, E2, E3 | **MIXED**. Send pipeline and tab management (E4). Transcript flattening (`sessionChatItems`, `flattenTranscript`, `assembleChatItems`, footers, `lastChatUserPrompt`, `reconstructChatPrompt`, `loopGoalImages`) and session-config accessors (`chatLoopGoal`, `chatLoopPolicy`, `chatLoopStarted`, `chatModeFor`, `WorkspaceChatSelection`, `stampChatLoop`) go to E2. Loop engage from a chat (`sendLoopChatMessage`, `recordLoopEngageUser`, `startChatLoopWalk`, `hasOpenSiblingWalk`, `stampLoopOwner`, `stampLoopGoalImages`) goes to E3. |
| `workspace_create.go` | 718 | `recur_engine::workspace::create` (E1), `recur_engine::loops::lifecycle` (E3) | E1, E3 | **MIXED**. Persist + background allocate + setup + retry setup + rollback (E1). `BeginStartWorkspace*`, `queueStartWorkspace`, `abortQueuedStart`, `maybeDispatchAfterAllocate` (E3). |
| `workspace_id.go` | 79 | `recur_engine::workspace::ids` | E1 | Kitchen ids `{project}.{adj}-{dish}` (word lists byte-identical). |
| `workspace_pr.go` | 891 | `recur_engine::prs::workspace` | E5 | Land without PR, land state, update-with-base, pull PR head, open/sync/promote PRs, inbox, sibling PR drop. |
| `workspace_remote.go` | 194 | `recur_engine::workspace::remote` (E1), PR head fetch (E5) | E1, E5 | **MIXED**. Main working copy fetch/update/push (E1); `fetchOpenPRRemote` uses the forge PR head refspec (E5 deferred call site). |
| `ws_hooks.go` | 86 | `recur_engine::workspace::hooks` | E1 | `setup_cmd`/`teardown_cmd` with workspace env, 10-minute bound, feed notes. |

### 1.2 `internal/engine` tests (106 files, ~55k lines)

Each Go test file moves with the code it exercises. Files that test several slices list the split. `fixture_test.go` (468) is shared test scaffolding: E1 ports it first as `recur_engine::testing` (temp store, temp git repo, scripted provider wiring, bus recorder), every engine slice extends it.

| Slice | Test files |
|---|---|
| A | `streamjson_test.go`, `context_usage_test.go` (tracker half), `standing_test.go`, `models_test.go` (provider-neutral catalog half), `attachments_test.go` (marker cases), `permission_test.go` (label cases) |
| P1 | `claude_flags_test.go`, `claude_inject_test.go`, `codex_test.go`, `codex_smoke_test.go` (env-gated), `permission_test.go` (Claude flag half), `permission_wiring_test.go` (args half) |
| P2 | `cursor_test.go`, `cursor_flags_test.go`, `cursor_steer_test.go`, `cursor_active_run_test.go`, `cursor_smoke_test.go` (env-gated), `cursortools_test.go`, `provider_test.go` (pool half) |
| P3 | `harness_test.go` |
| F | `github_test.go` (forge half: create, push, checks, branch rules, merge planning), `github_thread_test.go` (parsers + mutations), `github_snapshot_test.go`, `github_snapshot_live_test.go` (env-gated), `github_reaction_test.go` (fetch/set), `github_mentions_test.go`, `pr_participants_test.go`, `pr_reviewers_test.go`, `user_mcp_test.go` (Linear preset half) |
| E1 | `fixture_test.go`, `engine_test.go`, `lifecycle_test.go`, `governor_test.go`, `keepawake_test.go`, `chat_cancel_test.go`, `chat_inbox_test.go`, `attention_test.go`, `diff_sync_test.go`, `fswatch_test.go`, `branch_sync_test.go`, `workspace_create_test.go` (allocate half), `workspace_id_test.go`, `setup_teardown_test.go`, `policy_test.go`, `roles_test.go`, `role_tools_test.go`, `restart_test.go` |
| E2 | `mirror_test.go`, `thinking_broadcast_test.go`, `context_usage_test.go` (mirror half), `chat_errors_test.go`, `checkpoint_test.go`, `composer_test.go`, `raw_regions_test.go`, `file_mentions_test.go`, `skill_mentions_test.go`, `skills_settings_test.go`, `terminal_mentions_test.go`, `artifact_mentions_test.go`, `artifacts_test.go`, `attachments_test.go`, `ask_user_test.go` (broker half), `permission_test.go` (broker half), `permission_e2e_test.go`, `mcp_required_test.go`, `mcp_tool_backfill_test.go`, `commit_message_test.go`, `plan_test.go`, `simplify_test.go`, `user_mcp_test.go` (generic half), `workspace_chat_items_test.go`, `workspace_chat_fresh_session_test.go`, `provider_test.go` (router half), `feedkind_test.go`, `harden_test.go` (host-tool hardening) |
| E3 | `walker_test.go`, `default_walk_test.go`, `converge_test.go`, `findings_checklist_test.go`, `escalation_test.go`, `fanout_test.go`, `handoff_test.go`, `planbrief_test.go`, `loop_prompts_test.go`, `maestro_test.go`, `maestro_hands_test.go`, `loop_engage_test.go`, `resume_test.go`, `verify_test.go`, `rails_test.go`, `m8_test.go`, `acceptance_test.go`, `real_e2e_test.go` (env-gated) |
| E4 | `workspace_chat_test.go`, `chat_child_test.go`, `chat_fork_test.go`, `chat_recover_test.go`, `chat_surface_test.go`, `switch_mode_test.go`, `modes_test.go`, `schedules_test.go`, `app_chat_test.go`, `briefing_test.go`, `feedproject_test.go`, `ask_user_test.go` (resume half) |
| E5 | `github_prs_test.go` (incl. hoist cases), `github_test.go` (engine glue half), `github_thread_test.go` (merge + holds half), `github_reaction_test.go` (hold half), `pr_test.go`, `merge_message_test.go`, `open_pr_result_test.go`, `rework_test.go`, `workspace_base_test.go`, `workspace_pr_test.go`, `projfile_test.go` (engine overlay case) |

`workspace_chat_test.go` (3.5k) and `github_prs_test.go` (3.5k) are the two largest. Split them by `t.Run` group into several Rust test modules; do not port them as one file.

### 1.3 `internal/agenttools` (3802 + 312 test) → `recur-agent-tools` (slice T)

| Go file | Rust module | Responsibility |
|---|---|---|
| `host.go` | `host` | The `Host` trait (engine seam) and its request/response types: findings, done, plan, escalation, ask_user, artifacts, renames, open_pr, children, permission, loop hands, skills, terminals, briefs. See 2.4. |
| `tool.go` | `tool` | `Tool` / `HaltTool` / `Permitter` traits, `Call` context, order-preserving `Registry`, `AdvertisedName`/`BareName`, `HaltAfter` (ask_user; switch_mode only when the result says "remounting"). |
| `catalog.go` | `catalog` | `host_tools()` (order is the `tools/list` order) and `coding_tools()` (harness only). |
| `json.go` | `json` | Strict decode (unknown fields rejected), `parse_severity`, catalog XML escape. |
| `path.go` | `path` | Worktree sandbox: SafeJoin + nearest-existing-ancestor symlink resolution, "path escapes worktree". `code.SafeJoin` (20 lines, pure) is ported into this module so Read/Edit/Write do not wait on slice C; C keeps its own `recur_code::safe_join` for the Observatory. |
| `coding.go` | `coding` | Limits (read 2000 lines x 2000 chars, bash 500 lines, grep 200 hits, bash 2 min default / 10 min max), binary sniff. |
| `bash.go` | `coding::bash` | `bash -lc` in the worktree, new process group, kill group on cancel, 2 s wait delay, capped buffers, output framing. Permitter gate unless bypass. |
| `read.go`, `edit.go`, `write.go`, `glob.go`, `grep.go` | `coding::{read,edit,write,glob,grep}` | Harness coding tools. Grep uses `recur-code::rg`. |
| `todos.go`, `todowrite.go` | `coding::todos` | `TodoStore` per session (merge by id) and the TodoWrite tool. |
| `activate_skill.go`, `ask_user.go`, `attach_evidence.go`, `briefing.go`, `create_artifact.go`, `create_workspace.go`, `dispatch_specialist.go`, `done.go`, `edit_plan.go`, `escalate.go`, `extend_iterations.go`, `extend_rounds.go`, `list_artifacts.go`, `list_findings.go`, `list_skills.go`, `list_terminals.go`, `merge_child.go`, `open_pr.go`, `permission_request.go`, `plan.go`, `present_artifact.go`, `read_artifact.go`, `read_child.go`, `read_terminal.go`, `reenter_phase.go`, `rename_branch.go`, `rename_session.go`, `rename_workspace.go`, `report_finding.go`, `report_result.go`, `rerun_review.go`, `resume_loop.go`, `run_phase.go`, `send_back.go`, `submit_plan.go`, `switch_mode.go`, `update_finding.go`, `update_plan_step.go` | `host_tools::<name>` | One host tool each: name, description and JSON schema (byte-identical, agents and tests read them), argument validation, one `Host` call, result text. `briefing.go` holds `list_workspace_briefs`, `get_workspace_context`, `present_workspace`. |
| `catalog_test.go`, `coding_test.go`, `path_test.go` | same modules | Catalog order and schema snapshots; coding tool behavior; sandbox escapes. |

### 1.4 `internal/mcp` (839 + 1550 test) → `recur-mcp` (slice T)

| Go file | Rust module | Responsibility |
|---|---|---|
| `protocol.go` | `protocol` | JSON-RPC 2.0 types, protocol version `2025-06-18` (echo client's), server info `recur 1.0.0`, error codes, initialize / tools list / call payloads. |
| `reporter.go` | (gone) | `Reporter` is an alias of `agenttools.Host`; `noopReporter` becomes the trait's default bodies (2.4). |
| `tools.go` | `tools` | Tool-name constants, `names_from_allowed_tools` (never null), `append_permission_request`, wrapping a host tool into a handler with `Call{workspace, agent, chat, args, host}`. Coding tools are not served over MCP. |
| `tools_briefing.go` | `tools` | Names of the three Ask Recur tools. |
| `token.go` | `token` | 16 random bytes, hex. |
| `server.go` | `server` | axum on `127.0.0.1:0`, `POST /<token>/mcp`, token bindings `{ws, agent, chat, allowed (None = all, empty = none), active, stable}`, stable token reuse per agent, extra tokens for overlapping registrations, 404 for inactive, 405 for GET/DELETE, 32 MiB body cap, `NoteToolArgs` before the handler, request cancellation = client disconnect, 3 s graceful close. |
| `server_test.go`, `tools_briefing_test.go`, `wrap_test.go` | same | Port in full; `server_test.go` pins the HTTP contract the three CLIs depend on. |

### 1.5 `internal/app` (8921 + 8554 test) → `recur-services` (slice SV) and others

| Go file | Lines | Rust home | Slice | Responsibility |
|---|---:|---|---|---|
| `services.go` | 1849 | `recur_services::{projects,workspaces,maestro,escalation,permissions,questions,artifacts,agents,prs,policy,inbox,settings}` | SV | **MIXED** by service struct (12 services, 133 methods). Thin delegation to engine/store plus: `NewServices` composition root, project add/delete/groups, run-config + `projfile` writes, settings save side effects (governor caps, provider remap, cursor pool reset, usage poller restart, LSP restart). Split one module per service; `NewServices` becomes `recur_services::Services::new` (provider/forge/tracker registration). |
| `briefing.go` | 67 | `recur_services::briefing` | SV | Ask Recur + Inbox facade over engine `app_chat`/`briefing`. |
| `bundle_service.go` | 1228 | `recur_services::bundles` | SV (needs `recur-bundles`) | Command/snippet/symbol/prompt-action/grammar/theme catalogs, shebang commands, TS module compile for the macro runtime, palette search, `ExpandGoal`. |
| `claude_status.go` | 208 | `recur_provider_claude::status` + `recur_services::providers` | P1, SV | Claude account probe (`claude auth status`), agent detection. Becomes `Provider::status`. |
| `code_service.go` | 968 | `recur_services::code` | SV (needs C) | Observatory file tree/read/write/search/replace/diff/commit/stage/outline/defs/refs/open-in. `GenerateCommitMessage` calls engine `turn::oneshot`. |
| `codex_service.go` | 197 | `recur_provider_codex::status` | P1 | Throwaway app-server probe, key test. |
| `compatible_service.go` | 44 | `recur_provider_compatible::status` | P3 | `/v1/responses` ping, flag presets. |
| `cursor_service.go` | 273 | `recur_provider_cursor::status` + `::install` | P2 | Throwaway bridge probe, key test, bridge install with `cursor.install` progress. |
| `editor_session.go` | 489 | `recur_services::editor_session` | SV | Per-workspace editor state (`editor-session.json.gz`). |
| `fileserver.go` | 300 | `recur_services::fileserver` | SV | Workspace file, chat image and external file serving (TIFF transcode on non-WebKit). In GPUI this becomes a byte-loading API, not HTTP. |
| `gh_image.go` | 207 | `recur_forge::github::images` | F | Authenticated fetch of GitHub attachment images. `Forge::fetch_image`. |
| `instance.go` | 24 | `recur-platform` | (platform port) | Instance label + window title. Listed for completeness; not an engine slice. |
| `issues.go` | 485 | `recur_issues::{github,linear}` + `recur_services::issues` | F, SV | **MIXED**. Linear GraphQL (assigned + unassigned up to 300) and `gh search issues` listing, merge/sort into `AssignedIssue`; `OpenLinearInApp`. Trackers go to F; the service merges registered trackers. |
| `loose_files.go` | 39 | `recur_services::code` | SV | Allowlist of user-opened absolute paths outside projects. |
| `lsp_service.go` | 176 | `recur_services::lsp` | P3 | LSP session facade, `lsp.message`/`lsp.exit` events. |
| `model_catalog.go` | 641 | `recur_services::model_catalog` (merge) + `Provider::models` (per provider) | SV, P1, P2, P3 | **MIXED**. One grouped picker list over all enabled providers. Today it switches on provider id to list Claude/Codex/Cursor/compatible models and flags; each branch becomes that provider's `models()` / `flag_dimensions()`. |
| `observatory_service.go` | 119 | `recur_services::observatory` | SV | Findings list/status/notes/create. |
| `open_path.go` | 251 | `recur_services::open_path` | SV | `recur .` / open-file routing: find-or-create project, focus working copy or workspace, homeless files. |
| `provider_usage.go` | 188 | `recur_services::usage` | P3 | Start/stop/get/refresh poller; Cursor usage connect/disconnect. |
| `schedules.go` | 72 | `recur_services::schedules` | SV | Facade over engine schedules + preview. |
| `shell.go` | 364 | `recur_services::shell` | SV | `shell.json`, `schedule-view.json`, `issues-view.json` via `write_data_json`. |
| `terminal_persist.go` | 381 | `recur_services::terminals::persist` | SV | `terminals.json.gz` per workspace, dock chrome. |
| `terminal_service.go` | 263 | `recur_services::terminals` | SV | PTY tabs over `recur-term`, `term.*` events. |
| `workspace_open.go` | 26 | `recur_services::workspaces` | SV | Open-in targets. |
| `bridge_bench.go` | 62 | not ported | - | Webview bridge benchmark (`RECUR_BENCH_BRIDGE`); meaningless without a webview. Note in PARITY. |

Tests: `services_test.go`, `workspace_*_test.go`, `shell_test.go`, `schedules_test.go`, `settings_*_test.go`, `observatory_test.go`, `open_path_test.go`, `editor_session_test.go`, `fileserver_test.go`, `terminal_*_test.go`, `code_service_test.go`, `bundle_service_test.go`, `instance_test.go` go to SV. `model_catalog_test.go` splits SV (merge/grouping) and providers (per-provider lists). `claude_status_test.go`, `cursor_service_test.go`, `compatible_service_test.go` go with their providers. `gh_image_test.go` and `issues_test.go`, `workspace_issue_test.go` go to F. `provider_usage_test.go` to P3.

### 1.6 Providers

| Go file | Rust module | Slice | Responsibility |
|---|---|---|---|
| `codex/client.go` | `recur_provider_codex::client` | P1 | Typed facade: account read, model list, thread start/resume, turn start/steer/interrupt. |
| `codex/locate.go` | `::locate` | P1 | Find the `codex` binary (configured, common bins). |
| `codex/mcp.go` | `::mcp` | P1 | `-c key=value` TOML overrides for MCP servers. |
| `codex/rpc.go` | `::rpc` | P1 | Newline JSON-RPC without the `jsonrpc` field: calls, notifications, server requests, reader task, fail pending on EOF. |
| `codex/server.go` | `::server` | P1 | Spawn `codex app-server` with Recur-owned `CODEX_HOME`, trust project, seed auth, handshake, reap signal, stderr ring, group kill. |
| `codex/types.go` | `::types` | P1 | Thread/turn/item wire types. |
| `cursor/attribution.go` | `recur_provider_cursor::attribution` | P2 | Patch the SDK bundle at pinned anchors (attribution minify + steer hook), attribution config JSON. Pin coupling: `pinnedSDKVersion` + `sdkAttributionAnchors`. |
| `cursor/bridge.go` | `::bridge` | P2 | Start the Node bridge, ready-line discovery on stderr, shell env, per-worktree log/pidfile. |
| `cursor/bridge_log.go` | `::bridge::log` | P2 | Redacted stderr tail. |
| `cursor/bridge_reap.go` | `::bridge::reap` | P2 | Orphan reaper via `ps` (crashed Recur, other data dirs). |
| `cursor/client.go` | `::client` | P2 | sdk.v1 calls: version caps, models, me, agent create/resume/close, Send stream, ObserveRun, CancelRun, SteerRun, run listing. |
| `cursor/connect.go` | `::connect` | P2 | Hand-rolled Connect protocol (unary + server-stream frames), error codes (Unauthenticated, Unimplemented, transient, ActiveRunConflict, BridgeUnreachable). |
| `cursor/context.go` | `::context` | P2 | Model context limits table. |
| `cursor/events.go` | `::events` | P2 | Run stream frame types, token usage, interaction deltas. |
| `cursor/install.go` | `::install` | P2 | Download + extract the pinned `cursor-sdk` wheel with progress. |
| `cursor/locate.go`, `manifest.go`, `projectdirs.go` | `::{locate,manifest,projectdirs}` | P2 | Bridge dir, SDK version read, `~/.cursor/projects/<slug>` ensure/remove. |
| `responses/client.go` | `recur_provider_compatible::client` | P3 | `POST {base}/v1/responses` stream / JSON fallback / ping, auth header modes. |
| `responses/flags.go` | `::flags` | P3 | Flag dimensions to wire JSON paths. |
| `responses/sse.go` | `::sse` | P3 | SSE parse. |
| `responses/types.go` | `::types` | P3 | Request/tool/input/usage/event types. |

Tests: `codex/*_test.go` → P1, `cursor/*_test.go` → P2 (`*_smoke_test.go` env-gated), `responses/*_test.go` → P3.

### 1.7 Support packages

| Go file | Rust module | Slice | Responsibility |
|---|---|---|---|
| `usage/types.go`, `parse.go`, `http.go` | `recur_usage::{types,parse,http}` | P3 | Snapshot payload (no tokens), quota parsing, HTTP helper. |
| `usage/claude.go`, `claude_creds.go` | `recur_usage::claude` | P3 | Claude OAuth usage endpoint; Keychain vs file credentials. |
| `usage/codex.go` | `recur_usage::codex` | P3 | Codex/ChatGPT rate limits. |
| `usage/cursor.go`, `cursor_oauth.go`, `cursor_session.go`, `connect.go` | `recur_usage::cursor` | P3 | Cursor usage, Recur Connect OAuth, local-only rows. |
| `usage/fetcher.go`, `poller.go` | `recur_usage::{fetcher,poller}` | P3 | Collect per provider (429 marks one provider), cache + conservative refresh, `provider.usage` publish. The provider-specific fetchers are reached through `Provider::usage_probe()`. |
| `lsp/frame.go`, `uri.go` | `recur_lsp::{frame,uri}` | P3 | Content-Length framing, path/URI canonicalization. |
| `lsp/servers.go` | `recur_lsp::servers` | P3 | Builtin server specs, bundle `[[languageServer]]` overrides. |
| `lsp/session.go`, `manager.go` | `recur_lsp::{session,manager}` | P3 | One process per workspace + server, emitter injection, lifecycle. |
| `lsp/managed/managed.go`, `helpers.go`, `gopls.go`, `marksman.go`, `npm.go`, `npm_servers.go`, `ruff.go`, `rust_analyzer.go`, `taplo.go`, `typescript.go` | `recur_lsp::managed::*` | P3 | Pinned downloads (gopls, marksman, ruff, rust-analyzer, taplo, typescript, npm servers) with digests. |
| `code/root.go`, `file.go`, `files.go`, `tree.go`, `ops.go`, `langs.go`, `changed.go` | `recur_code::*` | C | Worktree root/SafeJoin, file read/write/images, listing, git status tree, create/rename/delete/copy, source ext table, changed defs. |
| `code/fileindex.go`, `filesearch.go` | `recur_code::{fileindex,filesearch}` | C | Resident Quick Open index (byte-bounded), ranking. Publishes `workspace.file_index` through an injected emitter. |
| `code/search.go`, `outline.go`, `symbols.go` | `recur_code::{search,outline,symbols}` | C | Substring/regex search, outline, defs/refs index. |
| `code/skills.go` | `recur_code::skills` | C | Agent Skills discovery (worktree + home), resources. |
| `code/open.go`, `open_in.go`, `open_in_icon_darwin.go`, `open_in_icon_stub.go`, `drop_allow.go` | `recur_code::open` (or `recur-platform`) | C | External editor, reveal, open-in targets + icons, dropped-path allowlist. |
| `rg/rg.go`, `ensure.go` | `recur_code::rg` | C | Locate/run ripgrep, managed download. |
| `copyfiles/copyfiles.go` | `recur_code::copyfiles` | C | Glob-matched file copy into new worktrees. |
| `observe/analyze.go`, `checks.go`, `findings.go`, `impact.go`, `metrics.go` | `recur_code::observe` | C | Metrics, light checks, findings onto diff, impact, analysis run. |
| `projfile/projfile.go`, `write.go` | `recur_store::projfile` | store port | `.recur/project.toml` read overlay and write. Store depends on it, so the `recur-store` porter owns it; C reviews. |
| `runconfig/validate.go` | `recur_engine::runconfig` | C | Dev-server Boot probe, one-shot commands with output. |
| `wsenv/env.go`, `dotenv.go` | `recur_engine::wsenv` | C | Per-workspace env (ports, ids, dotenv expansion). |
| `simplify/simplify.go` | `recur_engine::simplify::text` | C | Simplify prompt text + `applies_to_chat`. |

Tests for each move with them (`usage/*_test.go`, `lsp/**/*_test.go` → P3; `code`, `rg`, `copyfiles`, `observe`, `runconfig`, `wsenv`, `simplify` tests → C; `projfile_test.go` → store port).

C slice files under `recur_engine::{runconfig,wsenv,simplify::text}` are leaf modules with no engine imports; C owns them even though they sit in the engine crate.

### 1.8 Test files outside the engine

Every test file moves with its code. `*_smoke_test.go` and `*_live_test.go` are env-gated in Go and stay `#[ignore]` unless their env var is set.

| Package | Slice | Test files |
|---|---|---|
| `internal/agenttools` | T | `catalog_test.go`, `coding_test.go`, `path_test.go` |
| `internal/mcp` | T | `server_test.go`, `tools_briefing_test.go`, `wrap_test.go` |
| `internal/app` | SV | `bundle_service_test.go`, `code_service_test.go`, `editor_session_test.go`, `fileserver_test.go`, `instance_test.go`, `observatory_test.go`, `open_path_test.go`, `schedules_test.go`, `services_test.go`, `settings_mcp_test.go`, `settings_prefix_test.go`, `shell_test.go`, `terminal_persist_test.go`, `terminal_service_test.go`, `workspace_base_test.go`, `workspace_create_test.go`, `workspace_main_test.go`, `workspace_open_test.go`, `workspace_pr_test.go` |
| `internal/app` | P1 | `claude_status_test.go` |
| `internal/app` | P3 | `compatible_service_test.go`, `provider_usage_test.go` |
| `internal/app` | P2 | `cursor_service_test.go` |
| `internal/app` | F | `gh_image_test.go`, `issues_test.go`, `workspace_issue_test.go` |
| `internal/app` | SV + P1/P2/P3 (per-provider cases) | `model_catalog_test.go` |
| `internal/codex` | P1 | `locate_test.go`, `mcp_test.go`, `rpc_test.go`, `server_test.go`, `types_test.go` |
| `internal/cursor` | P2 | `active_run_test.go`, `attribution_test.go`, `bridge_alive_test.go`, `bridge_log_test.go`, `bridge_reap_test.go`, `bridge_shell_test.go`, `bridge_smoke_test.go`, `client_images_test.go`, `connect_test.go`, `context_test.go`, `events_test.go`, `install_test.go`, `locate_test.go`, `manifest_test.go`, `projectdirs_test.go`, `run_smoke_test.go`, `user_test.go` |
| `internal/responses` | P3 | `client_test.go`, `flags_test.go` |
| `internal/usage` | P3 | `fetch_test.go`, `parse_test.go`, `poller_test.go` |
| `internal/projfile` | store port | `projfile_test.go` |
| `internal/runconfig` | C | `validate_test.go` |
| `internal/wsenv` | C | `dotenv_test.go`, `env_test.go` |
| `internal/observe` | C | `observe_test.go` |
| `internal/simplify` | C | `simplify_test.go` |
| `internal/copyfiles` | C | `copyfiles_test.go` |
| `internal/code` | C | `code_test.go`, `fileindex_test.go`, `filesearch_test.go`, `open_in_test.go`, `ops_test.go`, `search_replace_test.go`, `skills_test.go` |
| `internal/rg` | C | `ensure_test.go`, `rg_test.go` |
| `internal/lsp` | P3 | `frame_test.go`, `managed/assets_test.go`, `managed/managed_test.go`, `servers_test.go`, `session_test.go`, `uri_test.go` |

## 2. Trait designs

All three traits are object safe (`Arc<dyn Provider>` etc.), use `#[async_trait]`, and live below the engine. Registries are keyed by the ids Go already persists: providers `claude-code`, `codex`, `cursor` and each compatible provider's own id (`domain.ParseModelRef` splits `provider/id` on the first slash; an unqualified ref is `claude-code`); forges by registry id `github` with PR rows keeping `source = "gh"` (local PRs are `"local"` and are not a forge); trackers by `IssueRef.source` (`github`, `linear`). `ARCHITECTURE.md` is corrected to match (risk R1).

Every behavior the Go engine branches on by provider name is a capability or trait method below. After the port, `recur-engine` contains no `"cursor"`, `"codex"`, `"claude-code"` or `"gh"` string compare outside the registry lookup.

### 2.1 `recur_agent::Provider`

```rust
// crates/agent/src/provider.rs (as built in slice A; the source is authoritative)
pub type Flags = std::collections::BTreeMap<String, String>;

#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    fn id(&self) -> &str;                    // "claude-code", "codex", "cursor", compatible id
    fn label(&self) -> &str;                 // picker group label
    fn caps(&self) -> &ProviderCaps;
    /// The router never falls back: Unavailable(reason) is the visible turn error
    /// with Go's exact wording ("Codex is turned off in Settings → Providers").
    fn availability(&self, settings: &Settings) -> Availability;

    async fn status(&self, settings: &Settings) -> ProviderStatus;
    /// Default: status(settings) (providers without credentials).
    async fn test_credentials(&self, settings: &Settings, draft: &CredentialDraft) -> ProviderStatus;

    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError>;
    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice>;
    fn resolve_model(&self, model: &str, flags: &Flags) -> String;
    fn context_window(&self, model: &str, flags: &Flags) -> i64;   // 0 = unknown
    fn model_label(&self, model: &str) -> String;
    fn oneshot_flags(&self, model: &str) -> Flags;

    /// Go AgentRunner.Run. Events go to cx.sink in order, each run through a
    /// ContextUsageTracker; every provider result frame is also emitted as
    /// AgentEvent::TurnEnd (duration_ms > 0 marks a real turn end). On cancel:
    /// stop the remote run, emit nothing further, return
    /// Err(ProviderError::canceled().with_partial(result_so_far)).
    async fn run_turn(&self, req: TurnRequest, cx: TurnContext) -> Result<TurnResult, ProviderError>;

    fn resume_handle_fits(&self, handle: &str) -> bool;

    /// Raw JSON text in and out ("" = none). Defaults: passthrough. Callers use
    /// recur_agent::display::{display_tool_result, summarize_display_input},
    /// which add the provider-neutral rules (Task launch acks render empty).
    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay;
    fn display_tool_result(&self, name: &str, result: &str) -> String;

    fn usage_probe(&self) -> Option<Arc<dyn UsageProbe>> { None }
    /// Err = the sentence the workspace feed shows (see P2 as built).
    async fn release_worktree(&self, _worktree: &Path) -> Result<(), ProviderError> { Ok(()) }
    async fn on_idle(&self) {}
    async fn settings_changed(&self, _old: &Settings, _new: &Settings) {}
    async fn shutdown(&self) {}
}

pub struct ProviderCaps {                    // Copy
    pub permission: PermissionRoute,         // McpPromptTool | Callback | None
    pub steer: SteerSupport,                 // None | Live | LiveOrRestart
    pub history: HistoryMode,                // Native | App
    pub tool_transport: ToolTransport,       // Mcp | InProcess
    pub native_fork: bool,
    pub images: bool,
    pub can_disable_tools: bool,
    pub max_turns: bool,
    pub partials: bool,
    pub user_mcp: McpTransports,             // { http, stdio }
}
pub enum Availability { Ready, Unavailable(String) }
/// Union of Go ClaudeStatus / CodexStatus / CursorStatus / CompatibleTestResult.
pub struct ProviderStatus { installed, path, version, pinned_version, update_available,
    authenticated, provider, login_method, organization, email, plan, account,
    api_key_name, name, model_count: i64, error }
pub struct CredentialDraft { api_key, chatgpt_auth: bool, compatible: Option<domain::CompatibleProvider> }

// crates/agent/src/turn.rs. Strings follow Go zero values: "" = unset.
pub struct TurnRequest {
    workspace_id, agent_id, chat_code, chat_child: bool, role: AgentRole, model, flags: Flags,
    system_prompt, append_system_prompt, turn_append_prompt, refresh_standing: bool,
    allowed_tools: Option<Vec<String>>,      // None = no restriction (Go nil), Some([]) = none
    tools: Option<String>,                   // Claude --tools; Some("") disables
    permission_mode: String,                 // "" fails closed to dontAsk
    prompt, display_text, skip_record_user: bool, user_message_id,
    images: Vec<UserImage>, worktree: PathBuf, mcp_url, playwright: bool,
    user_mcp: Vec<McpAttach>, env_scrub: Vec<String>, resume_session, fork_session: bool,
    prompt_on_stdin: bool, route: SessionRoute, include_partials: bool, skip_thinking: bool,
    max_turns: u32,                          // 0 = provider default
    recur_session_id, history_after_id,
}
pub struct SessionRoute { workspace_id, kind, code, name }
pub struct McpAttach { name, url, headers: BTreeMap, command, args: Vec<String>, env: BTreeMap }
pub fn standing_in_user_prompt(req: &TurnRequest, resumed: bool) -> bool;

pub struct TurnContext {
    pub cancel: CancellationToken,
    pub sink: Arc<dyn EventSink>,            // impl'd for Fn(AgentEvent)
    pub steer: Option<SteerInbox>,
    pub permissions: Option<Arc<dyn PermissionAsker>>,   // None = Go's nil AskPermission
    pub history: Option<Arc<dyn HistorySource>>,
    pub tools: Option<Arc<dyn ToolHost>>,
}
pub struct SteerInbox(pub mpsc::Receiver<Steer>);        // recv, try_next, drain
pub struct Steer { pub prompt: String, pub display: Option<String>, pub ack: oneshot::Sender<bool> }
// Steer::{new, is_plain, plain_parts, stdin_text, user_event(live_steer), ack};
// format_steer_user_message, follow_up_text (Go injectStdinText, emitInjectUser,
// followUpInjectText, joinSteerInjects, formatSteerUserMessage).
#[async_trait] pub trait PermissionAsker { async fn ask(&self, req: domain::PermissionRequest)
    -> Result<domain::PermissionDecision, PermissionAbandoned>; }
#[async_trait] pub trait HistorySource { async fn transcript(&self, session_id: &str) -> Vec<Message>;
    async fn history_after(&self, session_id: &str) -> String; }
#[async_trait] pub trait ToolHost { fn tools(&self, allowed: Option<&[String]>) -> Vec<ToolSpec>;
    async fn call(&self, name: &str, args: &str, meta: ToolCallMeta) -> ToolOutcome; }
pub struct ToolSpec { name, description, schema: Value }
pub struct ToolCallMeta { call_id, cancel: CancellationToken }
pub struct ToolOutcome { text, is_error: bool, halt: bool }

// crates/agent/src/event.rs (Go ParsedEvent / Result). parent = Task tool-use id
// of a nested subagent ("" = parent turn); uuid = provider frame id.
pub enum AgentEvent {
    Init { session_id, model, mcp_servers: Vec<McpServerStatus> },
    Text { text, uuid, parent }, TextDelta { text, parent },
    Thinking { text, signature, uuid, parent }, ThinkingDelta { text, parent },
    ThinkingTokens { tokens: i64, uuid, parent },
    Step(ToolStep { id, name, summary, input /* raw JSON */, parent, uuid }),
    ToolResult(ToolResultEvent { id, name, input, text, is_error, truncated, parent, uuid }),
    Usage(TokenUsage), Notice { text },
    Compact { status, trigger, summary, pre_tokens: i64, uuid, parent },
    User { text, display, live_steer: bool },
    Fallback { from_model, to_model, retracted_uuids: Vec<String>, uuid },
    PlanProposal { body },
    TurnEnd(TurnResult),
}   // kind() (Go ParsedKind names), parent(), nested(), uuid()
pub struct TokenUsage { input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    context_window: i64 }                    // context_used(), prefer_context()
pub struct TurnResult { session_id, subtype, is_error, num_turns: i64, duration_ms: i64, text,
    turn_end_emitted: bool /* engine-set, serde skip */, usage: TokenUsage /* flattened */, stderr }
pub struct ContextUsageTracker;             // observe(&mut AgentEvent)

// crates/agent/src/error.rs
pub struct ProviderError { pub kind: ErrorKind, pub message: String, pub partial: Box<TurnResult> }
pub enum ErrorKind { Canceled, Transient, Auth, Unavailable, Failed }
// classify(Option<&dyn Error>, &TurnResult) -> (reason, transient): ProviderError of kind
// Transient anywhere in the source chain first, then TRANSIENT_MARKERS / known_agent_error.

// crates/agent/src/catalog.rs, quota.rs: serde-identical to app/model_catalog.go, usage/types.go
pub struct FlagOption { value, label, default: bool }
pub struct FlagDimension { id, label, options: Vec<FlagOption> }      // default_value()
pub struct ModelChoice { model_ref /* "ref" */, label, flags: Vec<FlagDimension>, context_window: i64 }
pub struct ModelGroup { provider, label, models: Vec<ModelChoice> }
pub fn default_flags_from_dims(&[FlagDimension]) -> Flags;
#[async_trait] pub trait UsageProbe { fn provider_id(&self) -> &str; async fn fetch(&self) -> ProviderQuota; }
```

How each provider maps:

| Concern | Claude | Codex | Cursor | Compatible |
|---|---|---|---|---|
| `permission` | `McpPromptTool` | `Callback` (JSON-RPC approval requests) | `None` | `Callback` (Bash gate) |
| `steer` | `Live` (stdin after next tool_result; plain steers ack after write) | `Live` (`turn/steer`, 30 s) | `LiveOrRestart` | `Live` (between rounds) |
| `history` | `Native` | `Native` | `Native` | `App` |
| `tool_transport` | `Mcp` | `Mcp` (config overrides) | `Mcp` | `InProcess` |
| `native_fork` | true | false | false | false |
| `can_disable_tools` | true | true | false | true |
| process model | one `claude -p` per turn, process group | one app-server per turn | pooled bridge per worktree (max 8, LRU) | HTTP only |
| self-heal | none | none | inside `run_turn`: bridge unreachable before stream starts → invalidate + retry once | none |
| `TurnEnd` | from `result` line, background-task hold up to 10 min | on `turn/completed`; error status returns `duration_ms = 0` | deferred until finish so live `User` precedes it | after last round |

The engine-side helpers that used to switch on provider move as follows: `standingInUserPrompt` → `recur_agent::standing_in_user_prompt(req, resumed)`; Claude `[1m]` and Cursor limits → `context_window`; mirror's "Cursor KindUser !LiveSteer resets seen steps" → the generic `User { live_steer: false }` rule; `createPlanBody` → `PlanProposal`; Cursor `wirePrompt` image wrapping and Claude attachment-id instruction → inside each provider's prompt builder.

**Deviations from the first draft (slice A, as built).** Each keeps Go behavior the draft lost:

- Strings, counts and limits use Go zero values (`""`, `0`) instead of `Option`: the engine ports test `!= ""` everywhere, and token counts are `i64` like `recur-domain`'s `Message`. `allowed_tools` and `tools` stay `Option` because Go distinguishes nil there (the harness treats a nil allowlist as "all").
- `TurnRequest` gains `skip_record_user`, `user_message_id` and `route` (the compatible history rebuild and harness agent code read them) and drops `access` (Go derives read-only from the allowlist and role; Codex and Cursor keep doing that).
- `AgentEvent::Step` carries the provider-computed feed `summary` (Go `ToolInput`) and raw `input` text; `uuid` and `parent` ride on every kind the mirror reads them from (Thinking, ThinkingTokens, ToolResult, Compact, Fallback); `ToolResult` keeps `name` and completion-time `input` (Codex MCP). `TurnEnd` is emitted for every provider result frame, as Go's `KindResult` is.
- `ProviderError.partial` is always present (`Box<TurnResult>`, default empty) because Go returns a `Result` with every error and classification reads its stderr/subtype/text; `stderr_tail` lives in `partial.stderr`.
- `TurnContext.permissions`, `history` and `tools` are `Option`: Go's `AskPermission`, `Store` and `Reporter` can be nil, and Codex auto-resolves approvals when nobody can answer. `PermissionAsker::ask` takes and returns the domain `PermissionRequest` / `PermissionDecision` the Codex path builds, with `Err(PermissionAbandoned)` for a turn that went away. `HistorySource` returns the whole transcript plus the session's `nativeHistoryAfter` (the harness filters itself).
- `Availability` is `Ready | Unavailable(reason)` with Go's exact per-provider wording; `test_credentials` returns a `ProviderStatus` (Go `TestCodexKey` / `TestCursorKey` return status cards, not just ok/err).
- Steering helpers that providers call (`injectStdinText`, `emitInjectUser`, `followUpInjectText`, `formatSteerUserMessage`) live on `recur_agent::Steer` / `turn` rather than in the engine's `loop.go` port, because providers cannot depend on the engine. A plain steer's display rides on the steer, so Go's `\x1e` encoding is not needed.
- `testing::ScriptedProvider` (behind the `testing` feature) replays frames (one per provider line; cancellation is checked between frames like Go's per-line check), holds drained steers unacked until the turn returns (Go's ScriptedRunner never acks, so `tryInject` reads false at turn end), and `imitating(provider)` borrows a real provider's id, caps, models, context window and display so engine tests keep Go's "ScriptedRunner under the claude-code rules" behavior.

### 2.2 `recur_forge::Forge`

"Local PR" is not a forge. The engine owns the PR model, local review, local merge and the Local PR number space. A forge is consulted when `pr.source == forge.pr_source()`, and for projects whose remote it detects.

```rust
// crates/forge/src/lib.rs
#[async_trait::async_trait]
pub trait Forge: Send + Sync + 'static {
    fn id(&self) -> &str;                       // registry key: "github"
    fn pr_source(&self) -> recur_domain::PRSource; // persisted: "gh"
    fn label(&self) -> &str;                    // "GitHub" (user-facing copy)
    /// Parse a remote URL (Go OwnerRepoFromRemoteURL). None = not this forge.
    fn detect(&self, remote_url: &str) -> Option<RepoRef>;
    /// Stable id for imported PR rows: "gh-<projectID>-<number>" (must not change).
    fn imported_pr_id(&self, project_id: &str, number: u64) -> String;
    /// Tool installed + authenticated (gh auth status), cached 30 s by the impl.
    async fn availability(&self) -> ForgeAvailability;
    async fn viewer(&self, repo: &RepoCtx) -> Result<Viewer, ForgeError>; // login, permission, merge methods

    // ---- PR sync (github_prs.go)
    async fn list_prs(&self, repo: &RepoCtx, q: PrListQuery) -> Result<Vec<PrListRow>, ForgeError>;
    /// State probe for rows missing from list results (reconcileMissingGitHubPRs).
    async fn pr_state(&self, repo: &RepoCtx, number: u64) -> Result<PrStateProbe, ForgeError>;
    /// Full conversation snapshot: PR, comments, reviews, commits, threads,
    /// requested reviewers, viewer permission, merge settings, merge queue, timeline,
    /// optionally the check rollup.
    async fn snapshot(&self, repo: &RepoCtx, number: u64, opts: SnapshotOpts) -> Result<PrSnapshot, ForgeError>;
    async fn diff(&self, repo: &RepoCtx, number: u64) -> Result<String, ForgeError>;
    async fn checks(&self, repo: &RepoCtx, number: u64) -> Result<ChecksReport, ForgeError>;

    // ---- Create, push, branches
    async fn create_pr(&self, repo: &RepoCtx, req: CreatePr) -> Result<CreatedPr, ForgeError>;
    /// Push a PR head with lease; handles fork heads (push URL from head repo).
    async fn push_head(&self, repo: &RepoCtx, push: PushHead) -> Result<(), ForgeError>;
    /// Refspec + tracking ref for a PR head ("pull/N/head", "refs/recur/pr/N").
    fn head_ref(&self, number: u64) -> HeadRef;
    async fn retarget_base(&self, repo: &RepoCtx, number: u64, base: &str) -> Result<(), ForgeError>;
    async fn edit_pr(&self, repo: &RepoCtx, number: u64, edit: PrEdit) -> Result<(), ForgeError>;
    async fn set_draft(&self, repo: &RepoCtx, number: u64, draft: bool) -> Result<(), ForgeError>;
    /// Whether the branch requires a PR (rulesets + classic protection). The impl
    /// caches 10 min per (repo, branch), dedupes in flight, refreshes in background,
    /// and calls `on_change` so the engine can publish `github.branch_rules`.
    async fn branch_rules(&self, repo: &RepoCtx, branch: &str) -> Result<BranchRules, ForgeError>;

    // ---- Conversation (github_thread.go, github_reaction.go)
    async fn post_comment(&self, repo: &RepoCtx, number: u64, c: NewComment) -> Result<recur_domain::PRComment, ForgeError>;
    async fn reply(&self, repo: &RepoCtx, number: u64, to: &CommentRef, body: &str) -> Result<recur_domain::PRComment, ForgeError>;
    async fn edit_comment(&self, repo: &RepoCtx, c: &CommentRef, body: &str) -> Result<(), ForgeError>;
    async fn delete_comment(&self, repo: &RepoCtx, c: &CommentRef) -> Result<(), ForgeError>;
    async fn submit_review(&self, repo: &RepoCtx, number: u64, r: ReviewSubmission) -> Result<(), ForgeError>;
    async fn set_thread_resolved(&self, repo: &RepoCtx, thread: &ThreadRef, resolved: bool) -> Result<(), ForgeError>;
    async fn reactions(&self, repo: &RepoCtx, subjects: &[ReactionSubject]) -> Result<Vec<ReactionGroup>, ForgeError>;
    async fn set_reaction(&self, repo: &RepoCtx, subject: &ReactionSubject, reaction: &str, on: bool) -> Result<(), ForgeError>;
    async fn mentionable_users(&self, repo: &RepoCtx, query: &str) -> Result<Vec<recur_domain::PRParticipant>, ForgeError>;

    // ---- Merge
    /// Merge or enqueue. GitHub: REST merge (retry while mergeability is pending),
    /// GraphQL fallback, `gh pr merge`, merge queue via auto-merge or enqueuePullRequest.
    async fn merge(&self, repo: &RepoCtx, number: u64, req: MergeRequest) -> Result<MergeOutcome, ForgeError>;

    // ---- Message hooks (pr.go formatSquash/MergeCommitMessage, githubPRMarkdown)
    /// Commit text for a forge merge. GitHub: "<title> (#n)", empty title = let the
    /// forge default, squash title+commits omits the body when there are no commits.
    fn merge_message(&self, input: &MergeMessageInput) -> MergeCommitText;
    /// PR body as the forge stores it (markdown conversion, 60000-char cap).
    fn format_pr_body(&self, body: &str) -> String;
    /// Title limits: (preferred max, retry max) = (256, 80) for GitHub.
    fn title_limits(&self) -> (usize, usize);

    // ---- Images and agent-facing text
    /// Authenticated fetch for forge-hosted attachment images; Ok(None) = not ours.
    async fn fetch_image(&self, url: &str) -> Result<Option<ImageBytes>, ForgeError>;
    /// One sentence for the chat surface prompt ("use `gh pr view` / `gh pr diff`").
    fn agent_subject_hint(&self, pr: &PrSubject) -> String;
    /// User-facing failure diagnosis (ghFailReason / ghFailDiag / auth failure note).
    fn failure_note(&self, err: &ForgeError) -> String;
}

pub struct RepoRef { pub owner: String, pub name: String, pub host: String }
pub struct RepoCtx { pub repo: RepoRef, pub checkout: std::path::PathBuf } // cwd for CLI forges
pub struct PrListQuery { pub state: PrListState, pub limit: u32 }        // Open 100 / Merged 20
pub enum MergeOutcome { Merged { sha: Option<String> }, Queued }
pub struct ForgeError { pub kind: ForgeErrorKind, pub message: String, pub stderr: String }
// ForgeErrorKind: Unavailable | Auth | NotFound | Conflict | TitleTooLong | Pending | Failed
```

Engine-side logic that stays forge-neutral and must not move into the trait: snapshot merging into the stored PR (`mergeGitHubComments`, keep Recur-only comments), dropped-comment / resolved-thread / reaction holds against stale snapshots, sync single-flight + TTL + sweep, throttles, workspace-to-PR linking by head branch, PR chat hosts, Local PR operations. Pure helpers that GitHub and future forges share live as free functions in `recur_forge::model` (participants merge, reviewer ranking, check buckets and durations, attention reasons).

GitLab fit: `detect` on gitlab hosts, `head_ref` = `refs/merge-requests/N/head`, `merge_message` without `(#n)`, `branch_rules` from protected branches. Nothing in the engine changes.

### 2.3 `recur_issues::IssueTracker`

Task-prompt expansion stays in the shipped `bundles/Linear` and `bundles/GitHub` prompt actions (`[[prompt]]`, `[[search]]`, `[[expand]]`), exactly as today. The trait covers what Go code does.

```rust
// crates/issues/src/lib.rs
#[async_trait::async_trait]
pub trait IssueTracker: Send + Sync + 'static {
    fn id(&self) -> &str;          // IssueRef.source: "github" | "linear"
    fn label(&self) -> &str;
    fn configured(&self, settings: &Settings) -> bool;
    /// Open issues assigned to the user plus unassigned ones in their repos/teams
    /// (Linear unassigned stops at 300; keep the simplify note).
    async fn list_open(&self, cx: &IssueContext) -> Result<Vec<AssignedIssue>, IssueError>;
    /// Fill `AssignedIssue.project_id` (git remotes incl. upstream for GitHub,
    /// LinearProjects setting for Linear).
    fn link_projects(&self, issues: &mut [AssignedIssue], projects: &[recur_domain::Project], cx: &IssueContext);
    /// Assign to the viewer when unowned (SetWorkspaceIssue). Errors are logged, not surfaced.
    async fn claim(&self, issue: &recur_domain::IssueRef, settings: &Settings) -> Result<(), IssueError>;
    /// Native-app deep link for a web URL (OpenLinearInApp).
    fn app_url(&self, web_url: &str) -> Option<String> { let _ = web_url; None }

    // ---- Agent wiring (engine user_mcp.go, commit_message.go)
    /// Settings MCP preset id this tracker provides ("linear").
    fn mcp_preset(&self) -> Option<&str> { None }
    /// Build the MCP attachment for a turn (Linear: URL by mode, bearer key from
    /// bundle setting or LINEAR_API_KEY).
    fn agent_mcp(&self, settings: &Settings, mode: AgentMcpMode) -> Option<McpAttach> { let _ = (settings, mode); None }
    /// Appended system text when the MCP is attached ("Linear MCP is connected. Default team key: X").
    fn agent_hint(&self, settings: &Settings) -> Option<String> { let _ = settings; None }
    /// Env vars spawned agent CLIs must not inherit (LINEAR_API_KEY).
    fn secret_env(&self) -> &[&str] { &[] }
}

pub struct IssueContext { pub settings: Settings, pub projects: Vec<recur_domain::Project>,
                          pub github_remotes: std::collections::HashMap<String, String> }
pub enum AgentMcpMode { Loop, ReadOnly, Agent }
```

`AssignedIssue`/`IssueLabel` move from `app/issues.go` into `recur_issues` with serde names unchanged. The `Identifier:` / `Title:` header that `loopCommitMessage` reads from expanded goals is a bundle-output convention, so it stays a generic parser in `recur_issues::goal_meta` rather than a trait method. Jira fit: `list_open` via JQL, `claim` assigns, `agent_mcp` returns the Atlassian MCP.

### 2.4 `recur_agent_tools::Host` (the engine seam)

Go `agenttools.Host` (= `mcp.Reporter`) is ported as one trait, method for method. Default bodies are Go's `noopReporter`: empty results, and `request_permission` denies with reason `"no permission broker is attached"`. `recur_engine::host::EngineHost` implements it; the slice that owns each method is in brackets. Return strings are the exact tool-result texts Go returns, because agents read them.

```rust
// crates/agent-tools/src/host.rs   (ws = workspace id, agent = agent code)
#[async_trait::async_trait]
pub trait Host: Send + Sync {
    // Loop flow
    fn report_finding(&self, ws: &str, agent: &str, f: Finding) -> recur_domain::Finding;                 // [E1]
    fn done(&self, ws: &str, agent: &str, summary: &str, notes: &str) -> Result<(), HostError>;          // [E1]
    fn list_findings(&self, ws: &str, agent: &str, status: &str) -> Result<Vec<recur_domain::Finding>, HostError>; // [E1]
    fn update_finding(&self, ws: &str, agent: &str, finding_id: &str, status: &str, reason: &str)
        -> Result<recur_domain::Finding, HostError>;                                                     // [E1]
    fn report_result(&self, ws: &str, agent: &str, passed: bool, summary: &str);                         // [E1]
    fn plan(&self, ws: &str, agent: &str, summary: &str);                                                // [E3]
    fn escalate(&self, ws: &str, agent: &str, e: EscalationRequest);                                     // [E1]
    fn attach_evidence(&self, ws: &str, agent: &str, e: Evidence);                                       // [E1]
    fn note_tool_args(&self, ws: &str, agent: &str, tool: &str, args: &str);                            // [E2]

    // Chat
    async fn ask_user(&self, ws: &str, agent: &str, req: AskUserRequest) -> String;                      // [E2]
    async fn switch_mode(&self, ws: &str, agent: &str, mode: &str) -> String;                            // [E4]
    async fn create_artifact(&self, ws: &str, agent: &str, req: CreateArtifactRequest)
        -> Result<recur_domain::Artifact, HostError>;                                                    // [E2]
    async fn list_artifacts(&self, ws: &str, agent: &str) -> Result<Vec<recur_domain::Artifact>, HostError>; // [E2]
    async fn read_artifact(&self, ws: &str, agent: &str, artifact_id: &str) -> Result<String, HostError>; // [E2]
    async fn present_artifact(&self, ws: &str, agent: &str, artifact_id: &str) -> String;                // [E2]
    async fn rename_session(&self, ws: &str, agent: &str, owner_chat: &str, title: &str) -> String;      // [E2]
    async fn rename_workspace(&self, ws: &str, agent: &str, title: &str) -> String;                      // [E2]
    async fn rename_branch(&self, ws: &str, agent: &str, name: &str) -> String;                          // [E2]
    async fn open_pr(&self, ws: &str, agent: &str, title: &str, body: &str, draft: bool) -> String;      // [E5]
    async fn create_workspace(&self, ws: &str, agent: &str, req: CreateWorkspaceRequest) -> String;      // [E4]
    async fn merge_child(&self, ws: &str, agent: &str, child_id: &str) -> String;                        // [E4]
    async fn read_child(&self, ws: &str, agent: &str, child_id: &str) -> String;                         // [E4]
    async fn archive_child(&self, ws: &str, agent: &str, child_id: &str) -> String;                      // [E4]
    async fn message_child(&self, ws: &str, agent: &str, child_id: &str, message: &str) -> String;       // [E4]
    async fn read_child_artifact(&self, ws: &str, agent: &str, child_id: &str, artifact_id: &str)
        -> Result<String, HostError>;                                                                    // [E4]

    /// The only long-blocking call. Must return when `cancel` fires (HTTP client
    /// disconnected or turn ended) and honor the broker's own 10-minute timeout.
    async fn request_permission(&self, cancel: &CancellationToken, ws: &str, agent: &str, ask: PermissionAsk)
        -> Result<PermissionVerdict, HostError>;                                                         // [E2]

    // Conductor hands
    async fn send_back(&self, ws: &str, agent: &str, instruction: &str) -> String;                       // [E5]
    async fn edit_plan(&self, ws: &str, agent: &str, plan: &str) -> String;                              // [E3]
    async fn rerun_review(&self, ws: &str, agent: &str) -> String;                                       // [E3]
    async fn dispatch_specialist(&self, ws: &str, agent: &str, role: &str, instruction: &str) -> String; // [E3]
    async fn extend_iterations(&self, ws: &str, agent: &str, n: i64) -> String;                          // [E3]
    async fn extend_rounds(&self, ws: &str, agent: &str, n: i64) -> String;                              // [E3]
    async fn reenter_phase(&self, ws: &str, agent: &str, phase_id: &str) -> String;                      // [E3]
    async fn run_phase(&self, ws: &str, agent: &str, phase_id: &str) -> String;                          // [E3]
    async fn resume_loop(&self, ws: &str, agent: &str) -> String;                                        // [E3]

    // Plans
    async fn submit_plan(&self, ws: &str, agent: &str, p: PlanSubmission) -> String;                     // [E2]
    async fn update_plan_step(&self, ws: &str, agent: &str, step_id: &str, status: &str, note: &str) -> String; // [E2]

    // Skills, terminals
    async fn list_skills(&self, ws: &str, agent: &str) -> Result<Vec<recur_domain::AgentSkill>, HostError>; // [E2]
    async fn activate_skill(&self, ws: &str, agent: &str, name: &str) -> Result<String, HostError>;      // [E2]
    async fn list_terminals(&self, ws: &str, agent: &str) -> Option<Vec<TerminalInfo>>;                  // [E2]
    async fn read_terminal(&self, ws: &str, agent: &str, id: &str, tail_lines: i64) -> Result<String, HostError>; // [E2]

    // Ask Recur
    async fn list_workspace_briefs(&self) -> recur_domain::BriefingSnapshot;                             // [E4]
    async fn workspace_context(&self, id: &str) -> Result<recur_domain::WorkspaceContext, HostError>;    // [E4]
    async fn present_workspace(&self, target: &str) -> String;                                           // [E4]
}
```

Every method above has a default body in the real file (omitted here for length). Sync methods stay sync because Go callers rely on them completing before the tool response is written (`report_finding` returns the stored row, `note_tool_args` runs before the handler). The `report_finding` / `done` / `escalate` / `attach_evidence` / `report_result` bodies only touch the store and the running loop's `LoopHandle`, which is why E1 owns them.

Slice T deviations from the sketch above, all to keep Go's observable behavior:

- `note_tool_args` takes the raw arguments as `&str`, empty when the tools/call carried none (Go passes a nil `json.RawMessage`; `RawValue` cannot be empty). `"null"` stays `"null"`.
- `extend_iterations`, `extend_rounds` and `read_terminal`'s `tail_lines` are `i64`: they are Go `int` values decoded from agent JSON and passed through unclamped (a negative `tail_lines` reaches the host, as in Go).
- `list_terminals` returns `Option<Vec<_>>`: `None` is Go's nil slice, which list_terminals encodes as `"terminals":null` (the engine returns `nil` when no terminal manager is attached, an empty list otherwise).
- `NoopHost` is the public unit type carrying the default bodies (Go `noopReporter`); `McpServer::new(None)` uses it.

## 3. Event bus

Go's bus (`engine/bus.go`) is `Publish(Event{Name, Data any})`, synchronous fan-out to subscribers in the publisher's goroutine. Subscribers today: `main.go` (Wails emit of every event, OS notification for `notify`, dock badge from `attention.updated`), `app.Services` (file-index pin on `workspace.watched`), tests.

Rust: `recur_engine::bus::Bus` carries a typed `EngineEvent` enum. Each variant serializes to the Go name and the exact Go JSON payload (`#[serde(tag = ...)]` is not used; the name and payload are separate, as today). `recur-services` maps every `EngineEvent` plus its own events into `AppEvent` 1:1.

Invariants to keep:

- **Never drop.** Use an unbounded `tokio::sync::mpsc` per subscriber (or synchronous callbacks that only enqueue). `tokio::sync::broadcast` is not acceptable: lagging receivers lose events and the UI has no resync for most of them.
- **Order per publisher.** Events from one task arrive in publish order. The session stream depends on it (`session.step` before its `session.tool_result`, `session.user` before the turn's deltas).
- **Persisted events.** Before publishing, the engine appends `workspace.event`, `workspace.updated` and `escalation.raised` to the `events` table (`Store.AppendEvent`, payload JSON, ms timestamp). Feed reconstruction (`feedproject.go`) reads them back, so their payload JSON is a storage format.
- **Publishing never holds a store or engine lock.** Go publishes after releasing locks in nearly every path; the Rust port must keep that (subscribers call back into services).

### 3.1 Engine events

Base for session events: `S = {surface?: "app", workspaceId?, code, ownerCode?}`. `seq` is process-wide monotonic per (workspace, code, kind) for top-level streams; nested (subagent) streams keep per-parent counters.

| Event | Payload | Publisher (slice) | UI |
|---|---|---|---|
| `workspace.updated` | Loop shape: `{workspaceId, state, phase, iteration, convergence, archived, group, statusLabel, progress, goal, short, plan, definitionOfDone, branch, baseBranch, diff, behindBase, worktreePath, port, lastError, hostPrId, loopChatCode, scheduleId}`. Hoist shape: `{workspaceId, fromWorkspaceId, codeRemap, movedCodes, chatsChanged, focusChatCode?}`. Diff sync shape: subset. Persisted. | E1 (`publishLoopUpdated`, diff sync), E5 (hoist) | `workspace.updated` |
| `workspace.event` | `{workspaceId, event: FeedItem}`. Persisted. | E1 (`emitWorkspaceEvent*`), SV (`recordHumanEdit`) | `workspace.event` |
| `workspace.deleted` | `{workspaceId}` | E1 | `workspace.deleted` |
| `workspace.removed` | `{workspaceId}` (archived) | E1 | `workspace.removed` |
| `workspace.watched` | `{workspaceId}` ("" = none) | E1 (fswatch) | internal: services pin the file index |
| `workspace.fs_changed` | `{workspaceId, rescan, paths?}` | E1 (fswatch), E5 (base change) | `workspace.fs_changed` |
| `workspace.chat_tab` | `{workspaceId, code, title}` | E2 (renames), E4 (provisional titles) | `workspace.chat_tab` |
| `agent.updated` | `{workspaceId, agentId, status}`; app chats add `{surface: "app", code}` with `workspaceId: ""` | E1 (`finishAgentWrite`, `updateAgentStatus`), E4 (app chat) | `agent.updated` |
| `agent.step` | `{workspaceId, agentId, step}` | E3 (loop turns), E3 (maestro "chat: ...") | `agent.step` |
| `resource.updated` | `Resource{loopsRunning, needsYou, itersToday, gitWarning, ...}` | E1 | `resource.updated` |
| `inbox.updated` | `{unread}` | E1 and every slice that writes inbox rows | `inbox.updated` |
| `attention.updated` | `{unseen, inboxUnread, dock, byWorkspace, byProject, appUnseen, workspaceId, code, tabUnseen, clearAllUnseen?}` | E1 | `attention.updated` (+ dock badge in desktop) |
| `briefing.updated` | `{workspaceId?}` | E4 | `briefing.updated` |
| `notify` | `{title, body, target?}` | E1, E2, E3 | internal: desktop shows an OS notification; click emits `notification.open` |
| `permission.requested` | `PermissionRequest` | E2 (broker) | `permission.requested` |
| `permission.resolved` | `{id, allow, reason}` | E2 | `permission.resolved` |
| `question.asked` | `{workspaceId, question}` | E2 (ask_user broker) | `question.asked` |
| `question.answered` | `{id, answer}` (`answer: {}` when cleared) | E2, E4 | `question.answered` |
| `escalation.raised` | `{workspaceId, escalation}`. Persisted. | E3 | `escalation.raised` |
| `escalation.resolved` | Live: `{workspaceId, answer}`. Answer path: `{workspaceId, answer, agentCode, open}` | E3 | `escalation.resolved` |
| `findings.updated` | `{workspaceId}` | E1 (host findings), SV (observatory edits) | `findings.updated` |
| `artifact.created` | `{workspaceId, artifact}` | E2 | `artifact.created` |
| `artifact.presented` | `{workspaceId, artifact}` | E2 | `artifact.presented` |
| `chat.mode_changed` | `{workspaceId, agentCode, mode, from}` | E4 | `chat.mode_changed` |
| `chat.renamed` | `{workspaceId, agentCode, title}` | E2 | internal (no UI listener; keep for parity of the bus) |
| `app.chat_tab` | `{code, title}` | E4 | internal (no UI listener today) |
| `app.focused` | `{}` when the app regains focus | E1 | `app.focused` |
| `session.user` | `S + {text}` | E2 | `session.user` |
| `session.delta` | `S + {text (cumulative), seq, replace, done?, uuid?, parentId?}` | E2 | `session.delta` |
| `session.thinking` | `S + {text, seq, replace, done?, uuid?, parentId?}` or `S + {tokens, parentId?}` | E2 | `session.thinking` |
| `session.step` | `S + {id, tool, toolInput, description, input, parentId?, uuid?}` | E2 | `session.step` |
| `session.tool_result` | `S + {id, result, isError, input?, toolInput?}` | E2 | `session.tool_result` |
| `session.tool_input` | `{workspaceId, code, id, input, toolInput}` | E2 (backfill) | `session.tool_input` |
| `session.compact` | `S + {status, trigger, summary?, preTokens?}` | E2 | `session.compact` |
| `session.fallback` | `S + {from, to, fromLabel, toLabel, retracted?}` | E2 | `session.fallback` |
| `session.context_usage` | `S + {inputTokens, cacheCreationTokens, cacheReadTokens, contextWindow}` | E2 | `session.context_usage` |
| `session.turn_end` | `S + {durationMs, ...context usage}` | E2 | `session.turn_end` |
| `session.notice` | `S + {text}` | E2 | `session.notice` |
| `pr.updated` | `{prId, state, group, checksOk, hasChecks, fileCount, title, number, workspaceId, projectId, source, branch, base, when, diff, behindBase, updatingBase, pendingComments, draft, mergeable, mergeStateStatus, reviewDecision, reviewRequested, attentionReasons, participants, reviewers, allowedMergeMethods, ...}`; `pendingOnly: true` for draft-comment-only updates | E5 | `pr.updated` |
| `pr.comment` | `{prId, comment}` | E5 | `pr.comment` |
| `pr.removed` | `{prId}` | E5 | `pr.removed` |
| `prs.synced` | `{projectId, prs}` | E5 | `prs.synced` |
| `github.branch_rules` | `{projectId, repo, remote, branch, requirePR}` | E5 (through `Forge::branch_rules` change hook) | `github.branch_rules` (name kept; payload is forge neutral) |

### 3.2 Service and support events (not engine)

| Event | Payload | Publisher (slice) | UI |
|---|---|---|---|
| `workspace.file_index` | `{workspaceId, gen, ready, building, ...}` | SV (via `recur-code` index hub) | `workspace.file_index` |
| `cursor.install` | `{phase, message, current, total}` / `{phase: "error", message, percent: -1}` | P2 | `cursor.install` |
| `provider.usage` | `usage.Snapshot` | P3 (poller) | `provider.usage` |
| `app.open` | `OpenPathResult` | SV (`OpenPath`), desktop (`open_boot.go`) | `app.open` |
| `term.data` / `term.exit` / `term.busy` / `term.idle` | `{id, data(b64)}` / `{id, workspaceId, exitCode}` / `{id, workspaceId}` | recur-term via SV | same |
| `lsp.message` / `lsp.exit` | `{id, message}` / `{id}` | P3 (`recur-lsp`) via SV | same |

Desktop-only events in PARITY (`app.ready`, `files.dropped`, `menu.*`, `notification.open`, `system.theme`, `updater.check`, `wails:updater:*`, `window.titlebarDblclick`) are emitted by the desktop crate, not by any slice here.

## 4. Concurrency model

### 4.1 Primitive mapping

| Go | Rust | Notes |
|---|---|---|
| goroutine | `tokio::spawn` (tracked) | Every long-lived task is tracked in a `TaskTracker` group (`loops`, `teardown`, `schedules`, `chat_turns`) so Shutdown can join them, like Go's `loopWG`/`teardownWG`/`scheduleWG`. |
| `context.Context` | `CancellationToken` (child tokens for derived contexts) | `context.WithTimeout` = child token + `tokio::time::timeout`. `context.WithoutCancel` (Cursor CloseAgent) = fresh token. |
| `context.AfterFunc(ctx, broadcast)` | `select!` on `token.cancelled()` next to the wait | Used by `Loop.checkpoint`, `awaitAnswer`, governor waits. |
| `sync.Mutex` held across blocking I/O | `tokio::sync::Mutex` | Only where Go holds it across git/process calls (`dispatchMu`, `scheduleMu`, `openMainMu`, per-PR merge locks, `mergeChildLocks`). |
| `sync.Mutex` around plain state | `parking_lot::Mutex` | Never held across `.await`. Enforce with `clippy::await_holding_lock`. |
| `sync.Cond` | `tokio::sync::Notify` + state under a mutex | Loop pause/escalation, governor, chat tab FIFO. Re-check state in a loop after every wake (Go code already does). |
| `sync.Map` | `DashMap` or `Mutex<HashMap>` | `loops`, `chats`, `creating`, `childWaves`, `chatChildWatchers`, `resumingChats`. `LoadOrStore`/`CompareAndDelete` semantics must be kept: use `entry()` APIs, compare by `Arc::ptr_eq`. |
| `sync.Once` | `std::sync::Once` / `OnceLock` / an `AtomicBool` swap | `lifeLock` unlock idempotence, `stopChatInject`, Cursor cancel-all. |
| `atomic.*` | `std::sync::atomic` | Cursor `steerUnavailable`, `inUse`, tab `stopGen`. |
| unbuffered chan + ack | `mpsc::channel(1)` + `oneshot` ack | Steering (2.1). Delivery is only true after the ack. |
| buffered chan (cap N), non-blocking send | `mpsc::channel(N)` + `try_send` | Loop `cmds` (8), `activeInject` (1), Claude stdout lines (16), ack chan (8). |
| `time.AfterFunc` | spawned `sleep` task holding an abort handle | Turn deadline (`arm`), debounces. Cancel = `abort()`. |
| `time.Ticker` | `tokio::time::interval` with `MissedTickBehavior::Delay` | Schedules (1 s), PR sweep (3 min), fswatch poll (7 s), usage poller. |
| `exec.Cmd` + `Setpgid` | `tokio::process::Command` + `process_group(0)` + `kill_on_drop(false)` | Group kill SIGTERM → 2 s → SIGKILL via `nix::sys::signal::killpg`. One reaper task owns `wait()`. |
| `bufio.Scanner` (8 MiB lines) | `BufReader::lines()` with a length cap | Claude stdout, Codex RPC, LSP frames. |

### 4.2 Locks and their order

Keep these names as fields on the Rust `Engine` so reviews can map one to one.

| Lock | Guards | Order rule |
|---|---|---|
| `dispatchMu` (async) | D26 promote critical section: `dispatch`, `drainQueued`, `resumeNotRunningLocked`, `resumeAfterAnswer`, fan-out `startStepChild` (held across queue → metadata → dispatch), `idleWalkByChat` | Outermost engine lock. Never taken while holding a chat life lock unless Go does (`dispatch` takes `lockChatLife` inside it). |
| `chatLifeMu` + per-tab `lifeLock` | open/send/delete/hoist of one chat tab | `dispatchMu` → `lifeLock`. `lifeLock` must be **released** before steering (Claude acks after a tool_result; `switch_mode` needs the lock) and before waiting on the FIFO ticket; `sendWorkspaceTurn` unlocks → claims → relocks. |
| `chatTabSlot` FIFO | per-tab send order; `stopGen` | Ticket taken before `lifeLock`. Rust: async ticket gate (`AtomicU64 next`, `cur` + `Notify`); "holding the slot" = being `cur`. Go's `claim` returns holding the mutex, which cannot cross an await in Rust; pause/resume become no-ops. Must stay FIFO by `begin()` order. |
| `teardownMu` | `shuttingDown`, Pause/Stop command delivery vs Shutdown | Taken briefly; never across a loop wait. |
| `hoistMu` | PR chat host hoisting vs `reserveDeleting` | Reserve-deleting and hoist are mutually exclusive and atomic with respect to each other. |
| `watchMu` → `chatFocusMu` | watched workspace, chat focus | Fixed order. `SetAppFocused` must not call `WatchedWorkspace` (would invert it; Go comment marks the deadlock). |
| `chatUnseenMu` | unseen flags + inbox writes | Taken inside `updateAgentStatus`; re-check focus after the write. |
| `scheduleMu` (async) | schedule config, occurrence claims, loop-mode scheduled launch registration, Shutdown | Shutdown closes `scheduleStop` and waits `scheduleDone` under it. |
| Store `mu` (store-wide) | in-memory cache + write-through | `UpdateWorkspace(id, closure)` runs the closure under it: closures must not call back into the engine or publish. Store `feedMu` → `mu`. |
| `Loop.mu` + cond | paused/stopped/escalated/answer/pendingEsc/pendingSteer/grantedExtra/doneSummary | Written from MCP tasks; read by the loop task. `send()` mutates state and notifies **before** pushing to `cmds`. |
| `Governor.mu` + cond | loop/agent slot counts, caps | Release wakes one waiter (`Signal`); `SetCaps` wakes all. |
| `permissionBroker.mu`, `askUserBroker.mu` | pending maps | Leaf locks. |
| `diffSyncMu`, per-workspace apply mutex | debounce timers, generation | Apply mutex per workspace serializes store writes; generation check discards stale results. |
| `streamSeqMu` | process-wide session seq | Leaf. |
| Cursor `cursorTurn.mu`, `emitMu`, `sendMu` | per-turn fold state, emit vs steer flush, send generation | Provider-internal. |
| Codex `emitMu` + `emitClosed` | notifications vs final result | Provider-internal: no event after the result. |

### 4.3 Invariants

1. **Restart recovery.** `Recover()` runs before the UI loads. Interrupted loop states (planning/coding/reviewing/verifying, or queued with `ScheduledPromptPending`) become `paused` with phase exactly `"interrupted"` (or `"scheduled run interrupted"`); `AutoResume` keys on that string. Stopped/failed rows in the needs group return to active and drop inbox rows. Escalated-without-open-escalation becomes `paused` "answered \u{2014} waiting to resume". Live-but-dead chats are healed (streaming rows closed with StreamError, synthetic tool results, sessions idle) and queued; `AutoResume` then launches chats, resumes loops with checkpoints, launches held chats after each Resume, and re-arms chat-child watchers last. The order is: `SyncChatInbox` → launch chats except held → per loop Resume → its held chats → `rearmChatChildren`.
2. **Governor.** Loop slots are non-blocking (`TryAcquireLoop`; else `queued`). Agent slots block (`AcquireAgent(done)`), wake one on release, all on cap change. Caps ≤ 0 mean unlimited; shrinking caps never preempts. After `ApplyGovernorCaps`, `drainQueued` runs while `LoopsRunning` grows. `drainQueued` promotes the **oldest** queued workspace: it relies on `Store.Workspaces()` returning insertion order. Keep-awake and Cursor close-when-idle hook into acquire/release.
3. **Chat cancel.** `CancelChat` bumps the tab `stopGen` so queued sends return "Stopped by user" without running; a live turn is marked stopped (vs parked, vs timed out: three distinct reply texts), its inject channel is closed, its token cancelled. `ended` is closed exactly once, in `unregisterChatTurn`. `registerChatTurn` cancels any prior turn on the same key (swap), and refuses while shutting down or while removal is committed. `parkChatTurnIf(expect)` only parks the asking turn (ask_user, switch_mode) and never prints "Stopped".
4. **Loop walker.** One task per loop. Run's deferred order matters: panic → failed state before slot release → `loopExited` (map delete, release slot, settled wrap-up, unseen flag, `drainQueued`). Commands go through `send()` which mutates under `mu`, notifies, then `try_send`s. `checkpoint()` drains commands, raises a pending escalation (first wins), then waits while paused. Transient turn errors (D56) end the walk as retryable `failed`; genuine errors degrade or fail. Shutdown leaves escalation rows open; explicit stop retires them.
5. **Schedules.** One ticker task; each tick runs under `scheduleMu`. Startup tick marks running/waiting occurrences "interrupted" and counts anything ≥ 1 minute late as missed. Busy targets are skipped, not queued. Loop-mode launches hold `scheduleMu` around chat registration so Shutdown cannot interleave.
6. **Filesystem watching.** Exactly one watched workspace. Switching cancels the previous watcher. Events are batched (400 ms latency, first event immediate). `.git/HEAD` changes trigger branch sync; ignored paths are `.git/`, `.DS_Store`, `node_modules`. A 7 s poll computes a git fingerprint (status, HEAD, branch, upstream, base tip, compare tip) as a backstop; a missing root ends the watcher. Diff sync is debounced 600 ms per workspace with a generation guard; refresh-all is single-flight with one trailing rerun and parallelism 4.
7. **PR sync.** `SyncGitHubPRs` is single-flight with a 30 s TTL (callers wait for the in-flight sweep), 8 projects concurrently, two lists each. The 3-minute sweep publishes `pr.updated` only for rows that changed ignoring `when`. Conversation refresh is throttled 20 s and diff refresh 60 s, both single-flight. Local holds (dropped comments 2 min, resolved threads, reactions) override stale snapshots. PR state is recomputed from the live row inside `UpdatePR`, never from the snapshot alone.
8. **MCP.** Tool handlers run on HTTP tasks, concurrently with the loop task. `NoteToolArgs` runs before the handler. `request_permission` may block 10 minutes; the server sets no timeout, and client disconnect cancels the wait. Unregister at turn end resolves stale permission prompts with "the turn ended before this was answered".
9. **Shutdown order.** `shuttingDown = true` (under `teardownMu`) → close `scheduleStop`, wait `scheduleDone` → unwatch → cancel creating allocations → cancel loops → cancel chats, collecting `ended` → join schedule/loop/teardown tasks and chat `ended` with a 5 s cap → close MCP server (3 s graceful) → close Cursor pool → close keep-awake. Chats cancelled by shutdown keep their streamed text (no "Stopped by user").
10. **Bus.** See section 3: synchronous order, never drop, publish outside locks.

### 4.4 Known Go data race to fix, not port

- `syncWorkspaceBranch` writes `loop.Branch` from the fswatch goroutine while the loop reads it. Move `branch` into `LoopHandle` state under its mutex.

## 5. Work slices and waves

### 5.0 Prerequisites outside this plan (wave 0)

Ported by other agents in parallel; the slices below compile against their public APIs and must not re-implement them:

- `recur-domain` (all types in section 7), `recur-store` (methods in section 7, plus `projfile`), `recur-git` (the engine's `gitpkg.Manager`: worktrees, checkpoints, diffs, merges, fetch/push), `recur-bundles` (catalog, roles, loops, policies, prompt actions, `RunPrompt`), `recur-term` (`Manager`: list, active id, `read_plain`), `recur-platform` (power keeper, notifications).
- Until a prerequisite lands, a slice may code against its documented API and keep its tests that need it `#[ignore]`d with a comment naming the prerequisite. No stub implementations of prerequisite crates.

### 5.1 Overview

| Wave | Slice | Crates / modules | Go lines (src) |
|---|---|---|---:|
| 1 | **A** agent contracts | `recur-agent`, `recur-provider-claude::stream` + `::testing` | ~2.6k |
| 1 | **T** agent tools + MCP | `recur-agent-tools`, `recur-mcp` | ~4.6k |
| 1 | **C** code + support libs | `recur-code`, `recur_engine::{runconfig,wsenv,simplify::text}` | ~7.0k |
| 1 | **F** forge + issue trackers | `recur-forge`, `recur-issues` | ~6.3k |
| 1 | **E1** engine core | `recur_engine::{core,bus,publish,governor,chatlife,attention,loop_handle,loop_registry,workspace::*,fswatch,roles,guard}` | ~6.9k |
| 2 | **P1** Claude + Codex | `recur-provider-claude` (runner), `recur-provider-codex` | ~4.6k |
| 2 | **P2** Cursor | `recur-provider-cursor` | ~6.6k |
| 2 | **P3** compatible + usage + LSP | `recur-provider-compatible`, `recur-usage`, `recur-lsp` | ~7.1k |
| 2 | **E2** turn pipeline + sessions | `recur_engine::{turn::*,host,composer::*,artifacts,attachments,permissions,ask_user,plan,simplify,mcp_attach,renames}` | ~7.6k |
| 3 | **E3** loops | `recur_engine::loops::*` | ~7.7k |
| 3 | **E4** chat + coordination | `recur_engine::{chat::*,app_chat,schedules,briefing,feed}` | ~7.0k |
| 3 | **E5** pull requests | `recur_engine::{prs::*,hoist}` | ~5.6k |
| 4 | **SV** services | `recur-services` | ~7.0k |

Waves are dependency order, not calendar: a wave-2 slice may start as soon as the milestones it needs from wave 1 land.

### 5.2 Milestones (land first inside a slice, others depend on them)

| Milestone | Contents | Needed by |
|---|---|---|
| **A-M1** | `Provider`, `ProviderCaps`, `TurnRequest`, `TurnContext`, `EventSink`, `AgentEvent`, `TurnResult`, `ProviderError`, `Steer`, `PermissionAsker`, catalog + quota types, `ToolDisplay`, `recur_agent::testing::ScriptedProvider` (event scripts, edits, `on_run`, one steer drained after each tool result) | everyone |
| **A-M2** | Claude NDJSON parser (`recur_provider_claude::stream`) + `recur_provider_claude::testing::NdjsonScript` (Go `ScriptedRunner`: replays NDJSON lines through the real parser) behind a `testing` feature | engine tests (44 Go test files use `ScriptedRunner`), SV tests (33 uses) |
| **T-M1** | `Host` trait with defaults, request types, `McpServer` handle (`start`, `url`, `register_owned`, `unregister`, `drop_agent`, `drop_workspace`, `close`) | E1, E2, P3 |
| **F-M1** | `Forge` + `IssueTracker` traits, model types, `recur_forge::testing::FakeForge` replacing Go's 19 function-variable seams (`listGitHubPRsFn`, `postGitHubPRCommentFn`, `submitGitHubPRReviewFn`, `fetchGitHubBranchRequiresPRFn`, `githubViewerFn`, ...) | E5, E3, E1 |
| **E1-M1** | `Engine` struct with every field in 4.2, `EngineEvent` (all variants of section 3, serialization tests), `testing` fixture (temp store + repo + scripted provider + bus recorder), `LoopHandle` API, `chatlife` API, governor, `publish_*` helpers, `emit_workspace_event*` | E2, E3, E4, E5 |
| **E2-M1** | provider router `run_agent` (skill guidance, comment hygiene, user MCP, model ref split, partials), mirror + broadcast, `finalize_chat_reply`, `persist_chat_failure`, `register_mcp`, `expand_composer`, `prepare_chat_attachments` | E3, E4, E5 |
| **E3-M1** | `queue_start_workspace`, `start_run_on_workspace_with`, `dispatch_additional_chat_run`, `resume`, `send_maestro_message`, `send_loop_chat_message` | E4, E5 |
| **E4-M1** | `send_workspace_turn` / `send_workspace_message`, `create_workspace_chat`, `remember_workspace_chat_selection`, `ensure_pair_agent` | E3 (chat resume), E5 (PR chat host) |
| **E5-M1** | `publish_pr_updated`, `emit_pr_updated`, `link_workspace_to_matching_prs`, `drop_unmerged_sibling_prs`, `maybe_archive_linked_pr_chat_host`, `archive_pr_chat_hosts` | E1 (branch sync, chat life unlock), E3 (`open_pr`) |

### 5.3 Wave 1

#### A: agent contracts (`recur-agent`)

- **Go files:** `runner.go`, `toolview.go`, `agenterr.go` (classification), `attachments.go` (markers, `UserImage`, content blocks, ordering), `permission.go` (labels + `permissionSummary`, shared with Codex), `streamjson.go` (whole file: parser into `recur_provider_claude::stream`, display helpers into `recur_agent::display`), `scripted.go`, mirror's `contextUsageTracker`, model catalog types from `app/model_catalog.go`, quota types from `usage/types.go`.
- **Tests:** `streamjson_test.go`, `standing_test.go`, `context_usage_test.go` (tracker cases), `permission_test.go` (label cases), `attachments_test.go` (marker/content-block cases).
- **Frontend modules to absorb** (pure rules the Rust UI would otherwise duplicate; verify each when porting): `lib/contextUsage.ts`, `lib/tasktools.ts`, `lib/todos.ts`, `lib/webtools.ts`, `lib/mcp.ts`.
- **Prerequisites:** `recur-domain`.
- **Public API:** section 2.1; `recur_agent::display::{chat_tool_input, chat_tool_result, compact_tool_result, tool_description, summarize_tool_input, summarize_display_input, display_tool_result, canonical_claude_tool, create_plan_body, truncate, hard_truncate_runes, is_task_tool, is_create_plan_tool, is_subagent_launch_meta, is_list_findings_tool, FEED_INPUT_LIMIT, CHAT_DETAIL_LIMIT, PLAN_MAX_PERSIST_CHARS}` plus `display::{tasks, todos, web}`; `recur_agent::attach::{marker, parse_markers, strip_markers, ensure_markers, user_content_blocks, images_in_marker_order, ordinal_word, sanitize_attachment_id, chat_image_ext, chat_image_mime_for_ext, UserImage, DISPLAY_HIDDEN_SENTINEL, MAX_CHAT_ATTACHMENT_BYTES}`; `recur_agent::error::{classify, failure_reason, known_agent_error, tidy, first_line, has_transient_marker, TRANSIENT_MARKERS, CODEX_LOGIN_EXPIRED}`; `recur_agent::permission::{tool_label, summary, PROMPT_TOOL, RECUR_TOOL_LABELS}`; `recur_agent::turn::{format_steer_user_message, follow_up_text}`; `recur_agent::standing_in_user_prompt`; `recur_agent::{context_usage, mcp}` (frontend rules); `recur_agent::testing::{ScriptedProvider, EventRecorder}` and `recur_provider_claude::{stream, testing::{NdjsonScript, fixture_lines}}` behind `testing` features.

#### T: agent tools + MCP (`recur-agent-tools`, `recur-mcp`)

- **Go files:** all of `internal/agenttools` and `internal/mcp`; tool-name constants from `engine/roles.go` (`mcp__recur__*` names belong to the catalog).
- **Tests:** all `agenttools` and `mcp` tests. Add a snapshot test of `tools/list` output (names, order, descriptions, schemas) generated from the Go server so drift is caught.
- **Frontend modules:** `lib/permissionTools.ts`, `lib/plantools.ts`, `lib/briefingtools.ts`, `lib/open-pr-tool.ts`, `lib/reenter-phase-tool.ts` (tool input parsing for display).
- **Prerequisites:** `recur-domain`, `recur-code::rg` (C, for `grep`; `glob`/`grep` tests wait on it).
- **Public API:** `Host` (2.4), `Tool`, `Registry`, `host_tools()`, `coding_tools()`, `halt_after`, `TodoStore`; `McpServer` (T-M1) with exact URL shape `http://127.0.0.1:<port>/<token>/mcp`.
- **Landed as:** Glob and Grep (and `coding_tools()`, which lists them) compile behind the `recur-agent-tools` feature `code` until C and the store port land, because they call into crates that are still empty; every other tool, including Read/Edit/Write/Bash/TodoWrite, is always built. They are coded against `recur_code::rg::locate(data_dir: &Path) -> Result<PathBuf, E>` (Go `rg.Locate`), `recur_code::search_opts(cancel: &CancellationToken, root: &Path, query: &str, opts: &recur_domain::SearchOpts) -> Result<recur_domain::SearchResult, E: Display>` (sync, Go `code.SearchOpts`) and `recur_store::data_dir() -> Result<PathBuf, E>` (Go `store.DataDir`); a throwaway shim with exactly these signatures passed the gated tests (`cargo test -p recur-agent-tools --features code`) with a real ripgrep. When those exist, make `code` a default feature (or drop the gate) and run the `code`-gated tests. `McpServer` methods: `new(Option<Arc<dyn Host>>)`, `with_addr`, `start`, `close`, `port`, `url_for`, `register`, `register_owned`, `unregister -> Option<(ws, agent)>`, `drop_token` (Go `Drop`), `drop_agent`, `drop_workspace`; free functions `names_from_allowed_tools`, `append_permission_request`. Tool-name constants live in `recur_agent_tools::names` (bare) and `recur_agent_tools::names::mcp` (`mcp__recur__*`).

#### C: code + support libs

- **Go files:** `internal/code` (all), `internal/rg`, `internal/copyfiles`, `internal/observe`, `internal/simplify`. (`internal/projfile`, `internal/wsenv` and `internal/runconfig` are ported with the store into `recur-project`; use them from there.)
- **Tests:** all tests of those packages.
- **Frontend modules:** `lib/file-index.ts` (index protocol), `lib/fuzzy.ts` (only if it duplicates `filesearch` ranking), `lib/glob.ts`, `lib/changed-tree.ts`, `lib/finding-paths.ts`, `lib/skills.ts`, `lib/simplify.ts`.
- **Prerequisites:** `recur-domain`, `recur-git` (status/diff helpers).
- **Public API:** `recur_code::{Root, safe_join, read_file, write_file, list_tree, git_status, search, search_opts, replace_in_files, FileIndex, rank_files, outline, find_defs, find_refs, list_skills, list_skill_resources, open_in_editor, reveal, open_in_targets, rg::{ensure, run}, copyfiles::{parse_patterns, copy}, observe::{analyze_root, run_light_checks, project_findings_onto_diff, compute_impact}}`; `recur_project::{runconfig::{boot, run_cmd_output}, wsenv::{from_workspace, parse_dotenv}}` (store port), `recur_engine::simplify::text::{applies_to_chat, prompt}`.

- **As built (deviations from the API above):**
  - Walks are synchronous and take `&CancellationToken` (`search`, `search_opts`, `replace_in_files`, `list_files`, `find_defs`, `find_refs`, `index_symbols`, `list_skills`, `compute_impact`); anything that runs git or a subprocess is `async` (`git_status`, `list_tree`, `list_trees`, `file_diff`, `file_at_base`, `image_at_base`, `rg::{files, ensure}`, `observe::{run_checks, run_light_checks, analyze, analyze_root}`). Callers that need Go's partial-on-cancel behavior get `Error::Canceled`.
  - `rg::{locate, files_args, files, ensure}` replace `rg::{ensure, run}`; agent tools call `recur_store::paths::data_dir` (the store exports it there, not at the crate root).
  - Go's context deadlines: `copyfiles::{preview, copy}` take `(cancel, deadline: Option<Instant>)` (preview defaults to 15 s); `observe::{run_checks, run_light_checks}` take `Option<CancellationToken>`, and `None` is Go's nil context (90 s / 25 s bound). E5 passes a token that fires after 25 s for PR open, as Go's `WithTimeout`.
  - `observe` lives in `recur-code` (its consumers are E5 and services; it only needs `code`), as `recur_code::observe`.
  - The resident index is `IndexHub` (Go `IndexHub`, built on the current tokio runtime) with `FileIndex` / `Snapshot`; `Drop` is `drop_index`. Publishes go through `Publish = Arc<dyn Fn(&str, Snapshot)>`; services maps a snapshot to the `workspace.file_index` event.
  - `reveal` is `reveal_in_finder` and `open_in_targets` is `list_open_in_targets` (Go names); `open_in` opens a workspace folder in a target.
  - Go nil slices: `outline`, `find_defs`, `find_refs`, `extract_idents` return `Vec`, empty where Go returned nil, so services encodes an empty result as `null` to keep the wire shape. `SearchResult.hits` and `CopyFilesPreview.files` keep `Option` like the domain types.
  - The UI's pure file rules are `recur_code::{fuzzy, glob}` (shared with Go's ranking and glob translation) and `recur_code::view::{file_index, changed_tree, finding_paths, skills}`; `localeCompare` / `Intl.Collator` order is ICU root collation (`icu_collator`), and `lib/glob.ts` matches with JavaScript regex semantics (`regress`). `lib/simplify.ts` is `recur_engine::simplify::text::{simplify_enabled, simplify_from_selection}`.
  - Go behavior kept on purpose: the search, symbol and metrics walks skip a walked root whose own name starts with `.`, as `skipDir` did.

#### F: forge + issue trackers (`recur-forge`, `recur-issues`)

- **Go files:** `github.go` (forge half), `github_prs.go` (list parsing, attention, checks reconcile helpers), `github_thread.go` (parsers + mutations), `github_snapshot.go`, `github_reaction.go` (fetch/set), `pr_participants.go`, `pr_reviewers.go`, `app/gh_image.go`, `app/issues.go` (trackers), Linear preset half of `user_mcp.go`, `linearIssueMeta` from `commit_message.go` (as `recur_issues::goal_meta`).
- **Tests:** forge halves of `github_test.go`, `github_thread_test.go`, `github_reaction_test.go`; `github_snapshot_test.go`, `github_snapshot_live_test.go` (env-gated), `github_mentions_test.go`, `pr_participants_test.go`, `pr_reviewers_test.go`, `app/gh_image_test.go`, `app/issues_test.go`, `app/workspace_issue_test.go` (claim), Linear cases of `user_mcp_test.go`.
- **Frontend modules:** `lib/pr-attention.ts`, `lib/pr-checks.ts`, `lib/check-duration.ts`, `lib/pr-participants.ts`, `lib/comment-reactions.ts`, `lib/user-mention.ts`, `lib/comment-permalink.ts`, `lib/issue-link.ts`, `lib/lineartools.ts`.
- **Prerequisites:** `recur-domain` (PR, PRComment, PRParticipant, Check, IssueRef, Project, Settings), `recur-git` (push with lease, remote URLs).
- **Public API:** 2.2, 2.3, `recur_forge::github::GitHub`, `recur_issues::{github::GitHubIssues, linear::Linear}`, `recur_forge::model::*` helpers, `FakeForge`, `FakeTracker`.
- **As built (deviations from 2.2/2.3):**
  - Forge methods take `repo: &Path` (the project checkout, the CLI's cwd) instead of `RepoCtx`; owner/repo resolution stays inside the forge exactly as Go does it (`gh repo view`, the PR URL, `git remote get-url`). `detect` returns `owner/repo` (Go `OwnerRepoFromRemoteURL`, any host).
  - `branch_rules` is `branch_requires_pr(repo, owner_repo, branch) -> Option<bool>`, the uncached probe (Go `fetchGitHubBranchRequiresPR`), plus `remote_repo(repo, remote)`. The 10-minute TTL, in-flight dedupe, background refresh and `github.branch_rules` publish stay in E5 (`githubRemoteRequiresPR`) because the cache is the store's `GitHubBranchRequirePR` table.
  - `create_pr` and `retarget_base` take the base as the forge names it. E5 maps a compare ref (`origin/main`) through recur-git's `ShortBranchName` first (Go `githubPRBase`), so `recur-forge` depends on `recur-domain` only. The forge's own git plumbing (`push`, `remote get-url`, `rev-parse --verify`, `update-ref`) runs through `recur_forge::git` with `gitpkg.Command` semantics (`GIT_OPTIONAL_LOCKS=0`, process group SIGTERM then SIGKILL on cancel, 2 s grace for inherited pipes). `head_ref(n)` gives recur-git the `pull/N/head` refspec and `refs/recur/pr/N`.
  - `merge_comments` (Go `mergeGitHubComments` + `keepFreshGitHubInline`) moved from E5 into the forge: it keys on GitHub's comment-id scheme (R8). E5 keeps the holds (dropped comments, resolved threads, reactions) around it.
  - `PrSnapshot` and `PrListRow` are forge-neutral and pre-resolved: review requests, assignees and list participants/reviewers carry final avatars, commit authors are `CommitAuthor { name, avatar_url }`. Helpers that build participants from comments/commits take an avatar resolver (`|l, p| forge.avatar_url(l, p, 56)`). `snapshot` has no options: it always tries the check rollup and retries without it, as Go does.
  - Trait surface beyond 2.2: `avatar_url(login, provided, size)`, `login`, `reaction_subject`, `mentionable_users(repo, pr_url, query)`, `pr_label` + `pr_subject_prompt` (instead of one `agent_subject_hint`), `merge_failure_note` + `failure_diag` (instead of `failure_note`), and web links for the PR views (`comment_url`, `commit_url`, `blob_url`, `check_run_url`, `checks_url`). `ForgeError { kind: Unavailable | Invalid | Http(u16) | Failed, message }` keeps Go's error text as the message.
  - `fetch_image(url)` returns `Ok(None)` for URLs that are not this forge's and `ForgeErrorKind::Http(status)` with Go's status mapping; the `/gh-image` handler details (method, missing `url`, `Cache-Control`) are services.
  - `IssueTracker::list_open(IssueQuery { settings, projects, deadline })` returns issues already linked to projects (no `link_projects` method and no `github_remotes` in a shared context); `recur_issues::{list_assigned, claim}` are Go `ListAssignedIssues` / `claimIssue`. `agent_mcp(settings, mode)` returns `McpEndpoint { url, headers }`; E2 names the attach after the catalog server id. Added `issue_url` (the UI's issue-link fallback); `app_url` builds the `linear://` link while the macOS `open` stays in services.
  - The UI's pure PR rules live in `recur_forge::view` and `recur_issues::{view, linear::tools}`.

#### E1: engine core

- **Go files:** `bus.go`, `governor.go`, `keepawake.go`, `chat_cancel.go`, `chat_inbox.go`, `diff_sync.go`, `branch_sync.go`, `fswatch*.go`, `workspace_id.go`, `ws_hooks.go`, `roles.go`, `role_tools.go`, `policy.go`; E1 halves of `engine.go`, `loop.go` (control plane), `reporter.go` (loop registry), `workspace_create.go` (persist/allocate/setup/rollback), `workspace_remote.go` (main working copy).
- **Tests:** `fixture_test.go`, `engine_test.go`, `lifecycle_test.go`, `governor_test.go`, `keepawake_test.go`, `chat_cancel_test.go`, `chat_inbox_test.go`, `attention_test.go`, `diff_sync_test.go`, `fswatch_test.go`, `branch_sync_test.go`, `workspace_create_test.go` (allocate cases), `workspace_id_test.go`, `setup_teardown_test.go`, `policy_test.go`, `roles_test.go`, `role_tools_test.go`, `restart_test.go`.
- **Frontend modules:** `lib/inbox-read.ts`, `lib/remove-workspace.ts` (discard risk copy), `lib/wait-for-worktree.ts`.
- **Prerequisites:** domain, store, git, bundles, term, platform; A-M1, A-M2 (tests), T-M1, C (`copyfiles`, `wsenv`, `runconfig`).
- **Deferred call sites:** `link_workspace_to_matching_prs` in branch sync and allocate (E5-M1); `maybe_archive_linked_pr_chat_host` in `lifeLock` unlock and `unregister_chat_turn` (E5-M1); `heal_zombie_chat` in `cancel_chat` (E4); loop half of `Recover` and `recover_interrupted_chats` (E3, E4); `emit_settled_wrap_up` in `loop_exited` (E3).
- **Host methods:** `report_finding`, `done`, `list_findings`, `update_finding`, `report_result`, `escalate`, `attach_evidence`.
- **Public API:** `Engine::new(store, bus, git, bundles, term, registry: ProviderRegistry, forges: ForgeRegistry, trackers: TrackerRegistry, settings)`, `open_main_workspace`, `create_workspace`, `begin_create_workspace`, `retry_workspace_setup`, `remove_workspace`, `delete_workspace`, `workspace_discard_risk`, `delete_project`, `clear_all`, `pause`, `stop`, `shutdown`, `apply_governor_caps`, `watch_workspace`, `watched_workspace`, `schedule_workspace_diff_sync`, `refresh_all_workspace_diffs`, `set_app_focused`, `set_chat_ui_occluded`, `set_workspace_chat_focus`, `mark_workspace_chat_seen`, `mark_all_chats_seen`, `sync_chat_inbox`, `cancel_chat`, `fetch_working_copy_upstream` (main), `update_working_copy_from_remote`, `push_working_copy`; internal: `LoopHandle`, `chatlife::{lock, register_turn, park_if, try_inject, ...}`, `Governor`, `publish_*`.
- **As built (E1-M1 API, deviations, notes for E2–E5 and SV):**
  - `Engine::new(store: Arc<Store>, bus: Arc<Bus>, git: GitSetup, bundles: DiscoverRoots, term: Option<Arc<dyn TerminalSource>>, registry: ProviderRegistry, forges: ForgeRegistry, trackers: TrackerRegistry, data_dir: PathBuf) -> Arc<Engine>`. The last parameter is the data dir, not settings: Settings stay in the store (Go read `st.Settings()` in `New`), and the engine reads no process env (no `store.DataDir()`, `BundlesDir()`, `WorktreesDir()`), so engine tests run in parallel. `GitSetup { worktree_root, warning_title, warning }` (`GitSetup::probe(root)` runs Go's `Probe` + `UnsupportedAlert`) replaces `SetWorktreeRoot` and the probe in `New`. `term` replaces `SetTerminals`.
  - Registries live in `recur_engine::registry`: `ProviderRegistry::{new, pinned, pin, get, for_model, all, lifecycle, replace}` (a pin is Go `SetRunner`: every turn goes to it, `pinned(None)` fails turns closed), `ForgeRegistry::{get, for_pr_source, detect, all}`, `TrackerRegistry::{get, all, secret_env}`. Provider lifecycle calls (`release_worktree` on teardown, `on_idle` at zero agent slots, `shutdown`) go to every registered provider; no id compares (R7).
  - R16: `recur_engine::terminals::TerminalSource { list, active_id, read_plain(id, tail_lines: i64) }`, implemented for `recur_term::Manager`.
  - Bus: `EngineEvent` (one variant per event of §3; `WorkspaceUpdated` and `WorkspaceHoisted` both name `workspace.updated`), `name()`, `payload_json()` (Go `json.Marshal` bytes, persisted form). Payload structs declare fields in Go's sorted map-key order and serialize straight to the writer. `Bus::subscribe()` (unbounded channel) and `subscribe_fn` (sync callback; must not block or publish). `SessionRoute::{user, delta, thinking, step, tool_result, compact, fallback, context_usage, turn_end, notice}` build the `session.*` payloads with Go's base-field rules; `PrUpdated::from_pr` and `AttentionUpdated::new` the others. `workspace.watched` is still published under the watch lock (Go order), so callback subscribers must not call `watched_workspace` / `watch_workspace`.
  - Locks: every §4.2 lock is a named `Engine` field (table in `core.rs`). `dispatch_mu`, `schedule`, `open_main_mu`, `id_mu`, `mcp_started` and the per-tab life locks are tokio mutexes; the rest are `parking_lot` and never cross an await (`clippy::await_holding_lock` is denied). Added order: `loops` map → a loop handle's state.
  - `chatlife`: `chat_tab_slot(ws, code).begin() -> (seq, stop_gen)`, `claim(seq).await` (waits for its turn without holding anything), `lock()` (Go's held slot mutex for the short sync sections), `finish(guard)`, `stop_gen()`. Go `pause`/`resume` are dropping and retaking `lock()`. `lock_chat_life(ws, code).await -> (ChatLifeGuard, held)`; `ChatLifeGuard::unlock` is idempotent and Drop unlocks. `register_chat_turn(ws, code, CancellationToken)`, `register_chat_turn_if_idle`, `unregister_chat_turn`, `live_chat_turn`, `chat_turn_ended -> Option<CancellationToken>` (the `ended` signal), `park_chat_turn_if(ws, code, Option<&Arc<ChatTurn>>)`, `chat_turn_was_parked`, `chat_turn_was_timed_out`, `try_inject_chat(ws, code, prompt, display)`, `cancel_chat(ws, code).await`, continue gens (`alloc_continue_gen`, `continue_gen_live`, `drop_continue_gen`, `invalidate_continue_gens`), tombstones (`reserve_deleting`, `release_deleting`, `is_deleting`, `chat_life_reserved`, `workspace_remove_blocks_chat`, `workspace_teardown_committed`, `wait_chat_life_idle`, `drop_chat_life_for_workspace`). `ChatTurn::arm_chat_inject() -> Option<SteerInbox>` replaces Go's unbuffered `inject` + shared `injectAck`: a one-slot `mpsc` of `recur_agent::Steer`, each acked on its own oneshot (true only after the provider delivered it). A second arm returns `None` (the receiver cannot be shared). `arm(d)`/`disarm()` keep the wedged-turn deadline.
  - `LoopHandle::new(id, project_id, map_key, LoopState)`: Go `Loop.mu` + `cond` become `lock()` / `broadcast()` / `wait_while(cancel, blocked)` over `LoopState` (paused, stopped, park_idle, escalated, round_cap_esc, answer, pending_esc, pending_steer, granted_extra, active_inject, done_turn_code, done summaries/notes, pending_on_demand, exit_state/phase, chat_code, branch, worktree, `RoleView`). Methods: `send(Command)`, `drain_commands`, `wait_resumed(cancel)`, `request_idle`, `wants_idle`, `is_stopped`, `request/take/has_pending_escalation`, `grant_extension`, `take_granted_extension`, `queue_steer`, `bind_inject(code) -> (SteerInbox, InjectBinding)`, `holds_done_turn`, `slot_role_spec`, `accepts_done_summary`, done summary/notes, `begin_review_iteration`, `collect_finding`, `findings_this_iteration`, evidence. E3's walk keeps the rest of Go's `Loop` and must store the current phase's roles in `LoopState::roles` (Go's `acceptsDoneSummary` read the walk's phase unsynchronized). Go's `checkpoint` stays E3: drain, take the pending escalation, publish paused, then `wait_resumed`.
  - Registry: `register_loop(RunningLoop { handle, cancel }) -> bool` (Go `LoadOrStore`), `remove_loop`, `loop_map_key`, `loop_key_running`, `running_loops_on`, `running_loop_by_id`, `running_loop_for_done`, `running_loop_by_chat`, `chat_drives_live_loop`, `running_loop_for_hand`, `running_loop_record_by_chat`, `stop_loop_by_chat`, `idle_walk_by_chat`, `loop_exited(handle)` (E1, with E3 hooks), `begin_verify`. Walk tasks go in `engine.tasks.loops`.
  - Governor: `acquire_agent(cancel) -> bool` + `release_agent()`; `Engine::acquire_agent(cancel) -> Option<AgentPermit>` (Drop releases once, syncs keep-awake, calls providers' `on_idle` at zero). A cancelled waiter that consumed a wake passes it on.
  - `fetch_working_copy_upstream(id)` returns `()`: the land state Go returned is E5's `workspace_land_state`, which E5's public wrapper reads after this call.
  - Recover's loop half needs only E1 primitives and is ported in E1 (`lifecycle::recover`, phase constants `PHASE_INTERRUPTED`, `PHASE_SCHEDULED_INTERRUPTED`, `PHASE_ANSWERED_WAITING`); E3 keeps resume/AutoResume.
  - `CheckCursorBridge` is not in the engine (R7): it needs `recur_provider_cursor`'s locate + SDK version, so SV publishes that boot notify.
  - Small pure helpers from later-slice files that E1 calls are ported in E1 and are the shared versions: chat codes (`codes`), `AgentConfig` (`session_config`), `chat_tab_title` / `first_user_chat_title` / `CHAT_RETRY_DISPLAY_SENTINEL`, `flag_chat_unseen` (from `switch_mode.go`), `scheduled_pause_settled` (`schedules.go`), `run_progress` (`plan.go`), `prune_checkpoints` (`checkpoint.go`), `accumulate_handoff` (`handoff.go`), `plural` (`rails.go`), `copy_worktree_dirty` (`chat_fork.go`), `normalize_pr_branch`, `chat_selection_mode` (`WorkspaceChatSelection`'s mode, for finding attribution), `stream_seq_seed` / `remember_stream_seq` (`mirror.go`), and `publish_pr_updated` / `emit_pr_updated` (`pr.go`; E5-M1 no longer provides them).
  - Test seams (R15): `set_clock`, `set_promote_hook` + `run_promote_hook`, `set_start_walk_hook` + `run_start_walk_hook`, `set_child_queue_hook` + `run_child_queue_hook`, `set_kitchen_words`, `set_fs_timing` (setters behind `cfg(test)` / feature `testing`). `recur_engine::testing::{TestEngine, TestOptions, BusRecorder, TempRepo, init_repo, git, git_out, add_project, add_run_project, wait_for, TEST_LOOPS_TOML}`.
  - **Deferred call sites** (`deferred.rs`; each hook is the Go call, its body the no-subsystem answer): E2 `ask_user_pending_for` (`askUsers.pendingFor`), `clear_ask_users_for_workspace`; E3 `drain_queued` (in `apply_governor_caps` and `loop_exited`), `emit_settled_wrap_up` (`loop_exited`), `retire_open_escalations` (`persist_stop`), `maybe_dispatch_after_allocate` (create / allocate / retry setup); E4 `pending_mode_continue_for`, `cancel_mode_continue` (CancelChat's take + `drop_continue_gen` + uncount), `clear_mode_continues_for_workspace`, `heal_zombie_chat`, `chat_resuming`, `recover_interrupted_chats`, `mark_app_chat_seen`, `close_chat_child_on_remove`, `forget_child_wave` (`childWaves.Delete` in unregister), `touch_workspace_activity` (`set_workspace_state`); E5 `maybe_archive_linked_pr_chat_host` (life-lock unlock, unregister, `drop_continue_gen`), `link_workspace_to_matching_prs` (branch sync, allocate), `archive_pr_chat_hosts` (allocate from a PR head), `fetch_open_pr_remote` (`fetch_working_copy_upstream`).
  - Tests moved to later slices because they drive E2–E5 code: E3 takes the loop-driven halves of `engine_test.go` (end-to-end, governor queueing, caps drain, pause/resume/stop), `lifecycle_test.go` (runs to PR open), `restart_test.go` (Restart), the dispatch cases of `attention_test.go` and `workspace_id_test.go`, `TestStartRunOn*`/`TestBeginStartWorkspace*`, and `fixture_test.go`'s converge/docs/verify loops (the pack is `testing::TEST_LOOPS_TOML`); E4 the `SendWorkspaceMessage` cases of `chat_cancel_test.go`, `CloseWorkspaceChat` cases of `chat_inbox_test.go` / `attention_test.go`; E2 the ask_user park cases; E5 `TestOpenWorkspacePRAfterRename`.


### 5.4 Wave 2

#### P1: Claude + Codex providers

- **Go files:** `claude.go`, `claude_flags.go`, Claude halves of `permission.go` and `attachments.go`, `codex.go`, `internal/codex/*`, `app/claude_status.go`, `app/codex_service.go`, Claude/Codex branches of `app/model_catalog.go`.
- **Tests:** `claude_flags_test.go`, `claude_inject_test.go`, `codex_test.go`, `codex_smoke_test.go` (env-gated), Claude cases of `permission_test.go` / `permission_wiring_test.go`, `internal/codex/*_test.go`, `app/claude_status_test.go`, Claude/Codex cases of `app/model_catalog_test.go` and `models_test.go`.
- **Prerequisites:** A-M1, A-M2, P3's `recur-usage` probes for `usage_probe` (deferred edge).
- **Public API:** `ClaudeProvider::new(cfg)`, `CodexProvider::new(cfg)` implementing 2.1.
- **As built (deviations and notes for E2/SV):** no trait change.
  - `CodexProvider` keeps a settings snapshot (`CodexConfig::settings`, replaced by `settings_changed`) because `run_turn` takes no `Settings`; SV must call `settings_changed` on every save. `ClaudeProvider` reads no settings.
  - Tracker secrets: Go hard-coded `LINEAR_API_KEY` in both runners. Turns now scrub `TurnRequest::env_scrub` (the engine fills it from `IssueTracker::secret_env`, R8) and Codex probes scrub `CodexConfig::env_scrub`.
  - Codex leaves `PermissionRequest::surface` empty; the engine's `PermissionAsker` fills it (Go `permissionSurfaceFor` in the `codexRunner` closure).
  - Codex sends conductor steers without the Maestro prefix (Go behavior) and acks every steer after `turn/steer` succeeds; Claude prefixes conductor steers and acks them on receipt.
  - `CodexProvider::models` is `Ok(vec![])` when Codex is off or unconfigured and `Err` when the probe fails; either way the group is omitted. `cached_models` is the last good list from `models` or `status`. `context_window` is 0 for Codex (the window arrives on usage frames).
  - E2's `contextWindowForMessage` port keeps Go's guard (resolve only when a model or a `context` flag is known; else the stored window); `recur_provider_claude::flags::has_context_flag` is that check. The mirror's result stamp becomes `provider.context_window(model, flags)` when > 0.
  - `canonicalTool` (Cursor/Codex names) lives in `recur_provider_codex::display`; P2 needs the same table.
  - Robustness: Codex RPC calls fail fast after the reader hit EOF (Go waited on the context), and the Claude runner drains stdout after an over-long line instead of leaving the CLI blocked until cancel.

#### P2: Cursor provider

- **Go files:** `cursor.go`, `cursor_flags.go`, `cursortools.go`, Cursor pool half of `provider.go`, `wrapAttachedImagesForCursor`, `internal/cursor/*`, `app/cursor_service.go`, Cursor branch of `app/model_catalog.go`.
- **Tests:** `cursor_test.go`, `cursor_flags_test.go`, `cursor_steer_test.go`, `cursor_active_run_test.go`, `cursor_smoke_test.go` (env-gated), `cursortools_test.go`, pool cases of `provider_test.go`, `internal/cursor/*_test.go`, `app/cursor_service_test.go`.
- **Prerequisites:** A-M1. Node on `PATH` for smoke tests.
- **Public API:** `CursorProvider::new(cfg)` implementing 2.1 with `release_worktree`, `on_idle`, `settings_changed`, `install(progress)` for `cursor.install`.
- **Note:** keep the attribution pin coupling (`pinnedSDKVersion` + `sdkAttributionAnchors`) and port `check:cursor-sdk-pin` semantics.
- **As built (deviations and notes for E1/E2/SV):**
  - Trait change: `release_worktree` returns `Result<(), ProviderError>`. Go's teardown emitted "Could not remove Cursor project dirs: …" as a Maestro workspace event (or logged it when the workspace was gone); with `()` the engine could not surface it. The error message is the whole sentence; E1 HTML-escapes it into the event as Go did.
  - `CursorProvider::new(CursorConfig { data_dir, settings, home })`; like Codex it keeps a settings snapshot, so SV must call `settings_changed` on every save. `settings_changed` rewrites `cursor-attribution.json` (Go did this in SetSettings), closes the pool when Cursor is turned off and cancels a deferred close when it is on. The deferral checks the provider's own in-flight runners rather than the governor's agent count; `on_idle` (agent slots at zero) finishes it, as Go's `closeIfIdleRequested` did.
  - `install(cancel, progress)` returns the launcher; SV publishes `InstallProgress::event_payload()` / `install_error_payload` as `cursor.install`, saves `Settings.cursor_bridge_path` and then calls `status`. `cursor_status` / `test_cursor_key` return the Go `CursorStatus` JSON shape; `status` / `test_credentials` map it onto `ProviderStatus`. SV keeps `dropOrphanCursorKey` (it writes the store) before calling `status`.
  - The self-heal of Go `runAgent` (bridge unreachable on CreateAgent: invalidate and retry once) and the "Cursor is turned off in Settings → Providers: …" wrap live in `run_turn`; the router must not retry Cursor itself.
  - Cursor does not run events through `ContextUsageTracker`, as in Go: its result already meters the latest footprint with the current tool-wave and compaction state, and the tracker's last-usage overlay would be stale. `context_window` is the table + flags (the mirror's Cursor branch).
  - createPlan steps are followed by `AgentEvent::PlanProposal { body }` (the planner and plan-mode harvest, R7). `oneshot_flags` is empty (Go skipped the default effort for Cursor); `can_disable_tools = false` keeps the Read/Glob/Grep one-shot allowlist. `resume_handle_fits` is Go's `agent-` prefix rule.
  - Allowlists (R9): only "no Edit/Write/NotebookEdit → plan mode" and `mcp__recur__create_workspace` → subagent overlay are read; Cursor has no per-run allowlist.
  - `canonicalTool` is duplicated from `recur_provider_codex::display` (Go shared one function; providers cannot depend on each other).
  - Pin check: `RECUR_CURSOR_SDK_PIN_CHECK=1 cargo test -p recur-provider-cursor check_cursor_sdk_pin_drift` (Taskfile/CI entry for `check:cursor-sdk-pin`); `RECUR_CURSOR_ATTRIBUTION_PATCH_TEST=1 … attribution_patch_against_pinned_wheel` validates the anchors on the pinned wheel; `golden_tests::pin_and_anchors_match_go` fails if the Go pin or anchors move without the Rust ones.

#### P3: compatible provider, usage, LSP

- **Go files:** `harness.go`, `harness_history.go`, `harness_prompt.go`, `internal/responses/*`, `app/compatible_service.go`, compatible branch of `app/model_catalog.go`, `internal/usage/*`, `app/provider_usage.go`, `internal/lsp/**`, `app/lsp_service.go`.
- **Tests:** `harness_test.go`, `internal/responses/*_test.go`, `app/compatible_service_test.go`, `internal/usage/*_test.go`, `app/provider_usage_test.go`, `internal/lsp/**/*_test.go`.
- **Prerequisites:** A-M1, T (coding tools, host tool registry, `TodoStore`), `recur-bundles` (language server overrides).
- **Public API:** `CompatibleProvider::new(domain::CompatibleProvider)`; `recur_usage::{Poller, ClaudeProbe, CodexProbe, CursorProbe, connect_cursor}`; `recur_lsp::{Manager, Session, servers, managed}`.
- **As built (deviations and notes for E2/SV/editor):** no trait change.
  - `CompatibleProvider` runs turns through `TurnContext::{tools, history}`. The in-process toolbox is `recur_provider_compatible::HarnessTools` (Go `responsesTools`/`execOne`/`harnessPermitter`: allowlist, `permission_request` hidden, Bash gated through `Host::request_permission`, `harnessAgentCode`), a `ToolHost` built per turn from `(Option<Arc<dyn Host>>, Arc<TodoStore>, &TurnRequest)`. `recur-engine` cannot depend on a provider crate, so when E2 wires turns, move `tools.rs` into `recur-agent-tools` (it depends only on `recur-agent` and `recur-agent-tools`) and build it in the router for `ToolTransport::InProcess` providers. Without `cx.tools` the harness advertises no tools.
  - The transcript comes from `HistorySource::{transcript, history_after}` (Go read the store). Stored chat images reload under `recur_store::paths::data_dir()` (override with `with_data_dir`).
  - Steers are acked `true` once appended to the next round's input. Go never acked harness injects, so `tryInject` read false at turn end and the caller could resend a message the turn already took.
  - The picker label is fixed at construction; `settings_changed` refreshes the run config by slug (Go rebuilt the runner from settings every turn). SV labels groups with `Settings::display_label` and re-registers providers whose name changes. `availability` keeps Go's three messages.
  - `recur-usage`: `Fetcher` keeps Go's hook fields (`claude_creds`, `nudge_claude`, `codex_auth`, `cursor_token`, URLs, `want`, `cursor_stub`, `on_minted_api_key`); contexts are `recur_usage::Ctx` (token + deadline). `Poller::new(Arc<Fetcher>, Option<Publisher>)` publishes `("provider.usage", Snapshot)`. `UsageService` is `app/provider_usage.go` with settings read/save and the data dir injected; SV calls `start()` from the app entry only. There is no `connect_cursor` free function: Connect is `Poller::connect_cursor` / `UsageService::connect_cursor_usage`. `ClaudeProbe`/`CodexProbe`/`CursorProbe` wrap a shared `Arc<Fetcher>` (25 s each) for `Provider::usage_probe`; the poller still collects through `Fetcher::collect` so the Claude budget, connecting row and Cursor stub keep Go's behavior.
  - `recur-lsp`: Go's package-level server registry is `ServerRegistry`, owned by `Manager` (`manager.registry()`). The emitter is typed (`LspEvent::{Message, Exit}` with `name()` / `payload()`). `recur_lsp::service` is `app/lsp_service.go` minus root resolution: SV resolves the root (worktree, else repo: `code.ResolveRootInfo`) and the bundle roots, then calls `refresh_registry`, `ensure_lsp`, `list_language_servers`, `write_lsp`, and the manager's close methods. `managed::ensure(data_dir, id)` takes the data dir as a string, empty = PATH only.
  - Editor: the transport rules of `lib/lsp.ts` (server-request replies, `initialize` rewrites, the Vue `tsserver/request` bridge) are `recur_lsp::protocol`; the editor's LSP client applies `rewrite_outgoing` before `Manager::write` and `handle_server_request` to each `lsp.message` before its own handling. Documents, diagnostics and UI stay in `recur-editor`.

#### E2: turn pipeline and sessions

- **Go files:** `mirror.go`, `chat_errors.go`, `checkpoint.go`, `composer.go`, `raw_regions.go`, `file_mentions.go`, `skill_mentions.go`, `terminal_mentions.go`, `artifact_mentions.go`, `artifacts.go`, `attachments.go` (disk I/O), `ask_user.go` (broker), `permission.go` (broker), `plan.go`, `commit_message.go`, `simplify.go`, `user_mcp.go` (generic), `mcp_tool_backfill.go`, `rename_session.go`, `reporter.go` (`engineReporter` base + E2 methods, `permissionSurfaceFor`), router half of `provider.go`, `workspace_chat.go` transcript + session-config half.
- **Tests:** `mirror_test.go`, `thinking_broadcast_test.go`, mirror cases of `context_usage_test.go`, `chat_errors_test.go`, `checkpoint_test.go`, `composer_test.go`, `raw_regions_test.go`, `file_mentions_test.go`, `skill_mentions_test.go`, `skills_settings_test.go`, `terminal_mentions_test.go`, `artifact_mentions_test.go`, `artifacts_test.go`, disk cases of `attachments_test.go`, broker cases of `ask_user_test.go` and `permission_test.go`, `permission_e2e_test.go`, `mcp_required_test.go`, `mcp_tool_backfill_test.go`, `commit_message_test.go`, `plan_test.go`, `simplify_test.go`, generic cases of `user_mcp_test.go`, `workspace_chat_items_test.go`, `workspace_chat_fresh_session_test.go`, router cases of `provider_test.go`, `feedkind_test.go`, `harden_test.go`.
- **Frontend modules:** transcript and sentinel rules from `lib/agentchat.ts`, `lib/session-live.ts`, mention regexes from `lib/composerDom.ts` (`FILE_AT_RE`, terminal and artifact chips), `lib/artifacts.ts`, `lib/promptActions.ts` (slash prompt matching).
- **Prerequisites:** E1-M1, A-M1/A-M2, T-M1, C (skills, file index for mentions), F-M1 (`IssueTracker::agent_mcp`), bundles (prompt actions), term.
- **Deferred call sites:** E5's `open_pr` host method; `create_workspace` / children / `switch_mode` host methods (E4); loop-mode routing in the router is not here (E4 owns `send_workspace_turn`).
- **Host methods:** `note_tool_args`, `ask_user`, artifacts (4), renames (3), `request_permission`, `submit_plan`, `update_plan_step`, skills (2), terminals (2).
- **Public API:** `run_agent(spec) -> TurnResult`, `ensure_agent_session`, `session_chat_items`, `workspace_chat_items`, `workspace_chat_turn_diff`, `reset_workspace_chat`, `review_checkpoint`, `expand_composer`, `generate_commit_message`, `generate_pr_description`, artifacts CRUD, `answer_permission`, `pending_permissions`, `answer_ask_user` (without resume tail), `submit_human_plan`, `update_human_plan_step`, `approve_plan`, `remember_workspace_chat_simplify`, `chat_simplify`.

### 5.5 Wave 3

#### E3: loops

- **Go files:** `walker.go`, `loop_prompts.go`, `escalation.go`, `fanout.go`, `board.go`, `handoff.go`, `verify.go`, `resume.go`, `rails.go`, `maestro.go`, `maestro_hands.go`; E3 halves of `engine.go` (start/dispatch/resume/drain/loopExited/loop Recover), `loop.go` (walk data plane), `workspace_create.go` (`BeginStartWorkspace*`, `queueStartWorkspace`, `abortQueuedStart`, `maybeDispatchAfterAllocate`), `workspace_chat.go` (loop engage), `pr.go` (`Loop.openPR`, wrap-up, PR-created feed and inbox), `agenterr.go` (`turnError`).
- **Tests:** `walker_test.go`, `default_walk_test.go`, `converge_test.go`, `findings_checklist_test.go`, `escalation_test.go`, `fanout_test.go`, `handoff_test.go`, `planbrief_test.go`, `loop_prompts_test.go`, `maestro_test.go`, `maestro_hands_test.go`, `loop_engage_test.go`, `resume_test.go`, `verify_test.go`, `rails_test.go`, `m8_test.go`, `acceptance_test.go`, `real_e2e_test.go` (env-gated).
- **Frontend modules:** `lib/loops.ts`, `lib/loop-cockpit.ts` (loop state projection).
- **Prerequisites:** E1-M1, E2-M1, C (`runconfig`, `wsenv`, `observe` for open PR), E5-M1 (open PR), E4-M1 (resume interrupted chats).
- **Host methods:** `plan`, `edit_plan`, `rerun_review`, `dispatch_specialist`, `extend_iterations`, `extend_rounds`, `reenter_phase`, `run_phase`, `resume_loop`.
- **Public API:** E3-M1 plus `start_workspace`, `start_workspace_with`, `begin_start_workspace*`, `answer_escalation`, `answer_escalation_for`, `escalation_stats`, `restart`, `retry`, `auto_resume`, `recover` (loop half).

#### E4: chat and coordination

- **Go files:** `chat_child.go`, `chat_fork.go`, `chat_recover.go`, `chat_surface.go`, `switch_mode.go`, `modes.go`, `create_workspace_tool.go`, `schedules.go`, `app_chat.go`, `briefing.go`, `feedproject.go`; E4 halves of `workspace_chat.go` (send pipeline, tabs) and `ask_user.go` (resume).
- **Tests:** `workspace_chat_test.go` (split by group), `chat_child_test.go`, `chat_fork_test.go`, `chat_recover_test.go`, `chat_surface_test.go`, `switch_mode_test.go`, `modes_test.go`, `schedules_test.go`, `app_chat_test.go`, `briefing_test.go`, `feedproject_test.go`, resume cases of `ask_user_test.go`.
- **Frontend modules:** `lib/modes.ts` (mode list, privilege rule), `lib/chat-surface.ts` (marker encoding), `lib/chat-tab-session.ts`, `lib/subagent.ts`, `lib/new-workspace.ts` (launch validation).
- **Prerequisites:** E1-M1, E2-M1, E3-M1 (loop/maestro routing), F-M1 (`agent_subject_hint`).
- **Host methods:** `switch_mode`, `create_workspace`, children (5), `list_workspace_briefs`, `workspace_context`, `present_workspace`.
- **Public API:** E4-M1 plus `list_workspace_chats`, `reorder_workspace_chats`, `close_workspace_chat`, `list_closed_workspace_chats`, `reopen_workspace_chat`, `delete_closed_workspace_chat`, `fork_workspace_chat`, `fork_workspace_chat_to_workspace`, `switch_chat_mode`, `remap_chats_off_disabled_providers`, schedules CRUD + `start_scheduled_prompts` + `prune_scheduled_workspaces`, app chat API (12 methods), `briefing_snapshot`, `workspace_context`, `mark_workspace_viewed`, `reconstruct_feed`, `recover_interrupted_chats`, `launch_interrupted_chats*`.

#### E5: pull requests

- **Go files:** `pr.go` (minus E3 parts), `rework.go`, `workspace_base.go`, `workspace_pr.go`, engine halves of `github.go` (`mergeGitHub`, check refresh glue), `github_prs.go` (sync orchestration, apply rows, reconcile, linking, `CreateWorkspaceFromPR`, `EnsurePRChatHost`, diff hydration, PR chat host hoisting), `github_thread.go` (merge comments, holds, conversation refresh), `github_reaction.go` (hold), `workspace_remote.go` (PR head fetch).
- **Tests:** `github_prs_test.go` (split by group), engine halves of `github_test.go`, `github_thread_test.go`, `github_reaction_test.go`, `pr_test.go`, `merge_message_test.go`, `open_pr_result_test.go`, `rework_test.go`, `workspace_base_test.go`, `workspace_pr_test.go`, `projfile_test.go` (engine overlay case).
- **Frontend modules:** `lib/hoist-chat-keys.ts` (apply `codeRemap`), `lib/pr-workspace.ts`, `lib/pr-merge-action.ts`, `lib/pr-thread-resolved.ts`, `lib/comment-thread.ts`, `lib/pr-detail-pending.ts`, `lib/pr-fix-checks.ts`.
- **Prerequisites:** E1-M1, E2-M1, F (whole slice, through `FakeForge` in tests), E4-M1 (PR chat host tabs), C (`observe`).
- **Host methods:** `open_pr`, `send_back`.
- **Public API:** E5-M1 plus `sync_prs`, `run_pr_sweep`, `refresh_pr_conversation`, `schedule_refresh_pr_conversation`, `refresh_pr_diff`, `add_pr_comment`, `submit_review`, `update_pr_details`, `update_pr_comment`, `delete_pr_comment`, `reply_pr_comment`, `toggle_pr_comment_reaction`, `set_pr_thread_resolved`, `merge_pr`, `set_pr_draft`, `refresh_pr_checks`, `refresh_pr_commits`, `ensure_commit_diff`, `refresh_pr_behind`, `merge_workspace`, `workspace_land_state`, `update_workspace_with_base`, `pull_workspace_pr`, `open_workspace_pr*`, `set_workspace_base_branch`, `send_back_to_loop`, `update_pr_with_base`, `create_workspace_from_pr`, `ensure_pr_chat_host`, `branch_requires_pr`.

### 5.6 Wave 4

#### SV: services (`recur-services`)

- **Go files:** `app/services.go` (12 services), `briefing.go`, `bundle_service.go`, `code_service.go`, `editor_session.go`, `fileserver.go`, `loose_files.go`, `observatory_service.go`, `open_path.go`, `schedules.go`, `shell.go`, `terminal_persist.go`, `terminal_service.go`, `workspace_open.go`, merge half of `model_catalog.go`, service half of `issues.go`; startup wiring from `main.go` (`Recover` before UI, bus subscription, `CheckGit`, `StartScheduledPrompts`, `AutoResume` and PR sweep tasks, shutdown hook).
- **Tests:** every remaining `app/*_test.go`.
- **Frontend modules:** `lib/api.ts` is the contract: every method it calls must exist on `recur_services` with the same semantics (PARITY "Application services"). Also `lib/models.ts`, `lib/providers.ts`, `lib/compatible-providers.ts`, `lib/providerUsage.ts`, `lib/shell-state.ts`, `lib/open-path.ts`, `lib/run-dev.ts` (if it encodes run-config rules).
- **Prerequisites:** all engine slices, all providers, F, P3, bundles, term.
- **Public API:** `Services::new(config) -> Services` (registers `ClaudeProvider`, `CodexProvider`, `CursorProvider`, one `CompatibleProvider` per configured endpoint, `GitHub`, `GitHubIssues`, `Linear`), `Services::start()`, `Services::shutdown()`, `subscribe() -> mpsc::UnboundedReceiver<AppEvent>`, and one async method per PARITY service method.

## 6. Risks and hidden coupling

Ordered by how badly they break things if missed.

| # | Risk | Where | Mitigation |
|---|---|---|---|
| R1 | Registry ids in `ARCHITECTURE.md` (`claude`, `compatible:<id>`) do not match persisted data (`claude-code`, compatible id as is; PR source `gh`). | `domain/session.go`, `domain/model.go`, `domain/types.go` | Use the persisted ids (section 2). Fix `ARCHITECTURE.md` in the same PR that adopts this plan. |
| R2 | Byte-identical text is a contract: role and mode prompts, tool names/descriptions/schemas, tool result strings, `recurCodingPrompt`, chat sentinels (U+2060 and U+2064 based), markers (`⟦attachment:…⟧`, `⟦raw…⟧`, `⟦recur-surface⟧`), regexes shared with the frontend (`FILE_AT_RE`, terminal and artifact chips), kitchen id word lists. Agents, stored transcripts and the UI all match on them. | `roles.go`, `modes.go`, `chat_errors.go`, `harness_prompt.go`, `agenttools/*`, `file_mentions.go`, `workspace_id.go` | Each owning slice adds a golden test that compares its constants to a JSON dump generated from the Go package (one throwaway Go test per slice, output committed under `rust/crates/<crate>/tests/golden/`). |
| R3 | Engine tests replay Claude NDJSON (`ScriptedRunner` in 44 engine test files, 33 uses in `app` tests), so every engine slice depends on the Claude parser. | `scripted.go`, `streamjson.go` | Parser and `NdjsonScript` are milestone A-M2 in wave 1, behind a `testing` feature of `recur-provider-claude`; engine and services take it as a dev-dependency only. |
| R4 | Store semantics the engine relies on: `Workspaces()` returns insertion order (`drainQueued` promotes the oldest), `UpdateWorkspace(id, closure)` runs the closure under the store-wide lock, writes are in-memory first with best-effort SQLite write-through, reads return clones. | `store/store.go` | Store port must keep all four. Engine closures must stay pure (no engine calls, no publish). |
| R5 | Locks held across blocking work in Go become `.await` points: `chatTabSlot.claim` returns holding a mutex; `dispatchMu` is held across worktree creation and setup hooks. | `chat_cancel.go`, `engine.go` | Ticket gate redesign (4.2); `tokio::sync::Mutex` for `dispatchMu`/`scheduleMu`; `clippy::await_holding_lock` deny. |
| R6 | Steering deadlocks: Claude acks a steer only after the next tool result; holding the chat life lock while steering blocks `switch_mode`, which needs it. Cursor joins steers into one follow-up Send and suppresses the cancelled result. | `workspace_chat.go`, `claude.go`, `cursor.go` | Steer with the life lock released (Go does). Port `claude_inject_test.go` and `cursor_steer_test.go` before anything else in P1/P2. |
| R7 | Provider name checks hidden in the engine: `toolview.go` switch, `resumeFitsProvider`, `cursor.Error.Transient()`, Cursor `createPlan` harvest in planner and plan-mode chat, Cursor "can't disable tools" in commit messages, `needsPermissionPromptTool`, Cursor self-heal retry, `ResolveClaudeContextWindow`/`ResolveCursorContextWindow`/`claudeModelLabel` in mirror and transcript flattening, compatible provider skipping fork preambles, Cursor `KindUser !LiveSteer` reset in the mirror. | listed files | All become `Provider` methods or caps (2.1). Grep for provider ids in `recur-engine` in CI. |
| R8 | Forge and tracker leaks: GitHub wording in chat surface prompts and open-PR results, `gh pr edit --base` in base retarget, PR head refspec in land state, `LINEAR_API_KEY` scrub in the Claude runner, Linear MCP preset in user MCP, `Identifier:`/`Title:` goal header. | `chat_surface.go`, `rename_session.go`, `workspace_base.go`, `workspace_remote.go`, `claude.go`, `user_mcp.go`, `commit_message.go` | `Forge::agent_subject_hint`, `failure_note`, `retarget_base`, `head_ref`; `IssueTracker::secret_env`, `agent_mcp`, `agent_hint`; `recur_issues::goal_meta`. |
| R9 | Allowlist syntax is Claude's (`Bash(git diff:*)` style `gitReadTools`, `mcp__<server>__*` globs) and is passed to every provider. | `roles.go`, `role_tools.go` | Keep the allowlist format as the engine's canonical form; each provider translates (Cursor/Codex ignore or map rule syntax as Go does). Document per provider in its crate. |
| R10 | Persisted event payloads are a storage format (`workspace.event`, `workspace.updated`, `escalation.raised` in the `events` table, read back by feed reconstruction). | `engine.go`, `feedproject.go` | Serialization tests against Go-produced JSON fixtures. |
| R11 | MCP URL and token behavior: stable token per agent reused across turns, extra tokens for overlapping registrations, 404 when inactive. Claude/Codex/Cursor configs embed the URL per turn. | `mcp/server.go` | Port `server_test.go` in full before wiring any provider. |
| R12 | Process control: process-group kill SIGTERM → 2 s → SIGKILL, single reaper, Cursor orphan bridge reaping via `ps`, Codex per-turn app-server, close stdin on Claude `result` (otherwise the CLI hangs). | `claude.go`, `cursor/bridge_reap.go`, `codex/server.go` | Linux and macOS CI for P1/P2; keep the "close stdin on result" comment as a test. |
| R13 | Cursor SDK pin: attribution patch anchors are tied to `pinnedSDKVersion`. | `cursor/attribution.go`, `install.go` | Port the weekly pin check; never bump one without the other. |
| R14 | Restart recovery depends on exact phase strings (`"interrupted"`, `"scheduled run interrupted"`, `"answered \u{2014} waiting to resume"`, `"idle"`, board cap phase) and on recovery order. | `engine.go`, `resume.go`, `chat_recover.go` | Constants module + `restart_test.go`, `resume_test.go`, `chat_recover_test.go` ported early in their slices. |
| R15 | Test seams are package-level function variables (19 GitHub `...Fn` vars, `SetRunner`, `promoteHook`, `startWalkHook`, `childQueueHook`, `fanOutPoll`, `nowFn`). | engine | Replace with injected traits (`FakeForge`, `ScriptedProvider`), hook fields on a `TestHooks` struct behind `cfg(test)`/`testing` feature, and an injectable clock. |
| R16 | `internal/term` coupling: terminal mentions, `list_terminals`/`read_terminal` host tools and `ShellService` all reach into `term.Manager`. | `terminal_mentions.go`, `reporter.go` | Depend on a `TerminalSource` trait (list, active id, read plain) implemented by `recur-term`. |
| R17 | `discoverLoopCatalog` re-reads bundles from disk on every call; loops snapshot the catalog at start. Caching it in Rust changes behavior when users edit bundles mid-run. | `policy.go`, `role_tools.go` | Keep per-call discovery; optimize only inside `recur-bundles` with mtime checks. |
| R18 | Go data race: `syncWorkspaceBranch` writes `loop.Branch` from the fswatch goroutine. | `branch_sync.go` | Store branch in `LoopHandle` under its mutex (E1). |
| R19 | Large mixed files (`workspace_chat.go`, `engine.go`, `github_prs.go`, `pr.go`, `github_thread.go`, `loop.go`) are split across three slices each; easy to double-port or drop functions. | section 1 | Each slice lists the Go functions it ported in its PR description; the E-slice owners cross-check the union against `go doc -all` output for those files. |
| R20 | `internal/app` tests drive the engine with `ScriptedRunner` and Wails-free services; services tests are the only end-to-end coverage of settings side effects (governor caps, provider remap, Cursor pool reset, usage restart). | `app/services_test.go` | SV ports them in full; they are the acceptance gate for wave 4. |

## 7. What slices need from `recur-domain` and `recur-store`

Counts are call sites in production code of `internal/engine`, `internal/app`, `internal/agenttools` and `internal/mcp` (tests excluded). They tell the domain and store porters what must be ergonomic and fast, and tell slice owners what they can rely on.

### 7.1 `recur-domain` types by use

| Type / item | Uses | Notes for porters |
|---|---:|---|
| `Workspace` | 322 | The central record. Cloned on every read; `UpdateWorkspace` closures mutate it. Fields used by the engine include state, phase, iteration, group, plan, checkpoint, `LoopChatCode`, `HostPRID`, `ChatTabOrder`, `FanOut`, `FanOutWaves`, `ParentID`, `ParentStepIDs`, `ScheduleID`, `ScheduledPromptPending`, `Issue`, `Archived`, activity watermarks. |
| `PR` | 200 | Local and forge PRs share it; `Source` is `gh` or `local`. |
| `PRComment` | 122 | Includes pending line comments, reactions, thread ids, Recur-only comments. |
| `WorkspaceChatTab` | 54 | |
| `UpdateWorkspaceResult` | 54 | |
| `SourceGitHub` | 54 | Becomes `forge.pr_source()` comparisons. |
| `Commit` | 47 | |
| `WorkspaceState` + `State*` constants | ~250 | `StatePaused` 46, `StateFailed` 38, `StateStopped` 31, `StatePROpen` 22, `StateCoding` 21, `StateQueued` 17, `StateMerged` 16, `StateEscalated` 13, `StatePlanning` 11, `StateReviewing` 10, `StateVerifying` 9. `RunActive(state)` helper. |
| `Message`, `ContentBlock`, `Session`, `ChatItem` | 45, 10, 31, 32 | Transcript model: dual tool ids, stream status, usage, config JSON. |
| `Artifact` | 43 | |
| `Project` | 41 | Run config overlays from `.recur/project.toml`. |
| `Finding` | 40 | |
| `Agent` | 39 | |
| `Provider*` constants | 71 | `ProviderCursor` 34, `ProviderClaudeCode` 19, `ProviderCodex` 18. These are the comparisons that move behind `Provider` (R7). |
| `Check`, `PRParticipant`, `PRReviewer`, `PRReaction`, `PendingLineComment` | 33, 32, 18, 11, 11 | |
| `Settings` | 31 | Model slots, governor caps, MCP catalog, compatible providers, provider enablement, schedules keep count. |
| `FeedItem` | 30 | |
| `Plan`, `PlanStep`, `Step*` | 18, 26, ~20 | `Plan.Ready`, `Plan.Text`, `Plan.Board`. |
| `ChatMode*`, `NormalizeChatMode` | ~60, 21 | ask / plan / agent / review / loop. |
| `UserQuestion`, `UserQuestionAnswer` | 22, 9 | |
| `Escalation` | 22 | |
| `PermissionRequest`, `PermissionDecision` | 18, 20 | |
| `WorktreeCommitResult`, `FileDiff`, `FileDiffResult` | 18, 16, 10 | |
| `WorkspaceBrief`, `BriefAction`, `BriefingSnapshot`, `WorkspaceContext` | 17, 17, 10, 9 | |
| `ParseModelRef` | 16 | Splits on the first `/`; unqualified = `claude-code`. |
| `MergeMethod`, `SquashCommitMessage`, `MergeCommitMessage` | 14, ... | Effective styles from project + settings. |
| `Role*`, `AutonomyLevel`, `Group*` | ~40 | |
| `ChatAttachment`, `ScheduledPrompt`, `OpenPathResult`, `BranchChoice`, `WorkspaceLandState`, `AppAgentCode` | 14, 12, 11, 10, 9, 10 | |
| `TranscriptXML` | fork preamble | Must match Go output exactly (fork-inject text reaches agents). |
| `NextScheduledTime`, `IssueRef`, `CompatibleProvider` (+ flag dims) | schedules, issues, P3 | |

### 7.2 `recur-store` methods by use

Engine-facing (production call sites):

| Method | Uses | Method | Uses |
|---|---:|---|---:|
| `Workspace` | 201 | `UpdateWorkspace` | 76 |
| `Project` | 71 | `PR` | 55 |
| `UpdatePR` | 48 | `SessionForLoopAgent` | 44 |
| `Agent` | 41 | `Workspaces` | 32 |
| `Settings` | 26 | `SessionTranscript` | 21 |
| `UnreadCount` | 19 | `PRByWorkspace` | 18 |
| `PRs` | 16 | `Agents` | 15 |
| `UpdateAgent` | 13 | `SessionsForLoop` | 11 |
| `OpenFindingsCount` | 9 | `MarkEscalationApplied` | 8 |
| `Escalation` | 8 | `AppendContentBlock` (mirror) | 7 |
| `UpdateSessionConfigJSON` | 7 | `RemoveInboxItem` / `RemoveInboxByTarget` / `AddInboxItem` | 7 each |
| `Findings` | 7 | `UpdateSessionProvider` | 6 |
| `Projects`, `Policy`, `PatchSessionConfigMerge` | 6 each | `OpenFindings`, `AddWorkspace` | 5 each |
| `PatchSessionConfigTitle`, `OpenPRByWorkspace`, `CreateSession`, `HasProjectGroup` | 4 each | `SetSessionStatus` (incl. mirror) | 5 |

Used once to three times (all required): `AddPR`, `AppendMessage`, `CompleteMessage`, `CreateMessage`, `UpdateSessionRef`, `WriteSessionContext`, `PatchToolUseInput`, `PatchFallbackFrom`, `DropSessionMessages`, `GetSession`, `LastAssistantText`, `Feed`, `PrependFeed`, `UpsertFeed`, `PatchFeedByToolUseID`, `Resource`, `SetResource`, `SessionsForAppKind`, `SessionForAppKind`, `SessionForChatAgent`, `StreamingAppSessions`, `SessionHasMessages`, `SessionFirstUserPrompt`, `SessionContainsMessage`, `SessionHeadersByWorkspace`, `ActiveSessionContext`, `TruncateSessionAfter`, `ForkSession`, `ForkSessionToWorkspace`, `ReassignWorkspaceSessions`, `ReassignWorkspaceFindings`, `MoveWorkspaceAgents`, `PatchSessionConfigDock`, `PatchSessionConfigSimplify`, `PatchSessionConfigSimplifyIfUnset`, `LatestToolUseByNameSuffix`, `UpsertAgent`, `DeleteAgent`, `DeleteClosedChat`, `DeleteWorkspace`, `DeletePR`, `NextPRNumber`, `AddFinding`, `UpdateFindingStatus`, `SetFindingStatus`, `FindingsByStatus`, `AddFindingNote`, `DeleteFindingNote`, `AddAnalysis`, `LatestAnalysis`, `AddEscalation`, `AnswerEscalationFor`, `HasOpenEscalation`, `Inbox`, `MarkInboxRead`, `MarkAllInboxRead`, `ClearInboxForWorkspace`, `RemoveChatInboxItems`, `AttentionCounts`, `CountGroup`, `AppendEvent`, `EventsForLoop`, `LatestEventTSByWorkspace`, `ScheduledPrompts`, `SaveScheduledPrompt`, `DeleteScheduledPrompt`, `DueScheduledPrompts`, `ClaimScheduledPrompt`, `ActiveScheduledPromptRuns`, `ScheduledPromptRuns`, `UpdateScheduledPromptRun`, `LinkScheduledPromptRun`, `GitHubBranchRequirePR`, `SetGitHubBranchRequirePR`, `SetPolicy`, `SetSettings`, `UpdateProjectRunConfig`, project and group CRUD (`AddProject`, `DeleteProject`, `ReorderProjects`, `ProjectGroups`, `AddProjectGroup`, `UpdateProjectGroup`, `DeleteProjectGroup`, `ReorderProjectGroups`, `HasProject`), `ClearAll`, `DataDir`, `AppChatDir`.

Store facts slice owners must not break:

- In-memory cache is the source of truth while running; SQLite writes are best effort and never fail an engine path.
- Reads return clones. Mutation only through `Update*` closures or explicit setters.
- `Workspaces()` is insertion ordered (R4).
- `UpdateWorkspace` closures run under the store-wide lock; lock order inside the store is `feedMu` → `mu`.
- Session config is a JSON column (`agentConfig`: `code, name, title, model, flags, mode, standingMode, standingPromptSha, simplify, loopId, policyId, goal, goalDisplay, goalImages, loopStarted, ownerCode, forkInject, promptGroup, nativeHistoryAfter`, plus app-chat `closed`, `unseen`); patch helpers merge keys and must keep unknown keys.
- `.recur/project.toml` (`projfile`) overlays project run config on read; writes go through `UpdateProjectRunConfig`.
- The session mirror's write pattern (one `CreateMessage` per assistant/user boundary, many `AppendContentBlock`, one `CompleteMessage`) is the hottest store path during turns; keep it O(1) per block.
