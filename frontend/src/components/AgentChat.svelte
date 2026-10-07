<script module lang="ts">
  // Hoisted so svelte2tsx can export AgentMsg / ChatLog* without a cycle
  // (AgentMsg → ChatLog → ChatLogSession.messages: AgentMsg[]).
  import type { DigestFindingRef } from '../lib/agentchat'

  export type ChatAttachmentDraft = {
    id: string
    name: string
    mime: string
    dataUrl: string
  }

  export type ChatAttachmentView = {
    id: string
    name?: string
    mime?: string
    url?: string
    dataUrl?: string
  }

  export type FindingRef = DigestFindingRef & {
    statusReason?: string
  }

  export type ChatLogFact = {
    kind: 'fact'
    id?: string
    text: string
    isDeviation?: boolean
    code?: string
    name?: string
    /** Typed card ("fanout") rendered from data instead of text. */
    card?: string
    data?: string
  }
  export type ChatLogSession = {
    kind: 'session'
    id?: string
    code: string
    name: string
    messages: AgentMsg[]
    live: boolean
  }
  /** Consecutive specialist rows packed into one fanout card. */
  export type ChatLogSessions = {
    kind: 'sessions'
    id?: string
    sessions: ChatLogSession[]
  }
  export type ChatLog = ChatLogSession | ChatLogSessions | ChatLogFact

  export type AgentMsg = {
    body?: string
    me?: boolean
    note?: boolean
    /** Notice row label; defaults to "Notice". */
    noteLabel?: string
    /** Engine-fact markup (commit/handoff). Ordinary notices stay escaped text. */
    noteHtml?: boolean
    streaming?: boolean
    /**
     * Trailing store-mapped assistant row after a remount lost `streaming`.
     * `applyStream` may widen this row; a non-prefix snapshot opens a new one.
     */
    widenStream?: boolean
    tool?: string
    toolInput?: string
    toolId?: string
    description?: string
    input?: string
    result?: string
    resultError?: boolean
    resultTruncated?: boolean
    resultPending?: boolean
    turnEnd?: boolean
    durationMs?: number
    messageId?: string
    /** Turn footer: this turn changed files (turn diff). */
    checkpoint?: boolean
    stopped?: boolean
    error?: boolean
    thinking?: boolean
    tokens?: number
    compact?: boolean
    compacting?: boolean
    fallback?: boolean
    providerUUID?: string
    attachments?: ChatAttachmentView[]
    queued?: boolean
    pendingId?: string
    optimistic?: boolean
    ts?: number
    log?: ChatLog
    /** Nested subagent output rendered inside the parent Task card. */
    children?: AgentMsg[]
  }
</script>

<script lang="ts">
  // Shared agent-chat surface. The loop's Maestro tab/dock and a workspace's coding
  // agent both mount this — the copy (empty text, placeholder) and an optional
  // in-input toolbar (e.g. a model selector) are props so it isn't loop-specific.
  //
  // It renders the full session: user/assistant text, the agent's reasoning, and its
  // tool calls. Steps (tools + thinking) render as collapsed one-line rows labeled by
  // the agent's own description; each expands to a detail card (Bash → mini terminal,
  // Edit → old/new diff, Read → file body). Contiguous runs of steps collapse into a
  // "N tool calls" header for the whole user turn. Inner assistant asides fold into
  // that group; the live line and the final reply stay visible. The first thinking
  // window stays open; after that a live turn stays collapsed, with only the
  // current step as a one-line row.
  import type { Snippet } from 'svelte'
  import { highlightBash } from '../lib/shhl'
  import { langFromPath, highlightLine, diffLines, type DiffRow, parseUnifiedDiff, looksLikeUnifiedDiff } from '../lib/codehl'
  import {
    attachmentMarker, ATTACHMENT_MARKER_RE, FILE_AT_RE, SKILL_SLASH_RE, TERMINAL_AT_RE, ARTIFACT_AT_RE,
    stripAttachmentMarkers,
    formatFileMentionChip, parseFileMentionChip, fileMentionLabel, dirFileMentionPath,
    prepareFileMentionInsert, repairFileMentionBoundaries,
    terminalMentionLabel,
    artifactMentionLabel,
    uniqueArtifactIdForTitle, collectLooseAtNames, tokenSpanOverlaps,
    imageFilesFromPaste, draftsFromImageFiles, draftsFromImagePaths, mentionQueryAt, skillQueryAt,
    extractAttachmentIds,
    stillOpenFindingsForTurn,
    lastAssistantTextInTurn,
    currentTurnHasEnded,
    agentErrorDetail, agentErrorTitle, stepRowKey,
    isTaskTool, isSubagentLaunchMeta,
    type TurnDigestFinding,
    type TurnDigestNote,
  } from '../lib/agentchat'
  import {
    listPromptActionsUnfiltered, parseCommittedPromptLine, promptActionQueryAt,
    promptArgsQueryAt, promptTakesArgs, promptArgSlotOptional, promptRequiredArgsFilled, promptArgSpecs, paramsFromArgs, completePromptArgs, type PromptActionInfo, type PromptArgsQuery,
    type PromptCompleteHit,
  } from '../lib/promptActions'
  import {
    serializeComposer, hydrateComposer, chipifyComposerText, composerCaretOffset,
    setComposerCaret, ensureComposerCaret, scrollComposerCaretIntoView, composerCaretClientRect, resizeComposerHeight,
    deleteAdjacentComposerChip, normalizeComposerDom, ensureChipBoundariesInDom,
    isCaretAfterFileChip, isCaretAfterSkillChip, isCaretAfterTerminalChip, isCaretAfterArtifactChip,
    insertComposerParagraphBreak,
    insertComposerChip, makeAttachmentChip, makeFileChip, makeSkillChip, makeTerminalChip, makeArtifactChip,
    findPromptChip, focusPromptChipArg, promptChipInputs, readPromptChipValues, serializePromptChip, parsePromptChipValues, isDirChipPath, type PromptChipSpec,
  } from '../lib/composerDom'
  import { Events } from '@wailsio/runtime'
  import {
    classifyOsDrop, encodeChatDropId, endFileDrag, fileDragPaths, filesFromWailsDrop,
    imageFilesFromDrop, isChipableFilePath, isComposerFileDrop, pathsFromOsDataTransfer,
  } from '../lib/file-drag'
  import { prepareFuzzy, fuzzyFilterPrepared } from '../lib/fuzzy'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import {
    searchFiles,
    subscribeFileIndexStatus,
    SEARCH_CANCELLED,
    type FileSearchHit,
  } from '../lib/file-index'
  import { tick, untrack } from 'svelte'
  import { slide } from 'svelte/transition'
  import { cubicOut } from 'svelte/easing'
  import { fileIconClass } from '../lib/file-icons'
  import { theme, permissionRequests, userQuestions, view, observatoryJump, pushToast, settings, projects, prs, openPR, routeTarget, workspaceTerminals, terminalTabsEqual, terminalFocus, setTerminalDockOpen, workspaceArtifacts, artifactsEqual, briefing, workspaces, openSettings, type SettingsSection } from '../lib/stores'
  import { chatModeMeta } from '../lib/modes'
  import {
    artifactCanOpen, artifactIcon, artifactKindTally, artifactWhen, resolveListArtifacts, artifactFromEvent,
    type ListedArtifact, type ListedArtifacts,
  } from '../lib/artifacts'
  import { CodeService, ObservatoryService, call, answerPermission, bridgeErrorMessage, onArtifactCreated } from '../lib/api'
  import { ListTerminals } from '../../bindings/recur/internal/app/terminalservice'
  import { ListArtifacts } from '../../bindings/recur/internal/app/artifactsservice'
  import PermissionCard from './PermissionCard.svelte'
  import FanOutCard from './FanOutCard.svelte'
  import PlanCard from './PlanCard.svelte'
  import QuestionCard from './QuestionCard.svelte'
  import type { Escalation } from '../../bindings/recur/internal/domain/models'
  import {
    isSubmitPlanTool, isUpdatePlanStepTool, isEditPlanTool,
    submitPlanLabel, updatePlanStepLabel, editPlanLabel, updatePlanStepMeta,
    parseSubmitPlanSteps, parseUpdatePlanStepTitle, parseEditPlan,
    parseSubmitPlanBody, parseSubmitPlanAttachments,
    prosePreview,
  } from '../lib/plantools'
  import {
    isClaudeTaskTool, formatTaskId, prettyTaskStatus, taskStatusChipClass,
    parseClaudeTaskArgs, parseTaskCreateId, parseTaskList, parseTaskGet,
    claudeTaskCounts, claudeTaskLabel, claudeTaskMeta, claudeTaskChipStatus,
    claudeTaskHasBody, claudeTaskResultIsAck,
    type ClaudeTaskView,
  } from '../lib/tasktools'
  import {
    isSubagentTool, isSendMessageTool,
    parseSubagentArgs, parseSendMessageArgs, isSubagentLaunchAck,
    sendMessageLabel, sendMessageMeta,
  } from '../lib/subagent'
  import { isOpenPRTool, openPRLabel, parseOpenPRCard } from '../lib/open-pr-tool'
  import {
    isReenterPhaseTool, parseReenterPhaseID, reenterPhaseFailed, reenterPhaseLabel,
  } from '../lib/reenter-phase-tool'
  import {
    isListWorkspaceBriefsTool, isGetWorkspaceContextTool, isPresentWorkspaceTool, isBriefingTool,
    parseListWorkspaceBriefs, parseWorkspaceContext, parsePresentWorkspace, parsePresentWorkspaceTarget,
    hydratePresentBrief,
    listWorkspaceBriefsLabel, getWorkspaceContextLabel, presentWorkspaceLabel, briefingCountsMeta,
    listWorkspaceBriefsSummary, workspaceContextSummary,
    type ListedWorkspaceBriefs, type WorkspaceContextView, type PresentWorkspaceView,
  } from '../lib/briefingtools'
  import { compactUrl, webFetchPrompt, webFetchUrl, webResultIsJson, webSearchQuery } from '../lib/webtools'
  import {
    isLinearTool, linearCountLabel, linearInputBody, linearItemUrl, linearToolLabel, linearToolMeta,
    parseLinearArgs, parseLinearResult, type LinearView,
  } from '../lib/lineartools'
  import { isExternalHttpUrl } from '../lib/open-external'
  import { parseTodos, todoCounts } from '../lib/todos'
  import StreamingMarkdown from './StreamingMarkdown.svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import AttachmentLightbox from './AttachmentLightbox.svelte'
  import ImageContextMenu, { openImageCtx, type ImageCtxMenu } from './ImageContextMenu.svelte'
  import ContextUsageMeter from './ContextUsageMeter.svelte'
  import Icon from './Icon.svelte'
  import WorkspaceBriefCard from './WorkspaceBriefCard.svelte'
  import Kbd from './Kbd.svelte'
  import {
    findingBodyNeeded, findingLocLabel, findingLocSuffix as locSuffixOf, findingMatchesRef, findingSevIcon, findingSeverityTally,
    findingTitle, parseListedFinding, resolveListFindings,
    type ListedFinding, type ListedFindings,
  } from '../lib/findings-ui'
  import { copyText } from '../lib/clipboard'
  import { isCmdOrCtrl, isLetterKey } from '../lib/shortcut'
  import { ComposerUndo } from '../lib/composerUndo'
  import { formatElapsed } from '../lib/format'
  import { formatTokens, type ContextUsage } from '../lib/contextUsage'
  import { agColor, prForWorkspace } from '../lib/colors'
  import { loopCompletionWrap, loopPRCompletion, stripLoopWrap } from '../lib/loop-cockpit'
  import {
    claimTranscriptUI,
    emptyTranscriptUI,
    expansionStillValid,
    loadTranscriptUI,
    saveTranscriptUI,
    saveTranscriptUIScroll,
    NEAR_BOTTOM_PX,
    nextNearBottom,
    scrollerNearBottom,
    scrollRestoreReady,
    snapshotTranscriptUI,
    transcriptBlockKey,
    transcriptMessageBlockLocal,
    transcriptSessionBlockLocal,
    transcriptStepBlockLocal,
    transcriptFingerprint,
    uniqueBlockKeys,
    restoreBlockHeights,
    type ChatTranscriptUIState,
  } from '../lib/caches/chat-transcript-ui'
  import { fadeScrollEdges } from '../lib/scroll-fade'
  import { inViewPaths, pickHydrated, recordStubHeight, samePathList, samePathSet } from '../lib/review-window'
  import { applyViewportPin, captureViewportPin, fileAtScrollTop } from '../lib/spy-pick'
  import {
    CHAT_HYDRATE_ALL_BELOW, CHAT_HYDRATE_MAX, CHAT_HYDRATE_OVERSCAN, CHAT_TAIL_LIVE, CHAT_VIEW_PAD_PX,
    estimateChatBlockHeight, reservedChatHeight,
  } from '../lib/chat-window'
  import { foldInnerNarration, foldSubagentNotice, hasLaterStep, isFirstCheckpoint, liveGroupOpen } from '../lib/chat-glance'

  let {
    messages = [],
    input = $bindable(''),
    attachments = $bindable<ChatAttachmentDraft[]>([]),
    sending = false,
    steerFollowUp = false,
    canSteer = false,
    /** Durable turn start (ms). When set, elapsed survives remounts / workspace switches. */
    turnStartedAt = 0,
    /** Setup / wait copy shown as a turn-status row while the worktree allocates. */
    statusText = '',
    onSend,
    /** Interrupt the in-flight turn (shown as Stop while sending or a live walk). */
    onStop,
    /** Loop walk is live on this tab — Stop even when the conductor isn't sending. */
    canStop = false,
    /** Re-run the last user prompt after an agent error (no new user bubble). */
    onRetry,
    /** Drop a follow-up still waiting in the queue (by pendingId). */
    onRemoveQueued,
    /** Idle with held follow-ups (e.g. after Stop) — Send/Enter flushes the queue. */
    canFlushQueue = false,
    /** ⌘. / Ctrl+. while the composer is focused — cycle Ask → Plan → Agent → Review → Loop. */
    onCycleMode,
    compact = false,
    /** Live workbench. A parked hot shell stays mounted but must not steal keys. */
    shellActive = true,
    /** Actual panel visibility, including closed docks and other app views. */
    presentationActive = true,
    /** Chat identity (workspace + agent code). Switching restores that tab's chrome. */
    resetKey = '',
    emptyText = 'Ask where this loop stands, or steer it — it can re-plan within your rails, but never cross them.',
    placeholder = 'Ask where things stand, or steer the loop…',
    /** Rendered inside the input box, bottom-left (e.g. a model selector). */
    toolbar,
    /** Last-turn context fill for the composer meter (hidden until ready). */
    contextUsage = null,
    /** Open a file the agent touched in the code editor (path as the tool saw it). */
    onOpenFile,
    /** Open an artifact from a list_artifacts row (scratch tab / preview). */
    onOpenArtifact,
    /** Worktree root, stripped from displayed paths (tools see absolute paths). */
    rootPath = '',
    /** Project + workspace for lazy ListFiles when @ is typed. */
    projectId = '',
    workspaceId = '',
    /** Agent tab code (PA / PA2 / MA) — scopes inline permission prompts. */
    agentCode = '',
    /** Open loop escalation — same inline card as ask_user, on the MA tab. */
    escalation = null as Escalation | null,
    onAnswerEscalation,
    /** Transcript-only mode: no composer dock (SessionCard in the Loop cockpit). */
    readonly = false,
    /** Workspace findings — used to resolve update_finding rows by finding_id. */
    findings = [] as FindingRef[],
    /** Branch this chat at a finished turn (message id from the turn footer). */
    onFork,
    /** Same fork point, into a new workspace in this project. */
    onForkToWorkspace,
    /** Show what the agent changed in the worktree during this turn. */
    onTurnDiff,
    /** Restore the worktree to this turn's end and drop later messages. */
    onReset,
    /** Invoke a bundle prompt action by slash name (immediate send, optional args). */
    onPromptAction,
    /** True while a prompt script is running (spinner / disable). */
    promptBusy = false,
    /** Loop cockpit: render a specialist SessionCard for a session log (and each row in a packed group). */
    sessionCard,
    /** Restricted Ask Recur surface: no mentions, attachments, forks, or workspace permission/question cards. */
    surface = 'workspace' as 'workspace' | 'app',
    /** Composer only (New-workspace modal): no transcript, queue, or send row. */
    composerOnly = false,
    empty,
  }: {
    messages?: AgentMsg[]
    input?: string
    attachments?: ChatAttachmentDraft[]
    sending?: boolean
    /** Busy turn: ⏎ injects the follow-up into the live turn instead of queueing. */
    steerFollowUp?: boolean
    /** Busy turn can take a steer; ⌘⏎ does whichever of steer/queue ⏎ does not. */
    canSteer?: boolean
    turnStartedAt?: number
    statusText?: string
    onSend?: (alt: boolean) => void
    onStop?: () => void
    canStop?: boolean
    onRetry?: () => void
    onRemoveQueued?: (pendingId: string) => void
    canFlushQueue?: boolean
    onCycleMode?: () => void
    /** Tighter padding for the Code-tab dock. */
    compact?: boolean
    /** Live workbench. Parked hot shells must not steal window chords. */
    shellActive?: boolean
    presentationActive?: boolean
    resetKey?: string
    emptyText?: string
    placeholder?: string
    toolbar?: Snippet
    contextUsage?: ContextUsage | null
    onOpenFile?: (path: string, line?: number, endLine?: number, startCol?: number, endCol?: number) => void
    onOpenArtifact?: (a: ListedArtifact) => void
    rootPath?: string
    projectId?: string
    workspaceId?: string
    agentCode?: string
    escalation?: Escalation | null
    onAnswerEscalation?: (choice: string, text: string) => void
    readonly?: boolean
    findings?: FindingRef[]
    onFork?: (messageId: string) => void
    onForkToWorkspace?: (messageId: string) => void
    onTurnDiff?: (messageId: string) => void
    onReset?: (messageId: string, restoresFiles: boolean) => void
    onPromptAction?: (slash: string, args?: string, alt?: boolean) => void
    promptBusy?: boolean
    sessionCard?: Snippet<[ChatLogSession]>
    surface?: 'workspace' | 'app'
    composerOnly?: boolean
    empty?: Snippet
  } = $props()

  const appSurface = $derived(surface === 'app')

  // Chat-surface prompts for this tab sit in the transcript (not the floating
  // stack) only while this workspace detail is on screen. WorkspaceView stays
  // mounted when other views are open — without the view gate we'd paint the
  // card twice (hidden inline + floating fallback).
  const chatPermissions = $derived(
    readonly || !workspaceId || !agentCode || $view !== 'detail' || !shellActive
      ? []
      : $permissionRequests.filter(
          (r) =>
            r.surface === 'chat' &&
            r.workspaceId === workspaceId &&
            r.agentCode === agentCode,
        ),
  )
  const chatQuestions = $derived(
    readonly || !workspaceId || !agentCode || $view !== 'detail'
      ? []
      : $userQuestions.filter(
          (q) => q.workspaceId === workspaceId && q.agentCode === agentCode,
        ),
  )
  const chatEscalation = $derived(
    readonly || $view !== 'detail' || escalation?.status !== 'open' || !escalation.question
      ? null
      : escalation,
  )

  // Contenteditable composer: file @mentions and pasted images render as inline
  // chips in the sentence. Wire text serializes to @path / ⟦attachment:id⟧.
  let composerEl = $state<HTMLDivElement | undefined>(undefined)
  let composerRendered = $state('')
  /** Skip hydrate when the composer itself just wrote `input` (typing must not
   *  turn `@app.c` into a chip — only an explicit picker commit does). */
  let skipComposerHydrate = false
  const composerUndo = new ComposerUndo<ChatAttachmentDraft>()
  // Send stays available while a turn is running — parent queues follow-ups.
  // After Stop, an empty composer can still flush held follow-ups.
  const canSend = $derived(
    !!stripAttachmentMarkers(input).trim() || attachments.length > 0 || canFlushQueue,
  )
  let forkMenu = $state<{ messageId: string; left: number; top: number } | null>(null)
  let forkMenuEl = $state<HTMLDivElement | null>(null)

  function closeForkMenu() {
    forkMenu = null
  }

  function onForkMenuKey(e: KeyboardEvent) {
    if (e.key !== 'Escape' || !forkMenu) return
    e.preventDefault()
    e.stopPropagation()
    closeForkMenu()
  }

  function toggleForkMenu(messageId: string, ev: MouseEvent) {
    ev.stopPropagation()
    if (!onForkToWorkspace) {
      onFork?.(messageId)
      return
    }
    if (forkMenu?.messageId === messageId) {
      forkMenu = null
      return
    }
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect()
    const pad = 8
    const mw = 220
    const mh = 84
    let left = r.left
    if (left + mw > window.innerWidth - pad) left = Math.max(pad, window.innerWidth - mw - pad)
    let top = r.bottom + 6
    if (top + mh > window.innerHeight - pad) {
      top = Math.max(pad, r.top - mh - 6)
    }
    forkMenu = {
      messageId,
      left,
      top,
    }
  }

  $effect(() => {
    if (!forkMenu || !forkMenuEl) return
    forkMenuEl.querySelector<HTMLElement>('[role=menuitem]')?.focus()
  })
  // Empty for placeholder: no visible text and no image chips.
  const composerEmpty = $derived(
    !stripAttachmentMarkers(input).trim() && attachments.length === 0,
  )

  // ---- @ file mentions + / prompt actions + / skill commands -----------------
  type SkillHit = { name: string; description: string; scope: string }
  type SlashHit =
    | { kind: 'prompt'; slash: string; name: string; description: string; takesArgs?: boolean; args?: { name: string }[]; mode?: string }
    | { kind: 'skill'; name: string; description: string; scope: string }
  type PromptArgHit =
    | { kind: 'run'; slash: string; args: string; name: string; description: string }
    | { kind: 'suggest'; hit: PromptCompleteHit }
  let fileIndexTick = $state(0) // bumped when Go index readiness changes
  let fileIndexLoading = $state(false)
  let skillIndex = $state<SkillHit[]>([])
  let skillIndexKey = $state('')
  let skillIndexLoading = $state(false)
  let promptIndex = $state<PromptActionInfo[]>([])
  let promptIndexLoading = $state(false)
  let mention = $state<{ start: number; end: number; query: string } | null>(null)
  let skillMention = $state<{ start: number; end: number; query: string } | null>(null)
  /** True when the slash query passes the prompt-action gate (offset 0 + trailing ws). */
  let promptGateOpen = $state(false)
  /** Whole-composer `/slash args` for a takesArgs prompt — parameter menu. */
  let promptArgs = $state<PromptArgsQuery | null>(null)
  let argsMenuEscaped = $state(false)
  let completeHits = $state<PromptCompleteHit[]>([])
  let completeMessage = $state('')
  let completeGen = 0
  let mentionSel = $state(0)
  let mentionListEl = $state<HTMLDivElement | undefined>(undefined)
  type MentionHit =
    | { kind: 'file'; item: string; label: string; score: number; dir: boolean }
    | { kind: 'terminal'; id: string; title: string; label: string; active: boolean; exited?: boolean }
    | { kind: 'artifact'; id: string; title: string; label: string; artifactKind: string }

  let mentionHits = $state<MentionHit[]>([])
  let mentionSearchGen = 0
  // Ignore hover selection until the pointer actually moves — opening the `/`
  // or `@` menu under a stationary cursor fires a synthetic mouseenter.
  let mentionHoverGate = createHoverGate()
  // Escape dismisses the menu but leaves `@query` in the composer; keyup would
  // reopen via syncMentionFromCaret — suppress until the query/start changes.
  let mentionEscaped: { kind: 'file' | 'skill'; start: number; query: string } | null = null

  // skillIndex is the full user-invocable catalog (ListAllSkills). Filter
  // enablement here so Settings/Project toggles update the `/` menu live —
  // including re-enables that a pre-filtered ListSkills fetch couldn't restore.
  const liveTerminals = $derived($workspaceTerminals[workspaceId] ?? [])
  const liveArtifacts = $derived($workspaceArtifacts[workspaceId] ?? [])

  function terminalTitleFor(id: string): string {
    if (!id) {
      const active = liveTerminals.find((t) => t.active && !t.exited)
      return active?.title || liveTerminals[0]?.title || ''
    }
    return liveTerminals.find((t) => t.id === id)?.title || ''
  }

  function matchingTerminalHits(query: string): MentionHit[] {
    const q = query.trim().toLowerCase()
    const tabs = liveTerminals
    if (!tabs.length) return []
    const showAll = !q || q === 'term' || q === 'terminal' || 'terminal'.startsWith(q)
    return tabs
      .filter((t) => {
        if (showAll) return true
        return t.title.toLowerCase().includes(q) || t.id.toLowerCase().startsWith(q)
      })
      .slice(0, 8)
      .map((t) => ({
        kind: 'terminal' as const,
        id: t.id,
        title: t.title,
        label: t.title,
        active: !!t.active,
        exited: !!t.exited,
      }))
  }

  function artifactTitleFor(id: string): string {
    if (!id) return liveArtifacts[0]?.title || ''
    return liveArtifacts.find((a) => a.id === id)?.title || ''
  }

  function artifactIdForTitle(title: string): string {
    return uniqueArtifactIdForTitle(title, liveArtifacts)
  }

  function matchingArtifactHits(query: string): MentionHit[] {
    const q = query.trim().toLowerCase()
    const items = liveArtifacts
    if (!items.length) return []
    const showAll = !q || (q.length >= 3 && 'artifact'.startsWith(q))
    const hits = items
      .filter((a) => {
        if (showAll) return true
        return (
          a.title.toLowerCase().includes(q) ||
          a.id.toLowerCase().startsWith(q) ||
          a.kind.toLowerCase().includes(q)
        )
      })
      .map((a) => ({
        kind: 'artifact' as const,
        id: a.id,
        title: a.title,
        label: a.title,
        artifactKind: a.kind,
      }))
    if (!showAll) {
      hits.sort((a, b) => artifactTitleRank(a.title, q) - artifactTitleRank(b.title, q))
    }
    return hits.slice(0, 8)
  }

  function artifactTitleRank(title: string, q: string): number {
    const t = title.toLowerCase()
    if (t === q) return 0
    if (t.startsWith(q)) return 1
    if (t.includes(q)) return 2
    return 3
  }

  function applyListedArtifacts(ws: string, list: { id: string; title?: string; kind?: unknown; path: string }[]) {
    workspaceArtifacts.update((m) => {
      const next = list.map((a) => ({
        id: a.id,
        title: a.title || a.id,
        kind: String(a.kind || ''),
        path: a.path,
      }))
      const prev = m[ws] ?? []
      if (artifactsEqual(prev, next)) return m
      return { ...m, [ws]: next }
    })
  }

  function mentionHitKey(h: MentionHit): string {
    if (h.kind === 'terminal') return 't:' + h.id
    if (h.kind === 'artifact') return 'a:' + h.id
    return 'f:' + h.item
  }

  function setMentionHits(next: MentionHit[]) {
    const prev = untrack(() => mentionHits)
    if (
      prev.length === next.length &&
      prev.every((h, i) => {
        const o = next[i]!
        if (mentionHitKey(h) !== mentionHitKey(o)) return false
        if (h.kind === 'terminal' && o.kind === 'terminal') {
          return h.title === o.title && h.active === o.active && !!h.exited === !!o.exited
        }
        if (h.kind === 'artifact' && o.kind === 'artifact') {
          return h.title === o.title && h.artifactKind === o.artifactKind
        }
        return h.kind === 'file' && o.kind === 'file' && h.dir === o.dir
      })
    ) {
      return
    }
    const sel = untrack(() => mentionSel)
    const keep = prev[sel]
    mentionHits = next
    if (!keep) return
    const i = next.findIndex((h) => mentionHitKey(h) === mentionHitKey(keep))
    if (i >= 0) mentionSel = i
    else if (sel >= next.length) mentionSel = Math.max(0, next.length - 1)
  }

  const enabledSkillIndex = $derived.by(() => {
    const proj = $projects.find((p) => p.id === projectId)
    const globalDisabled = $settings?.disabledSkills
    const projectDisabled = proj?.disabledSkills
    const projectEnabled = proj?.enabledSkills
    return skillIndex.filter((s) => {
      if (projectDisabled?.includes(s.name)) return false
      if (projectEnabled?.includes(s.name)) return true
      return !globalDisabled?.includes(s.name)
    })
  })
  const preparedSkills = $derived(
    prepareFuzzy(enabledSkillIndex, (s) => `${s.name} ${s.description}`),
  )
  const skillHits = $derived(
    skillMention ? fuzzyFilterPrepared(preparedSkills, skillMention.query, 12) : [],
  )
  const enabledPromptIndex = $derived.by(() => {
    const disabled = $settings?.disabledBundlePrompts
    if (!disabled?.length) return promptIndex
    const off = new Set(disabled)
    return promptIndex.filter((p) => !off.has(p.id))
  })
  const preparedPrompts = $derived(
    prepareFuzzy(enabledPromptIndex, (p) => `${p.slash} ${p.name} ${p.description}`),
  )
  const promptHits = $derived(
    skillMention && promptGateOpen && attachments.length === 0
      ? fuzzyFilterPrepared(preparedPrompts, skillMention.query, 12)
      : [],
  )
  const promptChipSpecs = $derived.by((): PromptChipSpec[] =>
    enabledPromptIndex
      .filter((p) => promptTakesArgs(p))
      .map((p) => ({ slash: p.slash, args: promptArgSpecs(p) })),
  )
  /** Prompts first (alpha via list), then skills; prompts win same-name collisions in the menu. */
  const slashHits = $derived.by(() => {
    if (!skillMention) return [] as { item: SlashHit; label: string }[]
    const promptNames = new Set(promptHits.map((h) => (h.item as PromptActionInfo).slash))
    const prompts = promptHits.map((h) => {
      const p = h.item as PromptActionInfo
      return {
        label: h.label,
        item: {
          kind: 'prompt' as const,
          slash: p.slash,
          name: p.name,
          description: p.description || '',
          takesArgs: promptTakesArgs(p),
          args: promptArgSpecs(p).map((a) => ({ name: a.name })),
          mode: p.mode,
        },
      }
    })
    const skills = skillHits
      .filter((h) => !promptNames.has((h.item as SkillHit).name))
      .map((h) => {
        const s = h.item as SkillHit
        return {
          label: h.label,
          item: {
            kind: 'skill' as const,
            name: s.name,
            description: s.description,
            scope: s.scope,
          },
        }
      })
    return [...prompts, ...skills]
  })
  const promptArgHits = $derived.by((): PromptArgHit[] => {
    const q = promptArgs
    if (!q || argsMenuEscaped || attachments.length) return []
    const info = enabledPromptIndex.find((p) => p.slash === q.slash)
    const args = q.args.trim()
    const rows: PromptArgHit[] = []
    rows.push({
      kind: 'run',
      slash: q.slash,
      args,
      name: info?.name || q.slash,
      description: args
        ? (info?.description || 'Run this prompt')
        : (info?.argPlaceholder || 'Type an argument, then Enter'),
    })
    for (const h of completeHits) {
      rows.push({ kind: 'suggest', hit: h })
    }
    return rows
  })
  const promptArgInfo = $derived(enabledPromptIndex.find((p) => p.slash === promptArgs?.slash))
  const promptArgAllowEmpty = $derived(promptArgSlotOptional(promptArgInfo, promptArgs?.arg))
  const promptArgCanInvoke = $derived.by(() => {
    const info = promptArgInfo
    const q = promptArgs
    const chip = composerEl ? findPromptChip(composerEl) : null
    const values = chip ? readPromptChipValues(chip) : q ? [q.args] : []
    return promptRequiredArgsFilled(info, values)
  })
  const promptArgIcon = $derived(promptArgInfo?.iconSvg?.trim() ?? '')
  const activeMenuHits = $derived(mention ? mentionHits : promptArgs ? promptArgHits : slashHits)

  $effect(() => {
    const q = promptArgs
    const info = q ? enabledPromptIndex.find((p) => p.slash === q.slash) : undefined
    if (!q || argsMenuEscaped || !info?.hasComplete) {
      completeHits = []
      completeMessage = ''
      return
    }
    const gen = ++completeGen
    const chip = composerEl ? findPromptChip(composerEl) : null
    let params = paramsFromArgs(info, q.args)
    if (chip) {
      const specs = promptArgSpecs(info)
      const values = readPromptChipValues(chip)
      params = Object.fromEntries(specs.map((s, i) => [s.name, values[i] ?? '']))
    }
    const t = setTimeout(() => {
      void completePromptArgs(info, q.args.trim(), {
        workspaceId,
        arg: q.arg || promptArgSpecs(info)[0]?.name,
        params,
      })
        .then((res) => {
          if (gen !== completeGen) return
          completeHits = res.hits ?? []
          completeMessage = res.message ?? ''
        })
        .catch(() => {
          if (gen !== completeGen) return
          completeHits = []
          completeMessage = ''
        })
    }, 200)
    return () => { clearTimeout(t) }
  })

  $effect(() => {
    const key = workspaceId || projectId
    if (!key) return
    return subscribeFileIndexStatus(key, () => { fileIndexTick++ })
  })

  // Rank in Go via SearchFiles. Re-run when the query changes or the index
  // finishes/refreshes (status tick) so partial builds fill in.
  // Do not read mentionHits or liveTerminals here — a fresh hits array or tab
  // snapshot would re-trigger this effect and flash the menu.
  $effect(() => {
    void fileIndexTick
    if (!mention || (!workspaceId && !projectId)) {
      mentionHits = []
      fileIndexLoading = false
      return
    }
    const q = mention.query
    const my = ++mentionSearchGen
    fileIndexLoading = untrack(() => mentionHits.every((h) => h.kind !== 'file'))
      && untrack(() => matchingTerminalHits(q)).length === 0
      && untrack(() => matchingArtifactHits(q)).length === 0
    if (!projectId) {
      fileIndexLoading = false
      return
    }
    void searchFiles(projectId, workspaceId, q, 50).then((res) => {
      if (res === SEARCH_CANCELLED || my !== mentionSearchGen) return
      const files: MentionHit[] = res.hits.map((h: FileSearchHit) => ({
        kind: 'file', item: h.item, label: h.label, score: h.score, dir: !!h.dir,
      }))
      const terms = untrack(() => matchingTerminalHits(q))
      const arts = untrack(() => matchingArtifactHits(q))
      setMentionHits([...terms, ...arts, ...files])
      fileIndexLoading = res.building && files.length === 0 && terms.length === 0 && arts.length === 0
    })
  })

  // Splice live terminal rows without restarting SearchFiles (OSC title / focus).
  $effect(() => {
    if (!mention) return
    const q = mention.query
    const terms = matchingTerminalHits(q)
    const arts = matchingArtifactHits(q)
    const files = untrack(() => mentionHits).filter((h) => h.kind === 'file')
    setMentionHits([...terms, ...arts, ...files])
  })

  // Refresh the tab list once per @-menu open (not on every keyup). Identical
  // snapshots must not rewrite the store — that retriggers the search effect.
  let listedTerminalsForMention = false
  $effect(() => {
    if (!mention || !workspaceId) {
      listedTerminalsForMention = false
      return
    }
    if (listedTerminalsForMention) return
    listedTerminalsForMention = true
    const ws = workspaceId
    void ListTerminals(ws).then((list) => {
      if (!list) return
      workspaceTerminals.update((m) => {
        const prev = m[ws] ?? []
        const byId = new Map(prev.map((t) => [t.id, t]))
        const next = list.map((info) => {
          const old = byId.get(info.id)
          return {
            id: info.id,
            title: old?.title || info.title || 'zsh',
            exited: old?.exited ?? !!info.exited,
            active: old?.active ?? false,
          }
        })
        if (terminalTabsEqual(prev, next)) return m
        return { ...m, [ws]: next }
      })
    }).catch(() => {})
  })

  // Prefetch so history/composer chips can show titles and open without typing @.
  $effect(() => {
    const ws = workspaceId
    if (!ws) return
    void ListArtifacts(ws).then((list) => {
      if (!list) return
      applyListedArtifacts(ws, list)
    }).catch(() => {})
  })

  let listedArtifactsForMention = false
  $effect(() => {
    if (!mention || !workspaceId) {
      listedArtifactsForMention = false
      return
    }
    if (listedArtifactsForMention) return
    listedArtifactsForMention = true
    const ws = workspaceId
    void ListArtifacts(ws).then((list) => {
      if (!list) return
      applyListedArtifacts(ws, list)
    }).catch(() => {})
  })

  $effect(() => {
    return onArtifactCreated((ws, raw) => {
      const a = artifactFromEvent(raw)
      if (!a?.id || !ws) return
      workspaceArtifacts.update((m) => {
        const prev = m[ws] ?? []
        const row = { id: a.id, title: a.title || a.id, kind: a.kind, path: a.path }
        if (prev.some((x) => x.id === a.id)) {
          return { ...m, [ws]: prev.map((x) => (x.id === a.id ? row : x)) }
        }
        return { ...m, [ws]: [row, ...prev] }
      })
    })
  })

  $effect(() => {
    const key = `${projectId}\t${workspaceId}`
    if (key === skillIndexKey) return
    skillIndexKey = key
    skillIndex = []
    skillMention = null
  })

  // A new draft owner (chat tab) never inherits the previous tab's undo history.
  $effect(() => {
    void resetKey
    untrack(() => composerUndo.reset({ text: input, atts: attachments, caret: input.length }, true))
  })

  // Parent/tab swaps set `input` / attachments — rehydrate chips from wire text.
  $effect(() => {
    const el = composerEl
    if (!el) return
    const attKey = attachments.map((a) => a.id).join('\0')
    const specKey = promptChipSpecs.map((s) => s.slash).join(',')
    const key = `${input}\n${attKey}\n${specKey}`
    if (key === composerRendered) return
    if (skipComposerHydrate) {
      composerRendered = key
      return
    }
    // Restore @path + /skill chips from a persisted draft; never for live typing.
    const hadFocus = document.activeElement === el
    hydrateComposer(el, input, attachments, {
      chipFiles: true,
      chipSkills: true,
      terminalTitle: (id) => untrack(() => terminalTitleFor(id)),
      artifactTitle: (id) => untrack(() => artifactTitleFor(id)),
      artifactIdForTitle: (title) => untrack(() => artifactIdForTitle(title)),
      promptChips: untrack(() => promptChipSpecs),
    })
    composerRendered = key
    composerUndo.reset({ text: input, atts: attachments, caret: input.length })
    // Hydrate replaces <p> nodes — re-park caret inside the new paragraph or
    // WebKit leaves the selection on the root (high caret / first char outside).
    if (hadFocus) ensureComposerCaret(el, serializeComposer(el).length)
    queueMicrotask(() => {
      if (!composerEl) return
      if (!composerOnly) resizeComposerHeight(composerEl)
      if (document.activeElement === composerEl) {
        ensureComposerCaret(composerEl, serializeComposer(composerEl).length)
      }
    })
  })

  $effect(() => {
    void activeMenuHits
    if (mentionSel >= activeMenuHits.length) mentionSel = Math.max(0, activeMenuHits.length - 1)
  })

  // Re-arm the hover gate whenever the menu opens/closes or its rows change so
  // a stationary cursor never steals the keyboard default (index 0).
  $effect(() => {
    void mention
    void skillMention
    void promptArgs
    void activeMenuHits
    resetHoverGate(mentionHoverGate)
    if (composerOnly) queueMicrotask(placeComposerMentionMenu)
  })

  $effect(() => {
    void mentionSel
    void activeMenuHits
    queueMicrotask(() => {
      mentionListEl?.querySelector<HTMLElement>('.chat-mention-row.on')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function onMentionRowMouseMove(e: MouseEvent, i: number) {
    if (hoverGateAllows(mentionHoverGate, e.clientX, e.clientY)) mentionSel = i
  }

  function onMentionRowMouseEnter(i: number) {
    if (mentionHoverGate.armed) mentionSel = i
  }

  async function ensureSkillIndex() {
    if (skillIndex.length || skillIndexLoading) return
    skillIndexLoading = true
    const list = await call(
      CodeService.ListAllSkills(projectId, workspaceId),
      'List skills failed',
    )
    skillIndexLoading = false
    if (list) {
      skillIndex = list
        .filter((s) => s.userInvocable)
        .map((s) => ({
          name: s.name,
          description: s.description || '',
          scope: s.scope || '',
        }))
    }
  }

  async function ensurePromptIndex() {
    if (promptIndex.length || promptIndexLoading) return
    promptIndexLoading = true
    try {
      promptIndex = await listPromptActionsUnfiltered()
    } finally {
      promptIndexLoading = false
    }
  }

  function syncInputFromComposer(recordUndo = true) {
    const el = composerEl
    if (!el) return
    chipifyComposerText(el, attachments)
    // WebKit injects a trailing <br> after a chip when the space after it is
    // deleted — strip that so the next Backspace sees the chip, not a newline.
    // Also restore a leading space before file/skill chips so wire text keeps a
    // FILE_AT_RE boundary (deleting it must not demote chips on the next hydrate).
    normalizeComposerDom(el)
    ensureChipBoundariesInDom(el)
    const next = serializeComposer(el)
    // Backspace over an image chip drops its draft too.
    const alive = new Set(extractAttachmentIds(next))
    if (attachments.some((a) => !alive.has(a.id))) {
      attachments = attachments.filter((a) => alive.has(a.id))
    }
    skipComposerHydrate = true
    composerRendered = `${next}\n${attachments.map((a) => a.id).join('\0')}\n${promptChipSpecs.map((s) => s.slash).join(',')}`
    input = next
    if (recordUndo) composerUndo.record({ text: next, atts: attachments, caret: composerCaretOffset(el) })
    queueMicrotask(() => { skipComposerHydrate = false })
  }

  function applyComposerUndo(redo: boolean) {
    const el = composerEl
    const snap = redo ? composerUndo.redo() : composerUndo.undo()
    if (!el || !snap) return
    attachments = [...snap.atts]
    skipComposerHydrate = true
    hydrateComposer(el, snap.text, attachments, {
      chipFiles: true,
      chipSkills: true,
      terminalTitle: terminalTitleFor,
      artifactTitle: artifactTitleFor,
      artifactIdForTitle,
      promptChips: promptChipSpecs,
    })
    setComposerCaret(el, snap.caret)
    syncInputFromComposer(false)
    syncMentionFromCaret()
    layoutComposer()
  }

  function syncMentionFromCaret() {
    const el = composerEl
    if (!el || appSurface) {
      mention = null
      skillMention = null
      promptArgs = null
      return
    }
    // Chip wire forms must not reopen their menus when the caret is flush after.
    const chip = findPromptChip(el)
    if (chip) {
      mention = null
      skillMention = null
      promptGateOpen = false
      if (argsMenuEscaped) {
        promptArgs = null
        return
      }
      const slash = chip.dataset.promptSlash || ''
      const inputs = promptChipInputs(chip)
      const active = document.activeElement as HTMLInputElement
      const i = Math.max(0, inputs.indexOf(active))
      const args = (inputs[i]?.value ?? readPromptChipValues(chip).join(' ')).trimEnd()
      const arg = inputs[i]?.closest('[data-arg-name]')?.getAttribute('data-arg-name') || undefined
      if (!promptArgs || promptArgs.slash !== slash || promptArgs.args !== args || promptArgs.arg !== arg) mentionSel = 0
      promptArgs = { slash, args, argsStart: slash.length + 2, arg }
      return
    }
    if (isCaretAfterFileChip(el) || isCaretAfterSkillChip(el) || isCaretAfterTerminalChip(el) || isCaretAfterArtifactChip(el)) {
      mention = null
      skillMention = null
      promptArgs = null
      return
    }
    const caret = composerCaretOffset(el)
    const fileQ = mentionQueryAt(input, caret)
    const skillQ = skillQueryAt(input, caret)
    if (fileQ) {
      if (
        mentionEscaped?.kind === 'file' &&
        mentionEscaped.start === fileQ.start &&
        mentionEscaped.query === fileQ.query
      ) {
        mention = null
        skillMention = null
        promptArgs = null
        return
      }
      mentionEscaped = null
      skillMention = null
      promptArgs = null
      if (
        mention &&
        mention.query === fileQ.query &&
        mention.start === fileQ.start &&
        mention.end === fileQ.end
      ) {
        return
      }
      if (!mention || mention.query !== fileQ.query || mention.start !== fileQ.start) {
        mentionSel = 0
      }
      mention = fileQ
      return
    }
    mention = null
    if (skillQ) {
      if (
        mentionEscaped?.kind === 'skill' &&
        mentionEscaped.start === skillQ.start &&
        mentionEscaped.query === skillQ.query
      ) {
        skillMention = null
        promptGateOpen = false
        promptArgs = null
        return
      }
      mentionEscaped = null
      const promptQ = promptActionQueryAt(input, caret)
      promptGateOpen = !!promptQ && attachments.length === 0
      promptArgs = null
      if (
        skillMention &&
        skillMention.query === skillQ.query &&
        skillMention.start === skillQ.start &&
        skillMention.end === skillQ.end
      ) {
        return
      }
      if (!skillMention || skillMention.query !== skillQ.query || skillMention.start !== skillQ.start) {
        mentionSel = 0
        void ensureSkillIndex()
        if (promptGateOpen) void ensurePromptIndex()
      }
      skillMention = skillQ
      return
    }
    mentionEscaped = null
    skillMention = null
    promptGateOpen = false
    void ensurePromptIndex()
    const argsQ = promptArgsQueryAt(
      input,
      caret,
      (slash) => {
        const info = enabledPromptIndex.find((p) => p.slash === slash)
        if (info) return promptTakesArgs(info)
        return promptIndex.length === 0
      },
    )
    if (argsQ && attachments.length === 0) {
      if (argsMenuEscaped) {
        promptArgs = null
        return
      }
      if (!promptArgs || promptArgs.slash !== argsQ.slash || promptArgs.args !== argsQ.args) {
        mentionSel = 0
      }
      promptArgs = argsQ
      return
    }
    promptArgs = null
  }

  function placeComposerMentionMenu() {
    if (!composerOnly) return
    const host = composerEl?.closest('.chat-in') as HTMLElement | null
    if (!host || !composerEl) return
    if (!mention && !skillMention && !promptArgs) {
      host.style.removeProperty('--mention-top')
      return
    }
    const hostRect = host.getBoundingClientRect()
    const caret = composerCaretClientRect(composerEl)
    const lineBottom = caret?.bottom ?? hostRect.top + 40
    host.style.setProperty('--mention-top', `${Math.max(0, Math.round(lineBottom - hostRect.top + 4))}px`)
  }

  function layoutComposer() {
    queueMicrotask(() => {
      if (!composerEl) return
      if (!composerOnly) resizeComposerHeight(composerEl)
      scrollComposerCaretIntoView(composerEl)
      placeComposerMentionMenu()
    })
  }

  function onComposerInput() {
    argsMenuEscaped = false
    syncInputFromComposer()
    syncMentionFromCaret()
    layoutComposer()
  }

  export function focusComposer() {
    const el = composerEl
    if (!el) return
    el.focus()
    ensureComposerCaret(el)
  }

  function onComposerFocus() {
    const el = composerEl
    if (!el) return
    const ae = document.activeElement as HTMLElement | null
    if (ae?.classList.contains('chat-composer-prompt-arg-input')) return
    // Click/tab into an empty composer can land the caret on the root.
    ensureComposerCaret(el)
  }

  function pickMentionHit(hit: MentionHit) {
    if (hit.kind === 'terminal') pickTerminalMention(hit.id, hit.title)
    else if (hit.kind === 'artifact') pickArtifactMention(hit.id, hit.title)
    else pickMention(hit.dir ? dirFileMentionPath(hit.item) : hit.item)
  }

  function pickArtifactMention(id: string, title: string) {
    const el = composerEl
    const m = mention
    if (!el || !m) return
    const before = input.slice(0, m.start)
    const after = input.slice(m.end)
    const needsSpace = !after.startsWith(' ') && !after.startsWith('\n')
    skipComposerHydrate = true
    hydrateComposer(el, before + after, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    setComposerCaret(el, before.length)
    insertComposerChip(el, makeArtifactChip(id, title), { trailingSpace: needsSpace })
    mention = null
    skillMention = null
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => {
      focusComposer()
      skipComposerHydrate = false
    })
  }

  function pickTerminalMention(id: string, title: string) {
    const el = composerEl
    const m = mention
    if (!el || !m) return
    const before = input.slice(0, m.start)
    const after = input.slice(m.end)
    const needsSpace = !after.startsWith(' ') && !after.startsWith('\n')
    skipComposerHydrate = true
    hydrateComposer(el, before + after, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    setComposerCaret(el, before.length)
    insertComposerChip(el, makeTerminalChip(id, title), { trailingSpace: needsSpace })
    mention = null
    skillMention = null
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => {
      focusComposer()
      skipComposerHydrate = false
    })
  }

  function pickMention(path: string) {
    const el = composerEl
    const m = mention
    if (!el || !m) return
    // Explicit picker commit only — strip the @query, insert a chip at that spot.
    // Do not hydrate @paths from typed text (that auto-chipped prefixes like @app.c).
    const before = input.slice(0, m.start)
    const after = input.slice(m.end)
    const needsSpace = !after.startsWith(' ') && !after.startsWith('\n')
    skipComposerHydrate = true
    hydrateComposer(el, before + after, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    setComposerCaret(el, before.length)
    insertComposerChip(el, makeFileChip(path), { trailingSpace: needsSpace })
    mention = null
    skillMention = null
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => {
      focusComposer()
      skipComposerHydrate = false
    })
  }

  /**
   * Insert an @file chip at the caret (Cmd+L / “add current file”). Unlike
   * pickMention, does not require an in-progress @ query. Optional 1-based
   * loc becomes `@path:line` / `@path:start-end` / `@path:line:col-line:col`.
   * Ensures a whitespace boundary before the chip (and clears an in-progress
   * @/skill query) so the mention serializes as a real attach, not `…this@path`.
   */
  export function insertFileMention(path: string, startLine?: number, endLine?: number, startCol?: number, endCol?: number) {
    const el = composerEl
    const chip = formatFileMentionChip(path, startLine, endLine, startCol, endCol)
    if (!el || !chip || readonly) return
    el.focus()
    ensureComposerCaret(el)
    ensureChipBoundariesInDom(el)
    const text = serializeComposer(el)
    const caret = composerCaretOffset(el)
    // Same ambiguity as syncMentionFromCaret: a chip's wire form looks like an
    // in-progress @query when its trailing space is gone — keep the chip.
    const afterChip = isCaretAfterFileChip(el) || isCaretAfterSkillChip(el) || isCaretAfterTerminalChip(el) || isCaretAfterArtifactChip(el)
    const q = afterChip ? null : (mentionQueryAt(text, caret) ?? skillQueryAt(text, caret))
    skipComposerHydrate = true
    if (q) {
      // Strip @/skill query — hydrate, but repair so existing flush chips survive.
      const { before, after } = prepareFileMentionInsert(text, caret, { stripQuery: true })
      const caretAt = repairFileMentionBoundaries(before).length
      hydrateComposer(el, before + after, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
      setComposerCaret(el, caretAt)
    } else {
      // Surgical insert — do not rehydrate (that would plain-text chips that lost
      // their leading space in the serialized wire form).
      const before = text.slice(0, caret)
      if (before.length > 0 && !/\s$/.test(before)) {
        if (!document.execCommand('insertText', false, ' ')) {
          const sel = window.getSelection()
          if (sel?.rangeCount) {
            const range = sel.getRangeAt(0)
            const node = document.createTextNode(' ')
            range.insertNode(node)
            range.setStartAfter(node)
            range.collapse(true)
            sel.removeAllRanges()
            sel.addRange(range)
          }
        }
      }
    }
    const afterCaret = serializeComposer(el).slice(composerCaretOffset(el))
    const needsSpace = !afterCaret.startsWith(' ') && !afterCaret.startsWith('\n')
    insertComposerChip(el, makeFileChip(chip), { trailingSpace: needsSpace })
    mention = null
    skillMention = null
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => {
      focusComposer()
      skipComposerHydrate = false
    })
  }

  let fileDropOver = $state(false)

  function onChatDragEnter(e: DragEvent) {
    if (readonly || appSurface || !isComposerFileDrop(e.dataTransfer)) return
    e.preventDefault()
    fileDropOver = true
  }

  function onChatDragOver(e: DragEvent) {
    if (readonly || appSurface || !isComposerFileDrop(e.dataTransfer)) return
    e.preventDefault()
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
    fileDropOver = true
  }

  function onChatDragLeave(e: DragEvent) {
    if (!fileDropOver) return
    const next = e.relatedTarget as Node | null
    if (next && (e.currentTarget as HTMLElement).contains(next)) return
    fileDropOver = false
  }

  async function insertDroppedDrafts(added: ChatAttachmentDraft[]) {
    const el = composerEl
    if (!added.length || !el) return
    attachments = [...attachments, ...added]
    for (const att of added) insertComposerChip(el, makeAttachmentChip(att))
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => { focusComposer() })
  }

  async function applyOsDrop(paths: string[], imageFiles: File[]) {
    if (readonly || appSurface) return
    const { mentions, images, imagePaths, skipped } = classifyOsDrop({ paths, images: imageFiles, rootPath })
    if (skipped.length) {
      const name = skipped[0].split(/[\\/]/).pop() || skipped[0]
      pushToast('info', `Can't mention “${name}” — paths with spaces aren't supported yet`)
    }
    for (const path of mentions) insertFileMention(path)
    const fromBlobs = await draftsFromImageFiles(images)
    const fromPaths = await draftsFromImagePaths(imagePaths)
    if (imagePaths.length && !fromPaths.length) {
      pushToast('error', 'Could not attach dropped image')
    }
    await insertDroppedDrafts([...fromBlobs, ...fromPaths])
  }

  function onChatDrop(e: DragEvent) {
    const explorerPaths = fileDragPaths(e.dataTransfer)
    fileDropOver = false
    endFileDrag()
    if (readonly || appSurface) return
    if (explorerPaths.length) {
      e.preventDefault()
      e.stopPropagation()
      for (const path of explorerPaths) {
        if (!isChipableFilePath(path)) {
          const name = path.split(/[\\/]/).pop() || path
          pushToast('info', `Can't mention “${name}” — paths with spaces aren't supported yet`)
          continue
        }
        insertFileMention(path)
      }
      return
    }
    const paths = pathsFromOsDataTransfer(e.dataTransfer)
    const images = imageFilesFromDrop(e.dataTransfer)
    if (!paths.length && !images.length) return
    e.preventDefault()
    e.stopPropagation()
    void applyOsDrop(paths, images)
  }

  $effect(() => {
    if (readonly || appSurface) return
    const id = encodeChatDropId(resetKey)
    const off = Events.On('files.dropped', (e: { data?: unknown }) => {
      const { files, dropId: target } = filesFromWailsDrop(e?.data)
      if (!files.length) return
      if (target !== id) return
      void applyOsDrop(files, [])
    })
    return () => { off?.() }
  })

  function openFileChip(chipPath: string) {
    if (isDirChipPath(chipPath)) return
    const { path, startLine, endLine, startCol, endCol } = parseFileMentionChip(chipPath)
    if (!path) return
    onOpenFile?.(
      path,
      startLine && startLine > 0 ? startLine : undefined,
      endLine && startLine && endLine > startLine ? endLine : undefined,
      startCol && startCol > 0 ? startCol : undefined,
      endCol && endCol > 0 ? endCol : undefined,
    )
  }

  function openTerminalChip(id: string) {
    const tid = id || liveTerminals.find((x) => x.active)?.id || liveTerminals[0]?.id || ''
    if (!tid || !workspaceId) return
    setTerminalDockOpen(workspaceId, true)
    terminalFocus.set({ workspaceId, id: tid })
  }

  function openArtifactChip(id: string) {
    const a = id
      ? liveArtifacts.find((x) => x.id === id)
      : liveArtifacts[0]
    if (!a) {
      if (id) pushToast('info', 'Artifact not found')
      return
    }
    onOpenArtifact?.({
      id: a.id,
      title: a.title,
      kind: a.kind,
      path: a.path,
      createdAt: '',
      createdBy: '',
    })
  }

  function pickSkill(name: string) {
    const el = composerEl
    const m = skillMention
    if (!el || !m || !name) return
    const before = input.slice(0, m.start)
    const after = input.slice(m.end)
    const needsSpace = !after.startsWith(' ') && !after.startsWith('\n')
    skipComposerHydrate = true
    hydrateComposer(el, before + after, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    setComposerCaret(el, before.length)
    insertComposerChip(el, makeSkillChip(name), { trailingSpace: needsSpace })
    mention = null
    skillMention = null
    promptGateOpen = false
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => {
      focusComposer()
      skipComposerHydrate = false
    })
  }

  function enterPromptArgs(slash: string, args = '') {
    const el = composerEl
    if (!el) return
    skipComposerHydrate = true
    const rest = args.trim()
    const next = rest ? `/${slash} ${rest}` : `/${slash} `
    hydrateComposer(el, next, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    mention = null
    skillMention = null
    promptGateOpen = false
    argsMenuEscaped = false
    mentionEscaped = null
    syncInputFromComposer()
    syncMentionFromCaret()
    layoutComposer()
    queueMicrotask(() => {
      const chip = findPromptChip(el)
      if (chip) focusPromptChipArg(chip, 0)
      else {
        ensureComposerCaret(el, next.length)
        focusComposer()
      }
      skipComposerHydrate = false
    })
  }

  function promptChipArgs(slash: string, fallback: string): string {
    const chip = composerEl ? findPromptChip(composerEl) : null
    if (!chip) return fallback
    const wire = serializePromptChip(chip)
    const prefix = `/${slash}`
    return wire.startsWith(prefix) ? wire.slice(prefix.length).trim() : fallback
  }

  function applyPromptArgs(slash: string, args: string, invoke: boolean) {
    const el = composerEl
    if (!el || !onPromptAction) return
    const remainder = promptChipArgs(slash, args)
    if (invoke) {
      mention = null
      skillMention = null
      promptGateOpen = false
      promptArgs = null
      mentionEscaped = null
      nearBottom = true
      onPromptAction(slash, remainder)
      return
    }
    const next = remainder ? `/${slash} ${remainder}` : `/${slash} `
    skipComposerHydrate = true
    hydrateComposer(el, next, attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    mention = null
    skillMention = null
    promptGateOpen = false
    promptArgs = { slash, args: remainder, argsStart: slash.length + 2 }
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    queueMicrotask(() => {
      const chip = findPromptChip(el)
      if (chip) focusPromptChipArg(chip, 0)
      else {
        ensureComposerCaret(el, next.length)
        focusComposer()
      }
      skipComposerHydrate = false
    })
  }

  function pickPromptCompleteHit(hit: PromptCompleteHit, invoke: boolean) {
    const a = (hit.action ?? '').trim()
    if (a.startsWith('settings:')) {
      openSettings(a.slice('settings:'.length) as SettingsSection)
      return
    }
    const insert = (hit.insert || hit.label).trim()
    if (!insert) return
    const el = composerEl
    const chip = el ? findPromptChip(el) : null
    if (chip) {
      const inputs = promptChipInputs(chip)
      const active = document.activeElement as HTMLInputElement
      const i = Math.max(0, inputs.indexOf(active))
      const input = inputs[i]
      if (input) {
        input.value = insert
        input.dispatchEvent(new Event('input', { bubbles: true }))
        syncInputFromComposer()
        const values = readPromptChipValues(chip)
        if (!invoke || !promptRequiredArgsFilled(promptArgInfo, values)) {
          if (invoke) {
            const specs = promptArgSpecs(promptArgInfo)
            for (let step = 1; step < inputs.length; step++) {
              const idx = (i + step) % inputs.length
              if (!specs[idx]?.optional && !(values[idx] ?? '').trim()) {
                focusPromptChipArg(chip, idx)
                break
              }
            }
          }
          syncMentionFromCaret()
          layoutComposer()
          return
        }
      }
    }
    if (promptArgs) {
      const remainder = chip ? promptChipArgs(promptArgs.slash, insert) : insert
      const values = parsePromptChipValues(remainder, { slash: promptArgs.slash, args: promptArgSpecs(promptArgInfo) })
      applyPromptArgs(promptArgs.slash, insert, invoke && promptRequiredArgsFilled(promptArgInfo, values))
    }
  }

  function pickPrompt(slash: string) {
    const el = composerEl
    const m = skillMention
    if (!el || !m || !slash || !onPromptAction || promptBusy || sending) return
    const info = enabledPromptIndex.find((p) => p.slash === slash)
    // takesArgs always chips on menu pick. Optional empty-invoke is chip Enter / send.
    if (promptTakesArgs(info)) {
      const committed = parseCommittedPromptLine(input)
      const existingArgs = committed?.slash === slash ? committed.args : ''
      enterPromptArgs(slash, existingArgs)
      return
    }
    // Clear the /query — prompt actions invoke immediately (no chip).
    skipComposerHydrate = true
    hydrateComposer(el, '', attachments, {
      chipFiles: true, chipSkills: true, terminalTitle: terminalTitleFor, artifactTitle: artifactTitleFor,
      promptChips: promptChipSpecs,
    })
    // Hydrate replaces <p> nodes — re-park caret inside the empty paragraph or
    // WebKit leaves the selection on the root (high caret / first char outside).
    ensureComposerCaret(el, 0)
    mention = null
    skillMention = null
    promptGateOpen = false
    mentionEscaped = null
    syncInputFromComposer()
    layoutComposer()
    // Same as send(): prompt actions bypass onSend, so pin before the async
    // route/script posts the user bubble or it can land off-screen.
    nearBottom = true
    queueMicrotask(() => {
      focusComposer()
      skipComposerHydrate = false
    })
    onPromptAction(slash, '')
  }

  function onComposerClick(e: MouseEvent) {
    const t = e.target as HTMLElement | null
    const remove = t?.closest?.('[data-chip-remove]') as HTMLElement | null
    if (remove) {
      e.preventDefault()
      const chip = remove.closest('[data-file-path], [data-attachment-id], [data-skill-name], [data-terminal-chip], [data-artifact-chip], [data-prompt-slash]') as HTMLElement | null
      const attId = chip?.dataset.attachmentId
      chip?.remove()
      if (attId) attachments = attachments.filter((a) => a.id !== attId)
      syncInputFromComposer()
      mention = null
      skillMention = null
      focusComposer()
      layoutComposer()
      return
    }
    // File chip — open in the Observatory explorer (line span → jump to start).
    const fileChip = t?.closest?.('[data-file-path]') as HTMLElement | null
    if (fileChip?.dataset.filePath) {
      e.preventDefault()
      if (!fileChip.dataset.dir) openFileChip(fileChip.dataset.filePath)
      return
    }
    // Image chip — lightbox (same as the old above-input strip).
    const attChip = t?.closest?.('[data-attachment-id]') as HTMLElement | null
    if (attChip?.dataset.attachmentId) {
      e.preventDefault()
      const att = attachments.find((a) => a.id === attChip.dataset.attachmentId)
      if (att) openLightbox(att.dataUrl, att.name)
      return
    }
    const termChip = t?.closest?.('[data-terminal-chip]') as HTMLElement | null
    if (termChip) {
      e.preventDefault()
      const id = termChip.dataset.terminalId || liveTerminals.find((x) => x.active)?.id || liveTerminals[0]?.id || ''
      if (id && workspaceId) {
        setTerminalDockOpen(workspaceId, true)
        terminalFocus.set({ workspaceId, id })
      }
      return
    }
    const artChip = t?.closest?.('[data-artifact-chip]') as HTMLElement | null
    if (artChip) {
      e.preventDefault()
      openArtifactChip(artChip.dataset.artifactId || '')
      return
    }
    syncMentionFromCaret()
  }

  function onMentionKeydown(e: KeyboardEvent): boolean {
    if (promptArgs && !mention && !skillMention) {
      const hits = promptArgHits
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        if (hits.length) mentionSel = Math.min(mentionSel + 1, hits.length - 1)
        return true
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        mentionSel = Math.max(mentionSel - 1, 0)
        return true
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        argsMenuEscaped = true
        promptArgs = null
        return true
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        const hit = hits[mentionSel]
        if (!hit) {
          if (e.key === 'Tab') return false
          if (!promptArgs.args.trim()) {
            e.preventDefault()
            if (promptArgCanInvoke) applyPromptArgs(promptArgs.slash, '', true)
            return true
          }
          return false
        }
        if (hit.kind === 'suggest') {
          e.preventDefault()
          pickPromptCompleteHit(hit.hit, e.key === 'Enter')
          if (e.key === 'Tab') {
            const chip = composerEl ? findPromptChip(composerEl) : null
            if (chip) {
              const inputs = promptChipInputs(chip)
              const i = inputs.indexOf(document.activeElement as HTMLInputElement)
              if (i >= 0 && i < inputs.length - 1) focusPromptChipArg(chip, i + 1)
            }
          }
          return true
        }
        if (hit.kind === 'run') {
          if (e.key === 'Tab') {
            const chip = composerEl ? findPromptChip(composerEl) : null
            if (chip) {
              const inputs = promptChipInputs(chip)
              const i = inputs.indexOf(document.activeElement as HTMLInputElement)
              if (!hit.args && !promptArgAllowEmpty) {
                e.preventDefault()
                return true
              }
              e.preventDefault()
              if (i >= 0 && i < inputs.length - 1) focusPromptChipArg(chip, i + 1)
              return true
            }
            return false
          }
          if (!promptArgCanInvoke) {
            e.preventDefault()
            return true
          }
          e.preventDefault()
          applyPromptArgs(hit.slash, hit.args, true)
          return true
        }
      }
      return false
    }
    if (!mention && !skillMention) return false
    const hits = mention ? mentionHits : slashHits
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      if (hits.length) mentionSel = Math.min(mentionSel + 1, hits.length - 1)
      return true
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      mentionSel = Math.max(mentionSel - 1, 0)
      return true
    }
    if (e.key === 'Enter' || e.key === 'Tab') {
      if (mention) {
        const hit = mentionHits[mentionSel]
        if (hit) {
          e.preventDefault()
          pickMentionHit(hit)
          return true
        }
      } else {
        const hit = slashHits[mentionSel]
        if (hit) {
          e.preventDefault()
          const item = hit.item as SlashHit
          if (item.kind === 'prompt') pickPrompt(item.slash)
          else pickSkill(item.name)
          return true
        }
        const exact = skillMention
          ? enabledPromptIndex.find((p) => p.slash === skillMention!.query)
          : undefined
        if (exact && promptTakesArgs(exact) && (e.key === 'Tab' || e.key === 'Enter')) {
          e.preventDefault()
          pickPrompt(exact.slash)
          return true
        }
      }
      if (e.key === 'Tab') {
        mention = null
        skillMention = null
        promptGateOpen = false
        return false
      }
      // Enter while the menu is open but empty: dismiss — never fall through to Send.
      e.preventDefault()
      mention = null
      skillMention = null
      return true
    }
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      if (mention) {
        mentionEscaped = { kind: 'file', start: mention.start, query: mention.query }
      } else if (skillMention) {
        mentionEscaped = { kind: 'skill', start: skillMention.start, query: skillMention.query }
      }
      mention = null
      skillMention = null
      return true
    }
    return false
  }

  /** True after keydown already ate a chip — beforeinput must not delete a second. */
  let chipDeleteLocked = false

  function commitChipDelete(removed: HTMLElement) {
    const attId = removed.dataset.attachmentId
    if (attId) attachments = attachments.filter((a) => a.id !== attId)
    syncInputFromComposer()
    layoutComposer()
  }

  /** Backspace/Delete removes a whole adjacent chip in one keypress. */
  function onChipDeleteKey(e: KeyboardEvent): boolean {
    if (e.key !== 'Backspace' && e.key !== 'Delete') return false
    const t = e.target as HTMLElement | null
    if (t?.classList.contains('chat-composer-prompt-arg-input')) {
      const input = t as HTMLInputElement
      if (e.key !== 'Backspace') return false
      if (input.selectionStart !== 0 || input.selectionEnd !== 0) return false
      const chip = input.closest('[data-prompt-slash]') as HTMLElement | null
      if (!chip) return false
      const inputs = promptChipInputs(chip)
      const i = inputs.indexOf(input)
      if (input.value) return false
      e.preventDefault()
      if (i > 0) {
        focusPromptChipArg(chip, i - 1)
        return true
      }
      chip.remove()
      promptArgs = null
      syncInputFromComposer()
      layoutComposer()
      focusComposer()
      return true
    }
    const el = composerEl
    if (!el || mention || skillMention) return false
    const removed = deleteAdjacentComposerChip(el, e.key)
    if (!removed) return false
    e.preventDefault()
    chipDeleteLocked = true
    commitChipDelete(removed)
    queueMicrotask(() => { chipDeleteLocked = false })
    return true
  }

  /**
   * beforeinput backup: WebKit sometimes moves the caret on Backspace past a
   * contenteditable=false chip before keydown sees the original selection.
   */
  function onComposerBeforeInput(e: InputEvent) {
    const t = e.target as HTMLElement | null
    if (t?.classList.contains('chat-composer-prompt-arg-input')) return
    if (e.inputType === 'historyUndo' || e.inputType === 'historyRedo') {
      e.preventDefault()
      applyComposerUndo(e.inputType === 'historyRedo')
      return
    }
    if (chipDeleteLocked || mention || skillMention) return
    const el = composerEl
    if (!el) return
    const key = e.inputType === 'deleteContentBackward' ? 'Backspace'
      : e.inputType === 'deleteContentForward' ? 'Delete'
      : null
    if (!key) return
    const removed = deleteAdjacentComposerChip(el, key)
    if (!removed) return
    e.preventDefault()
    commitChipDelete(removed)
  }

  type Lightbox = { src: string; name: string }
  let lightbox = $state<Lightbox | null>(null)
  let imgCtx = $state<ImageCtxMenu | null>(null)

  function openLightbox(src: string, name: string) {
    if (!src) return
    lightbox = { src, name }
  }
  function closeLightbox() {
    lightbox = null
  }

  type BubblePart =
    | { kind: 'text'; text: string }
    | { kind: 'image'; id: string; src: string; name: string }
    | { kind: 'file'; path: string; dir?: boolean }
    | { kind: 'skill'; name: string }
    | { kind: 'terminal'; id: string }
    | { kind: 'artifact'; id: string }

  function splitInlineTokens(text: string): BubblePart[] {
    type Tok = { index: number; len: number; part: BubblePart }
    const toks: Tok[] = []
    const artRe = new RegExp(ARTIFACT_AT_RE.source, 'g')
    for (const match of text.matchAll(artRe)) {
      if (match.index == null) continue
      toks.push({ index: match.index, len: match[0].length, part: { kind: 'artifact', id: match[1] || '' } })
    }
    const termRe = new RegExp(TERMINAL_AT_RE.source, 'g')
    for (const match of text.matchAll(termRe)) {
      if (match.index == null) continue
      toks.push({ index: match.index, len: match[0].length, part: { kind: 'terminal', id: match[1] || '' } })
    }
    const fileRe = new RegExp(FILE_AT_RE.source, 'g')
    for (const match of text.matchAll(fileRe)) {
      if (match[1] == null || match.index == null) continue
      toks.push({ index: match.index, len: match[0].length, part: { kind: 'file', path: match[1], dir: isDirChipPath(match[1]) } })
    }
    const skillRe = new RegExp(SKILL_SLASH_RE.source, 'g')
    for (const match of text.matchAll(skillRe)) {
      if (match[1] == null || match.index == null) continue
      const slash = match[0].indexOf('/')
      const index = match.index + (slash >= 0 ? slash : 0)
      toks.push({ index, len: 1 + match[1].length, part: { kind: 'skill', name: match[1] } })
    }
    // Loose @Title last, after file/terminal/skill, so overlap matches composer hydrate.
    for (const hit of collectLooseAtNames(text)) {
      const id = artifactIdForTitle(hit.token)
      if (!id) continue
      if (tokenSpanOverlaps(hit.index, hit.len, toks)) continue
      toks.push({ index: hit.index, len: hit.len, part: { kind: 'artifact', id } })
    }
    toks.sort((a, b) => a.index - b.index)
    const parts: BubblePart[] = []
    let last = 0
    for (const tok of toks) {
      if (tok.index < last) continue
      if (tok.index > last) parts.push({ kind: 'text', text: text.slice(last, tok.index) })
      parts.push(tok.part)
      last = tok.index + tok.len
    }
    if (last < text.length) parts.push({ kind: 'text', text: text.slice(last) })
    if (parts.length === 0 && text) parts.push({ kind: 'text', text })
    return parts
  }

  function bubbleParts(m: AgentMsg): BubblePart[] {
    const body = m.body ?? ''
    const byId = new Map((m.attachments ?? []).map((a) => [a.id, a]))
    const parts: BubblePart[] = []
    let last = 0
    const re = new RegExp(ATTACHMENT_MARKER_RE.source, 'g')
    for (const match of body.matchAll(re)) {
      const idx = match.index ?? 0
      if (idx > last) parts.push(...splitInlineTokens(body.slice(last, idx)))
      const id = match[1]
      const att = byId.get(id)
      const src = att?.dataUrl || att?.url || ''
      if (src) parts.push({ kind: 'image', id, src, name: att?.name || id })
      else parts.push({ kind: 'text', text: match[0] })
      last = idx + match[0].length
    }
    if (last < body.length) parts.push(...splitInlineTokens(body.slice(last)))
    if (parts.length === 0 && body) parts.push(...splitInlineTokens(body))
    for (const a of m.attachments ?? []) {
      if (!body.includes(attachmentMarker(a.id))) {
        const src = a.dataUrl || a.url || ''
        if (src) parts.push({ kind: 'image', id: a.id, src, name: a.name || a.id })
      }
    }
    return parts
  }

  async function onPaste(e: ClipboardEvent) {
    if (appSurface) {
      // Plain text only — no image attachments on Ask Recur.
      const images = imageFilesFromPaste(e)
      if (images.length) e.preventDefault()
    }
    const images = appSurface ? [] : imageFilesFromPaste(e)
    if (images.length) {
      e.preventDefault()
      await insertDroppedDrafts(await draftsFromImageFiles(images))
      return
    }
    // Contenteditable otherwise keeps source HTML/styles — force plain text.
    e.preventDefault()
    const text = (e.clipboardData?.getData('text/plain') ?? '').replace(/\r\n?/g, '\n')
    if (!text) return
    if (!document.execCommand('insertText', false, text)) {
      const sel = window.getSelection()
      if (!sel?.rangeCount || !composerEl) return
      const range = sel.getRangeAt(0)
      range.deleteContents()
      const node = document.createTextNode(text)
      range.insertNode(node)
      range.setStartAfter(node)
      range.collapse(true)
      sel.removeAllRanges()
      sel.addRange(range)
    }
    onComposerInput()
    // Long pastes leave the caret below the fold — scroll after layout.
    requestAnimationFrame(() => {
      if (composerEl) scrollComposerCaretIntoView(composerEl)
    })
  }

  // Follow-ups wait in a strip above the composer — not in the scrollable transcript.
  const queuedMessages = $derived(messages.filter((m) => m.queued && m.me))
  const transcriptMessages = $derived(messages.filter((m) => !m.queued))
  const cockpitStartTs = $derived(
    transcriptMessages.reduce((min, m) => {
      const t = m.ts || 0
      // ts: 1 is the synthetic opening-prompt row — not a real loop start.
      return t > 1 && t < min ? t : min
    }, Number.POSITIVE_INFINITY),
  )

  // Retry only on the trailing error — later user/assistant text means the
  // turn already continued, so a historical failure card stays read-only.
  const lastRetryErrorIndex = $derived.by(() => {
    if (readonly || sending || !onRetry) return -1
    const msgs = transcriptMessages
    for (let i = msgs.length - 1; i >= 0; i--) {
      const m = msgs[i]
      if (m.me) return -1
      if (m.error) return i
      if (m.log || m.turnEnd || m.tool || m.thinking || m.compact || m.note || m.fallback || m.stopped) continue
      if (m.body) return -1
    }
    return -1
  })

  // ---- transcript grouping: contiguous steps (tools + thinking) form one block ----
  type Row = { m: AgentMsg; i: number }
  type Block = { kind: 'msg'; m: AgentMsg; i: number } | { kind: 'steps'; start: number; rows: Row[] }
  const blocks = $derived.by<Block[]>(() => {
    const out: Block[] = []
    const msgs = transcriptMessages
    let steps: Extract<Block, { kind: 'steps' }> | null = null
    msgs.forEach((m, i) => {
      if (m.log) {
        steps = null
        out.push({ kind: 'msg', m, i })
      } else if (m.tool && isSubagentCreate(m)) {
        return
      } else if (
        ((m.tool || m.thinking || m.compact || m.fallback) && !m.turnEnd) ||
        foldSubagentNotice(m, hasLaterStep(msgs, i))
      ) {
        if (steps) steps.rows.push({ m, i })
        else {
          steps = { kind: 'steps', start: i, rows: [{ m, i }] }
          out.push(steps)
        }
      } else if (m.me || m.turnEnd || m.note || m.stopped || m.error) {
        steps = null
        out.push({ kind: 'msg', m, i })
      } else if (steps && foldInnerNarration(m, hasLaterStep(msgs, i))) {
        steps.rows.push({ m, i })
      } else {
        out.push({ kind: 'msg', m, i })
      }
    })
    return out
  })

  function blockKey(b: Block): string {
    if (b.kind === 'steps') {
      const first = b.rows[0]?.m
      const live = b.rows.some((r) => r.m.streaming || r.m.resultPending)
      const last = blocks[blocks.length - 1] === b
      return transcriptBlockKey(
        resetKey,
        transcriptStepBlockLocal(b.start, first, live && last),
      )
    }
    const m = b.m
    if (m.log?.kind === 'session' || m.log?.kind === 'sessions') {
      return transcriptBlockKey(resetKey, transcriptSessionBlockLocal(m.log))
    }
    if (m.log?.kind === 'fact') return transcriptBlockKey(resetKey, `fact:${m.log.id || `${m.ts}:${m.log.text.slice(0, 48)}`}`)
    if (m.pendingId) return transcriptBlockKey(resetKey, `q:${m.pendingId}`)
    if (m.streaming) return transcriptBlockKey(resetKey, m.thinking ? 'm:live-think' : 'm:live-text')
    return transcriptBlockKey(resetKey, transcriptMessageBlockLocal(b.i, m))
  }
  const blockKeys = $derived(uniqueBlockKeys(blocks.map(blockKey)))

  const windowChat = $derived(blocks.length > CHAT_HYDRATE_ALL_BELOW)
  let chatHydrated = $state.raw<Set<string>>(new Set())
  let chatInView = $state.raw<string[]>([])
  const blockEls = new Map<string, HTMLElement>()
  const blockHByChat = new Map<string, Map<string, number>>()
  const blockH = $derived.by(() => {
    const key = resetKey
    let m = blockHByChat.get(key)
    if (!m) {
      m = new Map()
      restoreBlockHeights(m, loadTranscriptUI(key)?.blockHeights)
      blockHByChat.set(key, m)
    }
    return m
  })
  let chatPinGen = 0

  function blockEstimate(b: Block): number {
    if (b.kind === 'steps') return estimateChatBlockHeight({ kind: 'steps' })
    return estimateChatBlockHeight({ kind: 'msg', me: b.m.me, turnEnd: b.m.turnEnd, body: b.m.body })
  }

  function blockRanges() {
    const root = chatScrollEl
    if (!root) return []
    const pr = root.getBoundingClientRect()
    return [...blockEls.entries()]
      .map(([path, el]) => {
        const r = el.getBoundingClientRect()
        const top = root.scrollTop + (r.top - pr.top)
        return { path, top, bottom: top + el.offsetHeight }
      })
      .sort((a, b) => a.top - b.top)
  }

  function registerChatBlock(key: string) {
    return (el: HTMLElement) => {
      blockEls.set(key, el)
      const stop = recordStubHeight(el, key, blockH, pinBottom)
      return () => {
        stop()
        blockEls.delete(key)
      }
    }
  }

  function refreshChatWindow() {
    const root = chatScrollEl
    if (!root || !windowChat) return
    const keys = blockKeys
    const ranges = blockRanges()
    const pin = captureViewportPin(ranges, root.scrollTop, root.clientHeight, root.scrollHeight)
    const stick = nearBottom
    const nextInView = inViewPaths(ranges, root.scrollTop, root.clientHeight, CHAT_VIEW_PAD_PX)
    const selected = stick
      ? (keys[keys.length - 1] ?? '')
      : (fileAtScrollTop(ranges, root.scrollTop) || keys[keys.length - 1] || '')
    const nextHydrated = pickHydrated(keys, selected, nextInView, CHAT_HYDRATE_MAX, CHAT_HYDRATE_OVERSCAN)
    if (revealKey && blockEls.has(revealKey)) nextHydrated.add(revealKey)
    const swapped = !samePathSet(chatHydrated, nextHydrated)
    if (!samePathList(chatInView, nextInView)) chatInView = nextInView
    if (swapped) chatHydrated = nextHydrated
    if (!swapped) return
    const capturedTop = root.scrollTop
    const token = ++chatPinGen
    void tick().then(() => {
      requestAnimationFrame(() => {
        if (token !== chatPinGen) return
        applyViewportPin(root, blockRanges(), { ...pin, atEnd: stick }, capturedTop)
      })
    })
  }

  // Expansion state. Groups default collapsed — except the first thinking window.
  // After that a live turn stays collapsed like a settled one; the current step
  // stays visible as a one-line row. Rows default collapsed, click to expand.
  // Seeded from the per-key cache so a remount paints the last expand/collapse
  // instead of flashing the defaults and restoring a frame later.
  const initialUI = untrack(() => (resetKey ? claimTranscriptUI(resetKey) : undefined))
  let openGroups = $state<Record<number, boolean>>(initialUI?.openGroups ?? {})
  let openRows = $state<Record<string | number, boolean>>(initialUI?.openRows ?? {})
  /** Drop restored expand maps if a refetch rewrites the list; cleared on toggle. */
  let expansionGuard = $state<Pick<ChatTranscriptUIState, 'rowCount' | 'fingerprint'> | null>(
    initialUI && (initialUI.rowCount || initialUI.fingerprint) ? initialUI : null,
  )
  function groupOpen(b: Extract<Block, { kind: 'steps' }>, isLast: boolean): boolean {
    return liveGroupOpen(openGroups[b.start], isLast && firstCheckpoint)
  }
  function toggleGroup(b: Extract<Block, { kind: 'steps' }>, isLast: boolean) {
    expansionGuard = null
    openGroups = { ...openGroups, [b.start]: !groupOpen(b, isLast) }
    persistTranscriptUI()
  }
  function toggleRow(key: string | number, visualOpen = !!openRows[key]) {
    expansionGuard = null
    openRows = { ...openRows, [key]: !visualOpen }
    persistTranscriptUI()
  }
  function rowIsOpen(key: string | number, fallback = false): boolean {
    return key in openRows ? !!openRows[key] : fallback
  }
  function hasLiveChildren(m: AgentMsg): boolean {
    return (m.children ?? []).some((c) => c.streaming || c.resultPending || hasLiveChildren(c))
  }
  function groupSummary(rows: Row[], fileRows = rows): string {
    const tools = rows.filter((r) => r.m.tool).length
    const compacts = rows.filter((r) => r.m.compact).length
    const fallbacks = rows.filter((r) => r.m.fallback).length
    const thoughts = rows.filter((r) => r.m.thinking).length
    const files = new Set<string>()
    for (const r of fileRows) {
      const f = fileOf(r.m)
      if (f) files.add(f)
    }
    const parts: string[] = []
    if (tools) parts.push(`${tools} tool call${tools === 1 ? '' : 's'}`)
    if (thoughts) parts.push(`${thoughts} thought${thoughts === 1 ? '' : 's'}`)
    if (files.size) parts.push(`${files.size} file${files.size === 1 ? '' : 's'}`)
    if (compacts) parts.push(compacts === 1 ? 'compaction' : `${compacts} compactions`)
    if (fallbacks) parts.push(fallbacks === 1 ? 'model fallback' : `${fallbacks} model fallbacks`)
    return parts.join(' · ') || 'steps'
  }

  // ---- per-tool presentation ----
  function parsedInput(m: AgentMsg): Record<string, any> | null {
    if (!m.input) return null
    try {
      const v = JSON.parse(m.input)
      return v && typeof v === 'object' ? v : null
    } catch {
      return null // truncated / non-JSON input — the card falls back to raw text
    }
  }
  function linearViewOf(m: AgentMsg): LinearView | null {
    if (m.resultError || m.resultPending || !m.result) return null
    return parseLinearResult(m.result)
  }
  function baseName(p: string): string {
    const i = p.lastIndexOf('/')
    return i >= 0 ? p.slice(i + 1) : p
  }
  // relPath strips the worktree root from a tool's absolute path for display.
  function relPath(p: string): string {
    if (rootPath && p.startsWith(rootPath)) {
      const rel = p.slice(rootPath.length).replace(/^\//, '')
      return rel || '.'
    }
    return p
  }
  function fileOf(m: AgentMsg): string {
    // update_finding only has finding_id in args — resolve path from store.
    if (isUpdateFinding(m)) return resolveUpdateFinding(m)?.file || ''
    const p = parsedInput(m)
    // Read/Edit/Write use file_path; report_finding uses file.
    const fp = p?.file_path ?? p?.file
    return typeof fp === 'string' ? fp : ''
  }
  // toolMeta is the dim header summary; Grep/Glob rebuild it with relative paths
  // (the backend summary carries the absolute worktree path).
  function toolMeta(m: AgentMsg): string {
    const p = parsedInput(m)
    if (m.tool === 'TodoWrite') {
      // The label already carries "· done/total", so the meta shows only what is
      // currently running (and nothing at all when nothing is in flight).
      return todoItems(m).find((t) => t.status === 'in_progress')?.content ?? ''
    }
    if (isCreatePlan(m)) {
      // Collapsed row: first non-empty line of the plan, not a JSON dump.
      const line = planBody(m).split('\n').find((l) => l.trim())
      return line?.trim() ?? ''
    }
    if (isSubmitPlan(m)) {
      // Summary preview only — step list lives in the card body.
      return submitPlanSummary(m).replace(/\s+/g, ' ').trim()
    }
    if (isUpdatePlanStep(m)) {
      // Status is a chip on the row — meta is id + title/note, never JSON.
      // (Activity-feed summarizeToolInput still uses "S2 → done" via updatePlanStepMeta.)
      const id = updatePlanStepId(m)
      const title = updatePlanStepTitle(m)
      const note = updatePlanStepNote(m).replace(/\s+/g, ' ').trim()
      const parts: string[] = []
      if (id) parts.push(id)
      if (title) parts.push(title)
      else if (note) parts.push(note)
      return parts.join(' · ') || updatePlanStepMeta(id, updatePlanStepStatus(m), note)
    }
    if (isEditPlan(m)) {
      return prosePreview(editPlanText(m))
    }
    if (isClaudeTaskTool(m.tool)) {
      // Subject — never the truncated JSON dump of {subject,description,…}.
      // #id is a chip on the row; status is a chip on TaskUpdate / TaskGet.
      return claudeTaskMeta(m.tool!, claudeTaskArgsOf(m), m.result ?? '')
    }
    if (isSubagentTool(m.tool) || isTaskTool(m.tool)) {
      // Type + background are chips on the row — never a JSON dump of the prompt.
      return ''
    }
    if (isSendMessageTool(m.tool)) {
      const a = parseSendMessageArgs(parsedInput(m))
      return sendMessageMeta(a.description || m.description || '', a.message)
    }
    if (isReportFinding(m) || isUpdateFinding(m)) {
      // Message preview (and path when there's no file chip). Severity / status
      // are chips on the row — not restated here.
      const file = findingFile(m)
      const loc = findingLocSuffix(m)
      const msg = findingMessage(m).replace(/\s+/g, ' ').trim()
      const parts: string[] = []
      if (!onOpenFile && file) parts.push(relPath(file) + loc)
      else if (!onOpenFile && loc) parts.push(`line ${loc.slice(1)}`)
      if (msg) parts.push(msg)
      return parts.join(' · ')
    }
    if (isListFindings(m)) {
      const listed = listFindingsResult(m)
      if (listed) {
        const n = listed.count
        const st = listed.status || 'open'
        return n === 0 ? `no ${st}` : `${n} ${st}`
      }
      const st = parsedInput(m)?.status
      return typeof st === 'string' && st ? st : 'open'
    }
    if (isListArtifacts(m)) {
      const listed = listArtifactsResult(m)
      if (listed) {
        const n = listed.count
        return n === 0 ? 'none' : `${n}`
      }
      return ''
    }
    if (isDone(m)) return doneSummary(m).replace(/\s+/g, ' ').trim()
    if (isAskUser(m)) {
      // Park design: tool result is a short ack; the answer is a later user message.
      return askUserQuestion(m).replace(/\s+/g, ' ').trim()
    }
    if (isSwitchMode(m)) {
      // Never dump {"mode":…} JSON — transition lives in the row label.
      return ''
    }
    if (isRenameTool(m)) {
      // Title/name lives in the row label / chip — not raw JSON.
      return ''
    }
    if (isChildTool(m)) {
      // Child id lives in the row label; the report body is for the model.
      return ''
    }
    if (isOpenPR(m)) {
      // Title lives in the row; never dump {"title","body","draft"} JSON.
      return ''
    }
    if (isReenterPhase(m)) {
      // Phase lives in the row label / chip — never dump {"phase_id":…} JSON.
      return ''
    }
    if (isCreateArtifact(m) || isPresentArtifact(m)) {
      // Never dump title/kind/content JSON — row carries the label.
      return ''
    }
    if (isBriefingTool(m.tool)) {
      // Cards carry identity; never dump the briefing JSON blob.
      if (isListWorkspaceBriefs(m)) {
        const listed = listWorkspaceBriefsResult(m)
        return listed ? briefingCountsMeta(listed.counts) : ''
      }
      if (isGetWorkspaceContext(m)) {
        return workspaceContextResult(m)?.brief.now || ''
      }
      const card = presentWorkspaceResult(m)
      return card?.brief?.now || card?.target || ''
    }
    if (isReadLints(m)) {
      const lint = readLintsResult(m)
      if (lint) {
        const n = lint.totalDiagnostics
        const files = lint.totalFiles
        if (n === 0) return files ? `${files} file${files === 1 ? '' : 's'} · clean` : 'clean'
        return `${n} issue${n === 1 ? '' : 's'} · ${files} file${files === 1 ? '' : 's'}`
      }
      const paths = readLintsPaths(m)
      if (paths.length) {
        return paths.length === 1
          ? relPath(paths[0])
          : `${paths.length} files`
      }
      return ''
    }
    if (m.tool === 'ToolSearch') {
      const q = toolSearchQuery(m)
      const selects = toolSearchSelects(q)
      if (selects?.length) return selects.map(prettyToolName).join(', ')
      return q
    }
    if (m.tool === 'Delete') {
      // Label + file chip already name the file. Never dump {"file_path":…} —
      // Cursor used to summarize Delete as truncated JSON, and reloaded Codex
      // history hit the same path. A bare path (no parseable input) can still
      // show when there is no chip.
      if (fileOf(m)) return ''
      const ti = (m.toolInput ?? '').trim()
      return ti.startsWith('{') ? '' : ti
    }
    if (m.tool === 'WebFetch') {
      // URL lives in the header chip — never dump {"url","prompt"} JSON.
      if (fetchUrl(m)) return ''
      const ti = (m.toolInput ?? '').trim()
      return ti.startsWith('{') ? '' : ti
    }
    if (m.tool === 'WebSearch') {
      const q = searchQuery(m)
      return q ? `"${q}"` : ''
    }
    if (isLinearTool(m.tool)) {
      const args = parseLinearArgs(p)
      const view = linearViewOf(m)
      const label = linearToolLabel({
        tool: m.tool!,
        pending: !!m.resultPending,
        failed: !!m.resultError,
        args,
        view,
      })
      return linearToolMeta(args, view, label)
    }
    if ((m.tool === 'Grep' || m.tool === 'Glob') && typeof p?.pattern === 'string') {
      const path = typeof p.path === 'string' && p.path ? relPath(p.path) : ''
      return path ? `"${p.pattern}" in ${path}` : `"${p.pattern}"`
    }
    return m.toolInput ?? ''
  }
  function fetchUrl(m: AgentMsg): string {
    return webFetchUrl(parsedInput(m), m.toolInput)
  }
  function fetchPrompt(m: AgentMsg): string {
    return webFetchPrompt(parsedInput(m), m.toolInput)
  }
  function searchQuery(m: AgentMsg): string {
    return webSearchQuery(parsedInput(m), m.toolInput)
  }
  // Expand only when there is a prompt, page body, or error to show — the URL
  // chip already names the page.
  function webHasCard(m: AgentMsg): boolean {
    if (m.resultPending || m.resultError || (m.result ?? '').trim()) return true
    return m.tool === 'WebFetch' && !!fetchPrompt(m)
  }
  // grepFlags renders a Grep call's options as terse chips (-i, glob, mode, …).
  function grepFlags(p: Record<string, any>): string[] {
    const out: string[] = []
    for (const k of ['-i', '-n', '-A', '-B', '-C', 'multiline', 'head_limit']) {
      const v = p[k]
      if (v === true) out.push(k)
      else if (typeof v === 'number') out.push(`${k} ${v}`)
    }
    if (typeof p.glob === 'string' && p.glob) out.push(`glob ${p.glob}`)
    if (typeof p.type === 'string' && p.type) out.push(`type ${p.type}`)
    // output_mode is plumbing (files_with_matches/content), not worth a chip.
    return out
  }
  // toolLabel is the row title: the agent's own description when it wrote one
  // (Claude Code sets input.description on Bash/Task), else a derived verb.
  function toolLabel(m: AgentMsg): string {
    if (isClaudeTaskTool(m.tool)) {
      return claudeTaskLabel(m.tool!, !!m.resultPending, claudeTaskArgsOf(m), claudeTaskListCounts(m))
    }
    if (isSendMessageTool(m.tool)) {
      const a = parseSendMessageArgs(parsedInput(m))
      return sendMessageLabel(!!m.resultPending, a.description || m.description || '')
    }
    if (m.description) return m.description
    if (isLinearTool(m.tool)) {
      return linearToolLabel({
        tool: m.tool!,
        pending: !!m.resultPending,
        failed: !!m.resultError,
        args: parseLinearArgs(parsedInput(m)),
        view: linearViewOf(m),
      })
    }
    // A call still in flight reads in the present tense, so a collapsed row does
    // not claim work that has not finished ("Edited game.go" while editing).
    if (m.resultPending || (isTaskTool(m.tool) && isSubagentLaunchMeta(m.result) && !m.resultError)) {
      return pendingLabel(m).replace(/…$/, '')
    }
    const f = fileOf(m)
    switch (m.tool) {
      case 'Bash': return 'Ran command'
      case 'Read': return f ? `Read ${baseName(f)}` : 'Read file'
      case 'Edit':
        // Legacy Codex creates were stored as Edit with the file body in result.
        if (hasWriteBody(m) && !hasEditDiff(m)) return f ? `Wrote ${baseName(f)}` : 'Wrote file'
        return f ? `Edited ${baseName(f)}` : 'Edited file'
      case 'Write': return f ? `Wrote ${baseName(f)}` : 'Wrote file'
      case 'Grep': return 'Grep'
      case 'Glob': return 'Glob'
      case 'LSP': return 'Queried language server'
      case 'WebFetch': return 'Fetched a page'
      case 'WebSearch': return 'Searched the web'
      case 'Task':
      case 'task':
      case 'Agent':
        return m.resultPending || (isSubagentLaunchAck(m.result ?? '') && !m.resultError)
          ? 'Running a subagent'
          : 'Ran a subagent'
      case 'NotebookEdit': return 'Edited notebook'
      case 'ToolSearch':
        // select:… is a direct fetch; anything else is a keyword search.
        return /^select:/i.test(toolSearchQuery(m)) ? 'Selected tools' : 'Searched tools'
      // Cursor-only tools, mapped to Recur's vocabulary in engine/cursortools.go.
      case 'Delete': return f ? `Deleted ${baseName(f)}` : 'Deleted file'
      case 'TodoWrite': {
        // The row header carries the progress; the card carries the checklist.
        const c = todoCounts(todoItems(m))
        return c.total ? `Updated todos · ${c.done}/${c.total}` : 'Updated todos'
      }
      case 'CreatePlan':
      case 'createPlan':
        return 'Wrote plan'
      case 'readLints':
      case 'ReadLints':
        return m.resultPending ? 'Reading lints' : 'Read lints'
    }
    if (isReportFinding(m)) {
      return m.resultPending ? 'Reporting finding' : 'Reported finding'
    }
    if (isListFindings(m)) {
      return m.resultPending ? 'Listing findings' : 'Listed findings'
    }
    if (isUpdateFinding(m)) {
      // Same shape as report_finding ("Reported finding") — status lives in a
      // chip; file:line is the header file chip, not the title.
      const st = resolveUpdateFinding(m)?.status
      if (m.resultPending) return 'Updating finding'
      if (st === 'resolved') return 'Marked resolved'
      if (st === 'invalid') return 'Marked invalid'
      return 'Updated finding'
    }
    if (isDone(m)) return m.resultPending ? 'Wrapping up' : 'Turn complete'
    if (isAskUser(m)) {
      if (m.resultError) return 'Ask failed'
      return m.resultPending ? 'Asking' : 'Asked'
    }
    if (isSwitchMode(m)) {
      if (m.resultError) return 'Mode switch failed'
      return m.resultPending ? 'Switching mode' : 'Switched mode'
    }
    if (isRenameTool(m)) {
      if (m.resultError || renameToolFailed(m)) return 'Rename failed'
      const t = renameToolValue(m)
      const noun = renameToolNoun(m)
      if (m.resultPending) return t ? `Renaming ${noun} to ${t}` : `Renaming ${noun}`
      return t ? `Renamed ${noun} to ${t}` : `Renamed ${noun}`
    }
    if (isChildTool(m)) return childToolLabel(m)
    if (isOpenPR(m)) return openPRLabel(openPRCard(m))
    if (isReenterPhase(m)) {
      return reenterPhaseLabel({
        pending: !!m.resultPending,
        failed: m.resultError || reenterPhaseFailed(m.result ?? ''),
        phase: reenterPhaseID(m),
      })
    }
    if (isListArtifacts(m)) {
      return m.resultPending ? 'Listing artifacts' : 'Listed artifacts'
    }
    if (isCreateArtifact(m)) {
      if (m.resultError || createArtifactFailed(m)) return 'Create artifact failed'
      const t = createArtifactTitle(m)
      if (m.resultPending) return t ? `Creating ${t}` : 'Creating artifact'
      return t ? `Created ${t}` : 'Created artifact'
    }
    if (isPresentArtifact(m)) {
      if (m.resultError || presentArtifactFailed(m)) return 'Present artifact failed'
      const t = presentArtifactTitle(m)
      if (m.resultPending) return t ? `Presenting ${t}` : 'Presenting artifact'
      return t ? `Presented ${t}` : 'Presented artifact'
    }
    if (isListWorkspaceBriefs(m)) return listWorkspaceBriefsLabel(!!m.resultPending, listWorkspaceBriefsResult(m))
    if (isGetWorkspaceContext(m)) {
      const ctx = workspaceContextResult(m)
      const title = ctx?.brief.title || presentWorkspaceInputId(m)
      return getWorkspaceContextLabel(!!m.resultPending, title, !!m.resultError)
    }
    if (isPresentWorkspace(m)) return presentWorkspaceLabel(!!m.resultPending, presentWorkspaceResult(m))
    if (isSubmitPlan(m)) return submitPlanLabel(!!m.resultPending, submitPlanSteps(m).length)
    if (isUpdatePlanStep(m)) return updatePlanStepLabel(!!m.resultPending)
    if (isEditPlan(m)) return editPlanLabel(!!m.resultPending)
    if (m.tool?.startsWith('mcp__')) {
      const name = m.tool.split('__').pop() ?? m.tool
      return name.replaceAll('_', ' ')
    }
    return m.tool ?? 'Tool'
  }
  type ToolIcon = 'term' | 'file' | 'pencil' | 'trash' | 'search' | 'globe' | 'bot' | 'todo' | 'plan' | 'finding' | 'done' | 'tools' | 'artifact' | 'plug' | 'workspace' | 'linear'
  function toolIcon(m: AgentMsg): ToolIcon {
    if (isDone(m)) return 'done'
    if (isAskUser(m)) return 'bot'
    if (isSwitchMode(m)) return 'plan'
    if (isOpenPR(m)) return 'file'
    if (isReportFinding(m) || isListFindings(m) || isUpdateFinding(m)) return 'finding'
    if (isListArtifacts(m) || isCreateArtifact(m) || isPresentArtifact(m)) return 'artifact'
    if (isBriefingTool(m.tool)) return 'workspace'
    if (isLinearTool(m.tool)) return 'linear'
    if (isSubmitPlan(m) || isUpdatePlanStep(m) || isEditPlan(m)) return 'plan'
    switch (m.tool) {
      case 'Bash': return 'term'
      case 'Read': return 'file'
      case 'Edit': case 'Write': case 'NotebookEdit': return 'pencil'
      case 'Delete': return 'trash'
      case 'TodoWrite': return 'todo'
      case 'CreatePlan': case 'createPlan': return 'plan'
      case 'Grep': case 'Glob': case 'LSP': case 'readLints': case 'ReadLints': return 'search'
      case 'WebFetch': case 'WebSearch': return 'globe'
      case 'Task': case 'task': case 'Agent': case 'SendMessage':
      case 'TaskCreate': case 'TaskUpdate': case 'TaskGet': case 'TaskList':
        return 'bot'
      case 'ToolSearch': return 'tools'
    }
    return 'plug'
  }

  // ---- Task / Agent (subagent) ----
  // Both providers share {description, prompt, subagent_type?, model?} once
  // normalized. Description is the row title; the card shows the prompt.
  // Claude Code renamed Task → Agent in v2.1.63; isSubagentTool accepts both.
  function subagentOf(m: AgentMsg) {
    return parseSubagentArgs(parsedInput(m))
  }
  function taskSubagentType(m: AgentMsg): string {
    return subagentOf(m).subagentType
  }
  function taskModel(m: AgentMsg): string {
    return subagentOf(m).model
  }
  function taskBackground(m: AgentMsg): boolean {
    return subagentOf(m).background || isSubagentLaunchAck(m.result ?? '')
  }
  function claudeTaskArgsOf(m: AgentMsg) {
    return parseClaudeTaskArgs(parsedInput(m))
  }
  function claudeTaskListItems(m: AgentMsg): ClaudeTaskView[] | null {
    return parseTaskList(m.result ?? '')
  }
  function claudeTaskListCounts(m: AgentMsg): { done: number; total: number } | undefined {
    if (m.tool !== 'TaskList' || m.resultPending || m.resultError) return undefined
    const items = claudeTaskListItems(m)
    if (!items?.length) return undefined
    const c = claudeTaskCounts(items)
    return c.total ? c : undefined
  }
  function claudeTaskGetView(m: AgentMsg): ClaudeTaskView | null {
    if (m.resultError) return null
    return parseTaskGet(m.result ?? '')
  }
  function claudeTaskRowId(m: AgentMsg): string {
    const args = claudeTaskArgsOf(m)
    if (args.taskId) return formatTaskId(args.taskId)
    if (m.tool === 'TaskCreate') return formatTaskId(parseTaskCreateId(m.result ?? ''))
    if (m.tool === 'TaskGet') return formatTaskId(claudeTaskGetView(m)?.id ?? '')
    return ''
  }
  // ---- ToolSearch ----
  // Deferred-tool discovery: query is either `select:Name[,Name…]` or keywords.
  // The card shows selected names as chips (or the query like Grep) — never the
  // raw {query,max_results} JSON dump.
  function toolSearchQuery(m: AgentMsg): string {
    const q = parsedInput(m)?.query
    return typeof q === 'string' ? q : ''
  }
  function toolSearchMax(m: AgentMsg): number {
    const n = parsedInput(m)?.max_results
    return typeof n === 'number' && n > 0 ? n : 0
  }
  function toolSearchSelects(query: string): string[] | null {
    const m = /^select:(.+)$/i.exec(query)
    if (!m) return null
    return m[1].split(',').map((s) => s.trim()).filter(Boolean)
  }
  function prettyToolName(name: string): string {
    if (name.startsWith('mcp__')) {
      const tool = name.split('__').pop() ?? name
      return tool.replaceAll('_', ' ')
    }
    return name
  }
  // ---- readLints (Cursor) ----
  // IDE diagnostics for one or more paths. Never dump the minified
  // {fileDiagnostics,totalFiles,…} JSON — show paths + issues like findings.
  function isReadLints(m: AgentMsg): boolean {
    return m.tool === 'readLints' || m.tool === 'ReadLints'
  }
  function readLintsPaths(m: AgentMsg): string[] {
    const p = parsedInput(m)
    const raw = p?.paths
    if (!Array.isArray(raw)) return []
    return raw.filter((x): x is string => typeof x === 'string' && !!x)
  }
  type LintDiagnostic = {
    severity: string
    message: string
    line: number
    endLine: number
    source: string
  }
  type LintFile = {
    path: string
    diagnostics: LintDiagnostic[]
  }
  type LintResult = {
    files: LintFile[]
    totalFiles: number
    totalDiagnostics: number
  }
  function lintSeverity(raw: unknown): string {
    if (typeof raw === 'string' && raw) return raw.toLowerCase()
    if (typeof raw === 'number') {
      // LSP DiagnosticSeverity: 1 Error, 2 Warning, 3 Info, 4 Hint
      return ({ 1: 'error', 2: 'warning', 3: 'info', 4: 'hint' } as Record<number, string>)[raw] ?? 'info'
    }
    return 'info'
  }
  function lintLine(d: any): number {
    // LSP/Cursor ranges are 0-based; Monaco-style startLineNumber is 1-based.
    if (d?.range?.start && typeof d.range.start.line === 'number') {
      return d.range.start.line + 1
    }
    const line = d?.startLineNumber ?? d?.line ?? d?.lnum
    return typeof line === 'number' && line > 0 ? line : 0
  }
  function lintEndLine(d: any): number {
    if (d?.range?.end && typeof d.range.end.line === 'number') {
      return d.range.end.line + 1
    }
    const line = d?.endLineNumber ?? d?.endLine
    return typeof line === 'number' && line > 0 ? line : 0
  }
  function parseLintDiagnostic(d: any): LintDiagnostic {
    return {
      severity: lintSeverity(d?.severity ?? d?.severityName),
      message: String(d?.message ?? d?.msg ?? ''),
      line: lintLine(d),
      endLine: lintEndLine(d),
      source: typeof d?.source === 'string' ? d.source : (typeof d?.code === 'string' ? d.code : ''),
    }
  }
  function readLintsResult(m: AgentMsg): LintResult | null {
    if (!m.result || m.resultError) return null
    try {
      const raw = JSON.parse(m.result)
      if (!raw || typeof raw !== 'object' || !Array.isArray(raw.fileDiagnostics)) return null
      const files: LintFile[] = raw.fileDiagnostics.map((f: any) => ({
        path: String(f?.path ?? f?.file ?? ''),
        diagnostics: Array.isArray(f?.diagnostics) ? f.diagnostics.map(parseLintDiagnostic) : [],
      }))
      const totalDiagnostics = typeof raw.totalDiagnostics === 'number'
        ? raw.totalDiagnostics
        : files.reduce((n, f) => n + f.diagnostics.length, 0)
      const totalFiles = typeof raw.totalFiles === 'number' ? raw.totalFiles : files.length
      return { files, totalFiles, totalDiagnostics }
    } catch {
      return null
    }
  }
  function lintLocSuffix(d: LintDiagnostic): string {
    if (!d.line) return ''
    if (d.endLine && d.endLine !== d.line) return `:${d.line}–${d.endLine}`
    return `:${d.line}`
  }
  function lintSevClass(sev: string): string {
    if (sev === 'error' || sev === 'fatal') return 'high'
    if (sev === 'warning' || sev === 'warn') return 'medium'
    return 'info'
  }
  // ---- report_finding (Recur MCP) ----
  // Reviewers file structured findings; the expanded card shows severity +
  // location chips and the message as prose (markdown), never the raw args JSON
  // which re-escapes newlines into a wall of `\n`.
  function isReportFinding(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__report_finding') || m.tool === 'report_finding'
  }
  function isListFindings(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__list_findings') || m.tool === 'list_findings'
  }
  /** Claude Code native Skill tool, or Recur mcp__recur__activate_skill. */
  function isSkillActivation(m: AgentMsg): boolean {
    return m.tool === 'Skill' ||
      !!m.tool?.endsWith('__activate_skill') ||
      m.tool === 'activate_skill'
  }
  function isListSkills(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__list_skills') || m.tool === 'list_skills'
  }
  function isAskUser(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__ask_user') || m.tool === 'ask_user'
  }
  function isSwitchMode(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__switch_mode') || m.tool === 'switch_mode'
  }
  function isRenameSession(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__rename_session') || m.tool === 'rename_session'
  }
  function isRenameWorkspace(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__rename_workspace') || m.tool === 'rename_workspace'
  }
  function isRenameBranch(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__rename_branch') || m.tool === 'rename_branch'
  }
  function isOpenPR(m: AgentMsg): boolean {
    return isOpenPRTool(m.tool)
  }
  function isReenterPhase(m: AgentMsg): boolean {
    return isReenterPhaseTool(m.tool)
  }
  function reenterPhaseID(m: AgentMsg): string {
    return parseReenterPhaseID(parsedInput(m))
  }
  function openPRCard(m: AgentMsg) {
    return parseOpenPRCard(parsedInput(m), m.result ?? '', {
      pending: !!m.resultPending,
      error: !!m.resultError,
    })
  }
  function openPRId(m: AgentMsg): string {
    const card = openPRCard(m)
    if (card.number <= 0) return ''
    const hits = $prs.filter((p) => p.number === card.number && (!workspaceId || p.workspaceId === workspaceId))
    if (hits.length === 1) return hits[0]!.id
    if (workspaceId) {
      const ranked = prForWorkspace($prs, workspaceId)
      if (ranked?.number === card.number) return ranked.id
    }
    return hits[0]?.id ?? ''
  }
  function isRenameTool(m: AgentMsg): boolean {
    return isRenameSession(m) || isRenameWorkspace(m) || isRenameBranch(m)
  }
  function isMergeChild(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__merge_child') || m.tool === 'merge_child'
  }
  function isReadChild(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__read_child') || m.tool === 'read_child'
  }
  function isArchiveChild(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__archive_child') || m.tool === 'archive_child'
  }
  function isMessageChild(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__message_child') || m.tool === 'message_child'
  }
  function isChildTool(m: AgentMsg): boolean {
    return isMergeChild(m) || isReadChild(m) || isArchiveChild(m) || isMessageChild(m)
  }
  function childToolID(m: AgentMsg): string {
    const id = (parsedInput(m) as { workspace_id?: unknown } | null)?.workspace_id
    return typeof id === 'string' ? id : ''
  }
  /** Engine refusals come back as "merge_child: …" text, not tool errors. */
  function childToolFailed(m: AgentMsg): boolean {
    return !!m.resultError || /^(merge|read|archive|message)_child:/.test(m.result ?? '')
  }
  function childToolVerb(m: AgentMsg): [string, string, string] {
    if (isMergeChild(m)) return ['Merging', 'Merged', 'Merge']
    if (isArchiveChild(m)) return ['Archiving', 'Archived', 'Archive']
    if (isMessageChild(m)) return ['Messaging', 'Messaged', 'Message']
    return ['Reading', 'Read', 'Read']
  }
  function childToolLabel(m: AgentMsg): string {
    const [doing, done, base] = childToolVerb(m)
    const id = childToolID(m)
    const who = id ? `subagent ${id}` : 'subagent'
    if (childToolFailed(m)) return `${base} refused: ${who}`
    return m.resultPending ? `${doing} ${who}` : `${done} ${who}`
  }
  /** One-line result for the row: refusal text, or the merge outcome. */
  function childToolNote(m: AgentMsg): string {
    if (m.resultPending || !m.result) return ''
    const first = m.result.split('\n')[0].replace(/^(merge|read|archive|message)_child:\s*/, '').trim()
    if (isReadChild(m) && !childToolFailed(m)) return ''
    return first
  }
  /** create_workspace that made (or is making) a subagent: the fan-out card
   *  is its row, so the tool step is dropped. Refusals and standalone
   *  creations keep their row. */
  function isSubagentCreate(m: AgentMsg): boolean {
    if (!(m.tool?.endsWith('__create_workspace') || m.tool === 'create_workspace')) return false
    if (m.resultError) return false
    const input = parsedInput(m) as { standalone?: unknown } | null
    if (input?.standalone === true) return false
    const r = m.result ?? ''
    return !r || r.startsWith('Created subagent')
  }
  function renameToolNoun(m: AgentMsg): string {
    if (isRenameWorkspace(m)) return 'workspace'
    if (isRenameBranch(m)) return 'branch'
    return 'chat'
  }
  function isListArtifacts(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__list_artifacts') || m.tool === 'list_artifacts'
  }
  function isCreateArtifact(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__create_artifact') || m.tool === 'create_artifact'
  }
  function isPresentArtifact(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__present_artifact') || m.tool === 'present_artifact'
  }
  function isListWorkspaceBriefs(m: AgentMsg): boolean {
    return isListWorkspaceBriefsTool(m.tool)
  }
  function isGetWorkspaceContext(m: AgentMsg): boolean {
    return isGetWorkspaceContextTool(m.tool)
  }
  function isPresentWorkspace(m: AgentMsg): boolean {
    return isPresentWorkspaceTool(m.tool)
  }
  function listWorkspaceBriefsResult(m: AgentMsg): ListedWorkspaceBriefs | null {
    if (m.resultError) return null
    return parseListWorkspaceBriefs(m.result ?? '')
  }
  function workspaceContextResult(m: AgentMsg): WorkspaceContextView | null {
    if (m.resultError) return null
    return parseWorkspaceContext(m.result ?? '')
  }
  function presentWorkspaceInputId(m: AgentMsg): string {
    const p = parsedInput(m)
    return strField(p?.workspace_id) || strField(p?.workspaceId) || ''
  }
  function strField(v: unknown): string {
    return typeof v === 'string' ? v.trim() : ''
  }
  function presentWorkspaceResult(m: AgentMsg): PresentWorkspaceView | null {
    const target = parsePresentWorkspaceTarget(parsedInput(m))
    if (m.resultPending) {
      return hydratePresentBrief(
        { ok: false, target, kind: target.split(':')[0] || '', brief: null, error: '' },
        $briefing.workspaces, $workspaces, $projects,
      )
    }
    const card = parsePresentWorkspace(m.result ?? '', target)
    if (!card.target && !card.brief && !card.error && !m.result) return null
    return hydratePresentBrief(card, $briefing.workspaces, $workspaces, $projects)
  }
  function lastPresentedInTurn(turnEndIndex: number): PresentWorkspaceView | null {
    if (turnEndIndex <= 0) return null
    for (let i = turnEndIndex - 1; i >= 0; i--) {
      const m = transcriptMessages[i]
      if (!m || m.me || m.turnEnd) break
      if (!isPresentWorkspace(m) || m.resultError || m.resultPending) continue
      const card = presentWorkspaceResult(m)
      if (card?.ok && (card.brief || card.target)) return card
    }
    return null
  }
  /** Parse create_artifact result JSON for title/kind when input is incomplete. */
  function createArtifactResult(m: AgentMsg): { title: string; kind: string; id: string } | null {
    const r = (m.result ?? '').trim()
    const i = r.indexOf('{')
    if (i < 0) return null
    try {
      const o = JSON.parse(r.slice(i)) as { title?: unknown; kind?: unknown; id?: unknown }
      return {
        title: typeof o.title === 'string' ? o.title.trim() : '',
        kind: typeof o.kind === 'string' ? o.kind.trim().toLowerCase() : '',
        id: typeof o.id === 'string' ? o.id.trim() : '',
      }
    } catch {
      return null
    }
  }
  function createArtifactTitle(m: AgentMsg): string {
    const t = parsedInput(m)?.title
    if (typeof t === 'string' && t.trim()) return t.trim()
    return createArtifactResult(m)?.title || ''
  }
  function createArtifactKind(m: AgentMsg): string {
    const k = parsedInput(m)?.kind
    if (typeof k === 'string' && k.trim()) return k.trim().toLowerCase()
    return createArtifactResult(m)?.kind || ''
  }
  function createArtifactFailed(m: AgentMsg): boolean {
    if (m.resultError) return true
    const r = (m.result ?? '').trim()
    if (!r || m.resultPending) return false
    return /^create_artifact:/i.test(r) || /^unsupported artifact/i.test(r)
  }
  function presentArtifactId(m: AgentMsg): string {
    const id = parsedInput(m)?.id
    return typeof id === 'string' ? id.trim() : ''
  }
  /** "Presented artifact <id> (<title>)." */
  function presentArtifactTitle(m: AgentMsg): string {
    const r = (m.result ?? '').trim()
    const ok = r.match(/^Presented artifact\s+\S+\s+\((.+)\)\.?$/i)
    if (ok?.[1]?.trim()) return ok[1].trim()
    return presentArtifactId(m)
  }
  function presentArtifactFailed(m: AgentMsg): boolean {
    if (m.resultError) return true
    const r = (m.result ?? '').trim()
    if (!r || m.resultPending) return false
    return /^Artifact not found/i.test(r) || /^Could not list artifacts/i.test(r)
  }
  function renameToolValue(m: AgentMsg): string {
    const p = parsedInput(m)
    const raw = p?.title ?? p?.name
    if (typeof raw === 'string' && raw.trim()) return raw.trim()
    const r = (m.result ?? '').trim()
    const ok = r.match(/^Renamed (?:chat|workspace|branch) to\s+(.+?)\.?$/i)
    return ok?.[1]?.trim() ?? ''
  }
  function renameToolFailed(m: AgentMsg): boolean {
    if (m.resultError) return true
    const r = (m.result ?? '').trim()
    if (!r || m.resultPending) return false
    return /^rename_(?:session|workspace|branch):/i.test(r)
  }
  function switchModeTarget(m: AgentMsg): string {
    const mode = parsedInput(m)?.mode
    return typeof mode === 'string' ? mode.trim().toLowerCase() : ''
  }
  /** Pretty mode label (plan → Plan). */
  function modeLabel(mode: string): string {
    return chatModeMeta(mode).label
  }
  function modeTint(mode: string): string {
    return chatModeMeta(mode).tint
  }
  /** Parse "Switched chat mode from X to Y" (or refusal) from the tool result. */
  function switchModeTransition(m: AgentMsg): { from: string; to: string } | null {
    const r = (m.result ?? '').trim()
    const ok = r.match(/Switched chat mode from\s+(\w+)\s+to\s+(\w+)/i)
    if (ok) return { from: ok[1].toLowerCase(), to: ok[2].toLowerCase() }
    const to = switchModeTarget(m)
    return to ? { from: '', to } : null
  }
  function switchModeFailed(m: AgentMsg): boolean {
    if (m.resultError) return true
    const r = (m.result ?? '').trim()
    if (!r || m.resultPending) return false
    return /^(Cannot switch|Unknown mode|switch_mode is only)/i.test(r)
  }
  function askUserQuestion(m: AgentMsg): string {
    const q = parsedInput(m)?.question
    return typeof q === 'string' ? q.trim() : ''
  }
  function askUserContext(m: AgentMsg): string {
    const c = parsedInput(m)?.context
    return typeof c === 'string' ? c.trim() : ''
  }
  function skillActivationName(m: AgentMsg): string {
    const p = parsedInput(m)
    if (typeof p?.skill === 'string' && p.skill.trim()) return p.skill.trim()
    if (typeof p?.name === 'string' && p.name.trim()) return p.name.trim()
    const ti = (m.toolInput ?? '').trim().replace(/^\//, '')
    if (ti && !ti.startsWith('{') && ti.toLowerCase() !== 'skill') {
      return ti.split(/\s+/)[0] || ''
    }
    // Fallback: activate_skill result wraps `<skill_content name="…">`.
    const fromResult = /<skill_content\s+name="([^"]+)"/i.exec(m.result ?? '')
    if (fromResult?.[1]) return fromResult[1]
    return ''
  }
  function isUpdateFinding(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__update_finding') || m.tool === 'update_finding'
  }
  function isDone(m: AgentMsg): boolean {
    return !!m.tool?.endsWith('__done') || m.tool === 'done'
  }
  function doneSummary(m: AgentMsg): string {
    const s = parsedInput(m)?.summary
    return typeof s === 'string' ? s : ''
  }
  // ---- submit_plan / update_plan_step / edit_plan ----
  // Mirror TodoWrite / findings: human labels + prose/checklist body, never the
  // raw {summary,steps} / {plan} JSON dump that made these rows noisy.
  function isSubmitPlan(m: AgentMsg): boolean {
    return isSubmitPlanTool(m.tool)
  }
  function isUpdatePlanStep(m: AgentMsg): boolean {
    return isUpdatePlanStepTool(m.tool)
  }
  function isEditPlan(m: AgentMsg): boolean {
    return isEditPlanTool(m.tool)
  }
  function editPlanText(m: AgentMsg): string {
    return parseEditPlan(parsedInput(m))
  }
  function submitPlanSummary(m: AgentMsg): string {
    const s = parsedInput(m)?.summary
    return typeof s === 'string' ? s.trim() : ''
  }
  function submitPlanBody(m: AgentMsg): string {
    return parseSubmitPlanBody(parsedInput(m))
  }
  function submitPlanAttIds(m: AgentMsg): string[] {
    return parseSubmitPlanAttachments(parsedInput(m)?.attachments)
  }
  function submitPlanSteps(m: AgentMsg) {
    return parseSubmitPlanSteps(parsedInput(m)?.steps)
  }
  function updatePlanStepId(m: AgentMsg): string {
    const id = parsedInput(m)?.step_id
    return typeof id === 'string' ? id : ''
  }
  function updatePlanStepStatus(m: AgentMsg): string {
    const st = parsedInput(m)?.status
    return typeof st === 'string' ? st : ''
  }
  function updatePlanStepNote(m: AgentMsg): string {
    const n = parsedInput(m)?.note
    return typeof n === 'string' ? n.trim() : ''
  }
  function updatePlanStepTitle(m: AgentMsg): string {
    if (m.resultError) return ''
    return parseUpdatePlanStepTitle(m.result ?? '')
  }
  const PLAN_STEP_MARK: Record<string, string> = {
    pending: '○', doing: '◐', done: '●', skipped: '⊘',
    todo: '○', in_progress: '◐', completed: '●',
  }
  function listFindingsResult(m: AgentMsg): ListedFindings | null {
    return resolveListFindings(m.result, m.resultError, findings)
  }
  function listArtifactsResult(m: AgentMsg): ListedArtifacts | null {
    return resolveListArtifacts(m.result, m.resultError)
  }
  function updateFindingResult(m: AgentMsg): ListedFinding | null {
    if (!m.result || m.resultError) return null
    try {
      const raw = JSON.parse(m.result)
      if (!raw || typeof raw !== 'object' || Array.isArray(raw) || !raw.id) return null
      return parseListedFinding(raw)
    } catch {
      return null
    }
  }
  function updateFindingFromInput(m: AgentMsg): ListedFinding | null {
    const p = parsedInput(m)
    if (!p) return null
    const id = typeof p.finding_id === 'string' ? p.finding_id : ''
    const status = typeof p.status === 'string' ? p.status : ''
    if (!id && !status) return null
    return {
      id,
      file: '',
      severity: '',
      message: '',
      status,
      statusReason: typeof p.reason === 'string' ? p.reason : '',
    }
  }
  function findingFromStore(id: string): ListedFinding | null {
    if (!id) return null
    const f = findings.find((x) => findingMatchesRef(x, id))
    return f ? parseListedFinding(f) : null
  }
  // finding_id is in the tool input — look up file/message/severity from the
  // workspace findings list (same source as the Findings panel). Status/reason
  // come from this call's args; JSON tool results are only a fallback.
  function resolveUpdateFinding(m: AgentMsg): ListedFinding | null {
    const fromInput = updateFindingFromInput(m)
    const fromResult = updateFindingResult(m)
    const id = fromInput?.id || fromResult?.id || ''
    const fromStore = findingFromStore(id)
    if (!fromStore && !fromResult && !fromInput) return null
    return {
      id,
      status: fromInput?.status || fromResult?.status || fromStore?.status || '',
      file: fromStore?.file || fromResult?.file || '',
      line: (fromStore?.line && fromStore.line > 0 ? fromStore.line : 0)
        || (fromResult?.line && fromResult.line > 0 ? fromResult.line : 0)
        || 0,
      endLine: (fromStore?.endLine && fromStore.endLine > 0 ? fromStore.endLine : 0)
        || (fromResult?.endLine && fromResult.endLine > 0 ? fromResult.endLine : 0)
        || 0,
      severity: fromStore?.severity || fromResult?.severity || '',
      message: fromStore?.message || fromResult?.message || '',
      statusReason: fromInput?.statusReason || fromResult?.statusReason || fromStore?.statusReason || '',
    }
  }
  function listedLocSuffix(f: ListedFinding): string {
    return locSuffixOf(f.line, f.endLine)
  }
  function findingSeverity(m: AgentMsg): string {
    if (isUpdateFinding(m)) return resolveUpdateFinding(m)?.severity || ''
    const s = parsedInput(m)?.severity
    return typeof s === 'string' ? s : ''
  }
  function findingFile(m: AgentMsg): string {
    if (isUpdateFinding(m)) return resolveUpdateFinding(m)?.file || ''
    const f = parsedInput(m)?.file
    return typeof f === 'string' ? f : ''
  }
  function findingLine(m: AgentMsg): number {
    if (isUpdateFinding(m)) {
      const n = resolveUpdateFinding(m)?.line
      return typeof n === 'number' && n > 0 ? n : 0
    }
    const n = parsedInput(m)?.line
    return typeof n === 'number' && n > 0 ? n : 0
  }
  function findingEndLine(m: AgentMsg): number {
    if (isUpdateFinding(m)) {
      const n = resolveUpdateFinding(m)?.endLine
      return typeof n === 'number' && n > 0 ? n : 0
    }
    const n = parsedInput(m)?.end_line
    return typeof n === 'number' && n > 0 ? n : 0
  }
  function findingLocSuffix(m: AgentMsg): string {
    const line = findingLine(m)
    if (!line) return ''
    const end = findingEndLine(m)
    return locSuffixOf(line, end)
  }
  function findingMessage(m: AgentMsg): string {
    if (isUpdateFinding(m)) return resolveUpdateFinding(m)?.message || ''
    const msg = parsedInput(m)?.message
    return typeof msg === 'string' ? msg : ''
  }
  function findingStatus(m: AgentMsg): string {
    if (!isUpdateFinding(m)) return ''
    return resolveUpdateFinding(m)?.status || ''
  }
  function findingStatusReason(m: AgentMsg): string {
    if (!isUpdateFinding(m)) return ''
    return resolveUpdateFinding(m)?.statusReason || ''
  }

  function locForDigest(file: string, line: number, endLine: number): string {
    if (!file) return ''
    if (!line || line <= 0) return file
    if (endLine > line) return `${file}:${line}-${endLine}`
    return `${file}:${line}`
  }

  function turnDigestAt(endIdx: number): TurnDigestFinding[] {
    // endIdx is into transcriptMessages (queued follow-ups are excluded from blocks).
    return stillOpenFindingsForTurn(transcriptMessages, endIdx, findings)
  }

  let digestCommentFor = $state('')
  let digestCommentDraft = $state('')
  let digestCommentBusy = $state('')
  let digestCommentInput = $state<HTMLTextAreaElement | undefined>(undefined)
  /** Compact accordion open keys for list_findings + turn digests. */
  let findingAccordOpen = $state<Record<string, boolean>>(initialUI?.findingAccordOpen ?? {})
  const findingAccordSlide = (() => {
    let reduce = false
    try {
      reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    } catch { /* ignore */ }
    return { duration: reduce ? 0 : 200, easing: cubicOut }
  })()

  function toggleFindingAccord(key: string) {
    if (!key) return
    findingAccordOpen = { ...findingAccordOpen, [key]: !findingAccordOpen[key] }
    persistTranscriptUI()
  }

  async function openDigestCompose(id: string) {
    if (!id) return
    digestCommentFor = id
    digestCommentDraft = ''
    findingAccordOpen = { ...findingAccordOpen, [id]: true }
    await tick()
    digestCommentInput?.focus()
  }
  /** Notes saved from this chat before the parent findings prop refreshes. */
  let digestNotesById = $state<Record<string, TurnDigestNote[]>>({})
  /** `findings` array reference the overlay was written against — stale after refresh. */
  let digestNotesAtFindings: FindingRef[] | undefined = undefined
  /** Status flips from this chat before the parent findings prop refreshes. */
  let digestStatusById = $state<Record<string, string>>({})
  let digestStatusBusy = $state('')

  // Drop the optimistic overlay once the parent findings prop refreshes so
  // Findings-panel (or other chat) edits aren't permanently shadowed.
  $effect(() => {
    const current = findings
    untrack(() => {
      if (
        digestNotesAtFindings !== undefined
        && digestNotesAtFindings !== current
        && Object.keys(digestNotesById).length
      ) {
        digestNotesById = {}
        digestNotesAtFindings = undefined
      }
    })
  })

  function notesForDigest(f: TurnDigestFinding): TurnDigestNote[] {
    if (
      f.id
      && Object.hasOwn(digestNotesById, f.id)
      && digestNotesAtFindings === findings
    ) {
      return digestNotesById[f.id] ?? []
    }
    return f.notes ?? []
  }

  function turnDigestVisible(endIdx: number): TurnDigestFinding[] {
    return turnDigestAt(endIdx).filter((f) => {
      if (!f.id) return true
      const st = digestStatusById[f.id]
      return !st || st === 'open'
    })
  }

  async function setDigestFindingStatus(id: string, status: 'ignored' | 'invalid') {
    if (!workspaceId || !id || digestStatusBusy) return
    digestStatusBusy = id
    const prev = digestStatusById[id]
    digestStatusById = { ...digestStatusById, [id]: status }
    if (digestCommentFor === id) {
      digestCommentFor = ''
      digestCommentDraft = ''
    }
    try {
      await ObservatoryService.SetFindingStatus(workspaceId, id, status)
    } catch (err) {
      if (prev) digestStatusById = { ...digestStatusById, [id]: prev }
      else {
        const next = { ...digestStatusById }
        delete next[id]
        digestStatusById = next
      }
      pushToast('error', bridgeErrorMessage(err) || 'Could not update finding')
    } finally {
      digestStatusBusy = ''
    }
  }

  function jumpToFinding(id: string, focusNote = false) {
    const f = id ? findings.find((x) => findingMatchesRef(x, id)) : undefined
    observatoryJump.set({
      workspaceId: workspaceId || undefined,
      projectId: projectId || undefined,
      panel: 'findings',
      findingId: id || undefined,
      focusNote: !!id && focusNote,
      path: f?.file || undefined,
      line: f?.line && f.line > 0 ? f.line : undefined,
    })
  }

  function applyDigestNotes(id: string, notes: TurnDigestNote[]) {
    digestNotesById = {
      ...digestNotesById,
      [id]: notes
        .map((n) => ({
          id: n.id || '',
          author: n.author,
          body: n.body || '',
          createdAt: n.createdAt,
        }))
        .filter((n) => n.body.trim()),
    }
    digestNotesAtFindings = untrack(() => findings)
  }

  async function submitDigestComment(id: string) {
    const body = digestCommentDraft.trim()
    if (!workspaceId || !id || !body || digestCommentBusy) return
    digestCommentBusy = id
    try {
      const updated = await ObservatoryService.AddFindingNote(workspaceId, id, body)
      digestCommentDraft = ''
      digestCommentFor = ''
      applyDigestNotes(id, updated.notes ?? [])
    } catch (err) {
      pushToast('error', bridgeErrorMessage(err) || 'Could not add note')
    } finally {
      digestCommentBusy = ''
    }
  }

  async function deleteDigestNote(findingId: string, noteId: string) {
    if (!workspaceId || !findingId || !noteId || digestCommentBusy) return
    const current = digestNotesById[findingId]
      ?? findings.find((f) => findingMatchesRef(f, findingId))?.notes
      ?? []
    digestCommentBusy = findingId
    applyDigestNotes(findingId, current.filter((n) => n.id !== noteId))
    try {
      const updated = await ObservatoryService.DeleteFindingNote(workspaceId, findingId, noteId)
      applyDigestNotes(findingId, updated.notes ?? [])
    } catch (err) {
      applyDigestNotes(findingId, current)
      pushToast('error', bridgeErrorMessage(err) || 'Could not delete note')
    } finally {
      digestCommentBusy = ''
    }
  }

  // ---- Todos (TodoWrite) ----
  // Both providers send the same shape once normalized: {todos:[{content,status}]}
  // with status pending | in_progress | completed.
  //
  // Named "todo", not "plan": this is a flat checklist the agent keeps for
  // itself, and Recur has separate planning concepts (the Maestro's plan, plan
  // mode) that must not get tangled up with it.
  type Todo = { content: string; status: string }
  function todoItems(m: AgentMsg): Todo[] {
    return parseTodos(m.input)
  }

  // ---- CreatePlan (Cursor plan mode) ----
  // Args carry the plan markdown; the result is an empty ack (same pattern as
  // TodoWrite). Accept both the common name and the raw Cursor spelling so an
  // unmapped older transcript still renders.
  function isCreatePlan(m: AgentMsg): boolean {
    return m.tool === 'CreatePlan' || m.tool === 'createPlan'
  }
  function planBody(m: AgentMsg): string {
    const p = parsedInput(m)?.plan
    return typeof p === 'string' ? p.trim() : ''
  }

  // While a call is in flight there is no result to render, and dumping the raw
  // input JSON is the least useful thing we can show. Cursor's edit sends only
  // {"path": …} on its running frames — no partial content — so streaming the
  // diff is not possible; the honest alternative is to say what is happening.
  function pendingLabel(m: AgentMsg): string {
    const f = fileOf(m)
    const name = f ? baseName(f) : ''
    switch (m.tool) {
      case 'Edit': return name ? `Editing ${name}…` : 'Applying edit…'
      case 'Write': return name ? `Writing ${name}…` : 'Writing file…'
      case 'Read': return name ? `Reading ${name}…` : 'Reading file…'
      case 'Delete': return name ? `Deleting ${name}…` : 'Deleting file…'
      case 'Bash': return 'Running command…'
      case 'Grep': case 'Glob': return 'Searching…'
      case 'WebFetch': return 'Fetching page…'
      case 'WebSearch': return 'Searching the web…'
      case 'ToolSearch':
        return /^select:/i.test(toolSearchQuery(m)) ? 'Selecting tools…' : 'Searching tools…'
      case 'TodoWrite': return 'Updating todos…'
      case 'TaskCreate': return 'Creating task…'
      case 'TaskUpdate': {
        switch (claudeTaskArgsOf(m).status) {
          case 'in_progress': return 'Starting task…'
          case 'completed': return 'Completing task…'
          case 'deleted': return 'Deleting task…'
          default: return 'Updating task…'
        }
      }
      case 'TaskGet': return 'Reading task…'
      case 'TaskList': return 'Listing tasks…'
      case 'SendMessage':
        return 'Sending message…'
      case 'Task':
      case 'task':
      case 'Agent': {
        // Type-aware copy so a collapsed/expanded wait state is not the
        // generic "Running Task…" Cursor shows.
        const bg = taskBackground(m)
        switch (taskSubagentType(m).toLowerCase()) {
          case 'explore': return bg ? 'Exploring in the background…' : 'Exploring…'
          case 'shell': return bg ? 'Running shell agent in the background…' : 'Running shell agent…'
          case 'browser-use': return bg ? 'Browsing in the background…' : 'Browsing…'
          case 'best-of-n-runner': return 'Running parallel attempt…'
          default: return bg ? 'Running a subagent in the background…' : 'Running subagent…'
        }
      }
      case 'CreatePlan':
      case 'createPlan':
        return 'Writing plan…'
      case 'readLints':
      case 'ReadLints':
        return 'Reading lints…'
    }
    if (isReportFinding(m)) return 'Reporting finding…'
    if (isListFindings(m)) return 'Listing findings…'
    if (isListArtifacts(m)) return 'Listing artifacts…'
    if (isUpdateFinding(m)) return 'Updating finding…'
    if (isDone(m)) return 'Wrapping up…'
    if (isAskUser(m)) return 'Asking…'
    if (isSwitchMode(m)) {
      const to = switchModeTarget(m)
      return to ? `Switching to ${modeLabel(to)}…` : 'Switching mode…'
    }
    if (isRenameTool(m)) {
      const t = renameToolValue(m)
      const noun = renameToolNoun(m)
      return t ? `Renaming ${noun} to ${t}…` : `Renaming ${noun}…`
    }
    if (isOpenPR(m)) {
      const t = openPRCard(m).title
      return t ? `Opening ${t}…` : 'Opening pull request…'
    }
    if (isChildTool(m)) return `${childToolLabel(m)}…`
    if (isReenterPhase(m)) {
      const phase = reenterPhaseID(m)
      return phase ? `Re-entering ${phase}…` : 'Re-entering phase…'
    }
    if (isCreateArtifact(m)) {
      const t = createArtifactTitle(m)
      return t ? `Creating ${t}…` : 'Creating artifact…'
    }
    if (isPresentArtifact(m)) {
      const t = presentArtifactTitle(m)
      return t ? `Presenting ${t}…` : 'Presenting artifact…'
    }
    if (isListWorkspaceBriefs(m)) return 'Listing workspaces…'
    if (isGetWorkspaceContext(m)) {
      const t = presentWorkspaceInputId(m)
      return t ? `Looking up ${t}…` : 'Looking up workspace…'
    }
    if (isPresentWorkspace(m)) {
      const t = parsePresentWorkspaceTarget(parsedInput(m))
      return t ? `Presenting ${t}…` : 'Presenting workspace…'
    }
    if (isSubmitPlan(m)) return 'Submitting plan…'
    if (isUpdatePlanStep(m)) return 'Updating plan step…'
    if (isEditPlan(m)) return 'Editing plan…'
    if (isLinearTool(m.tool)) {
      return linearToolLabel({
        tool: m.tool!,
        pending: true,
        failed: false,
        args: parseLinearArgs(parsedInput(m)),
        view: null,
      }) + '…'
    }
    return m.tool ? `Running ${m.tool}…` : 'Running…'
  }

  function prettyInput(m: AgentMsg): string {
    const p = parsedInput(m)
    if (!p) return m.input ?? ''
    try { return JSON.stringify(p, null, 2) } catch { return m.input ?? '' }
  }
  // Pretty-print JSON tool results so unmapped tools don't dump one minified line.
  function prettyResult(text: string): string {
    const t = text.trim()
    if (!t || (t[0] !== '{' && t[0] !== '[')) return text
    try { return JSON.stringify(JSON.parse(t), null, 2) } catch { return text }
  }
  function thinkingPreview(m: AgentMsg): string {
    return (m.body ?? '').replace(/\s+/g, ' ').trim()
  }

  // Keep a live-thinking viewport pinned to its bottom so the fade window always
  // shows the newest lines as units land ({@html} swaps don't always
  // fire ResizeObserver on the wrapper).
  function pinThinkLive(node: HTMLElement) {
    let last = 0
    let raf = 0
    const tick = () => {
      const h = node.scrollHeight
      if (h !== last) {
        last = h
        node.scrollTop = h
      }
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return { destroy() { cancelAnimationFrame(raf) } }
  }

  // ---- Edit/Write mini code viewers ----
  function editDiff(m: AgentMsg): DiffRow[] {
    const p = parsedInput(m)
    // Claude Code's Edit carries before/after strings and we diff them here.
    if (p?.old_string !== undefined) {
      return diffLines(String(p.old_string ?? ''), String(p.new_string ?? ''))
    }
    // Cursor's edit reports only the path and hands back a ready-made unified
    // diff as the tool result, so parse that into the same rows.
    return parseUnifiedDiff(m.result ?? '')
  }

  // hasEditDiff decides whether the Edit card can show a diff at all, for either
  // provider's shape.
  function hasEditDiff(m: AgentMsg): boolean {
    if (m.tool !== 'Edit') return false
    if (parsedInput(m)?.old_string !== undefined) return true
    return looksLikeUnifiedDiff(m.result ?? '')
  }

  /** Write / Codex-create body: prefer args.content, else a non-diff result. */
  function writeBody(m: AgentMsg): string {
    const fromArgs = parsedInput(m)?.content
    if (typeof fromArgs === 'string') return fromArgs
    if (m.resultError) return ''
    if (m.tool === 'Write' && m.result && !looksLikeUnifiedDiff(m.result)) return m.result
    // Legacy Codex creates were stored as Edit with the file body in result.
    if (m.tool === 'Edit' && m.result && !looksLikeUnifiedDiff(m.result) && parsedInput(m)?.old_string === undefined) {
      return m.result
    }
    return ''
  }
  function hasWriteBody(m: AgentMsg): boolean {
    return (m.tool === 'Write' || m.tool === 'Edit') && writeBody(m) !== ''
  }
  function diffCounts(rows: DiffRow[]): { add: number; del: number } {
    let add = 0
    let del = 0
    for (const r of rows) {
      if (r.kind === 'add') add++
      else if (r.kind === 'del') del++
    }
    return { add, del }
  }

  // ---- Bash terminal card (the card IS the tool render, not a row that nests one) ----
  function bashCommand(m: AgentMsg): string {
    const c = parsedInput(m)?.command
    return typeof c === 'string' ? c : (m.input ?? '')
  }
  let copiedRow = $state<string | number>(-1)
  function copyCommand(m: AgentMsg, key: string | number, e: Event) {
    e.stopPropagation()
    void navigator.clipboard?.writeText(bashCommand(m))
    copiedRow = key
    setTimeout(() => { if (copiedRow === key) copiedRow = -1 }, 1200)
  }

  /** Plain text of a user bubble (attachment markers omitted). */
  function bubblePlainText(m: AgentMsg): string {
    return stripAttachmentMarkers(m.body ?? '')
  }
  let copiedBubble = $state<string | null>(null)
  function copyBubble(m: AgentMsg, key: string, e: Event) {
    e.stopPropagation()
    const text = bubblePlainText(m)
    if (!text) return
    void copyText(text).then(() => {
      copiedBubble = key
      setTimeout(() => { if (copiedBubble === key) copiedBubble = null }, 1200)
    })
  }

  // ---- scroll pinning ----
  let chatScrollEl = $state<HTMLDivElement | undefined>(undefined)
  let chatColEl = $state<HTMLDivElement | undefined>(undefined)
  let nearBottom = $state(initialUI?.nearBottom ?? true)
  // Same slack as auto-follow so a streaming pin does not flash `.fade-bot`.
  // pinnedToBottom: restore is after first paint; keep the leading fade on.
  const fadeChatScroll = fadeScrollEdges({
    botSlack: NEAR_BOTTOM_PX,
    pinnedToBottom: () => nearBottom,
  })
  /** Last measured offset while this chat's scroller was visible. */
  let lastScrollTop = initialUI?.scrollTop ?? 0
  /** Expansion guard pair — always written together (not on scroll). */
  let lastRowCount = initialUI?.rowCount ?? 0
  let lastFingerprint = initialUI?.fingerprint ?? ''
  /** Scroll restore waiting for the transcript to have a real height. */
  let pendingScroll: ChatTranscriptUIState | null = initialUI && !initialUI.nearBottom ? initialUI : null
  /** rAF attempts while a mid-scroll restore cannot yet honor the saved offset. */
  let restoreAttempts = 0
  const RESTORE_LAYOUT_FRAMES = 16
  /** True until the post-restore rAFs run — swallows the clamp `scroll` from a tab swap. */
  let ignoreSwapScroll = false

  function persistTranscriptUI() {
    if (!resetKey) return
    lastRowCount = transcriptMessages.length
    lastFingerprint = transcriptFingerprint(transcriptMessages)
    saveTranscriptUI(resetKey, snapshotTranscriptUI({
      openGroups,
      openRows,
      findingAccordOpen,
      scrollTop: lastScrollTop,
      nearBottom,
      scrollerHidden: false,
      rowCount: lastRowCount,
      fingerprint: lastFingerprint,
      blockHeights: Object.fromEntries(blockH),
    }))
  }

  function persistScroll() {
    if (!resetKey) return
    saveTranscriptUIScroll(resetKey, lastScrollTop, nearBottom)
  }

  function chatScrollerParked(el: HTMLElement | undefined): boolean {
    return !!el?.closest('#wsv-park')
  }

  function onChatScroll() {
    const el = chatScrollEl
    if (!el || el.clientHeight === 0 || chatScrollerParked(el)) return
    // Tab swaps clamp scrollTop and fire `scroll` after restore is armed but
    // before applyPendingScroll. That event is not a user gesture.
    if (ignoreSwapScroll) return
    nearBottom = nextNearBottom(nearBottom, lastScrollTop, el)
    lastScrollTop = el.scrollTop
    if (pendingScroll) pendingScroll = null
    persistScroll()
    if (forkMenu) forkMenu = null
    refreshChatWindow()
  }

  export function send(alt = false) {
    if (!canSend) return
    if (!attachments.length && onPromptAction) {
      const committed = parseCommittedPromptLine(input)
      if (committed) {
        const info = enabledPromptIndex.find((p) => p.slash === committed.slash)
        if (info) {
          if (promptBusy) return
          const values = parsePromptChipValues(committed.args, { slash: info.slash, args: promptArgSpecs(info) })
          if (promptTakesArgs(info) && !promptRequiredArgsFilled(info, values)) {
            enterPromptArgs(committed.slash, committed.args)
            return
          }
          mention = null
          skillMention = null
          promptArgs = null
          nearBottom = true
          pendingScroll = null
          persistTranscriptUI()
          onPromptAction(committed.slash, promptChipArgs(committed.slash, committed.args), alt)
          return
        }
      }
    }
    mention = null
    skillMention = null
    nearBottom = true
    pendingScroll = null
    persistTranscriptUI()
    onSend?.(alt)
  }

  function stop() {
    if (!sending && !canStop) return
    onStop?.()
  }

  function retry() {
    if (sending || readonly || !onRetry) return
    nearBottom = true
    pendingScroll = null
    persistTranscriptUI()
    onRetry()
  }

  // ⌘⇧⌫ / Ctrl+Shift+Backspace — stop only while the live agent panel has focus.
  $effect(() => {
    if (!shellActive || !(sending || canStop) || !onStop) return
    const onKey = (e: KeyboardEvent) => {
      if (!e.shiftKey || e.altKey) return
      if (!isCmdOrCtrl(e)) return
      if (e.key !== 'Backspace') return
      const t = e.target as HTMLElement | null
      if (!t?.closest?.('.obs-left, .chat')) return
      e.preventDefault()
      stop()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  // Escape denies the oldest inline permission for this tab (same as the floating card).
  $effect(() => {
    const req = chatPermissions[0]
    if (!req) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      void answerPermission(req.id, false, { reason: 'denied by the human' })
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  function pinBottom() {
    const el = chatScrollEl
    if (el && nearBottom) {
      el.scrollTo({ top: el.scrollHeight })
      lastScrollTop = el.scrollTop
    }
  }

  function captureTranscriptUI(key: string): ChatTranscriptUIState {
    // Teardown / key-swap: messages already belong to the incoming chat.
    // The expansion pair was last written together by persistTranscriptUI.
    return snapshotTranscriptUI({
      openGroups,
      openRows,
      findingAccordOpen,
      scrollTop: lastScrollTop,
      nearBottom,
      scrollerHidden: true,
      rowCount: lastRowCount,
      fingerprint: lastFingerprint,
      blockHeights: Object.fromEntries(blockHByChat.get(key) ?? []),
    }, loadTranscriptUI(key))
  }

  function applyPendingScroll() {
    const saved = pendingScroll
    const el = chatScrollEl
    if (!saved || !el) return
    const gate = scrollRestoreReady(saved, {
      clientHeight: el.clientHeight,
      scrollHeight: el.scrollHeight,
      rowCount: transcriptMessages.length,
    })
    if (gate === 'wait') return
    const maxScroll = Math.max(0, el.scrollHeight - el.clientHeight)
    const honored = saved.nearBottom || saved.scrollTop <= 0 || maxScroll >= saved.scrollTop
    // Markdown can still be growing after the rows are in the DOM. Retry a
    // bounded number of frames so a mid-scroll restore is not committed as a
    // pin; once the budget is spent, clamp and follow the gate's verdict.
    if (!honored && gate !== 'abandon' && restoreAttempts < RESTORE_LAYOUT_FRAMES) {
      restoreAttempts++
      nearBottom = false
      requestAnimationFrame(applyPendingScroll)
      return
    }
    if (gate === 'apply') {
      if (saved.nearBottom) el.scrollTop = el.scrollHeight
      else el.scrollTop = saved.scrollTop
    }
    pendingScroll = null
    restoreAttempts = 0
    lastScrollTop = el.scrollTop
    // Clamp / abandon can leave the view at the bottom with no `scroll` event.
    nearBottom = scrollerNearBottom(el)
    persistScroll()
  }

  function restoreTranscriptUI(saved: ChatTranscriptUIState | undefined) {
    const next = saved ?? emptyTranscriptUI()
    openGroups = next.openGroups
    openRows = next.openRows
    findingAccordOpen = next.findingAccordOpen
    nearBottom = next.nearBottom
    lastScrollTop = next.scrollTop
    lastRowCount = next.rowCount
    lastFingerprint = next.fingerprint
    expansionGuard = next.rowCount || next.fingerprint ? next : null
    // First visit (no cache): follow the default pin. Arming pending here would
    // swallow the user's first real scroll and persist the default pin instead.
    pendingScroll = saved ?? null
    restoreAttempts = 0
    ignoreSwapScroll = true
    void tick().then(() => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          applyPendingScroll()
          ignoreSwapScroll = false
        })
      })
    })
  }

  /** Re-apply the offset after this workbench is moved out of `#wsv-park`. */
  export function restoreParkedScroll() {
    const el = chatScrollEl
    if (!el || chatScrollerParked(el)) return
    if (nearBottom) {
      pinBottom()
      return
    }
    if (lastScrollTop > 0) el.scrollTop = lastScrollTop
  }

  /** Block a deep link is steering to. refreshChatWindow keeps it hydrated so
   *  the viewport-driven window cannot swap it back to a stub mid-reveal. */
  let revealKey = ''
  const frame = () => new Promise<void>((r) => requestAnimationFrame(() => r()))

  /** Bring an engine-fact block (e.g. a fan-out wave card) to the top of the
   *  scrollport. A tab swap restores the saved scroll over several frames and
   *  stub→card swaps shift everything above the target, so this pursues the
   *  block for a bounded window rather than scrolling once: each frame it
   *  re-hydrates the key, cancels bottom pins and pending restores, and pins
   *  the block's top, stopping once the card is real and has held still.
   *  Scrolls the chat container directly — scrollIntoView would also drag
   *  every ancestor scroller. Resolves true once the block was found. */
  export async function revealFact(factId: string): Promise<boolean> {
    const key = transcriptBlockKey(resetKey, `fact:${factId}`)
    revealKey = key
    try {
      // The loop tab may still be mounting or fetching when the link fires.
      const deadline = performance.now() + 4000
      while (!blockEls.has(key) && performance.now() < deadline) await frame()
      if (!blockEls.has(key)) return false
      let held = 0
      for (let i = 0; i < 90 && held < 4; i++) {
        const root = chatScrollEl
        const el = blockEls.get(key)
        if (!root || !el) return false
        if (revealKey !== key) return true
        if (windowChat && !chatHydrated.has(key)) chatHydrated = new Set([...chatHydrated, key])
        nearBottom = false
        pendingScroll = null
        const top = root.scrollTop + (el.getBoundingClientRect().top - root.getBoundingClientRect().top) - 6
        const target = Math.max(0, Math.min(Math.round(top), root.scrollHeight - root.clientHeight))
        if (Math.abs(root.scrollTop - target) > 1) {
          root.scrollTop = target
          lastScrollTop = root.scrollTop
          held = 0
        } else if (!el.classList.contains('stub')) {
          held++
        }
        await frame()
      }
      return true
    } finally {
      if (revealKey === key) revealKey = ''
    }
  }

  /** Pin the transcript to the bottom (e.g. prompt-action submit bypasses send()). */
  export function scrollToBottom() {
    nearBottom = true
    pendingScroll = null
    persistTranscriptUI()
    requestAnimationFrame(pinBottom)
  }

  // Elapsed turn clock: ticks while `sending` and the current turn is still
  // open. A trailing turnEnd (session.turn_end) must hide it even if Send or
  // agentsRunning lag — otherwise a "completed in" footer sits above a
  // phantom running timer.
  const showTurnElapsed = $derived(
    sending &&
      !currentTurnHasEnded(messages, turnStartedAt) &&
      chatPermissions.length === 0 &&
      chatQuestions.length === 0 &&
      !chatEscalation,
  )
  let turnElapsedSec = $state(0)
  let clockOrigin = 0
  let clockKey = ''
  $effect(() => {
    if (!showTurnElapsed) {
      turnElapsedSec = 0
      clockOrigin = 0
      clockKey = ''
      return
    }
    const key = resetKey
    if (turnStartedAt > 0) {
      clockOrigin = turnStartedAt
      clockKey = key
    } else if (clockOrigin === 0 || clockKey !== key) {
      // One AgentChat instance serves every tab — never reuse another chat's origin.
      clockOrigin = Date.now()
      clockKey = key
    }
    const started = clockOrigin
    const tick = () => {
      turnElapsedSec = Math.max(0, Math.floor((Date.now() - started) / 1000))
    }
    tick()
    const id = setInterval(tick, 250)
    return () => clearInterval(id)
  })

  const liveStepRows = $derived.by(() => {
    const last = blocks[blocks.length - 1]
    return last?.kind === 'steps' ? last.rows : []
  })
  const firstCheckpoint = $derived(
    isFirstCheckpoint(showTurnElapsed, turnElapsedSec, liveStepRows.map((r) => r.m)),
  )

  // formatDuration renders a finished turn's wall time. Sub-minute turns read
  // better in seconds ("12.4s") than as 0:12.
  function formatDuration(ms: number): string {
    if (ms <= 0) return ''
    const sec = ms / 1000
    if (sec < 60) return `${sec < 10 ? sec.toFixed(1) : Math.round(sec)}s`
    const total = Math.round(sec)
    const m = Math.floor(total / 60)
    const s = String(total % 60).padStart(2, '0')
    if (m >= 60) return `${Math.floor(m / 60)}h ${m % 60}m ${s}s`
    return `${m}m ${s}s`
  }

  // Complete lines grow the canvas without changing message state — follow
  // content height so a pinned view stays pinned while characters land.
  $effect(() => {
    const col = chatColEl
    if (!col) return
    const ro = new ResizeObserver(() => {
      // A parked pinned chat still follows growth so A→B→A lands on the
      // new tail, not the pixel offset from when we left. Skip pending
      // mid-scroll restore while hidden — that path is for tab swaps.
      if (chatScrollerParked(chatScrollEl)) {
        pinBottom()
        return
      }
      // Pin in this layout pass — deferring to rAF leaves one frame of
      // growth (often >24px when tokens land fast) and the follow-up
      // scroll event used to look like the user left the tail.
      applyPendingScroll()
      pinBottom()
      refreshChatWindow()
    })
    ro.observe(col)
    return () => ro.disconnect()
  })

  $effect(() => {
    const guard = expansionGuard
    if (!guard) return
    const rows = transcriptMessages
    const fp = transcriptFingerprint(rows, guard.rowCount)
    if (expansionStillValid(guard, fp, rows.length)) return
    openGroups = {}
    openRows = {}
    expansionGuard = null
  })

  // One AgentChat instance serves every tab: persist chrome for the key we're
  // leaving and restore the key we're entering (including an explicit collapse
  // of the live tail — missing keys would otherwise re-open while sending).
  // SessionCard / Activity omit resetKey — leave their instance-local chrome alone.
  // Scroll was already written by onChatScroll / toggles; cleanup must not read
  // the swapped (or detached) scroller.
  $effect(() => {
    const key = resetKey
    if (!key) return
    untrack(() => restoreTranscriptUI(claimTranscriptUI(key)))
    return () => {
      untrack(() => saveTranscriptUI(key, captureTranscriptUI(key)))
    }
  })

  $effect(() => {
    void resetKey
    chatHydrated = new Set()
    chatInView = []
    if (!untrack(() => windowChat)) return
    void tick().then(refreshChatWindow)
  })

  $effect(() => {
    void blocks.length
    if (!windowChat) {
      chatHydrated = new Set()
      return
    }
    void tick().then(refreshChatWindow)
  })
</script>

<svelte:window onclick={closeForkMenu} onresize={closeForkMenu} onkeydown={onForkMenuKey} />

{#snippet stepIcon(kind: ToolIcon | 'think' | 'compact' | 'fallback')}
  {@const name =
    kind === 'term' ? 'chevrons-right'
    : kind === 'file' ? 'file'
    : kind === 'pencil' ? 'pencil'
    : kind === 'trash' ? 'trash'
    : kind === 'search' ? 'search'
    : kind === 'globe' ? 'globe'
    : kind === 'bot' ? 'bot'
    : kind === 'todo' ? 'list-todo'
    : kind === 'plan' ? 'notebook-pen'
    : kind === 'finding' ? 'hat-glasses'
    : kind === 'artifact' ? 'grid'
    : kind === 'workspace' ? 'box'
    : kind === 'done' ? 'circle-check'
    : kind === 'tools' ? 'pocket-knife'
    : kind === 'think' ? 'lightbulb'
    : kind === 'compact' ? 'minimize'
    : kind === 'fallback' ? 'replace'
    : kind === 'linear' ? 'linear'
    : 'flask'}
  <Icon {name} />
{/snippet}

{#snippet skillRow(m: AgentMsg)}
  {@const p = parsedInput(m)}
  {@const name = skillActivationName(m)}
  {@const args = typeof p?.args === 'string' ? p.args : ''}
  <div class="skillrow" title={m.resultError ? (m.result || 'failed') : (name || 'Skill')}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name="sparkles" />
    </span>
    <span class="skill-name">{name || 'Unknown skill'}</span>
    {#if args}<span class="skill-args">{args}</span>{/if}
    <span class="skill-tag">
      {#if m.resultError}Failed{:else if m.resultPending}Activating…{:else}Activated{/if}
    </span>
  </div>
{/snippet}

{#snippet listSkillsRow(m: AgentMsg)}
  <div class="skillrow" title={m.resultError ? (m.result || 'failed') : ''}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name="sparkles" />
    </span>
    <span class="skill-name">
      {#if m.resultError}List skills{:else if m.resultPending}Listing skills{:else}Listed skills{/if}
    </span>
    {#if m.resultError}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet switchModeRow(m: AgentMsg)}
  {@const tr = switchModeTransition(m)}
  {@const toKey = tr?.to || switchModeTarget(m)}
  {@const fromKey = tr?.from || ''}
  {@const to = modeLabel(toKey)}
  {@const from = modeLabel(fromKey)}
  {@const failed = switchModeFailed(m)}
  <div class="skillrow" title={failed ? (m.result || 'failed') : ''}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name="switch-3" />
    </span>
    <span class="skill-name">
      {#if failed}
        Mode switch failed
      {:else if m.resultPending}
        Switching mode
      {:else if fromKey && toKey}
        Switched
        <span class="mode-tint" style:color={modeTint(fromKey)}>{from}</span>
        <span class="mode-arrow" aria-hidden="true">→</span>
        <span class="mode-tint" style:color={modeTint(toKey)}>{to}</span>
      {:else if toKey}
        Switched to
        <span class="mode-tint" style:color={modeTint(toKey)}>{to}</span>
      {:else}
        Switched mode
      {/if}
    </span>
    {#if failed}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet renameToolRow(m: AgentMsg)}
  {@const title = renameToolValue(m)}
  {@const noun = renameToolNoun(m)}
  {@const failed = renameToolFailed(m)}
  <div class="skillrow" title={failed ? (m.result || 'failed') : (title || '')}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name={isRenameBranch(m) ? 'git-branch' : 'a-large-small'} />
    </span>
    <span class="skill-name skill-name-ellipsis">
      {#if failed}
        Rename failed
      {:else if m.resultPending}
        {#if title}
          Renaming {noun} to <span class="rename-title">{title}</span>
        {:else}
          Renaming {noun}
        {/if}
      {:else if title}
        Renamed {noun} to <span class="rename-title">{title}</span>
      {:else}
        Renamed {noun}
      {/if}
    </span>
    {#if failed}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet reenterPhaseRow(m: AgentMsg)}
  {@const phase = reenterPhaseID(m)}
  {@const failed = !!m.resultError || reenterPhaseFailed(m.result ?? '')}
  <div class="skillrow" title={failed ? (m.result || 'failed') : (phase || '')}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name="refresh-cw" />
    </span>
    <span class="skill-name">
      {#if failed}
        Re-enter failed
      {:else if m.resultPending}
        Re-entering
      {:else}
        Re-entered
      {/if}
    </span>
    {#if phase}
      <span class="skill-args">{phase}</span>
    {/if}
    {#if failed}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet childToolRow(m: AgentMsg)}
  {@const failed = childToolFailed(m)}
  {@const note = childToolNote(m)}
  <div class="skillrow" title={m.result || ''}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name={isMergeChild(m) ? 'git-merge' : isArchiveChild(m) ? 'inbox' : isMessageChild(m) ? 'message' : 'eye'} />
    </span>
    <span class="skill-name skill-name-ellipsis">{childToolLabel(m)}</span>
    {#if note}
      <span class="skill-args">{note}</span>
    {/if}
    {#if failed}
      <span class="skill-tag">refused</span>
    {/if}
  </div>
{/snippet}

{#snippet openPRRow(m: AgentMsg)}
  {@const card = openPRCard(m)}
  {@const prId = openPRId(m)}
  {@const failed = card.failed}
  <div class="skillrow" title={failed ? (m.result || 'failed') : (card.title || '')}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name={card.draft ? 'git-pull-request-draft' : 'git-pull-request'} />
    </span>
    <span class="skill-name skill-name-ellipsis">
      {openPRLabel(card)}
    </span>
    {#if card.title && !failed}
      <span class="skill-args">{card.title}</span>
    {/if}
    {#if prId && card.number > 0 && !failed && !card.pending}
      <button type="button" class="turn-end-link" onclick={() => openPR(prId)}>View PR</button>
    {/if}
    {#if card.promoted && !failed}
      <span class="skill-tag">promoted</span>
    {/if}
    {#if failed}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet createArtifactRow(m: AgentMsg)}
  {@const title = createArtifactTitle(m)}
  {@const kind = createArtifactKind(m)}
  {@const failed = createArtifactFailed(m)}
  <div class="skillrow" title={failed ? (m.result || 'failed') : (title || '')}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name={artifactIcon(kind)} />
    </span>
    <span class="skill-name skill-name-ellipsis">
      {#if failed}
        Create failed
      {:else if m.resultPending}
        {#if title}
          Creating <span class="rename-title">{title}</span>
        {:else}
          Creating artifact
        {/if}
      {:else if title}
        Created <span class="rename-title">{title}</span>
      {:else}
        Created artifact
      {/if}
    </span>
    {#if kind && !failed}
      <span class="skill-tag">{kind}</span>
    {/if}
    {#if failed}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet presentArtifactRow(m: AgentMsg)}
  {@const title = presentArtifactTitle(m)}
  {@const failed = presentArtifactFailed(m)}
  <div class="skillrow" title={failed ? (m.result || 'failed') : (title || '')}>
    <span class="skill-ico" aria-hidden="true">
      <Icon name="maximize" />
    </span>
    <span class="skill-name skill-name-ellipsis">
      {#if failed}
        Present failed
      {:else if m.resultPending}
        {#if title}
          Presenting <span class="rename-title">{title}</span>
        {:else}
          Presenting artifact
        {/if}
      {:else if title}
        Presented <span class="rename-title">{title}</span>
      {:else}
        Presented artifact
      {/if}
    </span>
    {#if failed}
      <span class="skill-tag">failed</span>
    {/if}
  </div>
{/snippet}

{#snippet presentWorkspaceCard(card: PresentWorkspaceView, compact = false)}
  {#if card.brief}
    <WorkspaceBriefCard brief={card.brief} openTarget={card.target} compact={compact} />
  {/if}
{/snippet}

{#snippet briefingToolBody(m: AgentMsg, rowKey: string | number)}
  {#if m.resultPending}
    <div class="tc-wait">
      <span class="tc-wait-dot" aria-hidden="true"></span>
      <span>{pendingLabel(m)}</span>
    </div>
  {:else if isListWorkspaceBriefs(m)}
    {@const listed = listWorkspaceBriefsResult(m)}
    {#if listed}
      <div class="tc-finding-empty">{listWorkspaceBriefsSummary(listed)}</div>
    {:else if m.result}
      <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
    {:else}
      <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
    {/if}
  {:else if isGetWorkspaceContext(m)}
    {@const ctx = workspaceContextResult(m)}
    {#if ctx}
      <div class="tc-finding-empty">{workspaceContextSummary(ctx)}</div>
    {:else if m.result}
      <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
    {:else}
      <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
    {/if}
  {:else}
    {@const card = presentWorkspaceResult(m)}
    {#if m.resultError || card?.error}
      <pre class="tc-pre tc-out tc-outerr">{card?.error || m.result || 'failed'}</pre>
    {:else if card}
      <div class="tc-briefs">
        {@render presentWorkspaceCard(card, false)}
      </div>
    {:else if m.result}
      <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
    {:else}
      <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
    {/if}
  {/if}
{/snippet}

{#snippet artifactListRow(a: ListedArtifact, when: { rel: string; full: string } | null)}
  <span class="tc-art-ico" aria-hidden="true">
    <Icon name={artifactIcon(a.kind)} />
  </span>
  <span class="tc-finding-main">
    <span class="tc-finding-title">{a.title || a.id || 'Untitled'}</span>
    {#if a.createdBy || when}
      <span class="tc-finding-sub">
        {#if a.createdBy}<span class="tc-art-by">{a.createdBy}</span>{/if}
        {#if when}<span class="tc-art-age" title={when.full}>{when.rel}</span>{/if}
      </span>
    {/if}
  </span>
  {#if a.kind}
    <span class="tc-art-kind">{a.kind}</span>
  {/if}
{/snippet}

{#snippet nestedChild(m: AgentMsg, rowKey: string)}
  {#if m.tool}
    {@render toolCard(m, rowKey)}
  {:else if m.thinking}
    {@render thinkingCard(m, rowKey)}
  {:else if m.compact}
    {@render compactCard(m, rowKey)}
  {:else if m.body}
    <div class="tc-nested-text" class:live={m.streaming}>
      {#if m.streaming}
        <StreamingMarkdown text={m.body} active={shellActive && presentationActive} complete={!m.streaming} />
      {:else}
        <MarkdownHtml text={m.body} class="chat-canvas tc-finding-body" />
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet claudeTaskItem(td: ClaudeTaskView)}
  <li
    class="todo-row"
    class:done={td.status === 'completed'}
    class:active={td.status === 'in_progress'}
    class:blocked={td.blockedBy.length > 0}
  >
    <span class="todo-box" aria-hidden="true">
      {#if td.status === 'completed'}
        <Icon name="check" strokeWidth={3} />
      {:else if td.status === 'in_progress'}
        <Icon name="loader" />
      {/if}
    </span>
    {#if td.id}<span class="todo-id">{formatTaskId(td.id)}</span>{/if}
    <span class="todo-text">
      {td.subject}
      {#if td.owner}<span class="todo-extra">{td.owner}</span>{/if}
      {#if td.blockedBy.length}
        <span class="todo-extra">blocked by {td.blockedBy.map((id) => formatTaskId(id)).join(', ')}</span>
      {/if}
    </span>
  </li>
{/snippet}

{#snippet claudeTaskCard(m: AgentMsg)}
  {@const args = claudeTaskArgsOf(m)}
  {@const listed = m.tool === 'TaskList' && !m.resultError ? claudeTaskListItems(m) : []}
  {@const got = m.tool === 'TaskGet' ? claudeTaskGetView(m) : null}
  {#if m.resultPending}
    <div class="tc-wait">
      <span class="tc-wait-dot" aria-hidden="true"></span>
      <span>{pendingLabel(m)}</span>
    </div>
  {/if}
  {#if m.tool === 'TaskList'}
    {#if !m.resultPending && !m.resultError}
      {#if listed && listed.length}
        <ul class="todos">
          {#each listed as td, i ((td.id || td.subject) + ':' + i)}
            {@render claudeTaskItem(td)}
          {/each}
        </ul>
      {:else if listed === null}
        <pre class="tc-pre tc-out">{m.result}</pre>
      {:else}
        <div class="tc-finding-empty">No tasks.</div>
      {/if}
    {/if}
  {:else if m.tool === 'TaskGet'}
    {#if got}
      {#if got.description}
        <div class="tc-finding-msg">
          <MarkdownHtml text={got.description} class="chat-canvas tc-finding-body" />
        </div>
      {/if}
      {#if got.owner || got.blockedBy.length || got.blocks.length}
        <div class="tc-plan-update">
          {#if got.owner}
            <p class="tc-plan-update-note"><span>Owner</span> {got.owner}</p>
          {/if}
          {#if got.blockedBy.length}
            <p class="tc-plan-update-note"><span>Blocked by</span> {got.blockedBy.map((id) => formatTaskId(id)).join(', ')}</p>
          {/if}
          {#if got.blocks.length}
            <p class="tc-plan-update-note"><span>Blocks</span> {got.blocks.map((id) => formatTaskId(id)).join(', ')}</p>
          {/if}
        </div>
      {/if}
    {:else if !m.resultPending && !m.resultError && m.result}
      <pre class="tc-pre tc-out">{m.result}</pre>
    {:else if !m.resultPending && args.description}
      <div class="tc-finding-msg">
        <MarkdownHtml text={args.description} class="chat-canvas tc-finding-body" />
      </div>
    {/if}
  {:else if m.tool === 'TaskCreate'}
    <div class="tc-finding-msg">
      <MarkdownHtml text={args.description} class="chat-canvas tc-finding-body" />
    </div>
  {:else if m.tool === 'TaskUpdate'}
    {#if args.owner || args.addBlocks.length || args.addBlockedBy.length}
      <div class="tc-plan-update">
        {#if args.owner}
          <p class="tc-plan-update-note"><span>Owner</span> {args.owner}</p>
        {/if}
        {#if args.addBlockedBy.length}
          <p class="tc-plan-update-note"><span>Blocked by</span> {args.addBlockedBy.map((id) => formatTaskId(id)).join(', ')}</p>
        {/if}
        {#if args.addBlocks.length}
          <p class="tc-plan-update-note"><span>Blocks</span> {args.addBlocks.map((id) => formatTaskId(id)).join(', ')}</p>
        {/if}
      </div>
    {/if}
    {#if args.description}
      <div class="tc-finding-msg">
        <MarkdownHtml text={args.description} class="chat-canvas tc-finding-body" />
      </div>
    {/if}
  {/if}
  {#if m.resultError && m.result}
    <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
  {:else if m.result && m.tool && !claudeTaskResultIsAck(m.tool, m.result) && m.tool !== 'TaskList' && m.tool !== 'TaskGet'}
    <pre class="tc-pre tc-out">{m.result}</pre>
  {/if}
{/snippet}

{#snippet linearCard(m: AgentMsg)}
  {@const view = linearViewOf(m)}
  {@const inputBody = linearInputBody(parsedInput(m))}
  {@const item = view?.kind === 'item' ? view.items[0] : null}
  {@const body = item?.body || (view?.kind === 'list' ? '' : inputBody)}
  {#if m.resultPending}
    <div class="tc-wait">
      <span class="tc-wait-dot" aria-hidden="true"></span>
      <span>{pendingLabel(m)}</span>
    </div>
  {/if}
  {#if view?.kind === 'list'}
    {#if !view.items.length}
      <div class="tc-finding-empty">No {view.noun || 'results'}.</div>
    {:else}
      <div class="tc-findings-head">
        <span class="tc-findings-count">{linearCountLabel(view)}</span>
        {#if view.hasNextPage}
          <span class="tc-findings-more">more</span>
        {/if}
      </div>
      <ul class="tc-findings-list tc-lin-list">
        {#each view.items as it, ii (it.url || it.identifier || it.title + ':' + ii)}
          <li class="tc-finding-row">
            <div class="tc-finding-row-head">
              {#if it.color}
                <span class="tc-lin-dot" style:background={it.color} aria-hidden="true"></span>
              {/if}
              <span class="tc-finding-main">
                <span class="tc-finding-title">{it.title}</span>
                {#if it.summary}
                  <span class="tc-finding-sub">{it.summary}</span>
                {/if}
              </span>
              {#if it.identifier}
                <span class="tc-task-id">{it.identifier}</span>
              {/if}
              {#each it.chips as chip (chip)}
                <span class="tc-chip">{chip}</span>
              {/each}
              {#if it.url && isExternalHttpUrl(it.url)}
                <a
                  class="tc-url"
                  href={it.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  title={it.url}
                >{compactUrl(it.url)}</a>
              {:else if it.url}
                <span class="tc-url" title={it.url}>{compactUrl(it.url)}</span>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {:else if item}
    {#if item.title}
      <div class="tc-findings-head">
        {#if item.color}
          <span class="tc-lin-dot" style:background={item.color} aria-hidden="true"></span>
        {/if}
        <span class="tc-findings-count">{item.title}</span>
        {#if item.identifier && item.identifier !== item.title}
          <span class="tc-task-id">{item.identifier}</span>
        {/if}
      </div>
    {/if}
    {#if item.chips.length}
      <div class="tc-task-chips">
        {#each item.chips as chip (chip)}
          <span class="tc-chip">{chip}</span>
        {/each}
      </div>
    {/if}
    {#if body}
      <div class="tc-finding-msg">
        <MarkdownHtml text={body} class="chat-canvas tc-finding-body" />
      </div>
    {/if}
  {:else if body}
    <div class="tc-finding-msg">
      <MarkdownHtml text={body} class="chat-canvas tc-finding-body" />
    </div>
  {/if}
  {#if !view && m.resultError && m.result}
    <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
  {:else if !view && m.result && !m.resultPending}
    <pre class="tc-pre tc-out">{prettyResult(m.result)}</pre>
  {/if}
{/snippet}

{#snippet toolCard(m: AgentMsg, rowKey: string | number)}
  <!-- ToolSearch select:… is a flat one-liner (names live in the header meta).
       Expanding only re-stated the same chips — nothing to open unless it failed.
       A finished Delete is the same: path is in the label/chip, result is empty. -->
  {@const toolSearchSel = m.tool === 'ToolSearch' ? toolSearchSelects(toolSearchQuery(m)) : null}
  {@const deleteQuiet = m.tool === 'Delete' && !m.resultPending && !m.resultError && !(m.result ?? '').trim()}
  {@const linQuiet = isLinearTool(m.tool) && !m.resultPending && !m.resultError && !(m.result ?? '').trim() && !linearInputBody(parsedInput(m))}
  {@const webQuiet = (m.tool === 'WebFetch' || m.tool === 'WebSearch') && !webHasCard(m)}
  {@const claudeTask = isClaudeTaskTool(m.tool)}
  {@const claudeTaskBody = claudeTask && claudeTaskHasBody(m.tool!, claudeTaskArgsOf(m), {
    resultError: !!m.resultError,
    result: m.result,
    pending: !!m.resultPending,
  })}
  {@const expandable = (!(toolSearchSel?.length && !m.resultError) && !deleteQuiet && !linQuiet && !webQuiet && !(claudeTask && !claudeTaskBody && !m.resultError)) || !!(m.children?.length)}
  {@const liveNested = hasLiveChildren(m)}
  {@const open = expandable && rowIsOpen(rowKey, liveNested || isBriefingTool(m.tool))}
  {@const claudeStatus = claudeTask ? claudeTaskChipStatus(m.tool!, claudeTaskArgsOf(m), m.result ?? '') : ''}
  {@const claudeId = claudeTask ? claudeTaskRowId(m) : ''}
  <div class="tc" class:err={m.resultError}>
    <div
      class="tc-head"
      class:static={!expandable}
      role="button"
      tabindex={expandable ? 0 : -1}
      aria-disabled={expandable ? undefined : true}
      aria-expanded={expandable ? open : undefined}
      onclick={() => expandable && toggleRow(rowKey, open)}
      onkeydown={(e) => { if (expandable && e.key === 'Enter') toggleRow(rowKey, open) }}
    >
      <span class="tc-ico" aria-hidden="true">
        <span class="ico">{@render stepIcon(toolIcon(m))}</span>
        {#if expandable}<span class="ex">{open ? '−' : '+'}</span>{/if}
      </span>
      <span class="tc-label">{toolLabel(m)}</span>
      {#if (isReportFinding(m) || isUpdateFinding(m)) && findingSeverity(m)}
        <span class="finding-sev {findingSeverity(m)}" aria-hidden="true">
          <Icon name={findingSevIcon(findingSeverity(m))} />
        </span>
      {/if}
      {#if isUpdateFinding(m) && findingStatus(m)}
        <span class="tc-finding-status {findingStatus(m)}">{findingStatus(m)}</span>
      {/if}
      {#if isUpdatePlanStep(m) && updatePlanStepStatus(m)}
        <span class="tc-finding-status {updatePlanStepStatus(m)}">{updatePlanStepStatus(m)}</span>
      {/if}
      {#if claudeStatus}
        <span class="tc-finding-status {taskStatusChipClass(claudeStatus)}">{prettyTaskStatus(claudeStatus)}</span>
      {/if}
      {#if claudeId}
        <span class="tc-task-id">{claudeId}</span>
      {/if}
      {#if isSubagentTool(m.tool) || isTaskTool(m.tool)}
        {@const sa = subagentOf(m)}
        {#if sa.subagentType}<span class="tc-chip">{sa.subagentType}</span>{/if}
        {#if sa.model}<span class="tc-chip">{sa.model}</span>{/if}
        {#if taskBackground(m)}<span class="tc-chip bg">background</span>{/if}
      {/if}
      {#if fileOf(m) && onOpenFile}
        {@const openLine = (isReportFinding(m) || isUpdateFinding(m)) ? findingLine(m) : 0}
        {@const loc = (isReportFinding(m) || isUpdateFinding(m)) ? findingLocSuffix(m) : ''}
        <span
          class="tc-file"
          role="button"
          tabindex="0"
          title="Open in editor"
          onclick={(e) => { e.stopPropagation(); onOpenFile?.(fileOf(m), openLine > 0 ? openLine : undefined) }}
          onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); onOpenFile?.(fileOf(m), openLine > 0 ? openLine : undefined) } }}
        >{baseName(fileOf(m))}{loc || (openLine > 0 ? `:${openLine}` : '')}</span>
      {/if}
      {#if m.tool === 'WebFetch' && fetchUrl(m)}
        {@const href = fetchUrl(m)}
        {#if isExternalHttpUrl(href)}
          <a
            class="tc-url"
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            title={href}
            onclick={(e) => e.stopPropagation()}
            onkeydown={(e) => e.stopPropagation()}
          >{compactUrl(href)}</a>
        {:else}
          <span class="tc-url" title={href}>{compactUrl(href)}</span>
        {/if}
      {/if}
      {#if isLinearTool(m.tool)}
        {@const href = linearItemUrl(linearViewOf(m))}
        {#if href && isExternalHttpUrl(href)}
          <a
            class="tc-url"
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            title={href}
            onclick={(e) => e.stopPropagation()}
            onkeydown={(e) => e.stopPropagation()}
          >{compactUrl(href)}</a>
        {:else if href}
          <span class="tc-url" title={href}>{compactUrl(href)}</span>
        {/if}
      {/if}
      {#if hasEditDiff(m)}
        {@const c = diffCounts(editDiff(m))}
        <span class="tc-diffct"><i class="a">+{c.add}</i> <i class="d">−{c.del}</i></span>
      {/if}
      <!-- Finding / plan / lint bodies already show the full content; don't repeat previews. -->
      {#if toolMeta(m) && toolMeta(m) !== fileOf(m) && !(open && (isReportFinding(m) || isUpdateFinding(m))) && !(open && isListFindings(m)) && !(open && isListArtifacts(m)) && !(open && isDone(m)) && !(open && isAskUser(m)) && !(open && isReadLints(m)) && !(open && isSubmitPlan(m)) && !(open && isUpdatePlanStep(m)) && !(open && isEditPlan(m)) && !(open && isBriefingTool(m.tool)) && !(open && isLinearTool(m.tool))}
        <span class="tc-meta">{toolMeta(m)}</span>
      {/if}
      {#if m.resultError}<span class="tc-errbadge">failed</span>{/if}
      <!-- The provider clipped this payload; without saying so a fragment reads
           as the whole output. -->
      {#if m.resultTruncated}<span class="tc-trunc" title="The provider truncated this result because it was too large">truncated</span>{/if}
      {#if m.tool === 'Bash'}
        <button
          type="button"
          class="tc-copy"
          class:copied={copiedRow === rowKey}
          title="Copy command"
          aria-label="Copy command"
          onclick={(e) => copyCommand(m, rowKey, e)}
        >
          {#if copiedRow === rowKey}
            <Icon name="check" strokeWidth={2.4} />
          {:else}
            <Icon name="copy" />
          {/if}
        </button>
      {/if}
    </div>
    {#if open}
      <div class="tc-body">
        <div class="tc-card">
          {#if m.tool === 'TodoWrite' && todoItems(m).length}
            <!-- A checklist, not a JSON blob: a dumped todos array is unreadable
                 and hides the one thing that matters — what is done, what is
                 running, and what is left. -->
            <ul class="todos">
              {#each todoItems(m) as td, ti (ti)}
                <li class="todo-row" class:done={td.status === 'completed'} class:active={td.status === 'in_progress'}>
                  <span class="todo-box" aria-hidden="true">
                    {#if td.status === 'completed'}
                      <Icon name="check" strokeWidth={3} />
                    {/if}
                  </span>
                  <span class="todo-text">{td.content}</span>
                </li>
              {/each}
            </ul>
          {:else if claudeTask}
            {@render claudeTaskCard(m)}
          {:else if isTaskTool(m.tool)}
            <!-- Subagent card: prompt as prose. Never dump {description,prompt,…}
                 JSON, and never the model-only "Async agent launched…" ack. -->
            {@const sa = subagentOf(m)}
            {@const launched = !m.resultError && isSubagentLaunchAck(m.result ?? '')}
            {#if m.resultPending || launched}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{launched ? 'Running in the background' : pendingLabel(m)}</span>
              </div>
              {#if sa.prompt}
                <pre class="tc-pre tc-task-prompt">{sa.prompt}</pre>
              {/if}
            {:else}
              {#if sa.prompt}
                <pre class="tc-pre tc-task-prompt">{sa.prompt}</pre>
              {/if}
              {#if m.result}
                <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{m.result}</pre>
              {:else if !sa.prompt}
                <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
              {/if}
            {/if}
          {:else if isSendMessageTool(m.tool)}
            {@const sm = parseSendMessageArgs(parsedInput(m))}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if sm.message}
              <pre class="tc-pre tc-task-prompt">{sm.message}</pre>
            {/if}
            {#if m.result && !isSubagentLaunchAck(m.result)}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{m.result}</pre>
            {/if}
          {:else if isReportFinding(m) || isUpdateFinding(m)}
            <!-- Finding card: message as markdown. Severity + file:line already
                 live in the row header — chips here just restated them.
                 update_finding adds status (header) + optional reason below.
                 Never dump {file,line,severity,message} JSON. -->
            {@const msg = findingMessage(m)}
            {@const reason = findingStatusReason(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if msg}
              <div class="tc-finding-msg">
                <MarkdownHtml text={msg} class="chat-canvas tc-finding-body" />
              </div>
            {:else if !m.resultPending && !reason}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
            {#if reason}
              <p class="tc-finding-reason"><span>Reason</span> {reason}</p>
            {/if}
            {#if m.resultError && m.result}
              <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
            {/if}
          {:else if isDone(m)}
            <!-- Turn-complete card: summary as markdown prose. Never dump
                 {summary} JSON or the plain "Acknowledged…" result text. -->
            {@const summary = doneSummary(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if summary}
              <div class="tc-finding-msg">
                <MarkdownHtml text={summary} class="chat-canvas tc-finding-body" />
              </div>
            {:else if !m.resultPending}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
            {#if m.resultError && m.result}
              <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
            {/if}
          {:else if isAskUser(m)}
            <!-- Ask-user card: question as prose. Park returns a short ack; the
                 live inline QuestionCard holds the answer UI (off-tab is a toast);
                 the answer resumes as a user message. Never dump {}. -->
            {@const q = askUserQuestion(m)}
            {@const ctx = askUserContext(m)}
            {#if m.resultPending && !q}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if q}
              <div class="tc-finding-msg">
                <div class="tc-finding-body">
                  <p>{q}</p>
                  {#if ctx}<p class="ask-tool-ctx">{ctx}</p>{/if}
                </div>
              </div>
            {/if}
            {#if !m.resultPending && m.result && !m.result.includes('Question delivered')}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{m.result}</pre>
            {:else if m.resultError && m.result}
              <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
            {/if}
          {:else if isListFindings(m)}
            {@const listed = listFindingsResult(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {:else if listed}
              {#if !listed.findings.length}
                <div class="tc-finding-empty">No {listed.status || 'open'} findings.</div>
              {:else}
                {@const tally = findingSeverityTally(listed.findings)}
                {@const filter = listed.status || 'open'}
                <div class="tc-findings-head">
                  <span class="tc-findings-count">
                    {listed.count} {listed.count === 1 ? 'finding' : 'findings'}
                  </span>
                  {#if filter !== 'open'}
                    <span class="tc-findings-filter">{filter}</span>
                  {/if}
                  {#if listed.count > listed.findings.length}
                    <span class="tc-findings-more">showing {listed.findings.length}</span>
                  {/if}
                  {#if tally.length > 1 || (tally[0] && tally[0].severity !== 'info')}
                    <span class="tc-findings-tally">
                      {#each tally as t (t.severity)}
                        <span class="tc-findings-tally-item">
                          <span class="finding-sev {t.severity}" aria-hidden="true">
                            <Icon name={findingSevIcon(t.severity)} />
                          </span>
                          {t.count}
                        </span>
                      {/each}
                    </span>
                  {/if}
                </div>
                <ul class="tc-findings-list tc-finding-accord">
                  {#each listed.findings as f, fi (f.id || `${f.file}-${f.line}-${fi}`)}
                    {@const loc = listedLocSuffix(f)}
                    {@const locLabel = findingLocLabel(f.file, f.line, f.endLine)}
                    {@const title = findingTitle(f.message || f.summary || '')}
                    {@const accordKey = f.id || `listed-${fi}-${f.file}-${f.line}`}
                    {@const open = !!findingAccordOpen[accordKey]}
                    {@const noteCount = f.notes?.length ?? 0}
                    {@const showStatus = !!f.status && (filter === 'all' || f.status !== filter)}
                    <li class="tc-finding-row" class:open>
                      <button
                        type="button"
                        class="tc-finding-row-head"
                        aria-expanded={open}
                        onclick={() => toggleFindingAccord(accordKey)}
                      >
                        <span class="finding-sev {f.severity || 'low'}" aria-hidden="true">
                          <Icon name={findingSevIcon(f.severity)} />
                        </span>
                        <span class="tc-finding-main">
                          <span class="tc-finding-title">{title}</span>
                          {#if locLabel}
                            <span class="tc-finding-sub">
                              <span class="tc-finding-line" title={f.file ? `${relPath(f.file)}${loc}` : undefined}>{locLabel}</span>
                            </span>
                          {/if}
                        </span>
                        <span class="tc-finding-trail">
                          {#if noteCount > 0}
                            <span
                              class="finding-has-notes"
                              title={noteCount === 1 ? '1 note' : `${noteCount} notes`}
                              aria-label={noteCount === 1 ? '1 note' : `${noteCount} notes`}
                            >
                              <Icon name="notebook-pen" />
                            </span>
                          {/if}
                          {#if showStatus}
                            <span class="tc-finding-status {f.status}">{f.status}</span>
                          {/if}
                        </span>
                      </button>
                      {#if open}
                        <div class="tc-finding-detail" transition:slide={findingAccordSlide}>
                          {#if f.file}
                            <div class="tc-finding-detail-loc">
                              {#if onOpenFile}
                                <button
                                  type="button"
                                  class="tc-file"
                                  title="Open in editor"
                                  onclick={() => onOpenFile?.(
                                    f.file,
                                    f.line && f.line > 0 ? f.line : undefined,
                                    f.endLine && f.line && f.endLine > f.line ? f.endLine : undefined,
                                  )}
                                >{locLabel || `${baseName(f.file)}${loc}`}</button>
                              {:else}
                                <span class="tc-finding-loc">{relPath(f.file)}{loc}</span>
                              {/if}
                            </div>
                          {/if}
                          {#if f.message && findingBodyNeeded(f.message)}
                            <div class="tc-finding-msg">
                              <MarkdownHtml text={f.message} class="chat-canvas tc-finding-body" />
                            </div>
                          {/if}
                          {#if f.statusReason}
                            <p class="tc-finding-reason"><span>Reason</span> {f.statusReason}</p>
                          {/if}
                          {#if noteCount > 0}
                            <div class="tc-finding-notes">
                              {#each f.notes ?? [] as n (n.id || n.body)}
                                <div class="tc-finding-note-row">
                                  <p class="tc-finding-reason">
                                    <span>Note</span>
                                    {n.body}
                                  </p>
                                </div>
                              {/each}
                            </div>
                          {/if}
                        </div>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
            {:else if m.result}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
            {:else}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
          {:else if isListArtifacts(m)}
            <!-- Artifact list: title + kind rows, never the {artifacts:[…]} dump. -->
            {@const listed = listArtifactsResult(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {:else if listed}
              {#if listed.count === 0 && !listed.artifacts.length}
                <div class="tc-finding-empty">No artifacts yet.</div>
              {:else}
                {@const tally = artifactKindTally(listed.artifacts)}
                <div class="tc-findings-head">
                  <span class="tc-findings-count">
                    {listed.count} {listed.count === 1 ? 'artifact' : 'artifacts'}
                  </span>
                  {#if listed.count > listed.artifacts.length}
                    <span class="tc-findings-more">showing {listed.artifacts.length}</span>
                  {/if}
                  {#if tally.length > 1}
                    <span class="tc-findings-tally">
                      {#each tally as t (t.kind)}
                        <span class="tc-findings-tally-item">
                          <span class="tc-art-ico" aria-hidden="true">
                            <Icon name={artifactIcon(t.kind)} />
                          </span>
                          {t.count}
                        </span>
                      {/each}
                    </span>
                  {/if}
                </div>
                <ul class="tc-findings-list tc-art-list">
                  {#each listed.artifacts as a, ai (a.id || `${a.path}-${ai}`)}
                    {@const when = artifactWhen(a.createdAt)}
                    {@const canOpen = !!onOpenArtifact && artifactCanOpen(a)}
                    <li class="tc-finding-row">
                      {#if canOpen}
                        <button
                          type="button"
                          class="tc-art-row-head"
                          title={a.path ? `Open ${a.title || a.id}` : (a.title || a.id)}
                          onclick={() => onOpenArtifact?.(a)}
                        >
                          {@render artifactListRow(a, when)}
                        </button>
                      {:else}
                        <div class="tc-art-row-head static" title={a.path || a.title || a.id}>
                          {@render artifactListRow(a, when)}
                        </div>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
            {:else if m.result}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
            {:else}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
          {:else if isListWorkspaceBriefs(m) || isGetWorkspaceContext(m) || isPresentWorkspace(m)}
            {@render briefingToolBody(m, rowKey)}
          {:else if m.tool === 'ToolSearch'}
            <!-- Keyword search (select:… is a non-expandable one-liner above).
                 Never dump {query,max_results} JSON. -->
            {@const query = toolSearchQuery(m)}
            {@const max = toolSearchMax(m)}
            {#if query}
              <div class="tc-pre tc-grepq">
                <span class="sh-str">"{query}"</span>
                {#if max > 0}<span class="grep-flag">max {max}</span>{/if}
              </div>
            {:else}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {:else if m.result}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{m.result}</pre>
            {/if}
          {:else if isReadLints(m)}
            <!-- Lint card: path list + per-file diagnostics. Never dump the
                 minified {fileDiagnostics,…} JSON blob. -->
            {@const lint = readLintsResult(m)}
            {@const paths = readLintsPaths(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
              {#if paths.length}
                <ul class="tc-lint-paths">
                  {#each paths as p (p)}
                    <li class="tc-lint-path">
                      {#if onOpenFile}
                        <button type="button" class="tc-file" title="Open in editor" onclick={() => onOpenFile?.(p)}>{baseName(p)}</button>
                        <span class="tc-lint-rel">{relPath(p)}</span>
                      {:else}
                        <span class="tc-lint-rel">{relPath(p)}</span>
                      {/if}
                    </li>
                  {/each}
                </ul>
              {/if}
            {:else if lint}
              {#if lint.totalDiagnostics === 0}
                <div class="tc-finding-empty">
                  No linter issues{#if lint.totalFiles > 0} in {lint.totalFiles} file{lint.totalFiles === 1 ? '' : 's'}{/if}.
                </div>
                {#if lint.files.length}
                  <ul class="tc-lint-paths">
                    {#each lint.files as f (f.path)}
                      <li class="tc-lint-path">
                        {#if f.path && onOpenFile}
                          <button type="button" class="tc-file" title="Open in editor" onclick={() => onOpenFile?.(f.path)}>{baseName(f.path)}</button>
                          <span class="tc-lint-rel">{relPath(f.path)}</span>
                        {:else if f.path}
                          <span class="tc-lint-rel">{relPath(f.path)}</span>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                {/if}
              {:else}
                <ul class="tc-findings-list tc-lint-list">
                  {#each lint.files as f (f.path)}
                    {#if f.diagnostics.length}
                      {#each f.diagnostics as d, di (di)}
                        {@const loc = lintLocSuffix(d)}
                        <li class="tc-finding-row">
                          <div class="tc-finding-row-head">
                            {#if d.severity}
                              <span class="tc-sev {lintSevClass(d.severity)}">{d.severity}</span>
                            {/if}
                            {#if f.path && onOpenFile}
                              <button
                                type="button"
                                class="tc-file"
                                title="Open in editor"
                                onclick={() => onOpenFile?.(f.path, d.line > 0 ? d.line : undefined)}
                              >{baseName(f.path)}{loc}</button>
                            {:else if f.path}
                              <span class="tc-finding-loc">{relPath(f.path)}{loc}</span>
                            {/if}
                            {#if d.source}
                              <span class="tc-chip">{d.source}</span>
                            {/if}
                          </div>
                          {#if d.message}
                            <div class="tc-finding-msg">
                              <p class="tc-finding-body">{d.message}</p>
                            </div>
                          {/if}
                        </li>
                      {/each}
                    {/if}
                  {/each}
                </ul>
              {/if}
            {:else if m.result}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
            {:else if paths.length}
              <ul class="tc-lint-paths">
                {#each paths as p (p)}
                  <li class="tc-lint-path">
                    {#if onOpenFile}
                      <button type="button" class="tc-file" title="Open in editor" onclick={() => onOpenFile?.(p)}>{baseName(p)}</button>
                      <span class="tc-lint-rel">{relPath(p)}</span>
                    {:else}
                      <span class="tc-lint-rel">{relPath(p)}</span>
                    {/if}
                  </li>
                {/each}
              </ul>
            {:else}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
          {:else if isSubmitPlan(m)}
            <!-- Structured plan: summary + detailed body + checklist. Never dump JSON. -->
            {@const summary = submitPlanSummary(m)}
            {@const body = submitPlanBody(m)}
            {@const steps = submitPlanSteps(m)}
            {@const attIds = submitPlanAttIds(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if summary}
              <div class="tc-plan-summary">{summary}</div>
            {/if}
            {#if body}
              <MarkdownHtml text={body} class="chat-canvas tc-plan-body" />
            {/if}
            {#if steps.length}
              <ol class="tc-plan-steps">
                {#each steps as s (s.id)}
                  <li class="tc-plan-step" class:done={s.status === 'done' || s.status === 'completed'} class:skipped={s.status === 'skipped'}>
                    <span class="tc-plan-step-mark" aria-hidden="true">{PLAN_STEP_MARK[s.status] ?? '○'}</span>
                    <span class="tc-plan-step-id">{s.id}</span>
                    <span class="tc-plan-step-title">{s.title}</span>
                  </li>
                {/each}
              </ol>
            {:else if !summary && !body && !m.resultPending}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
            {#if attIds.length}
              <div class="tc-plan-atts">
                {#each attIds as id (id)}
                  <span class="tc-plan-att">{id}</span>
                {/each}
              </div>
            {/if}
            {#if m.resultError && m.result}
              <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
            {/if}
          {:else if isUpdatePlanStep(m)}
            <!-- Compact status flip: id + title + optional note. Never JSON. -->
            {@const id = updatePlanStepId(m)}
            {@const title = updatePlanStepTitle(m)}
            {@const note = updatePlanStepNote(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if id || title || note}
              <div class="tc-plan-update">
                <div class="tc-plan-update-row">
                  {#if id}<span class="tc-plan-update-id">{id}</span>{/if}
                  {#if title}<span class="tc-plan-update-title">{title}</span>{/if}
                </div>
                {#if note}
                  <p class="tc-plan-update-note"><span>Note</span> {note}</p>
                {/if}
              </div>
            {:else if !m.resultPending}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
            {#if m.resultError && m.result}
              <pre class="tc-pre tc-out tc-outerr">{m.result}</pre>
            {/if}
          {:else if isEditPlan(m)}
            <!-- Prose payload (plan), never {"plan":…} JSON. -->
            {@const body = editPlanText(m)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if body}
              <MarkdownHtml text={body} class="chat-canvas tc-plan" />
            {:else if !m.resultPending && !m.result}
              <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {/if}
            {#if m.result}
              <p class="tc-plan-ack" class:err={m.resultError}>{m.result}</p>
            {/if}
          {:else if isCreatePlan(m) && planBody(m)}
            <!-- Cursor plan-mode brief: markdown in args.plan, result is {}. -->
            <MarkdownHtml text={planBody(m)} class="chat-canvas tc-plan" />
          {:else if m.tool === 'Bash'}
            <pre class="tc-pre"><span class="sh-prompt">$ </span>{@html highlightBash(bashCommand(m))}{#if m.result}{'\n\n'}<span class="tc-outtext" class:tc-outerr={m.resultError}>{m.result}</span>{/if}</pre>
          {:else if (m.tool === 'Grep' || m.tool === 'Glob') && typeof parsedInput(m)?.pattern === 'string'}
            {@const p = parsedInput(m)!}
            <div class="tc-pre tc-grepq">
              <span class="sh-str">"{p.pattern}"</span>
              {#if typeof p.path === 'string' && p.path}<span class="tc-greppath">in {relPath(p.path)}</span>{/if}
              {#each grepFlags(p) as f (f)}<span class="grep-flag">{f}</span>{/each}
            </div>
            {#if m.result}<pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{m.result}</pre>{/if}
          {:else if m.tool === 'WebFetch' || m.tool === 'WebSearch'}
            <!-- URL / query live in the row; never dump {"url","prompt"} JSON.
                 Page body is markdown (Claude WebFetch returns GFM). -->
            {@const prompt = m.tool === 'WebFetch' ? fetchPrompt(m) : ''}
            {@const body = (m.result ?? '').trim()}
            {@const jsonBody = webResultIsJson(body)}
            {#if m.resultPending}
              <div class="tc-wait">
                <span class="tc-wait-dot" aria-hidden="true"></span>
                <span>{pendingLabel(m)}</span>
              </div>
            {/if}
            {#if prompt}
              <p class="tc-web-prompt"><span>Asked</span> {prompt}</p>
            {/if}
            {#if body}
              {#if m.resultError || jsonBody}
                <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result ?? '')}</pre>
              {:else}
                <div class="tc-web-msg">
                  <MarkdownHtml text={m.result ?? ''} class="chat-canvas tc-web-body" demoteImages />
                </div>
              {/if}
            {:else if m.resultError}
              <pre class="tc-pre tc-out tc-outerr">failed</pre>
            {/if}
          {:else if hasEditDiff(m)}
            <!-- Mini diff viewer: old_string carries its own context lines, so a
                 line-level diff shows changed lines in place with −/+ gutters. -->
            {@const lang = langFromPath(fileOf(m))}
            <div class="cv">
              {#each editDiff(m) as r, ri (ri)}
                <div class="cv-row" class:del={r.kind === 'del'} class:add={r.kind === 'add'}>
                  <span class="cv-mark">{r.kind === 'del' ? '−' : r.kind === 'add' ? '+' : ''}</span>
                  <span class="cv-code">{@html highlightLine(r.text, lang)}</span>
                </div>
              {/each}
            </div>
          {:else if hasWriteBody(m)}
            <!-- Write / Codex file create: show the file body, never path/content JSON. -->
            {@const lang = langFromPath(fileOf(m))}
            <div class="cv">
              {#each writeBody(m).replace(/\n$/, '').split('\n') as line, li (li)}
                <div class="cv-row">
                  <span class="cv-ln">{li + 1}</span>
                  <span class="cv-code">{@html highlightLine(line, lang)}</span>
                </div>
              {/each}
            </div>
          {:else if m.resultPending}
            <div class="tc-wait">
              <span class="tc-wait-dot" aria-hidden="true"></span>
              <span>{pendingLabel(m)}</span>
            </div>
          {:else if m.tool === 'Read'}
            {#if m.result}<pre class="tc-pre tc-out">{m.result}</pre>{:else}<pre class="tc-pre">{prettyInput(m)}</pre>{/if}
          {:else if m.tool === 'Edit' || m.tool === 'Write' || m.tool === 'Delete'}
            <!-- File tools: path lives in the header chip — don't dump JSON args. -->
            {#if m.result}
              <pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>
            {:else if m.resultError}
              <pre class="tc-pre tc-out tc-outerr">failed</pre>
            {/if}
          {:else if isLinearTool(m.tool)}
            {@render linearCard(m)}
          {:else}
            <pre class="tc-pre tc-cmd">{prettyInput(m)}</pre>
            {#if m.result}<pre class="tc-pre tc-out" class:tc-outerr={m.resultError}>{prettyResult(m.result)}</pre>{/if}
          {/if}
          {#if m.children?.length}
            <div class="tc-nested">
              {#each m.children as c, ci (stepRowKey(c.toolId, ci))}
                {@render nestedChild(c, String(rowKey) + ':' + ci)}
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet thinkingCard(m: AgentMsg, i: string | number)}
  {@const expandable = !!m.body}
  {@const open = expandable && rowIsOpen(i, !!(firstCheckpoint && m.streaming))}
  <div class="tc think" class:live={m.streaming && open}>
    <button
      type="button"
      class="tc-head"
      class:static={!expandable}
      onclick={() => expandable && toggleRow(i, open)}
      aria-expanded={expandable ? open : undefined}
    >
      <span class="tc-ico" aria-hidden="true">
        <span class="ico">{@render stepIcon('think')}</span>
        {#if expandable}<span class="ex">{open ? '−' : '+'}</span>{/if}
      </span>
      <span class="tc-label">{m.streaming ? 'Thinking' : 'Thought'}</span>
      {#if expandable && !open}
        <span class="tc-meta">{thinkingPreview(m)}</span>
      {:else if !expandable && m.tokens}
        <!-- Redacted thinking: the CLI withheld the text; show the estimate. -->
        <span class="tc-meta">~{m.tokens} tokens</span>
      {/if}
    </button>
    {#if open && m.streaming && expandable}
      <!-- Live window: ~4 lines, fade at the top, pinned to the newest text. -->
      <div class="tc-think-live" use:pinThinkLive>
        <StreamingMarkdown text={m.body ?? ''} active={shellActive && presentationActive} complete={!m.streaming} />
      </div>
    {:else if open}
      <div class="tc-body">
        <MarkdownHtml text={m.body ?? ''} class="chat-canvas tc-thought" />
      </div>
    {/if}
  </div>
{/snippet}

{#snippet compactCard(m: AgentMsg, i: string | number)}
  {@const expandable = !!m.body && !m.compacting}
  {@const open = expandable && !!openRows[i]}
  <div class="tc compact-ctx" class:live={m.compacting}>
    <button
      type="button"
      class="tc-head"
      class:static={!expandable}
      onclick={() => expandable && toggleRow(i)}
      aria-expanded={expandable ? open : undefined}
    >
      <span class="tc-ico" aria-hidden="true">
        <span class="ico">{@render stepIcon('compact')}</span>
        {#if expandable}<span class="ex">{open ? '−' : '+'}</span>{/if}
      </span>
      <span class="tc-label">{m.compacting ? 'Compacting context' : 'Context compacted'}</span>
      {#if m.tokens}
        <span class="tc-meta">was {formatTokens(m.tokens)}</span>
      {/if}
    </button>
    {#if open}
      <div class="tc-body">
        <MarkdownHtml text={m.body ?? ''} class="chat-canvas tc-thought" />
      </div>
    {/if}
  </div>
{/snippet}

{#snippet fallbackCard(m: AgentMsg)}
  <div class="tc fallback-ctx" role="status">
    <div class="tc-head static">
      <span class="tc-ico" aria-hidden="true">
        <span class="ico">{@render stepIcon('fallback')}</span>
      </span>
      <span class="tc-label">Fell back to {m.body || 'another model'}</span>
      {#if m.description}
        <span class="tc-meta">from {m.description}</span>
      {/if}
    </div>
  </div>
{/snippet}

{#snippet noticeCard(m: AgentMsg)}
  {@const wrap = m.noteLabel === 'Commit' || m.noteLabel === 'Handoff'}
  <div class="tc notice" class:loop-fact={wrap} role="status">
    <div class="tc-head static">
      <span class="tc-ico" aria-hidden="true">
        <span class="ico"><Icon name="activity" /></span>
      </span>
      <span class="tc-label">{m.noteLabel ?? 'Notice'}</span>
      <span class="tc-meta">{#if m.noteHtml}{@html m.body}{:else}{m.body}{/if}</span>
    </div>
  </div>
{/snippet}

{#snippet narrationRow(m: AgentMsg, i: string | number)}
  {@const expandable = !!m.body}
  {@const open = expandable && rowIsOpen(i)}
  <div class="tc">
    <button
      type="button"
      class="tc-head"
      class:static={!expandable}
      onclick={() => expandable && toggleRow(i, open)}
      aria-expanded={expandable ? open : undefined}
    >
      <span class="tc-label">{prosePreview(m.body ?? '')}</span>
    </button>
    {#if open}
      <div class="tc-body">
        <MarkdownHtml text={m.body ?? ''} class="chat-canvas" />
      </div>
    {/if}
  </div>
{/snippet}

{#snippet stepRow(r: Row)}
  {#if isSkillActivation(r.m)}
    {@render skillRow(r.m)}
  {:else if isListSkills(r.m)}
    {@render listSkillsRow(r.m)}
  {:else if isSwitchMode(r.m)}
    {@render switchModeRow(r.m)}
  {:else if isRenameTool(r.m)}
    {@render renameToolRow(r.m)}
  {:else if isChildTool(r.m)}
    {@render childToolRow(r.m)}
  {:else if isOpenPR(r.m)}
    {@render openPRRow(r.m)}
  {:else if isReenterPhase(r.m)}
    {@render reenterPhaseRow(r.m)}
  {:else if isCreateArtifact(r.m)}
    {@render createArtifactRow(r.m)}
  {:else if isPresentArtifact(r.m)}
    {@render presentArtifactRow(r.m)}
  {:else if r.m.tool}
    {@render toolCard(r.m, r.i)}
  {:else if r.m.compact}
    {@render compactCard(r.m, r.i)}
  {:else if r.m.fallback}
    {@render fallbackCard(r.m)}
  {:else if r.m.thinking}
    {@render thinkingCard(r.m, r.i)}
  {:else if r.m.note}
    {@render noticeCard(r.m)}
  {:else}
    {@render narrationRow(r.m, r.i)}
  {/if}
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="chat"
  class:compact
  class:composer-only={composerOnly}
  class:drop-file={fileDropOver}
  data-file-drop-target={readonly ? undefined : 'true'}
  data-chat-drop={readonly ? undefined : encodeChatDropId(resetKey)}
  ondragenter={onChatDragEnter}
  ondragovercapture={onChatDragOver}
  ondragleave={onChatDragLeave}
  ondropcapture={onChatDrop}
>
  {#if !composerOnly}
  <div class="chat-main">
    <div class="chat-scroll" bind:this={chatScrollEl} onscroll={onChatScroll} {@attach fadeChatScroll}>
      <div class="chat-col" bind:this={chatColEl}>
      {#if transcriptMessages.length === 0}
        {#if empty}
          <div class="chat-empty">{@render empty()}</div>
        {:else}
          <div class="chat-empty">{emptyText}</div>
        {/if}
      {/if}
      {#key resetKey}
      {#each blocks as b, bi (blockKeys[bi])}
        {@const k = blockKeys[bi]}
        {@const live = !windowChat || chatHydrated.has(k) || bi >= blocks.length - CHAT_TAIL_LIVE}
        {@const reserved = reservedChatHeight(live, blockH.get(k), blockEstimate(b))}
        <div
          class="cblk"
          class:stub={!live}
          style:min-height={reserved != null ? `${reserved}px` : undefined}
          {@attach registerChatBlock(k)}
        >
        {#if live}
        {#if b.kind === 'msg'}
          {#if b.m.log?.kind === 'fact'}
            {@const code = b.m.log.code || 'MA'}
            {@const you = code === 'AR'}
            {@const prDone = loopPRCompletion(b.m.log.text, {
              prs: $prs,
              workspaceId,
              factTs: b.m.ts,
              startTs: cockpitStartTs === Infinity ? 0 : cockpitStartTs,
            })}
            {@const wrap = loopCompletionWrap(b.m.log.text)}
            <div class="tc loop-fact" class:dev={b.m.log.isDeviation} class:loop-pr={!!prDone} class:loop-card={!!b.m.log.card} role="status">
              {#if b.m.log.card === 'fanout' && b.m.log.data}
                <FanOutCard data={b.m.log.data} fallback={b.m.log.text} />
              {:else if b.m.log.card === 'plan' && b.m.log.data}
                <PlanCard data={b.m.log.data} fallback={b.m.log.text} {workspaceId} />
              {:else}
              <div class="tc-head static">
                <span class="tc-ico" aria-hidden="true">
                  <span class="ico"><Icon name={you ? 'user' : 'activity'} /></span>
                </span>
                <span class="tc-label" style:color={you ? undefined : agColor[code]}>{b.m.log.name || code}</span>
                {#if prDone}
                  <span class="tc-meta">
                    {#if prDone.duration}Completed in {prDone.duration}{:else}Completed{/if}
                    <span class="turn-end-sep" aria-hidden="true">·</span>
                    {#if prDone.prId}
                      <button type="button" class="turn-end-link" onclick={() => openPR(prDone.prId!)}>{prDone.prLabel}</button>
                    {:else}
                      <span>{prDone.prLabel}</span>
                    {/if}
                  </span>
                {:else}
                  <span class="tc-meta">{@html stripLoopWrap(b.m.log.text)}</span>
                {/if}
              </div>
              {#if wrap}<div class="loop-wrap">{@html wrap}</div>{/if}
              {/if}
            </div>
          {:else if b.m.log?.kind === 'session' || b.m.log?.kind === 'sessions'}
            {#if sessionCard}
              {@const sessions = b.m.log.kind === 'sessions' ? b.m.log.sessions : [b.m.log]}
              <div class="fanout-card sesscard">
                {#each sessions as s (s.id || s.code)}
                  {@render sessionCard(s)}
                {/each}
              </div>
            {/if}
          {:else if b.m.turnEnd}
            <!-- Closes a finished turn with the run time that was ticking while
                 it ran, instead of letting it vanish when the turn ends. -->
            {@const digest = turnDigestVisible(b.i)}
            {@const presented = lastPresentedInTurn(b.i)}
            {@const reply = lastAssistantTextInTurn(transcriptMessages, b.i)}
            {@const copyKey = 'turn-' + (b.m.messageId || b.i)}
            {@const canCkpt = !readonly && !!b.m.messageId && !!b.m.checkpoint}
            <div class="turn-end-wrap">
              <div class="turn-end">
                <span>completed in {formatDuration(b.m.durationMs ?? 0)}</span>
                {#if presented?.ok && (presented.brief || presented.target)}
                  <span class="turn-end-sep" aria-hidden="true">·</span>
                  <button
                    type="button"
                    class="turn-end-link"
                    onclick={() => routeTarget(presented.brief?.primary?.target || presented.target || '')}
                  >Open {presented.brief?.title || presented.target}</button>
                {/if}
                {#if digest.length}
                  <span class="turn-end-sep" aria-hidden="true">·</span>
                  <button
                    type="button"
                    class="turn-end-link"
                    onclick={() => jumpToFinding(digest.find((d) => d.id)?.id || '', false)}
                  >{digest.length} finding{digest.length === 1 ? '' : 's'}</button>
                {/if}
                {#if reply || (!readonly && (onFork || onReset) && b.m.messageId) || canCkpt}
                  <div class="turn-end-acts">
                    {#if reply}
                      <button
                        type="button"
                        class="turn-end-act"
                        class:copied={copiedBubble === copyKey}
                        title="Copy reply"
                        aria-label="Copy last assistant message"
                        onclick={(e) => copyBubble(reply, copyKey, e)}
                      >
                        {#if copiedBubble === copyKey}
                          <Icon name="check" strokeWidth={2.4} />
                        {:else}
                          <Icon name="copy" />
                        {/if}
                      </button>
                    {/if}
                    {#if !readonly && onFork && b.m.messageId}
                      <div class="turn-end-fork-wrap">
                        <button
                          type="button"
                          class="turn-end-act turn-end-fork"
                          class:on={forkMenu?.messageId === b.m.messageId}
                          title="Fork"
                          aria-label="Fork from this turn"
                          aria-haspopup={onForkToWorkspace ? 'menu' : undefined}
                          aria-expanded={onForkToWorkspace ? forkMenu?.messageId === b.m.messageId : undefined}
                          onclick={(ev) => toggleForkMenu(b.m.messageId!, ev)}
                          onkeydown={onForkMenuKey}
                        >
                          <Icon name="split" />
                        </button>
                      </div>
                    {/if}
                    {#if canCkpt && onTurnDiff}
                      <button
                        type="button"
                        class="turn-end-act"
                        title="Changes this turn"
                        aria-label="Show worktree changes made during this turn"
                        onclick={() => onTurnDiff?.(b.m.messageId!)}
                      >
                        <Icon name="file-diff" />
                      </button>
                    {/if}
                    {#if !readonly && onReset && b.m.messageId && b.i < transcriptMessages.length - 1}
                      <button
                        type="button"
                        class="turn-end-act"
                        title="Reset to here"
                        aria-label="Rewind this chat to the end of this turn and remove later messages"
                        onclick={() => onReset?.(b.m.messageId!, transcriptMessages.slice(b.i + 1).some((m) => m.turnEnd && m.checkpoint))}
                      >
                        <Icon name="undo" />
                      </button>
                    {/if}
                  </div>
                {/if}
              </div>
              {#if presented?.ok && (presented.brief || presented.target)}
                <div class="tc-briefs turn-end-brief" role="region" aria-label="Presented workspace">
                  {@render presentWorkspaceCard(presented, false)}
                </div>
              {/if}
              {#if digest.length}
                <!-- Same card + row chrome as list_findings tool results. -->
                <div class="tc-card" role="region" aria-label="Findings filed">
                  <ul class="tc-findings-list tc-finding-accord">
                    {#each digest as f, fi (`${f.id || 'x'}-${fi}`)}
                      {@const loc = locSuffixOf(f.line, f.endLine)}
                      {@const locLabel = findingLocLabel(f.file, f.line, f.endLine)}
                      {@const title = findingTitle(f.message || f.text || '')}
                      {@const showBody = !!f.text && (
                        f.text !== (f.message || '').trim() || findingBodyNeeded(f.text)
                      )}
                      {@const accordKey = f.id || `digest-${fi}`}
                      {@const open = !!findingAccordOpen[accordKey] || (!!f.id && digestCommentFor === f.id)}
                      {@const noting = !!f.id && digestCommentFor === f.id}
                      {@const statusBusy = digestStatusBusy === f.id || digestCommentBusy === f.id}
                      {@const notes = notesForDigest(f)}
                      <li class="tc-finding-row" class:open>
                        <button
                          type="button"
                          class="tc-finding-row-head"
                          aria-expanded={open}
                          onclick={() => toggleFindingAccord(accordKey)}
                        >
                          <span class="finding-sev {f.severity || 'low'}" aria-hidden="true">
                            <Icon name={findingSevIcon(f.severity)} />
                          </span>
                          <span class="tc-finding-main">
                            <span class="tc-finding-title">{title}</span>
                            {#if locLabel}
                              <span class="tc-finding-sub">
                                <span class="tc-finding-line" title={locForDigest(f.file, f.line, f.endLine)}>{locLabel}</span>
                              </span>
                            {/if}
                          </span>
                        </button>
                        {#if open}
                          <div class="tc-finding-detail" transition:slide={findingAccordSlide}>
                            {#if f.file}
                              <div class="tc-finding-detail-loc">
                                {#if onOpenFile}
                                  <button
                                    type="button"
                                    class="tc-file"
                                    title={locForDigest(f.file, f.line, f.endLine)}
                                    onclick={() => onOpenFile?.(
                                      f.file,
                                      f.line || undefined,
                                      f.endLine && f.endLine > f.line ? f.endLine : undefined,
                                    )}
                                  >{locLabel || `${baseName(f.file)}${loc}`}</button>
                                {:else}
                                  <span class="tc-finding-loc">{locForDigest(f.file, f.line, f.endLine)}</span>
                                {/if}
                              </div>
                            {/if}
                            {#if showBody}
                              <div class="tc-finding-msg">
                                <MarkdownHtml text={f.text} class="chat-canvas tc-finding-body" />
                              </div>
                            {/if}
                            {#if notes.length}
                              <div class="tc-finding-notes">
                                {#each notes as n (n.id || n.body)}
                                  <div class="tc-finding-note-row">
                                    <p class="tc-finding-reason">
                                      <span>Note</span>
                                      {n.body}
                                    </p>
                                    {#if f.id && n.id}
                                      {@const noteId = n.id}
                                      <button
                                        type="button"
                                        class="tc-finding-note-del"
                                        title="Delete note"
                                        aria-label="Delete note"
                                        disabled={digestCommentBusy === f.id}
                                        onclick={() => void deleteDigestNote(f.id, noteId)}
                                      >
                                        <Icon name="trash" />
                                      </button>
                                    {/if}
                                  </div>
                                {/each}
                              </div>
                            {/if}
                            {#if f.id && !noting}
                              <div class="turn-finding-actions">
                                <button
                                  type="button"
                                  class="finding-act ignore"
                                  disabled={statusBusy}
                                  onclick={() => void setDigestFindingStatus(f.id, 'ignored')}
                                >
                                  <Icon name="eye-off" />
                                  Ignore
                                </button>
                                <button
                                  type="button"
                                  class="finding-act invalid"
                                  disabled={statusBusy}
                                  onclick={() => void setDigestFindingStatus(f.id, 'invalid')}
                                >
                                  <Icon name="bug-off" />
                                  Mark invalid
                                </button>
                                <button
                                  type="button"
                                  class="finding-act reopen"
                                  disabled={statusBusy}
                                  onclick={() => void openDigestCompose(f.id)}
                                >
                                  <Icon name="notebook-pen" />
                                  Add note
                                </button>
                              </div>
                            {/if}
                            {#if noting}
                              <div class="turn-finding-compose">
                                <textarea
                                  class="turn-finding-input"
                                  rows="2"
                                  placeholder="Add a note, clarification, or context…"
                                  disabled={digestCommentBusy === f.id}
                                  bind:this={digestCommentInput}
                                  bind:value={digestCommentDraft}
                                  onkeydown={(e) => {
                                    if (e.key === 'Enter' && isCmdOrCtrl(e)) {
                                      e.preventDefault()
                                      void submitDigestComment(f.id)
                                    }
                                    if (e.key === 'Escape') {
                                      e.preventDefault()
                                      digestCommentFor = ''
                                      digestCommentDraft = ''
                                    }
                                  }}
                                ></textarea>
                                <div class="turn-finding-compose-foot">
                                  <span class="turn-finding-hint"><Kbd text="⌘↵" /> to save</span>
                                  <div class="turn-finding-compose-actions">
                                    <button
                                      type="button"
                                      class="turn-finding-btn"
                                      disabled={digestCommentBusy === f.id}
                                      onclick={() => {
                                        digestCommentFor = ''
                                        digestCommentDraft = ''
                                      }}
                                    >Cancel</button>
                                    <button
                                      type="button"
                                      class="turn-finding-btn primary"
                                      disabled={digestCommentBusy === f.id || !digestCommentDraft.trim()}
                                      onclick={() => void submitDigestComment(f.id)}
                                    >Save</button>
                                  </div>
                                </div>
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                </div>
              {/if}
            </div>
          {:else if b.m.note}
            {@render noticeCard(b.m)}
          {:else if b.m.stopped}
            <div class="chat-stopped" role="status">
              <Icon name="octagon-x" />
              <span>{b.m.body || 'Stopped by user'}</span>
            </div>
          {:else if b.m.error}
            {@const title = agentErrorTitle(b.m.body)}
            {@const detail = agentErrorDetail(b.m.body)}
            {@const canRetry = b.i === lastRetryErrorIndex}
            <div class="chat-error" role="alert">
              <div class="chat-error-head">
                <Icon name="circle-alert" />
                <span class="chat-error-title">{title}</span>
              </div>
              {#if detail && detail !== title}
                <div class="chat-error-body">{detail}</div>
              {/if}
              {#if canRetry}
                <div class="chat-error-actions">
                  <button type="button" class="chat-error-retry" onclick={retry} title="Retry last message">
                    <Icon name="redo-2" strokeWidth={2.2} />
                    Retry
                  </button>
                </div>
              {/if}
            </div>
          {:else if b.m.me}
            {@const bubbleKey = 'm' + b.i}
            {@const canCopy = !!bubblePlainText(b.m)}
            <div class="chat-row me">
              <div class="chat-bubble" class:has-att={!!b.m.attachments?.length}>
                {#each bubbleParts(b.m) as part, pi (pi)}
                  {#if part.kind === 'text'}
                    {part.text}
                  {:else if part.kind === 'file'}
                    {#if part.dir}
                      <span class="chat-file-inline" data-dir="1" title={part.path}>
                        <span class="folder-ico" aria-hidden="true"><Icon name="folder" /></span>
                        <span class="chat-file-inline-name">{fileMentionLabel(part.path)}</span>
                      </span>
                    {:else}
                      <button
                        type="button"
                        class="chat-file-inline"
                        title={part.path}
                        disabled={!onOpenFile}
                        onclick={() => openFileChip(part.path)}
                      >
                        <span class="fico {fileIconClass(baseName(parseFileMentionChip(part.path).path || part.path), $theme)}" aria-hidden="true"></span>
                        <span class="chat-file-inline-name">{fileMentionLabel(part.path)}</span>
                      </button>
                    {/if}
                  {:else if part.kind === 'skill'}
                    <span class="chat-skill-inline" title="/{part.name}">
                      <span class="skill-ico" aria-hidden="true"><Icon name="slash" /></span>
                      <span class="chat-file-inline-name">{part.name}</span>
                    </span>
                  {:else if part.kind === 'terminal'}
                    <button
                      type="button"
                      class="chat-file-inline chat-terminal-inline"
                      title={terminalTitleFor(part.id) || 'Terminal'}
                      onclick={() => openTerminalChip(part.id)}
                    >
                      <span class="term-ico" aria-hidden="true"><Icon name="terminal" /></span>
                      <span class="chat-file-inline-name">{terminalMentionLabel(part.id ? `terminal:${part.id}` : 'terminal', terminalTitleFor(part.id))}</span>
                    </button>
                  {:else if part.kind === 'artifact'}
                    <button
                      type="button"
                      class="chat-file-inline chat-artifact-inline"
                      title={artifactTitleFor(part.id) || 'Artifact'}
                      onclick={() => openArtifactChip(part.id)}
                    >
                      <span class="art-ico" aria-hidden="true"><Icon name={artifactIcon(liveArtifacts.find((a) => a.id === part.id)?.kind || '')} /></span>
                      <span class="chat-file-inline-name">{artifactMentionLabel(part.id ? `artifact:${part.id}` : 'artifact', artifactTitleFor(part.id))}</span>
                    </button>
                  {:else}
                    <button
                      type="button"
                      class="chat-att-inline-btn"
                      aria-label="View {part.name}"
                      onclick={() => openLightbox(part.src, part.name)}
                      oncontextmenu={(e) => { imgCtx = openImageCtx(e, part.src) }}
                    >
                      <img class="chat-att-inline" src={part.src} alt={part.name} />
                    </button>
                  {/if}
                {/each}
                {#if canCopy}
                  <button
                    type="button"
                    class="chat-bubble-copy"
                    class:copied={copiedBubble === bubbleKey}
                    title="Copy message"
                    aria-label="Copy message"
                    onclick={(e) => copyBubble(b.m, bubbleKey, e)}
                  >
                    {#if copiedBubble === bubbleKey}
                      <Icon name="check" strokeWidth={2.4} />
                    {:else}
                      <Icon name="copy" />
                    {/if}
                  </button>
                {/if}
              </div>
            </div>
          {:else if b.m.body}
            <div class="chat-row assistant" class:streaming={b.m.streaming}>
              {#if b.m.streaming}
                <!-- Mount/unmount contract: unit reveal while live; settle snaps to full text. -->
                <StreamingMarkdown text={b.m.body} active={shellActive && presentationActive} complete={!b.m.streaming} />
              {:else}
                <MarkdownHtml text={b.m.body} />
              {/if}
            </div>
          {/if}
        {:else if b.rows.length === 1}
          <!-- A lone step needs no group header — the row is its own collapse. -->
          <div class="steps">
            {@render stepRow(b.rows[0])}
          </div>
        {:else}
          {@const isLast = bi === blocks.length - 1}
          {@const open = groupOpen(b, isLast)}
          {@const tail = !open && sending && isLast ? b.rows[b.rows.length - 1] : null}
          {@const headRows = tail ? b.rows.slice(0, -1) : b.rows}
          <div class="steps" class:open>
            <button type="button" class="steps-head" onclick={() => toggleGroup(b, isLast)} aria-expanded={open}>
              <span class="tc-chev" class:open aria-hidden="true">
                <Icon name="chevron-right" strokeWidth={2.4} />
              </span>
              {groupSummary(headRows, b.rows)}
            </button>
            {#if open}
              <div class="steps-rows">
                {#each b.rows as r (stepRowKey(r.m.toolId, r.i))}
                  {@render stepRow(r)}
                {/each}
              </div>
            {:else if tail}
              <div class="steps-rows">
                {@render stepRow(tail)}
              </div>
            {/if}
          </div>
        {/if}
        {/if}
        </div>
      {/each}
      {/key}
      {#each chatPermissions as req (req.id)}
        <div class="chat-perm" aria-live="assertive">
          <PermissionCard request={req} compact />
        </div>
      {/each}
      {#each chatQuestions as q (q.id)}
        <div class="chat-perm" aria-live="assertive">
          <QuestionCard question={q} />
        </div>
      {/each}
      {#if chatEscalation && onAnswerEscalation}
        <div class="chat-perm" aria-live="assertive">
          <QuestionCard
            question={chatEscalation}
            kicker="Maestro needs a decision"
            onAnswer={onAnswerEscalation}
          />
        </div>
      {/if}
      {#if showTurnElapsed}
        <div class="chat-turn" aria-live="polite" aria-label="Turn in progress, {formatElapsed(turnElapsedSec)}">
          <span class="chat-think-dots" aria-hidden="true"><i></i><i></i><i></i></span>
          <span class="chat-turn-elapsed">{formatElapsed(turnElapsedSec)}</span>
        </div>
      {:else if statusText}
        <div class="chat-turn" role="status" aria-live="polite">
          <span class="chat-think-dots" aria-hidden="true"><i></i><i></i><i></i></span>
          <span class="chat-turn-status">{statusText}</span>
        </div>
      {/if}
      </div>
    </div>
    {#if onForkToWorkspace && forkMenu}
      <div
        bind:this={forkMenuEl}
        class="turn-end-fork-menu"
        role="menu"
        tabindex="-1"
        style:left="{forkMenu.left}px"
        style:top="{forkMenu.top}px"
        onclick={(e) => e.stopPropagation()}
        onkeydown={onForkMenuKey}
      >
        <button
          type="button"
          role="menuitem"
          class="turn-end-fork-item"
          onclick={() => { const id = forkMenu!.messageId; closeForkMenu(); onFork?.(id) }}
        >
          <Icon name="split" />
          Fork chat
        </button>
        <button
          type="button"
          role="menuitem"
          class="turn-end-fork-item"
          onclick={() => { const id = forkMenu!.messageId; closeForkMenu(); onForkToWorkspace(id) }}
        >
          <Icon name="plus" />
          Fork to new workspace
        </button>
      </div>
    {/if}
  </div>
  {/if}
  {#if !readonly}
  <div class="chat-dock">
    {#if !composerOnly && queuedMessages.length}
      <div class="chat-queue" aria-label="Queued follow-ups">
        {#each queuedMessages as m (m.pendingId ?? m.body)}
          <div class="chat-row me queued">
            <div class="chat-bubble queued" class:has-att={!!m.attachments?.length}>
              <div class="chat-queued-head">
                <span class="chat-queued-label">Queued</span>
                {#if m.pendingId && onRemoveQueued}
                  <button
                    type="button"
                    class="chat-queued-x"
                    aria-label="Remove queued message"
                    title="Remove from queue"
                    onclick={() => onRemoveQueued?.(m.pendingId!)}
                  >×</button>
                {/if}
              </div>
              <div class="chat-queued-body">
                {#each bubbleParts(m) as part, pi (pi)}
                  {#if part.kind === 'text'}
                    {part.text}
                  {:else if part.kind === 'file'}
                    {#if part.dir}
                      <span class="chat-file-inline" data-dir="1" title={part.path}>
                        <span class="folder-ico" aria-hidden="true"><Icon name="folder" /></span>
                        <span class="chat-file-inline-name">{fileMentionLabel(part.path)}</span>
                      </span>
                    {:else}
                      <button
                        type="button"
                        class="chat-file-inline"
                        title={part.path}
                        disabled={!onOpenFile}
                        onclick={() => openFileChip(part.path)}
                      >
                        <span class="fico {fileIconClass(baseName(parseFileMentionChip(part.path).path || part.path), $theme)}" aria-hidden="true"></span>
                        <span class="chat-file-inline-name">{fileMentionLabel(part.path)}</span>
                      </button>
                    {/if}
                  {:else if part.kind === 'skill'}
                    <span class="chat-skill-inline" title="/{part.name}">
                      <span class="skill-ico" aria-hidden="true"><Icon name="slash" /></span>
                      <span class="chat-file-inline-name">{part.name}</span>
                    </span>
                  {:else if part.kind === 'terminal'}
                    <button
                      type="button"
                      class="chat-file-inline chat-terminal-inline"
                      title={terminalTitleFor(part.id) || 'Terminal'}
                      onclick={() => openTerminalChip(part.id)}
                    >
                      <span class="term-ico" aria-hidden="true"><Icon name="terminal" /></span>
                      <span class="chat-file-inline-name">{terminalMentionLabel(part.id ? `terminal:${part.id}` : 'terminal', terminalTitleFor(part.id))}</span>
                    </button>
                  {:else if part.kind === 'artifact'}
                    <button
                      type="button"
                      class="chat-file-inline chat-artifact-inline"
                      title={artifactTitleFor(part.id) || 'Artifact'}
                      onclick={() => openArtifactChip(part.id)}
                    >
                      <span class="art-ico" aria-hidden="true"><Icon name={artifactIcon(liveArtifacts.find((a) => a.id === part.id)?.kind || '')} /></span>
                      <span class="chat-file-inline-name">{artifactMentionLabel(part.id ? `artifact:${part.id}` : 'artifact', artifactTitleFor(part.id))}</span>
                    </button>
                  {:else}
                    <button
                      type="button"
                      class="chat-att-inline-btn"
                      aria-label="View {part.name}"
                      onclick={() => openLightbox(part.src, part.name)}
                    >
                      <img class="chat-att-inline" src={part.src} alt={part.name} />
                    </button>
                  {/if}
                {/each}
              </div>
            </div>
          </div>
        {/each}
      </div>
    {/if}
    <div class="chat-in">
      {#if mention && (projectId || mentionHits.length || fileIndexLoading || liveTerminals.length || liveArtifacts.length)}
        <div
          class="chat-mention"
          bind:this={mentionListEl}
          role="listbox"
          aria-label="Mention file, terminal, or artifact"
        >
          {#if fileIndexLoading && !mentionHits.length}
            <div class="chat-mention-empty">Loading files…</div>
          {:else}
            {#each mentionHits as h, i (h.kind === 'terminal' ? 't:' + h.id : h.kind === 'artifact' ? 'a:' + h.id : 'f:' + h.item)}
              <button
                type="button"
                class="chat-mention-row"
                class:on={i === mentionSel}
                class:titled={h.kind !== 'file'}
                role="option"
                aria-selected={i === mentionSel}
                onmousemove={(e) => onMentionRowMouseMove(e, i)}
                onmouseenter={() => onMentionRowMouseEnter(i)}
                onclick={() => pickMentionHit(h)}
              >
                {#if h.kind === 'terminal'}
                  <span class="term-ico" aria-hidden="true"><Icon name="terminal" /></span>
                  <span class="chat-mention-name">{h.title}</span>
                  <span class="chat-mention-path">{h.exited ? 'Exited terminal' : h.active ? 'Focused terminal' : 'Terminal'}</span>
                {:else if h.kind === 'artifact'}
                  <span class="art-ico" aria-hidden="true"><Icon name={artifactIcon(h.artifactKind)} /></span>
                  <span class="chat-mention-name">{h.title}</span>
                  <span class="chat-mention-path">{h.artifactKind ? h.artifactKind : 'Artifact'}</span>
                {:else if h.kind === 'file' && h.dir}
                  <span class="folder-ico" aria-hidden="true"><Icon name="folder" /></span>
                  <span class="chat-mention-name">{baseName(h.item)}</span>
                  <span class="chat-mention-path">{h.item.includes('/') ? h.item.slice(0, h.item.lastIndexOf('/')) : ''}</span>
                {:else}
                  <span class="fico {fileIconClass(baseName(h.item), $theme)}" aria-hidden="true"></span>
                  <span class="chat-mention-name">{baseName(h.item)}</span>
                  <span class="chat-mention-path">{h.item.includes('/') ? h.item.slice(0, h.item.lastIndexOf('/')) : ''}</span>
                {/if}
              </button>
            {:else}
              <div class="chat-mention-empty">{mention.query.trim() ? 'No matching files, terminals, or artifacts' : 'Type to filter files'}</div>
            {/each}
          {/if}
        </div>
      {:else if promptArgs && promptArgHits.length}
        <div
          class="chat-mention"
          bind:this={mentionListEl}
          role="listbox"
          aria-label="{promptArgs.slash} arguments"
        >
          {#each promptArgHits as hit, i (`${hit.kind}-${hit.kind === 'suggest' ? (hit.hit.insert || hit.hit.action || i) : hit.args}-${i}`)}
            <button
              type="button"
              class="chat-mention-row"
              class:on={i === mentionSel}
              role="option"
              aria-selected={i === mentionSel}
              disabled={hit.kind === 'run' && !promptArgCanInvoke}
              onmousemove={(e) => onMentionRowMouseMove(e, i)}
              onmouseenter={() => onMentionRowMouseEnter(i)}
              onclick={() => {
                if (hit.kind === 'suggest') pickPromptCompleteHit(hit.hit, true)
                else if (hit.kind === 'run' && promptArgCanInvoke) applyPromptArgs(hit.slash, hit.args, true)
              }}
            >
              {#if hit.kind === 'suggest'}
                <span class="skill-ico prompt-ico" aria-hidden="true"><Icon name="hash" svg={promptArgIcon} /></span>
                <span class="chat-mention-name">{hit.hit.label}</span>
                {#if hit.hit.detail}<span class="chat-mention-path">{hit.hit.detail}</span>{/if}
                {#if hit.hit.badge}<span class="chat-mention-badge">{hit.hit.badge}</span>{/if}
              {:else}
                <span class="skill-ico prompt-ico" aria-hidden="true"><Icon name="prompt" /></span>
                <span class="chat-mention-name">/{hit.slash}{hit.args ? ` ${hit.args}` : ''}</span>
                <span class="chat-mention-path">{completeMessage || hit.description}</span>
                <span class="chat-mention-badge">Prompt</span>
              {/if}
            </button>
          {/each}
        </div>
      {:else if skillMention && (projectId || skillIndex.length || skillIndexLoading || promptIndex.length || promptIndexLoading)}
        <div
          class="chat-mention"
          bind:this={mentionListEl}
          role="listbox"
          aria-label="Invoke prompt or skill"
        >
          {#if (skillIndexLoading || promptIndexLoading) && !slashHits.length}
            <div class="chat-mention-empty">Loading…</div>
          {:else}
            {#each slashHits as h, i (`${(h.item as SlashHit).kind}-${h.label}-${i}`)}
              {@const item = h.item as SlashHit}
              <button
                type="button"
                class="chat-mention-row"
                class:on={i === mentionSel}
                role="option"
                aria-selected={i === mentionSel}
                onmousemove={(e) => onMentionRowMouseMove(e, i)}
                onmouseenter={() => onMentionRowMouseEnter(i)}
                onclick={() => item.kind === 'prompt' ? pickPrompt(item.slash) : pickSkill(item.name)}
                disabled={item.kind === 'prompt' && (promptBusy || sending)}
              >
                {#if item.kind === 'prompt'}
                  {@const loop = item.mode === 'loop'}
                  <span
                    class="skill-ico prompt-ico"
                    class:prompt-ico-loop={loop}
                    aria-hidden="true"
                  ><Icon name={loop ? chatModeMeta('loop').icon : 'prompt'} /></span>
                  <span class="chat-mention-name">/{item.slash}</span>
                  {#if item.args?.length}
                    <span class="chat-mention-args">
                      {#each item.args as a (a.name)}
                        <span class="chat-mention-arg">{a.name}</span>
                      {/each}
                    </span>
                  {/if}
                  <span class="chat-mention-path">{item.description || item.name}</span>
                  <span class="chat-mention-badge">{loop ? 'Loop' : 'Prompt'}</span>
                {:else}
                  <span class="skill-ico" aria-hidden="true"><Icon name="slash" /></span>
                  <span class="chat-mention-name">{item.name}</span>
                  <span class="chat-mention-path">{item.description || (item.scope === 'user' ? 'user' : 'workspace')}</span>
                {/if}
              </button>
            {:else}
              <div class="chat-mention-empty">{skillMention.query.trim() ? 'No matching prompts or skills' : 'Type to filter'}</div>
            {/each}
          {/if}
        </div>
      {/if}
      <!-- Contenteditable: @file + /skill + pasted images render as inline chips. -->
      <div
        bind:this={composerEl}
        class="chat-composer"
        class:empty={composerEmpty}
        class:nt-in={composerOnly}
        role="textbox"
        tabindex="0"
        aria-multiline="true"
        aria-label={placeholder}
        data-placeholder={placeholder}
        contenteditable="true"
        spellcheck="false"
        autocorrect="off"
        autocapitalize="off"
        writingsuggestions="false"
        translate="no"
        onpaste={onPaste}
        onfocus={onComposerFocus}
        onfocusin={(e) => {
          const t = e.target as HTMLElement
          if (t.classList.contains('chat-composer-prompt-arg-input')) syncMentionFromCaret()
        }}
        onbeforeinput={onComposerBeforeInput}
        oninput={onComposerInput}
        onclick={onComposerClick}
        onkeyup={syncMentionFromCaret}
        onmouseup={syncMentionFromCaret}
        onkeydowncapture={(e) => { onChipDeleteKey(e) }}
        onkeydown={(e) => {
          if (onMentionKeydown(e)) return
          if (e.defaultPrevented) return
          if (isCmdOrCtrl(e) && !e.altKey && isLetterKey(e, 'z') &&
              !(e.target as HTMLElement).classList.contains('chat-composer-prompt-arg-input')) {
            e.preventDefault()
            applyComposerUndo(e.shiftKey)
            return
          }
          if (isCmdOrCtrl(e) && !e.shiftKey && !e.altKey &&
              (e.code === 'Period' || e.key === '.') && onCycleMode) {
            e.preventDefault()
            onCycleMode()
            return
          }
          // Shift+Enter — new <p> line (not WebKit’s loose <div>/<br> junk).
          // composerOnly matches the modal: unmodified Enter inserts, ⌘/Ctrl+Enter sends.
          if (e.key === 'Enter' && (e.shiftKey || (composerOnly && !isCmdOrCtrl(e)))) {
            e.preventDefault()
            if (composerEl && insertComposerParagraphBreak(composerEl)) {
              syncInputFromComposer()
              syncMentionFromCaret()
              layoutComposer()
            }
            return
          }
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault()
            send(isCmdOrCtrl(e))
          }
        }}
      ></div>
      {#if !composerOnly}
      <div class="chat-in-row">
        <div class="chat-in-tools">{@render toolbar?.()}</div>
        <div class="chat-in-actions">
          <ContextUsageMeter usage={contextUsage} />
          {#if onStop && (sending || canStop)}
            <button class="send stop" type="button" aria-label="Stop agent" title="Stop (⌘⇧⌫)" onclick={stop}>
              <Icon name="octagon-x" />
            </button>
          {/if}
          <button
            class="send"
            type="button"
            aria-label={sending ? (steerFollowUp ? 'Steer agent' : 'Queue follow-up') : canFlushQueue ? 'Send queued messages' : 'Send message'}
            title={sending && canSteer
              ? (steerFollowUp ? 'Steer (⏎) · Queue (⌘⏎)' : 'Queue (⏎) · Steer (⌘⏎)')
              : sending ? 'Queue follow-up (⏎)' : canFlushQueue ? 'Send queued (⏎)' : 'Send'}
            disabled={!canSend}
            onclick={(e) => send(isCmdOrCtrl(e))}
          >
            <Icon name="send" />
          </button>
        </div>
      </div>
      {/if}
    </div>
  </div>
  {/if}
  {#if lightbox}
    <AttachmentLightbox src={lightbox.src} name={lightbox.name} onclose={closeLightbox} />
  {/if}
  <ImageContextMenu bind:menu={imgCtx} />
</div>
