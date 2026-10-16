<script lang="ts">
  import { tick, untrack, onDestroy } from 'svelte'
  import { get } from 'svelte/store'
  import { workspaces, workspaceSeeds, takeWorkspaceSeed, observatoryJump, prDiffJump, pushToast, settings, agentsRunning, focusedChat, userQuestions, pendingChatFocus, workspaceScanPending, prs, openWorkspace, view, selectedPRId, briefing, type CodePanelKey, planPanelTab,
   childTranscriptRequest, openProject,
} from '../lib/stores'
  import {
    WorkspacesService, ObservatoryService, EscalationService, BriefingService, onSessionDelta, onSessionStep, onSessionThinking,
    onSessionToolResult, onSessionToolInput, onSessionContextUsage, onSessionTurnEnd, onSessionCompact, onSessionNotice, onSessionUser, onSessionFallback, onWorkspaceChatTab, onAttentionTab, onAgent,
    onFindingsUpdated, onChatsChanged, onChatModeChanged, onQuestionAnswered, onQuestionAnswerFailed, onEscalationRaised, onEscalationResolved, call, callOk, answerQuestion, answerEscalation, setWorkspacesFromList, upsertWorkspace, isAgentRunningStatus, linkWorkspaceIssue, type WorkspaceStep,
  } from '../lib/api'
  import { mapChatItems, mergePersistedWithLive, retainOptimisticUsers, appendStream as appendStreamRow, applyStream, addTool as addToolRow, addUser, attachToolResult, patchToolInput, finishTurn as finishTurnRows, noteThinkingTokens, endTurn as endTurnRow, applyCompact as applyCompactRow, applyNotice as applyNoticeRow, applyFallback as applyFallbackRow, dropOpenCompact, toSendAttachments, stripAttachmentMarkers, composeMarkedText, isChatStoppedReply, isChatInjectedReply, displayUserChatText, pendingSendBubbleBody, formatAskUserDisplayText, dropLiveAfterFooter, shouldRefreshTurnStamp, shouldSkipIdleTranscriptReset, currentTurnHasEnded, dropOptimisticUser, mergeRejectedSteerDraft, mergeRejectedSteerAttachments, CHAT_RETRY_DISPLAY } from '../lib/agentchat'
  import {
    buildPromptContext, findPromptBySlash, listPromptActionsUnfiltered, runPromptAction,
    promptActionErrorMessage, promptActionRequest, takePromptActionRequest, promptEnabled,
    paramsFromArgs, pickPromptTab,
  } from '../lib/promptActions'
  import { readText } from '../lib/clipboard'
  import { contextUsageAfterCompact, contextUsageFromChatItems, contextUsageFromTurn, type ContextUsage } from '../lib/contextUsage'
  import { restoredChatCode, saveActiveChatCode } from '../lib/chat-tab-session'
  import { applyChatTabOrder } from '../lib/chat-tab-reorder'
  import { TabStripDrag } from '../lib/tab-strip-drag.svelte'
  import { getChatStrip, putChatStrip } from '../lib/caches/chat-strip-cache'
  import { captureStripScrollLeft } from '../lib/tab-strip-scroll'
  import { dropChatHeapKey, getChatHeap, putChatHeap } from '../lib/caches/chat-heap-cache'
  import {
    absorbHoistSending, bustHoistedDestHeap, clearHoistSending, concatHoistList, concatHoistText, destChatKey, hoistCacheBustCodes, liveChatTarget,
    hoistSendCompleteApplies, noteHoistChatAlias, noteHoistLiveSource, onHoistSendComplete, remapPrefixedRecord, stashHoistDraft,
    stashHoistPending, stashHoistSending, takeHoistDestBust, takeHoistDrafts, takeHoistPending, takeHoistSending,
  } from '../lib/hoist-chat-keys'
  import { getObservatorySnapshot, observatoryCacheKey } from '../lib/caches/observatory-cache'
  import { dropTranscriptUI } from '../lib/caches/chat-transcript-ui'
  import {
    loadModelCatalog, peekModelCatalog, modelCatalogRev, normalizeFlags, flagsForModel, flagsEqual,
    type CatalogModel,
  } from '../lib/models'
  import { modelProvider, providerEnabled, chatSteerFollowUp } from '../lib/providers'
  import {
    chatModeOptions, chatModeMeta, cycleChatMode, normalizeChatMode, wsHasRun, wsRunActive,
    wsCreating, wsCreateFailed, wsChatHeld, wsMainBlocked, wsSetupStatus,
    resolvedDefaultForMode, pickerAfterModeChange, chatHasStarted, NEW_CHAT_TITLE, type ChatMode,
  } from '../lib/modes'
  import Select from '../components/Select.svelte'
  import LoopPolicyPickers from '../components/LoopPolicyPickers.svelte'
  import Observatory from '../components/Observatory.svelte'
  import AgentChat, { type AgentMsg, type ChatAttachmentDraft, type FindingRef } from '../components/AgentChat.svelte'
  import PlanPanel from '../components/PlanPanel.svelte'
  import SessionCard from '../components/SessionCard.svelte'
  import ModelPickers from '../components/ModelPickers.svelte'
  import TabStrip from '../components/TabStrip.svelte'
  import TabChip from '../components/TabChip.svelte'
  import Icon from '../components/Icon.svelte'
  import { Dialogs } from '@wailsio/runtime'
  import type { WorkspaceChatTab, Escalation, Commit } from '../../bindings/recur/internal/domain/models'
  import { SessionFeed } from '../lib/session-feed.svelte'
  import { isPairChatCode, isLoopCockpitTab, mergeLoopCockpit, specialistLiveCodes } from '../lib/loop-cockpit'
  import { formatRelativeAge } from '../lib/format'
  import { simplifyEnabled } from '../lib/simplify'
  import {
    enabledLoops, enabledPolicies,
    fallbackLoops, fallbackPolicies, listLoops, listPolicies,
    setWorkspaceLoopPolicy, setChatLoopPolicy, workspaceLoopId, workspacePolicyId,
    type LoopInfo, type PolicyInfo,
  } from '../lib/loops'
  import { attachChatSurface, chatSurfaceForView } from '../lib/chat-surface'
  import { latestTodosFromMessages } from '../lib/todos'
  import { removeWorkspace } from '../lib/remove-workspace'
  import { uuidv7 } from '../lib/uuid'

  // Bound workspace (sidebar selection or the PR's linked workspace). Chat
  // maps stay on this instance so switching detail ↔ PR does not reset a turn.
  // shellActive: this instance is the live workbench. A parked hot shell stays
  // mounted but must not steal chat focus, engine focus, or composer queries.
  let {
    workspaceId: boundWsId,
    shellActive = true,
  }: { workspaceId: string; shellActive?: boolean } = $props()

  // One workspace noun: the code Observatory plus chat
  // tabs, each in its own MODE. The chat renders the FULL agent turn — text plus
  // each tool call — live and on reload (history from the session store). When
  // the autopilot has been engaged (hasRun), the conductor tab (Loop mode)
  // owns the run. Tabs are drag-reorderable.
  const ws = $derived($workspaces.find((w) => w.id === boundWsId))
  const hasRun = $derived(!!ws && wsHasRun(ws))
  let catalogLoops = $state<LoopInfo[]>(fallbackLoops())
  let catalogPolicies = $state<PolicyInfo[]>(fallbackPolicies())
  const loops = $derived(enabledLoops(catalogLoops, $settings))
  const policies = $derived(enabledPolicies(catalogPolicies, $settings))
  // The main working copy tracks the project's real checkout: no autopilot, so
  // the Loop chat mode is omitted from the dock selector and never cycled into.
  const isMain = $derived(!!ws?.isMain)
  const isPRHost = $derived(!!(ws?.hostPrId ?? '').trim())
  const chatSurface = $derived(chatSurfaceForView($view, $prs.find((p) => p.id === $selectedPRId)))
  // Worktree still allocating (eager create / loop start). Composer send
  // queues until setupHold clears; the editor overlay lifts as soon as
  // worktreePath lands (wsMainBlocked) so files can paint while setup_cmd runs.
  const creating = $derived(wsCreating(ws))
  const createFailed = $derived(wsCreateFailed(ws))
  const setupHold = $derived(wsChatHeld(ws))
  // Same prefix RetryWorkspaceSetup checks. Copy-files and allocate errors
  // share createFailed but cannot retry setup.
  const setupFailed = $derived(!!(ws?.lastError ?? '').startsWith('setup command failed'))
  const setupStatus = $derived(wsSetupStatus(ws))
  let setupErrOpen = $state(false)
  let rootEl = $state<HTMLElement | undefined>()

  function retryWorkspaceSetup() {
    void callOk(WorkspacesService.RetryWorkspaceSetup(boundWsId), 'Could not retry setup')
  }

  // Chat-tab identity: the Maestro thread is MA (present once a run is engaged);
  // regular tabs are PA / PA2 / ….
  function chatKey(wsId: string, code: string) {
    return `${wsId}\t${code}`
  }
  function acceptsChatCode(wsId: string, code: string) {
    if (wsId !== boundWsId) return false
    if (!$workspaces.some((x) => x.id === wsId)) return false
    return code === 'MA' || isPairChatCode(code)
  }

  // Chat tabs (Agent panel): MA (Loop) + PA* on one strip, one surface.
  // Seeded from the process-wide strip cache so a warm remount paints the
  // header immediately instead of waiting on ListWorkspaceChats.
  const seedWsId = untrack(() => boundWsId)
  const seededStrip = getChatStrip(seedWsId)
  // Transcripts, composer drafts, and dock pickers — same heap snapshot.
  bustHoistedDestHeap(seedWsId, takeHoistDestBust(seedWsId))
  const seededHeap = getChatHeap<AgentMsg, ChatAttachmentDraft>(seedWsId)
  let tabsByWs = $state<Record<string, WorkspaceChatTab[]>>(
    seededStrip?.tabs.length ? { [seedWsId]: seededStrip.tabs } : {},
  )
  let closedTabsByWs = $state<Record<string, WorkspaceChatTab[]>>(
    seededStrip?.closed?.length ? { [seedWsId]: seededStrip.closed } : {},
  )
  let recentsOpen = $state(false)
  const DELETE_ARM_MS = 3000
  let armedDeleteCode = $state<string | null>(null)
  let deleteArmTimer: ReturnType<typeof setTimeout> | undefined
  let activeCodeByWs = $state<Record<string, string>>(
    seededStrip?.tabs.length
      ? { [seedWsId]: seededStrip.activeCode || restoredChatCode(seedWsId, seededStrip.tabs) }
      : {},
  )
  // Keep-alive remount: pin the saved strip offset instead of chasing the
  // active chip (same as Observatory file tabs). Hot-shell A→B→A restores
  // via stashHotShellGeometry — TabStrip stays mounted, so this is unused.
  let chatTabScrollRestore = $state<{ left: number; id: number } | undefined>(
    typeof seededStrip?.tabScrollLeft === 'number'
      ? { left: seededStrip.tabScrollLeft, id: 1 }
      : undefined,
  )
  let forkingByWs = $state<Record<string, boolean>>({})
  const chatTabs = $derived(tabsByWs[boundWsId] ?? [])
  const closedTabs = $derived(closedTabsByWs[boundWsId] ?? [])
  const activeCode = $derived(
    activeCodeByWs[boundWsId] || chatTabs[0]?.code || 'PA',
  )
  const activeTab = $derived(chatTabs.find((t) => t.code === activeCode))
  const dockLoopId = $derived(activeTab?.loopId?.trim() || workspaceLoopId(ws, $settings))
  const dockPolicyId = $derived(activeTab?.policyId?.trim() || workspacePolicyId(ws, $settings))
  const loopPickersPinned = $derived(
    !!activeTab?.loopStarted || (!!hasRun && conductorChatCode() === activeCode),
  )

  /** Chat tab that owns this workspace's run (legacy empty → MA). */
  function conductorChatCode(): string {
    const owner = ws?.loopChatCode?.trim()
    if (owner && chatTabs.some((t) => t.code === owner)) return owner
    const started = chatTabs.find((t) => t.loopStarted)?.code
    if (started) return started
    if (chatTabs.some((t) => t.code === 'MA')) return 'MA'
    return ''
  }

  function conductorTabLocked(code: string): boolean {
    if (!hasRun || !ws || code !== conductorChatCode()) return false
    return wsRunActive(ws)
  }

  const onConductorTab = $derived(!!activeCode && activeCode === conductorChatCode())

  $effect(() => {
    const id = boundWsId
    const tabs = tabsByWs[id]
    if (!tabs?.length) return
    putChatStrip(id, {
      tabs,
      closed: closedTabsByWs[id],
      activeCode: activeCodeByWs[id],
    })
  })

  // Open loop escalation on the conductor transcript (same card as ask_user).
  let liveEsc = $state<Escalation | null>(null)

  function takeOpenEscalation(e: Escalation | null | undefined): Escalation | null {
    if (e?.status === 'open' && e.question) return e
    return null
  }
  let escLoadGen = 0
  const activeKey = $derived(chatKey(boundWsId, activeCode))

  // Publish which chat tab is on screen so PermissionPrompts can suppress the
  // floating card when AgentChat is already showing that request inline.
  $effect(() => {
    if (!shellActive) return
    const id = boundWsId
    const code = activeCode
    if (!id || !code) {
      focusedChat.set(null)
      return
    }
    focusedChat.set({ workspaceId: id, code })
    return () => {
      focusedChat.update((f) =>
        f?.workspaceId === id && f.code === code ? null : f,
      )
    }
  })

  // Overflow on `.chat-scroll` resets when the shell is moved into `#wsv-park`.
  // Geometry restore puts pixels back (or pins to the tail if we left at the
  // bottom). This reapplies AgentChat's last measured offset after layout.
  let wasShellActive = true
  $effect(() => {
    const on = shellActive
    if (on && !wasShellActive) {
      const chat = untrack(() => agentChat)
      chat?.restoreParkedScroll()
      void tick().then(() => {
        chat?.restoreParkedScroll()
        requestAnimationFrame(() => chat?.restoreParkedScroll())
      })
      const id = untrack(() => boundWsId)
      if (id) {
        WorkspacesService.ListWorkspaceChats(id)
          .then((tabs) => {
            if (!tabs?.length) return
            tabsByWs = { ...tabsByWs, [id]: tabs }
            resyncActivatedChats(id, tabs)
          })
          .catch(() => {})
      }
    }
    wasShellActive = on
  })

  // Background hoist can land chats on an already-visible dest (no focusChatCode,
  // shellActive never flips). Refetch tabs and bust caches without changing tab.
  // Source shells remap in-flight pending/drafts so .finally sends to dest.
  $effect(() => onChatsChanged((wsId, detail) => {
    const fromId = detail?.fromWorkspaceId
    if (fromId && fromId === boundWsId) {
      applySourceHoist(wsId, detail?.codeRemap)
    }
    if (wsId !== boundWsId) return
    const refresh = () => {
      absorbHoistDrafts()
      WorkspacesService.ListWorkspaceChats(wsId)
        .then((tabs) => {
          if (!tabs?.length) return
          tabsByWs = { ...tabsByWs, [wsId]: tabs }
          resyncActivatedChats(wsId, tabs, fromId ? (detail?.codeRemap ?? {}) : undefined, fromId ? detail?.movedCodes : undefined)
        })
        .catch(() => {})
      void refreshClosedTabs(wsId)
    }
    if (fromId) queueMicrotask(refresh)
    else refresh()
  }))

  // takeHoistSending copies the reservation; source .finally must notify dest.
  $effect(() => onHoistSendComplete((key, gen) => {
    if (!boundWsId || !key.startsWith(boundWsId + '\t')) return
    if (!hoistSendCompleteApplies(gen, sendGenByKey[key] ?? 0, hoistSendGenByKey[key])) return
    if (key in hoistSendGenByKey) hoistSendGenByKey = omitChatKey(hoistSendGenByKey, key)
    if (sendingByKey[key]) sendingByKey = { ...sendingByKey, [key]: false }
    tryFlushPending(key)
  }))

  // ask_user toast / inbox "question:" — select the asking chat tab once tabs exist.
  // tabsByWs is cached; if the target code is missing (stale list / new tab), refetch
  // once. Give up only in the refetch .then — not on effect re-runs while in flight
  // (attention/title events rewrite tabsByWs and would otherwise discard the fetch).
  let pendingFocusRefetching = ''
  $effect(() => {
    const pending = $pendingChatFocus
    if (!pending) {
      pendingFocusRefetching = ''
      return
    }
    if (pending.workspaceId !== boundWsId) return
    const id = pending.workspaceId
    const code = pending.code
    const tabs = tabsByWs[id]
    if (!tabs?.length) return
    if (tabs.some((t) => t.code === code)) {
      activeCodeByWs = { ...activeCodeByWs, [id]: code }
      pendingChatFocus.set(null)
      pendingFocusRefetching = ''
      resyncActivatedChats(id, tabs)
      void WorkspacesService.ListWorkspaceChats(id)
        .then((fresh) => {
          if (!fresh?.length) return
          tabsByWs = { ...tabsByWs, [id]: fresh }
          resyncActivatedChats(id, fresh)
        })
        .catch(() => {})
      return
    }
    const key = `${id}\t${code}`
    if (pendingFocusRefetching === key) return
    pendingFocusRefetching = key
    WorkspacesService.ListWorkspaceChats(id)
      .then((fresh) => {
        if (pendingFocusRefetching !== key) return
        if (fresh?.length) {
          tabsByWs = { ...tabsByWs, [id]: fresh }
          if (fresh.some((t) => t.code === code)) {
            activeCodeByWs = { ...activeCodeByWs, [id]: code }
          }
          resyncActivatedChats(id, fresh)
        }
        pendingChatFocus.set(null)
        pendingFocusRefetching = ''
      })
      .catch(() => {
        if (pendingFocusRefetching !== key) return
        pendingChatFocus.set(null)
        pendingFocusRefetching = ''
      })
  })

  // Live loop escalation — load from the open row, not shared workspace state
  // (a sibling walk can overwrite state while this question is still unanswered).
  $effect(() => {
    const id = boundWsId
    if (!id) {
      liveEsc = null
      return
    }
    const gen = ++escLoadGen
    void EscalationService.GetEscalation(id)
      .then((e) => {
        if (gen === escLoadGen) liveEsc = takeOpenEscalation(e)
      })
      .catch(() => {
        if (gen === escLoadGen) liveEsc = null
      })
  })

  // Viewed watermark: committing to a workspace (not a rapid scan) marks its
  // activity seen, which clears the Inbox card's changed-dot.
  let viewedFor = $state('')

  $effect(() => {
    if (!shellActive) {
      viewedFor = ''
      return
    }
    if ($workspaceScanPending) return
    const id = boundWsId
    if (!id) return
    if (viewedFor === id) return
    if (!$briefing.generatedAt) return
    viewedFor = id
    const brief = $briefing.workspaces.find((w) => w.workspaceId === id)
    const activity = brief?.lastActivityAt ?? 0
    if (activity > 0) void BriefingService.MarkWorkspaceViewed(id, activity).catch(() => {})
  })

  async function refreshClosedTabs(id: string) {
    const list = await WorkspacesService.ListClosedWorkspaceChats(id).catch(() => null)
    closedTabsByWs = { ...closedTabsByWs, [id]: list ?? [] }
  }

  $effect(() => {
    if ($workspaceScanPending) return
    const id = boundWsId
    if (!id) return
    void refreshClosedTabs(id)
  })

  $effect(() => {
    if ($workspaceScanPending) return
    if (!boundWsId) return
    const id = boundWsId
    let cancelled = false
    WorkspacesService.ListWorkspaceChats(id)
      .then((tabs) => {
        if (cancelled) return
        const list = tabs?.length ? tabs : [{ code: 'PA', title: 'New chat', updatedAt: 0 } as WorkspaceChatTab]
        tabsByWs = { ...tabsByWs, [id]: list }
        if (!activeCodeByWs[id]) {
          activeCodeByWs = { ...activeCodeByWs, [id]: restoredChatCode(id, list) }
        }
      })
      .catch(() => {
        if (cancelled || tabsByWs[id]) return
        const list = [{ code: 'PA', title: 'New chat', updatedAt: 0 } as WorkspaceChatTab]
        tabsByWs = { ...tabsByWs, [id]: list }
        if (!activeCodeByWs[id]) {
          activeCodeByWs = { ...activeCodeByWs, [id]: restoredChatCode(id, list) }
        }
      })
    return () => { cancelled = true }
  })

  // Tell the engine which chat tab is selected. A finishing turn skips unseen
  // only when the surface is really visible (app focused, no modal occlusion).
  $effect(() => {
    const id = boundWsId
    const code = activeCode
    if (!shellActive) return
    if ($workspaceScanPending) return
    if (!id || !code || !tabsByWs[id]) return
    saveActiveChatCode(id, code)
    void WorkspacesService.SetWorkspaceChatFocus(id, code).catch(() => {})
    const tabs = tabsByWs[id]
    if (tabs.some((t) => t.code === code && t.unseen)) {
      tabsByWs = {
        ...tabsByWs,
        [id]: tabs.map((t) => (t.code === code ? { ...t, unseen: false } : t)),
      }
    }
  })

  /** Park keyboard focus in the agent composer after tab switch / new / close. */
  async function focusChatInput() {
    await tick()
    const el = rootEl?.querySelector<HTMLElement>('.obs-left .chat-in .chat-composer')
    el?.focus({ preventScroll: true })
  }

  /** Opens a coding chat tab. Returns its code, or null if creation failed. */
  async function newChatTab(): Promise<string | null> {
    const id = boundWsId
    if (!id) return null
    const tab = await WorkspacesService.CreateWorkspaceChat(id).catch(() => null)
    if (!tab?.code) return null
    const list = await WorkspacesService.ListWorkspaceChats(id).catch(() => null)
    tabsByWs = { ...tabsByWs, [id]: list?.length ? list : [...(tabsByWs[id] ?? []), tab] }
    activeCodeByWs = { ...activeCodeByWs, [id]: tab.code }
    await focusChatInput()
    return tab.code
  }

  /** Create a sibling workspace with this chat copied through the fork point. */
  async function forkChatToWorkspace(messageId: string) {
    const id = boundWsId
    const code = activeCode
    if (!id || !code || !messageId || forkingByWs[id]) return
    forkingByWs = { ...forkingByWs, [id]: true }
    try {
      const dest = await call(
        WorkspacesService.ForkWorkspaceChatToWorkspace(id, code, messageId),
        'Could not fork to a new workspace',
      )
      if (!dest?.id) return
      upsertWorkspace(dest)
      openWorkspace(dest.id)
    } finally {
      const { [id]: _, ...rest } = forkingByWs
      forkingByWs = rest
    }
  }

  /** Branch the open chat at a finished turn and switch to the new tab. */
  async function forkChatTab(messageId: string) {
    const id = boundWsId
    const code = activeCode
    if (!id || !code || !messageId || forkingByWs[id]) return
    forkingByWs = { ...forkingByWs, [id]: true }
    try {
      const tab = await WorkspacesService.ForkWorkspaceChat(id, code, messageId).catch(() => null)
      if (!tab?.code) {
        pushToast('error', 'Could not fork this chat')
        return
      }
      const list = await WorkspacesService.ListWorkspaceChats(id).catch(() => null)
      tabsByWs = { ...tabsByWs, [id]: list?.length ? list : [...(tabsByWs[id] ?? []), tab] }
      activeCodeByWs = { ...activeCodeByWs, [id]: tab.code }
      await focusChatInput()
    } finally {
      const { [id]: _, ...rest } = forkingByWs
      forkingByWs = rest
    }
  }

  /** Show what the agent changed between a turn's checkpoints in the
   *  Observatory review surface (as a commit scope). */
  async function showTurnDiff(messageId: string) {
    const id = boundWsId
    const code = activeCode
    if (!id || !code || !messageId) return
    const turn = await call(WorkspacesService.WorkspaceChatTurnDiff(id, code, messageId), 'Could not load this turn\'s changes')
    if (!turn || id !== boundWsId) return
    // An empty scope would fall back to `git show <hash>` — refuse it here.
    if (!turn.files?.length) { pushToast('info', 'The agent didn\'t change any files during this turn'); return }
    observatory?.openTurnReview(turn)
  }

  /** Drop messages after a turn and, when later turns edited files, restore the
   *  worktree to that turn's end checkpoint. */
  async function resetChatTo(messageId: string, restoresFiles: boolean) {
    const id = boundWsId
    const code = activeCode
    if (!id || !code || !messageId || sendingByKey[activeKey]) return
    let ok = false
    try {
      // window.confirm is a no-op in WKWebView — native dialog instead.
      const choice = await Dialogs.Question({
        Title: 'Reset to this point?',
        Message: restoresFiles
          ? 'Files in the worktree go back to how they were at the end of this turn, and every later message in this chat is removed. This cannot be undone.'
          : 'Every later message in this chat is removed. Files in the worktree are not changed. This cannot be undone.',
        Buttons: [{ Label: 'Cancel', IsCancel: true }, { Label: 'Reset', IsDefault: true }],
      })
      ok = choice === 'Reset'
    } catch {
      ok = false
    }
    if (!ok) return
    if (!(await callOk(WorkspacesService.ResetWorkspaceChat(id, code, messageId), 'Could not reset this chat'))) return
    resyncChat(id, code)
    pushToast('success', 'Reset to this point')
  }

  /** Close a coding chat tab. workspaceId defaults to the selected workspace;
   *  pass it explicitly when cleaning up a tab on a workspace the user left. */
  async function closeChatTab(code: string, workspaceId?: string) {
    const id = workspaceId || boundWsId
    if (!id) return
    const prev = tabsByWs[id] ?? []
    const idx = prev.findIndex((t) => t.code === code)
    const wasActive = activeCodeByWs[id] === code
    const list = await WorkspacesService.CloseWorkspaceChat(id, code).catch(() => null)
    if (!list?.length) return
    tabsByWs = { ...tabsByWs, [id]: list }
    if (wasActive) {
      // Neighbor that slid into this slot (right, or left if last) — same as
      // terminal/editor tabs; not always the first tab.
      const next = list[Math.min(Math.max(idx, 0), list.length - 1)]?.code ?? list[0].code
      activeCodeByWs = { ...activeCodeByWs, [id]: next }
    } else if (!list.some((t) => t.code === activeCodeByWs[id])) {
      activeCodeByWs = { ...activeCodeByWs, [id]: list[0].code }
    }
    // Drop in-memory transcript for the closed tab (history remains in recents).
    const key = chatKey(id, code)
    dropTranscriptUI(key)
    if (chatByKey[key]) {
      const { [key]: _, ...rest } = chatByKey
      chatByKey = rest
    }
    if (contextUsageByKey[key]) {
      const { [key]: _, ...rest } = contextUsageByKey
      contextUsageByKey = rest
    }
    // Only steal focus when closing on the workspace the user is viewing.
    if (wasActive && boundWsId === id) await focusChatInput()
    void refreshClosedTabs(id)
  }

  async function reopenClosedChat(code: string) {
    const id = boundWsId
    if (!id) return
    recentsOpen = false
    const list = await WorkspacesService.ReopenWorkspaceChat(id, code).catch(() => null)
    if (!list?.length) return
    tabsByWs = { ...tabsByWs, [id]: list }
    await refreshClosedTabs(id)
    selectChatTab(code)
  }

  function omitChatKey<T>(rec: Record<string, T>, key: string): Record<string, T> {
    if (!(key in rec)) return rec
    const { [key]: _, ...rest } = rec
    return rest
  }

  // Hoist can reuse dest PA* codes: parked transcript/picker caches are for
  // the placeholder, not the moved session. Ordinary activation passes no remap
  // so unsent model/flag picks survive a workspace switch.
  function bustHoistedChatCaches(id: string, codes: string[]) {
    for (const code of codes) {
      if (!code) continue
      const key = chatKey(id, code)
      if (loadedFor === key) loadedFor = ''
      transcriptGenByKey[key] = (transcriptGenByKey[key] ?? 0) + 1
      pickersByKey = omitChatKey(pickersByKey, key)
      simplifyByKey = omitChatKey(simplifyByKey, key)
      pickersFetched.delete(key)
      simplifyFetched.delete(key)
      pickerGenByKey[key] = (pickerGenByKey[key] ?? 0) + 1
      simplifyGenByKey[key] = (simplifyGenByKey[key] ?? 0) + 1
      delete pickersLoadingByKey[key]
    }
  }

  function resyncActivatedChats(id: string, tabs: WorkspaceChatTab[], remap?: Record<string, string> | null, moved?: readonly string[] | null) {
    bustHoistedChatCaches(id, hoistCacheBustCodes(tabs.map((t) => t.code), remap, moved))
    takeHoistDestBust(id)
    const code = untrack(() => activeCode)
    if (!code) return
    void loadChatPickers(id, code)
    if (code === 'MA') void loadSimplify(id, code)
  }

  function applySourceHoist(toId: string, remap?: Record<string, string>) {
    const fromId = boundWsId
    if (!fromId || !toId || fromId === toId) return
    const prefix = fromId + '\t'
    const bound = inputBoundKey
    if (bound.startsWith(prefix)) {
      stashHoistDraft(destChatKey(fromId, bound.slice(prefix.length), toId, remap), chatInput, chatAttachments)
    }
    for (const k of Object.keys(inputByKey)) {
      if (!k.startsWith(prefix) || k === bound) continue
      stashHoistDraft(destChatKey(fromId, k.slice(prefix.length), toId, remap), inputByKey[k], attachmentsByKey[k])
    }
    for (const k of Object.keys(attachmentsByKey)) {
      if (!k.startsWith(prefix) || k === bound || k in inputByKey) continue
      stashHoistDraft(destChatKey(fromId, k.slice(prefix.length), toId, remap), '', attachmentsByKey[k])
    }
    const nextPending = { ...pendingByKey }
    const nextHold = { ...holdPendingByKey }
    let droppedPending = false
    for (const k of Object.keys(pendingByKey)) {
      if (!k.startsWith(prefix)) continue
      const destKey = destChatKey(fromId, k.slice(prefix.length), toId, remap)
      noteHoistChatAlias(k, destKey)
      stashHoistPending(destKey, pendingByKey[k], !!holdPendingByKey[k])
      delete nextPending[k]
      delete nextHold[k]
      droppedPending = true
    }
    for (const k of Object.keys(holdPendingByKey)) {
      if (!k.startsWith(prefix) || k in pendingByKey) continue
      const destKey = destChatKey(fromId, k.slice(prefix.length), toId, remap)
      noteHoistChatAlias(k, destKey)
      stashHoistPending(destKey, [], true)
      delete nextHold[k]
      droppedPending = true
    }
    if (droppedPending) {
      pendingByKey = nextPending
      holdPendingByKey = nextHold
    }
    for (const k of Object.keys(sendingByKey)) {
      if (!k.startsWith(prefix) || !sendingByKey[k]) continue
      const destKey = destChatKey(fromId, k.slice(prefix.length), toId, remap)
      const gen = sendGenByKey[k] ?? 1
      stashHoistSending(destKey, gen)
      // Same-instance hoist remaps sending next; record identity before dest looks busy.
      if (!sendingByKey[destKey]) hoistSendGenByKey = { ...hoistSendGenByKey, [destKey]: gen }
    }
    inputByKey = remapPrefixedRecord(inputByKey, fromId, toId, remap, concatHoistText)
    attachmentsByKey = remapPrefixedRecord(attachmentsByKey, fromId, toId, remap, concatHoistList)
    sendingByKey = remapPrefixedRecord(sendingByKey, fromId, toId, remap, (d, s) => !!(d || s))
    sendGenByKey = remapPrefixedRecord(sendGenByKey, fromId, toId, remap, (d, s) => Math.max(d ?? 0, s))
    if (promptTargetKey.startsWith(prefix)) {
      promptTargetKey = destChatKey(fromId, promptTargetKey.slice(prefix.length), toId, remap)
    }
    if (bound.startsWith(prefix)) {
      inputBoundKey = destChatKey(fromId, bound.slice(prefix.length), toId, remap)
    }
  }

  function absorbHoistDrafts() {
    const bound = inputBoundKey
    if (bound) {
      inputByKey = { ...inputByKey, [bound]: chatInput }
      attachmentsByKey = { ...attachmentsByKey, [bound]: chatAttachments }
    }
    const got = takeHoistDrafts<ChatAttachmentDraft>(boundWsId)
    for (const [k, d] of Object.entries(got)) {
      if (d.input) inputByKey = { ...inputByKey, [k]: concatHoistText(inputByKey[k], d.input) }
      if (d.attachments?.length) {
        attachmentsByKey = { ...attachmentsByKey, [k]: concatHoistList(attachmentsByKey[k], d.attachments) }
      }
    }
    if (bound && got[bound]) {
      chatInput = inputByKey[bound] ?? chatInput
      chatAttachments = attachmentsByKey[bound] ?? chatAttachments
    }
    const inflight = takeHoistSending(boundWsId)
    if (Object.keys(inflight).length) {
      const next = absorbHoistSending(sendingByKey, sendGenByKey, hoistSendGenByKey, inflight)
      sendingByKey = next.sendingByKey
      sendGenByKey = next.sendGenByKey
      hoistSendGenByKey = next.hoistSendGenByKey
    }
    const qs = takeHoistPending<PendingChatSend>(boundWsId)
    for (const [k, q] of Object.entries(qs)) {
      if (q.items.length) pendingByKey = { ...pendingByKey, [k]: concatHoistList(pendingByKey[k], q.items) }
      if (q.hold) holdPendingByKey = { ...holdPendingByKey, [k]: true }
    }
    for (const k of Object.keys(qs)) tryFlushPending(k)
  }

  function dropChatClientState(key: string) {
    dropTranscriptUI(key)
    chatByKey = omitChatKey(chatByKey, key)
    contextUsageByKey = omitChatKey(contextUsageByKey, key)
    inputByKey = omitChatKey(inputByKey, key)
    attachmentsByKey = omitChatKey(attachmentsByKey, key)
    sendingByKey = omitChatKey(sendingByKey, key)
    sendGenByKey = omitChatKey(sendGenByKey, key)
    hoistSendGenByKey = omitChatKey(hoistSendGenByKey, key)
    pendingByKey = omitChatKey(pendingByKey, key)
    holdPendingByKey = omitChatKey(holdPendingByKey, key)
    turnStartedAtByKey = omitChatKey(turnStartedAtByKey, key)
    staleIdleArmedByKey = omitChatKey(staleIdleArmedByKey, key)
    turnGotTextByKey = omitChatKey(turnGotTextByKey, key)
    streamSeqByKey = omitChatKey(streamSeqByKey, key)
    pickersByKey = omitChatKey(pickersByKey, key)
    simplifyByKey = omitChatKey(simplifyByKey, key)
    pickersFetched.delete(key)
    simplifyFetched.delete(key)
    delete pickersLoadingByKey[key]
    pickerGenByKey[key] = (pickerGenByKey[key] ?? 0) + 1
    simplifyGenByKey[key] = (simplifyGenByKey[key] ?? 0) + 1
    transcriptGenByKey[key] = (transcriptGenByKey[key] ?? 0) + 1
  }

  function clearArmedDelete() {
    if (deleteArmTimer) {
      clearTimeout(deleteArmTimer)
      deleteArmTimer = undefined
    }
    armedDeleteCode = null
  }

  function onDeleteClosedChat(code: string) {
    if (armedDeleteCode !== code) {
      armedDeleteCode = code
      if (deleteArmTimer) clearTimeout(deleteArmTimer)
      deleteArmTimer = setTimeout(() => {
        deleteArmTimer = undefined
        if (armedDeleteCode === code) armedDeleteCode = null
      }, DELETE_ARM_MS)
      return
    }
    clearArmedDelete()
    void deleteClosedChat(code)
  }

  async function deleteClosedChat(code: string) {
    const id = boundWsId
    if (!id) return
    if ((tabsByWs[id] ?? []).some((t) => t.code === code)) {
      void refreshClosedTabs(id)
      return
    }
    const prev = closedTabsByWs[id] ?? []
    const next = prev.filter((t) => t.code !== code)
    closedTabsByWs = { ...closedTabsByWs, [id]: next }
    const menuWasOpen = recentsOpen
    if (!next.length) recentsOpen = false
    const list = await WorkspacesService.DeleteClosedWorkspaceChat(id, code).catch(() => null)
    if (!list) {
      closedTabsByWs = { ...closedTabsByWs, [id]: prev }
      recentsOpen = menuWasOpen
      pushToast('error', 'Could not delete this chat')
      return
    }
    closedTabsByWs = { ...closedTabsByWs, [id]: list }
    if (!list.length) recentsOpen = false
    const key = chatKey(id, code)
    dropChatClientState(key)
    dropChatHeapKey(id, key)
  }

  $effect(() => {
    void boundWsId
    recentsOpen = false
  })
  $effect(() => {
    if (!recentsOpen) clearArmedDelete()
  })
  $effect(() => {
    if (!recentsOpen) return
    const onDoc = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null
      if (!t?.closest?.('.wsv-recents')) recentsOpen = false
    }
    document.addEventListener('mousedown', onDoc)
    return () => document.removeEventListener('mousedown', onDoc)
  })

  function selectChatTab(code: string) {
    const id = boundWsId
    if (!id) return
    if (activeCodeByWs[id] === code) {
      void WorkspacesService.SetWorkspaceChatFocus(id, code).catch(() => {})
      void focusChatInput()
      return
    }
    activeCodeByWs = { ...activeCodeByWs, [id]: code }
    // Mark this tab focused + clear its unseen badge (backend + local chip).
    void WorkspacesService.SetWorkspaceChatFocus(id, code).catch(() => {})
    const tabs = tabsByWs[id]
    if (tabs?.some((t) => t.code === code && t.unseen)) {
      tabsByWs = {
        ...tabsByWs,
        [id]: tabs.map((t) => (t.code === code ? { ...t, unseen: false } : t)),
      }
    }
    void focusChatInput()
  }

  // The PR-detail dock is this same chat. A loop workspace's conversation
  // belongs on the conductor (Send Back / human messages), not a leftover Agent tab.
  $effect(() => {
    const prId = $selectedPRId
    if ($view !== 'prdetail' || !hasRun || !prId) return
    const owner = conductorChatCode()
    if (owner && activeCode !== owner) untrack(() => selectChatTab(owner))
  })

  $effect(() => onEscalationRaised((workspaceId, escalation) => {
    if (workspaceId !== boundWsId) return
    escLoadGen++
    liveEsc = takeOpenEscalation(escalation) ?? liveEsc
    const owner = conductorChatCode()
    if (owner) selectChatTab(owner)
  }))

  $effect(() => onEscalationResolved((workspaceId) => {
    if (workspaceId !== boundWsId) return
    const gen = ++escLoadGen
    void EscalationService.GetEscalation(workspaceId)
      .then((e) => {
        if (gen === escLoadGen) liveEsc = takeOpenEscalation(e)
      })
      .catch(() => {
        if (gen === escLoadGen) liveEsc = null
      })
  }))

  function rotateChatTab(dir: -1 | 1) {
    const tabs = chatTabs
    if (tabs.length < 2) return
    const i = tabs.findIndex((t) => t.code === activeCode)
    const next = tabs[(i < 0 ? 0 : i + dir + tabs.length) % tabs.length]
    selectChatTab(next.code)
  }

  function closeActiveChatTab() {
    if (activeCode) void closeChatTab(activeCode)
  }

  let chatTabStripEl = $state<HTMLDivElement | undefined>(undefined)
  let chatTabFloatEl = $state<HTMLDivElement | undefined>(undefined)

  $effect(() => {
    const strip = chatTabStripEl
    const id = boundWsId
    if (!strip || !id) return
    const save = () => {
      if (strip.closest('#wsv-park')) return
      const left = captureStripScrollLeft(strip)
      if (left == null) return
      const prev = getChatStrip(id)
      if (!prev) return
      putChatStrip(id, { ...prev, tabScrollLeft: left })
    }
    strip.addEventListener('scroll', save, { passive: true })
    return () => strip.removeEventListener('scroll', save)
  })
  let chatDragOwnerId = ''
  const chatTabDrag = new TabStripDrag<WorkspaceChatTab>({
    getStrip: () => chatTabStripEl,
    getFloat: () => chatTabFloatEl,
    getItems: () => {
      const id = boundWsId
      return id ? (tabsByWs[id] ?? []) : []
    },
    setItems: (next) => {
      const id = boundWsId
      if (!id) return
      tabsByWs = { ...tabsByWs, [id]: next }
    },
    keyOf: (t) => t.code,
    canStart: () => chatTabs.length > 1,
    bindOwner: () => {
      const id = boundWsId
      chatDragOwnerId = id
      return {
        getItems: () => (id ? (tabsByWs[id] ?? []) : []),
        setItems: (next) => {
          if (!id) return
          tabsByWs = { ...tabsByWs, [id]: next }
        },
      }
    },
    onCommit: async (current, snapshot) => {
      const id = chatDragOwnerId
      if (!id) return
      const ok = await callOk(
        WorkspacesService.ReorderWorkspaceChats(id, current.map((t) => t.code)),
        'Could not reorder chats',
      )
      if (!ok) chatTabDrag.rollback(snapshot)
    },
  })
  $effect(() => {
    boundWsId
    chatTabDrag.cancel()
  })
  onDestroy(() => chatTabDrag.cancel())
  const dragTab = $derived(
    chatTabDrag.fromKey ? chatTabs.find((t) => t.code === chatTabDrag.fromKey) : undefined,
  )

  function refreshTabTitles(wsId: string) {
    WorkspacesService.ListWorkspaceChats(wsId)
      .then((tabs) => {
        if (boundWsId !== wsId || !tabs?.length) return
        const current = tabsByWs[wsId]
        if ((chatTabDrag.active || chatTabDrag.committing) && current?.length) {
          tabsByWs = { ...tabsByWs, [wsId]: applyChatTabOrder(tabs, current.map((t) => t.code)) }
          return
        }
        tabsByWs = { ...tabsByWs, [wsId]: tabs }
      })
      .catch(() => {})
  }

  // Live tab title updates (provisional on send, LLM upgrade shortly after).
  $effect(() => onChatModeChanged((wsId, code, mode) => {
    if (wsId !== boundWsId) return
    const key = chatKey(wsId, code)
    const prev = pickersByKey[key]
    if (!prev) {
      pickersByKey = { ...pickersByKey, [key]: { model: '', flags: {}, mode: normalizeChatMode(mode) } }
      return
    }
    pickersByKey = { ...pickersByKey, [key]: { ...prev, mode: normalizeChatMode(mode) } }
  }))

  // ask_user answers resume via AnswerAskUser (not SendWorkspaceMessage from the
  // UI), so nothing was pushing a user bubble until post-turn resync. Paint one
  // immediately — same as a normal send — so the choice/text is visible while
  // the agent works.
  $effect(() => onQuestionAnswered((q, ans) => {
    if (!ws || q.workspaceId !== ws.id) return
    const key = chatKey(q.workspaceId, q.agentCode)
    const body = formatAskUserDisplayText(ans.choice, ans.freeText)
    const list = chatByKey[key] ?? []
    const last = list[list.length - 1]
    if (last?.me && last.body === body && !(ans.attachments?.length)) return
    pushMsg(key, {
      me: true,
      body,
      optimistic: true,
      ts: Date.now(),
      attachments: (ans.attachments ?? []).map((a) => ({
        id: a.id,
        name: a.name,
        mime: a.mime,
        dataUrl: a.data.startsWith('data:')
          ? a.data
          : `data:${a.mime || 'application/octet-stream'};base64,${a.data}`,
      })),
    })
  }))

  $effect(() => onQuestionAnswerFailed((q, ans) => {
    if (!ws || q.workspaceId !== ws.id) return
    const key = chatKey(q.workspaceId, q.agentCode)
    const body = formatAskUserDisplayText(ans.choice, ans.freeText)
    chatByKey = {
      ...chatByKey,
      [key]: (chatByKey[key] ?? []).filter((m) => !(m.optimistic && m.me && m.body === body)),
    }
  }))

  $effect(() => onWorkspaceChatTab((wsId, code, title) => {
    const tabs = tabsByWs[wsId]
    if (!tabs?.length || !title) return
    tabsByWs = {
      ...tabsByWs,
      [wsId]: tabs.map((t) => (t.code === code ? { ...t, title } : t)),
    }
  }))

  // Background turn finished (or was cleared) — patch the chip without refetch.
  $effect(() => onAttentionTab(({ workspaceId, code, tabUnseen, clearAllUnseen }) => {
    if (clearAllUnseen) {
      let next = tabsByWs
      let changed = false
      for (const [id, tabs] of Object.entries(tabsByWs)) {
        if (!tabs.some((t) => t.unseen)) continue
        changed = true
        next = { ...next, [id]: tabs.map((t) => (t.unseen ? { ...t, unseen: false } : t)) }
      }
      if (changed) tabsByWs = next
      return
    }
    if (!workspaceId || !code) return
    const tabs = tabsByWs[workspaceId]
    if (!tabs?.length) return
    // Focused tab is already seen server-side; keep the chip clear.
    if (!tabUnseen && boundWsId === workspaceId && activeCode === code) {
      tabsByWs = {
        ...tabsByWs,
        [workspaceId]: tabs.map((t) => (t.code === code ? { ...t, unseen: false } : t)),
      }
      return
    }
    tabsByWs = {
      ...tabsByWs,
      [workspaceId]: tabs.map((t) => (t.code === code ? { ...t, unseen: tabUnseen } : t)),
    }
  }))

  // Solo-chat model + flags: same T3 pair as Settings. Per chat tab — restore
  // the last model that session used, else Settings → Coder. Catalogue is
  // cached/prefetched at boot — wait for it before mounting pickers so the
  // dock never flashes a raw `provider/id` string. Warm remount seeds from the
  // chat heap (same snapshot as the composer draft) so the dock paints
  // immediately; WorkspaceChatSelection still runs and overwrites if the
  // backend disagrees.
  let catalogModels = $state<CatalogModel[]>(peekModelCatalog())
  let pickersByKey = $state<Record<string, { model: string; flags: Record<string, string>; mode: ChatMode }>>((() => {
    const out: Record<string, { model: string; flags: Record<string, string>; mode: ChatMode }> = {}
    for (const [key, p] of Object.entries(seededHeap?.pickers ?? {})) {
      if (!p?.model) continue
      out[key] = {
        model: p.model,
        flags: { ...p.flags },
        mode: normalizeChatMode(p.mode),
      }
    }
    return out
  })())
  /** In-flight hydrate promises keyed by chatKey — restore effect + prompt
   *  routing share one WorkspaceChatSelection RPC per tab. */
  const pickersLoadingByKey: Record<string, Promise<boolean>> = {}
  /** Tabs whose WorkspaceChatSelection already settled this mount. */
  const pickersFetched = new Set<string>()
  /** Bumped on local dock edits so an in-flight restore cannot clobber them. */
  const pickerGenByKey: Record<string, number> = {}
  /** Bumped on permanent delete so an in-flight WorkspaceChatItems cannot
   *  restore the deleted transcript (and later putChatHeap). */
  const transcriptGenByKey: Record<string, number> = {}
  const catalogReady = $derived(catalogModels.length > 0)
  // Require a real model — mode-only stubs from chat.mode_changed ({ model: '' })
  // must keep the dock skeleton up so onChatModeChange cannot persist defaults
  // over a still-in-flight session restore.
  const pickersReady = $derived(catalogReady && !!pickersByKey[activeKey]?.model)
  // The Maestro thread is pinned to Loop mode; its model comes from Settings.
  // Fall back to the tab's persisted mode (from ListWorkspaceChats) before
  // defaulting to agent — otherwise the dock flashes/sends as Agent while
  // WorkspaceChatSelection is still in flight, and a quick send can overwrite
  // a Review tab's persisted mode.
  const chatMode = $derived<ChatMode>(
    (() => {
      const m = normalizeChatMode(
        pickersByKey[activeKey]?.mode ??
          chatTabs.find((t) => t.code === activeCode)?.mode ??
          (activeCode === 'MA' ? 'loop' : ''),
      )
      if (isPRHost && (m === 'agent' || m === 'loop')) return 'review'
      return m
    })(),
  )
  const showLoopPickers = $derived(!!ws && !isMain && (chatMode === 'loop' || activeCode === 'MA'))

  $effect(() => {
    let cancelled = false
    void Promise.all([listLoops(), listPolicies()]).then(([ls, ps]) => {
      if (cancelled) return
      if (ls.length) catalogLoops = ls
      if (ps.length) catalogPolicies = ps
    })
    return () => { cancelled = true }
  })

  async function persistLoopPolicy(nextLoop: string, nextPolicy: string): Promise<boolean> {
    const id = ws?.id
    const code = activeCode
    if (!id || !code || loopPickersPinned) return false
    tabsByWs = {
      ...tabsByWs,
      [id]: (tabsByWs[id] ?? []).map((t) =>
        t.code === code ? { ...t, loopId: nextLoop, policyId: nextPolicy } : t,
      ),
    }
    try {
      await setChatLoopPolicy(id, code, nextLoop, nextPolicy)
      if (!hasRun) {
        const updated = await setWorkspaceLoopPolicy(id, nextLoop, nextPolicy)
        upsertWorkspace(updated)
      }
      return true
    } catch (err) {
      pushToast('error', err instanceof Error ? err.message : String(err))
      return false
    }
  }
  const model = $derived.by(() => {
    const raw = pickersByKey[activeKey]?.model || $settings?.modelAgent || 'claude-code/claude-sonnet-5-5'
    if (providerEnabled($settings?.disabledProviders, modelProvider(raw))) return raw
    return resolvedDefaultForMode(chatMode, $settings, catalogModels).model
  })
  const flags = $derived.by(() => {
    const raw = pickersByKey[activeKey]?.model || $settings?.modelAgent || 'claude-code/claude-sonnet-5-5'
    const cur = pickersByKey[activeKey]?.flags ?? {}
    if (providerEnabled($settings?.disabledProviders, modelProvider(raw))) return cur
    return resolvedDefaultForMode(chatMode, $settings, catalogModels).flags
  })

  let simplifyByKey = $state<Record<string, boolean>>((() => {
    const out: Record<string, boolean> = {}
    for (const [key, v] of Object.entries(seededHeap?.simplify ?? {})) {
      if (typeof v === 'boolean') out[key] = v
    }
    return out
  })())
  const simplifyFetched = new Set<string>(Object.keys(untrack(() => simplifyByKey)))
  const simplifyGenByKey: Record<string, number> = {}

  function bumpSimplifyGen(key: string) {
    simplifyGenByKey[key] = (simplifyGenByKey[key] ?? 0) + 1
  }

  function applySimplifySel(key: string, sel: { simplify?: boolean | null } | undefined, gen: number) {
    if ((simplifyGenByKey[key] ?? 0) !== gen) return
    simplifyFetched.add(key)
    if (typeof sel?.simplify !== 'boolean') return
    simplifyByKey = { ...simplifyByKey, [key]: sel.simplify }
  }

  async function loadSimplify(id: string, code: string) {
    if (!id || !code || !$settings) return
    const key = chatKey(id, code)
    if (simplifyFetched.has(key)) return
    const gen = simplifyGenByKey[key] ?? 0
    try {
      const sel = await WorkspacesService.WorkspaceChatSelection(id, code)
      applySimplifySel(key, sel, gen)
    } catch {
      // Leave unfetched so a later tab switch / Settings paint can retry.
    }
  }

  const simplifyOn = $derived(simplifyByKey[activeKey] ?? simplifyEnabled($settings))

  function freezeSimplify(key: string): boolean {
    const on = simplifyByKey[key] ?? simplifyEnabled($settings)
    if (simplifyByKey[key] !== on) {
      simplifyByKey = { ...simplifyByKey, [key]: on }
    }
    simplifyFetched.add(key)
    return on
  }

  function persistChatDock(
    id: string,
    code: string,
    simplify?: boolean,
    sel?: { model: string; flags: Record<string, string>; mode: string },
  ) {
    if (code === 'MA' && typeof simplify === 'boolean') {
      return WorkspacesService.RememberWorkspaceChatSimplify(id, code, simplify).catch(() => {})
    }
    const p = sel ?? pickersByKey[chatKey(id, code)]
    return WorkspacesService.RememberWorkspaceChatSelection(
      id, code, p?.model ?? '', p?.flags ?? {}, p?.mode ?? chatMode,
      typeof simplify === 'boolean' ? simplify : null,
    ).catch(() => {})
  }

  function toggleSimplify() {
    if (!boundWsId || !activeCode) return
    const next = !simplifyOn
    bumpSimplifyGen(activeKey)
    simplifyByKey = { ...simplifyByKey, [activeKey]: next }
    simplifyFetched.add(activeKey)
    void persistChatDock(boundWsId, activeCode, next)
  }

  // A tab's mode for the strip's identity icon: the live dock selection wins
  // (it changes before a send persists it), then the backend's persisted tab
  // mode, then agent. MA is always the Loop thread.
  function tabModeFor(tab: WorkspaceChatTab): ChatMode {
    if (tab.code === 'MA') return 'loop'
    return normalizeChatMode(pickersByKey[chatKey(boundWsId, tab.code)]?.mode ?? tab.mode)
  }

  $effect(() => {
    void $modelCatalogRev
    let cancelled = false
    loadModelCatalog()
      .then((models) => { if (!cancelled) catalogModels = models })
      .catch(() => {})
    return () => { cancelled = true }
  })

  function bumpPickerGen(key: string) {
    pickerGenByKey[key] = (pickerGenByKey[key] ?? 0) + 1
  }

  function applyChatPickers(
    key: string,
    next: { model: string; flags: Record<string, string>; mode: ChatMode },
  ) {
    const cur = pickersByKey[key]
    if (
      cur?.model === next.model
      && cur.mode === next.mode
      && flagsEqual(cur.flags, next.flags)
    ) return
    pickersByKey = { ...pickersByKey, [key]: next }
  }

  /** Load dock model/flags/mode for a tab (session selection or mode default).
   *  Shares in-flight work per tab; requires Settings + a non-empty catalogue
   *  so flag dims are not dropped as {}. Safe to await from prompt routing
   *  before onChatModeChange. A cache hit paints immediately and still
   *  refreshes from WorkspaceChatSelection. */
  async function loadChatPickers(id: string, code: string): Promise<boolean> {
    if (!id || !code || code === 'MA' || !$settings) return false
    const key = chatKey(id, code)
    const have = !!pickersByKey[key]?.model
    if (have && pickersFetched.has(key)) return true
    const inflight = pickersLoadingByKey[key]
    if (inflight) return have ? true : inflight

    // Register before the body runs so a sync early-return's `finally` cannot
    // delete a key that was never assigned (leaving a settled promise pinned).
    let settle!: (ok: boolean) => void
    const run = new Promise<boolean>((r) => { settle = r })
    pickersLoadingByKey[key] = run
    const gen = pickerGenByKey[key] ?? 0
    void (async () => {
      try {
        if (!catalogModels.length) {
          try {
            const models = await loadModelCatalog()
            if (models?.length) catalogModels = models
          } catch { /* catalogReady stays false below */ }
        }
        if (!catalogModels.length || !$settings) {
          settle(!!pickersByKey[key]?.model)
          return
        }
        const tabMode = normalizeChatMode((tabsByWs[id] ?? []).find((t) => t.code === code)?.mode)
        const write = (next: { model: string; flags: Record<string, string>; mode: ChatMode }) => {
          if ((pickerGenByKey[key] ?? 0) !== gen) return
          applyChatPickers(key, next)
        }
        try {
          const sel = await WorkspacesService.WorkspaceChatSelection(id, code)
          if (!simplifyFetched.has(key)) applySimplifySel(key, sel, simplifyGenByKey[key] ?? 0)
          // Keep a live mode from chat.mode_changed stubs — sel/tabMode can lag
          // the persist that preceded the event (or be empty on RPC failure).
          const mode = pickersByKey[key]?.mode
            ?? normalizeChatMode(sel?.mode || tabMode)
          const d = resolvedDefaultForMode(mode, $settings, catalogModels)
          const stored = sel?.model || ''
          const storedOk = !!stored && providerEnabled($settings?.disabledProviders, modelProvider(stored))
          const ref = storedOk ? stored : d.model
          const curFlags = storedOk ? (sel?.flags ?? {}) : d.flags
          write({
            model: ref,
            flags: normalizeFlags(flagsForModel(catalogModels, ref), curFlags),
            mode,
          })
          settle(!!pickersByKey[key]?.model)
        } catch {
          if (pickersByKey[key]?.model) {
            settle(true)
            return
          }
          if (!catalogModels.length || !$settings) {
            settle(false)
            return
          }
          const mode = pickersByKey[key]?.mode ?? tabMode
          const d = resolvedDefaultForMode(mode, $settings, catalogModels)
          write({ model: d.model, flags: d.flags, mode })
          settle(!!pickersByKey[key]?.model)
        }
      } catch {
        settle(!!pickersByKey[key]?.model)
      } finally {
        if ((pickerGenByKey[key] ?? 0) === gen && pickersByKey[key]?.model) pickersFetched.add(key)
        if (pickersLoadingByKey[key] === run) delete pickersLoadingByKey[key]
      }
    })()
    return have ? true : run
  }

  // Restore last session selection (or the mode's Settings-role default) per tab.
  $effect(() => {
    if (!catalogReady || !$settings) return
    const id = boundWsId
    const code = activeCode
    if (!id || !code) return
    void loadChatPickers(id, code)
  })

  $effect(() => {
    if (!$settings) return
    const id = boundWsId
    const code = activeCode
    if (!id || code !== 'MA') return
    void loadSimplify(id, code)
  })

  function onChatModelChange(ref: string, nextFlags: Record<string, string>) {
    bumpPickerGen(activeKey)
    pickersByKey = { ...pickersByKey, [activeKey]: { model: ref, flags: nextFlags, mode: chatMode } }
  }

  function onChatFlagsChange(next: Record<string, string>) {
    bumpPickerGen(activeKey)
    pickersByKey = { ...pickersByKey, [activeKey]: { model, flags: next, mode: chatMode } }
  }

  function onChatModeChange(next: string): Promise<void> {
    const m = normalizeChatMode(next)
    // The main working copy can't run the autopilot — ignore any Loop request.
    if (isMain && m === 'loop') return Promise.resolve()
    if (isPRHost && (m === 'loop' || m === 'agent')) return Promise.resolve()
    const owner = conductorChatCode()
    if (m !== 'loop' && activeCode === owner && hasRun && wsRunActive(ws)) {
      pushToast('error', 'This tab is running the loop — pause or stop it before leaving Loop mode.')
      return Promise.resolve()
    }
    // Unused chat still on this mode's Settings default → follow the new mode's
    // default. An in-progress session keeps its model (mode is tool policy).
    // An explicit pick is kept either way. Unhydrated pickers (coder fallback +
    // {} flags) must follow the new default — see pickerAfterModeChange.
    const hydrated = !!pickersByKey[activeKey]?.model
    const started = chatHasStarted(
      chatTabs.find((t) => t.code === activeCode)?.title,
      chatByKey[activeKey],
    )
    const { model: nextModel, flags: nextFlags } = pickerAfterModeChange(
      chatMode, m, model, flags, $settings, catalogModels, hydrated, started,
    )
    bumpPickerGen(activeKey)
    pickersByKey = { ...pickersByKey, [activeKey]: { model: nextModel, flags: nextFlags, mode: m } }
    // Keep the tab-strip icon in sync immediately (don't wait for a send).
    const id = boundWsId
    const tabs = tabsByWs[id] ?? []
    if (id && tabs.some((t) => t.code === activeCode)) {
      tabsByWs = {
        ...tabsByWs,
        [id]: tabs.map((t) => (t.code === activeCode ? { ...t, mode: m } : t)),
      }
    }
    // Persist mode (and model when it followed the mode default) so reload
    // restores the dock without requiring a send first.
    if (id) {
      return WorkspacesService.RememberWorkspaceChatSelection(
        id, activeCode, nextModel, nextFlags, m, null,
      ).catch(() => {})
    }
    return Promise.resolve()
  }

  // ⌘. while the composer is focused — advance the dock mode selector.
  // The Maestro tab is pinned to Loop, so cycling there is a no-op.
  // Locked while a turn is running: queued follow-ups inherit that turn's mode.
  // Also wait for pickersReady so we don't persist coder-default model/flags
  // over a Review tab's restored selection mid-restore.
  function cycleChatModeFromComposer() {
    if (composerBusy || activeCode === 'MA' || !pickersReady) return
    onChatModeChange(cycleChatMode(chatMode, isMain || isPRHost, isPRHost))
  }

  const seededChatOpen = (() => {
    const w = get(workspaces).find((x) => x.id === boundWsId)
    if (!w) return true
    return getObservatorySnapshot(observatoryCacheKey(w.projectId, w.id))?.leftOpen ?? true
  })()
  let chatOpen = $state(seededChatOpen)

  // Point at the Plan panel when a plan lands on the OPEN workspace (a
  // Plan-mode chat or the loop planner filed it).
  let hadPlanFor = $state('')
  $effect(() => {
    if (!ws) return
    const marker = `${ws.id}\t${ws.plan ? '1' : '0'}`
    if (hadPlanFor === marker) return
    const before = hadPlanFor
    hadPlanFor = marker
    // Only toast on a null→set transition for the SAME workspace (not on open).
    if (before === `${ws.id}\t0` && ws.plan) {
      void observatory?.selectCodePanel('plan')
      observatoryJump.set({ workspaceId: ws.id, panel: 'plan' })
      pushToast('success', 'Plan filed — review and approve it in the Plan panel')
    }
  })

  // Per chat-tab transcript/busy/draft state (workspaceId + agent code).
  // Background tabs can finish a turn without leaking dots onto the active tab.
  // Seeded from the process-wide heap so a warm remount paints bubbles on the
  // first frame (the editor already does this via observatory-cache). The
  // arrays are only ever reassigned, so $state.raw skips proxying hundreds of
  // message objects. WorkspaceChatItems still refetches in the background.
  let chatByKey = $state.raw<Record<string, AgentMsg[]>>(seededHeap?.chat ?? {})
  /** Last-turn context fill per chat tab (composer ring + popover). Copied
   *  off the heap so a warm remount paints the ring without proxying the
   *  cached snapshot (same copy-out as dock pickers). */
  let contextUsageByKey = $state.raw<Record<string, ContextUsage>>((() => {
    const out: Record<string, ContextUsage> = {}
    for (const [key, u] of Object.entries(seededHeap?.usage ?? {})) {
      if (!u) continue
      out[key] = { ...u }
    }
    return out
  })())
  let inputByKey = $state<Record<string, string>>(seededHeap?.input ?? {})
  let attachmentsByKey = $state<Record<string, ChatAttachmentDraft[]>>(seededHeap?.attachments ?? {})
  // Optimistic busy around the Send*Message RPC. Cleared in .finally even when
  // the engine turn is still running (e.g. binding timeout / remount) — the
  // footer also follows agentsRunning so dots/timer stay up with the sidebar.
  let sendingByKey = $state<Record<string, boolean>>({})
  // Model of the in-flight turn — Settings can change the composer picker mid-turn.
  let liveModelByKey = $state<Record<string, string>>({})
  // Monotonic dispatch generation per chat key — a prior send's .finally must
  // not clear busy / flush after a newer dispatch has already started.
  let sendGenByKey = $state<Record<string, number>>({})
  let steerClock = 0
  // Source-local gen of a hoisted in-flight send. Dest's leftover counter can be
  // higher; completion matches this identity, not sendGenByKey.
  let hoistSendGenByKey = $state<Record<string, number>>({})
  // Follow-ups typed while a turn is in flight; folded into one next turn on idle.
  type PendingChatSend = {
    id: string
    marked: string
    plain: string
    drafts: ChatAttachmentDraft[]
    mode: ChatMode
    model: string
    flags: Record<string, string>
    /** UI label for session display_text; empty → engine derives from wraps. */
    displayText?: string
    /** Prompt-action turns must not merge with normal queued sends. */
    promptAction?: boolean
    /** Send order for rejected-steer restore. */
    seq?: number
    /** When true, do not resume this tab's prior provider session. */
    freshSession?: boolean
  }
  let rejectedSteersByKey = $state<Record<string, PendingChatSend[]>>({})

  let observatory = $state<{
    getFocusedEditorContext: () => import('../lib/commands').EditorContext | null
    selectCodePanel: (next: CodePanelKey) => void | Promise<void>
    openArtifact: (a: { id?: string; title?: string; kind?: string; path?: string }) => Promise<boolean>
    openTurnReview: (c: Commit) => void
    showAgentPanel: () => void
  } | undefined>()
  let planPanelCmp = $state<PlanPanel | undefined>(undefined)
  let planTab = $state<'details' | 'todos' | 'attachments'>('details')
  let planEditing = $state(false)
  // boundWsId is re-passed (same string) whenever ListWorkspaces refreshes
  // the store — cycling a plan step must not reset the tab. Same trap as
  // Observatory's unseen-artifact badge (`void workspaceId`).
  let planUiForWs = ''
  $effect(() => {
    const id = boundWsId
    if (id === planUiForWs) return
    planUiForWs = id
    planTab = 'details'
    planEditing = false
  })
  // A plan-card click asked for a specific pane (steps → Todos).
  $effect(() => {
    const t = $planPanelTab
    if (!t) return
    planTab = t
    planPanelTab.set(null)
  })
  // A merged child's transcript was requested (Plan panel step link): switch to
  // the conductor tab first so sessionFeed loads that owner, then scroll the
  // wave card into view. Lookup after the switch — facts are filtered by tab.
  let childRevealFor = ''
  $effect(() => {
    const childId = $childTranscriptRequest
    if (!childId || !boundWsId) return
    const wsId = boundWsId
    const conductor = ws?.loopChatCode || 'MA'
    if (activeCode !== conductor) {
      activeCodeByWs = { ...activeCodeByWs, [wsId]: conductor }
      return
    }
    const fact = sessionFeed.facts.find((f) => f.card === 'fanout' && (f.data ?? '').includes(`"${childId}"`))
    if (!fact?.id) return
    const key = `${wsId}\t${childId}`
    if (childRevealFor === key) return
    childRevealFor = key
    observatory?.showAgentPanel()
    if (!chatOpen) chatOpen = true
    const factId = fact.id
    const reveal = async () => {
      const deadline = performance.now() + 2000
      while (!agentChat && performance.now() < deadline) await new Promise((r) => requestAnimationFrame(() => r(undefined)))
      await agentChat?.revealFact(factId)
    }
    void tick().then(reveal).finally(() => {
      if (childRevealFor === key) childRevealFor = ''
    })
  })
  function approvePlanFromHead() {
    const go = () => { void planPanelCmp?.approvePlan() }
    if (planPanelCmp) go()
    else void tick().then(go)
  }
  function executePlanFromHead() {
    const go = () => planPanelCmp?.executeInChat()
    if (planPanelCmp) go()
    else void tick().then(go)
  }
  function runPlanFromHead() {
    const go = () => { void planPanelCmp?.runAsLoop() }
    if (planPanelCmp) go()
    else void tick().then(go)
  }
  let promptRunning = $state(false)
  let promptAbort: AbortController | null = null
  /** chatKey the in-flight prompt action will post to (invoke tab while routing). */
  let promptTargetKey = $state('')
  let pendingByKey = $state<Record<string, PendingChatSend[]>>(
    (seededHeap?.pending ?? {}) as Record<string, PendingChatSend[]>,
  )
  // After Stop, keep follow-ups queued until the user explicitly sends again —
  // idle/finally must not auto-dispatch them.
  let holdPendingByKey = $state<Record<string, boolean>>(seededHeap?.holdPending ?? {})
  // Wall-clock start of the in-flight turn per chat key. Survives workspace
  // switches so AgentChat's elapsed timer doesn't reset to 0:00 on return.
  let turnStartedAtByKey = $state<Record<string, number>>(seededHeap?.turnStartedAt ?? {})
  // One-shot: the next idle after replacing a stamp over a footer is the
  // previous turn's idle reordering past this one — skip transcript reset only.
  let staleIdleArmedByKey = $state<Record<string, boolean>>({})
  let turnGotTextByKey = $state<Record<string, boolean>>(seededHeap?.turnGotText ?? {})
  // Newest applied stream seq per chat-tab+kind — drop a stale last-seen snapshot.
  let streamSeqByKey = $state<Record<string, number>>(seededHeap?.streamSeq ?? {})
  const sessionFeed = new SessionFeed()
  const feedWsId = $derived(ws?.id ?? '')
  $effect(() => {
    const id = feedWsId
    const code = activeCode
    untrack(() => sessionFeed.attach(id, code))
  })
  const stopHoistLive = noteHoistLiveSource(seedWsId)
  let queueOwner = true
  onDestroy(() => {
    stopHoistLive()
    sessionFeed.detach()
    queueOwner = false
    if (!get(workspaces).some((w) => w.id === boundWsId && !w.archived)) return
    putChatHeap(boundWsId, {
      chat: chatByKey,
      usage: contextUsageByKey,
      input: inputByKey,
      attachments: attachmentsByKey,
      turnStartedAt: turnStartedAtByKey,
      streamSeq: streamSeqByKey,
      turnGotText: turnGotTextByKey,
      pickers: pickersByKey,
      pending: pendingByKey,
      holdPending: holdPendingByKey,
      simplify: simplifyByKey,
    })
    pendingByKey = {}
    holdPendingByKey = {}
  })
  const planAttCount = $derived(ws?.plan?.attachments?.length ?? 0)
  const planNeedsApprove = $derived(!!ws?.plan && !ws.plan.approved)
  const planTodosBadge = $derived.by(() => {
    const steps = ws?.plan?.steps ?? []
    const stepsOpen = steps.filter((s) => s.status !== 'done' && s.status !== 'skipped').length
    let agentOpen = 0
    for (const s of sessionFeed.sessions) {
      agentOpen += latestTodosFromMessages(s.messages).filter((t) => t.status !== 'completed').length
    }
    return stepsOpen + agentOpen
  })
  const maMessages = $derived<AgentMsg[]>(chatByKey[activeKey] ?? [])
  const specialistLive = $derived(
    specialistLiveCodes(boundWsId, $agentsRunning, sessionFeed.liveCodes),
  )
  $effect(() => {
    const codes = specialistLive
    untrack(() => sessionFeed.markRunning(codes))
  })
  const showLoopCockpit = $derived(isLoopCockpitTab(activeCode, loopPickersPinned))
  // Subagent (chat child) cards are engine facts stamped with the creating
  // tab; a plain Agent tab shows those and nothing else from the cockpit.
  const childCards = $derived(sessionFeed.facts.filter((f) => f.card === 'fanout'))
  const chat = $derived<AgentMsg[]>(
    showLoopCockpit
      ? mergeLoopCockpit(maMessages, sessionFeed.sessions, sessionFeed.facts, specialistLive, ws?.goalDisplay || ws?.goal || '')
      : childCards.length
        ? mergeLoopCockpit(maMessages, [], childCards, new Set())
        : maMessages,
  )
  const contextUsage = $derived<ContextUsage | null>(contextUsageByKey[activeKey] ?? null)
  const agentBusy = $derived(
    !!boundWsId &&
      !!activeCode &&
      $agentsRunning.has(`${boundWsId}-${activeCode}`),
  )
  const sending = $derived(!!sendingByKey[activeKey] || agentBusy)
  const promptBusyHere = $derived(promptRunning && !!promptTargetKey && activeKey === promptTargetKey)
  const loopWalkLive = $derived(
    showLoopCockpit && !!ws && (specialistLive.size > 0 || wsRunActive(ws)),
  )
  const composerBusy = $derived(sending || promptBusyHere)
  // Subagents this tab spawned that have not reported yet: the parent is idle
  // but work is happening, so the tail shows dots + "Waiting for N subagents"
  // (same row the worktree-setup status uses).
  const waitingSubagents = $derived.by(() => {
    if (composerBusy || !boundWsId) return ''
    const n = $workspaces.filter((w) =>
      w.parentId === boundWsId && !w.archived && (w.fanOut?.chat || 'PA') === activeCode &&
      (!w.fanOut?.outcome || w.fanOut.outcome === 'report-pending'),
    ).length
    return n ? `Waiting for ${n} subagent${n === 1 ? '' : 's'}…` : ''
  })
  const turnStartedAt = $derived(turnStartedAtByKey[activeKey] ?? 0)
  const canFlushQueue = $derived(!composerBusy && !setupHold && !createFailed && (pendingByKey[activeKey]?.length ?? 0) > 0)


  // First time we see a tab busy without a local start (reload, or Send RPC
  // already returned while the engine is still working), stamp now once so
  // later workspace/panel switches don't keep resetting the elapsed clock.
  $effect(() => {
    const key = activeKey
    if (!sending || !key) return
    if (turnStartedAtByKey[key]) return
    turnStartedAtByKey = { ...turnStartedAtByKey, [key]: Date.now() }
  })

  function promptBusyFor(key: string): boolean {
    return promptRunning && !!promptTargetKey && key === promptTargetKey
  }

  function chatKeyBusy(key: string): boolean {
    if (sendingByKey[key]) return true
    const tab = key.indexOf('\t')
    if (tab < 0) return false
    return $agentsRunning.has(`${key.slice(0, tab)}-${key.slice(tab + 1)}`)
  }

  function pendingBubbles(key: string): AgentMsg[] {
    return (pendingByKey[key] ?? []).map((p) => ({
      me: true,
      body: pendingSendBubbleBody(p.displayText, p.marked),
      queued: true,
      pendingId: p.id,
      attachments: p.drafts.map((d) => ({
        id: d.id, name: d.name, mime: d.mime, dataUrl: d.dataUrl,
      })),
    }))
  }
  let chatInput = $state('')
  let chatAttachments = $state<ChatAttachmentDraft[]>([])
  let inputBoundKey = $state('')
  const steerByDefault = $derived(!$settings?.queueFollowUps)
  const canSteer = $derived(
    chatSteerFollowUp(
      chatKeyBusy(activeKey),
      chatMode,
      chatAttachments.length > 0,
      activeCode,
    ),
  )
  const steerFollowUp = $derived(canSteer && steerByDefault)
  absorbHoistDrafts()
  // Swap the composer draft when the active chat tab changes.
  $effect(() => {
    const key = activeKey
    if (key === inputBoundKey) return
    if (inputBoundKey) {
      inputByKey = { ...inputByKey, [inputBoundKey]: chatInput }
      attachmentsByKey = { ...attachmentsByKey, [inputBoundKey]: chatAttachments }
    }
    inputBoundKey = key
    chatInput = inputByKey[key] ?? ''
    chatAttachments = attachmentsByKey[key] ?? []
  })

  // Load the full transcript when the open workspace / chat tab changes.
  // A live turn may already have stream rows in memory (session.delta can
  // populate an empty bucket before this fetch returns). Merge those with the
  // durable history instead of skipping — skipping dropped earlier turns.
  // Always refetch on switch: that is the recovery path for a stuck streaming
  // row after a lost Wails idle/end event (applyTranscript overwrites when the
  // engine is idle). Live events keep background tabs current, but they are
  // not a substitute for a store-authoritative pass on focus.
  let loadedFor = $state('')
  $effect(() => {
    if ($workspaceScanPending) return
    const id = boundWsId
    const code = activeCode
    if (!id || !code) return
    const key = chatKey(id, code)
    if (key === loadedFor) return
    loadedFor = key
    const gen = transcriptGenByKey[key] ?? 0
    WorkspacesService.WorkspaceChatItems(id, code)
      .then((items) => {
        if ((transcriptGenByKey[key] ?? 0) !== gen) return
        applyTranscript(key, items)
      })
      .catch(() => {})
  })

  function applyTranscript(key: string, items: Parameters<typeof mapChatItems>[0]) {
    const mapped = mapChatItems(items)
    const live = chatByKey[key] ?? []
    // Only merge while the engine is actually running. A stuck streaming /
    // resultPending row after idle must not block the store-authoritative overwrite
    // that clears phantom spinners (lost Wails events).
    const merged = chatKeyBusy(key)
      ? mergePersistedWithLive(mapped, live)
      : retainOptimisticUsers(mapped, live)
    const pending = pendingBubbles(key)
    chatByKey = {
      ...chatByKey,
      [key]: pending.length ? [...merged.filter((m) => !m.queued), ...pending] : merged,
    }
    const usage = contextUsageFromChatItems(items)
    if (usage) contextUsageByKey = { ...contextUsageByKey, [key]: usage }
  }

  function pushMsg(key: string, m: AgentMsg) {
    let list = chatByKey[key] ?? []
    if (m.me) {
      list = list.map((row) => (row.streaming ? { ...row, streaming: false } : row))
    }
    chatByKey = { ...chatByKey, [key]: [...list, m] }
  }
  function liveTurnClosed(key: string): boolean {
    return dropLiveAfterFooter(
      chatByKey[key] ?? [],
      turnStartedAtByKey[key] ?? 0,
      chatKeyBusy(key),
    )
  }
  function appendStream(key: string, kind: 'text' | 'thinking', chunk: string) {
    if (kind === 'text' && chunk) turnGotTextByKey = { ...turnGotTextByKey, [key]: true }
    chatByKey = { ...chatByKey, [key]: appendStreamRow(chatByKey[key] ?? [], kind, chunk) }
  }
  function applyLiveStream(
    key: string,
    kind: 'text' | 'thinking',
    text: string,
    opts: { seq?: number; replace?: boolean; done?: boolean; parentId?: string; uuid?: string },
  ) {
    if (liveTurnClosed(key)) return
    const seqKey = `${key}:${kind}${opts.parentId ? ':' + opts.parentId : ''}`
    const seq = opts.seq ?? 0
    if (seq > 0) {
      const prev = streamSeqByKey[seqKey] ?? 0
      if (seq <= prev) return
      streamSeqByKey = { ...streamSeqByKey, [seqKey]: seq }
    }
    if (kind === 'text' && text && !opts.parentId) turnGotTextByKey = { ...turnGotTextByKey, [key]: true }
    if (opts.parentId) {
      chatByKey = { ...chatByKey, [key]: applyStream(chatByKey[key] ?? [], kind, text, { done: !!opts.done, parentId: opts.parentId, uuid: opts.uuid }) }
      return
    }
    if (opts.replace || opts.done) {
      chatByKey = { ...chatByKey, [key]: applyStream(chatByKey[key] ?? [], kind, text, { done: !!opts.done, uuid: opts.uuid }) }
    } else if (text) {
      appendStream(key, kind, text)
    }
  }
  function addTool(key: string, step: WorkspaceStep) {
    if (liveTurnClosed(key)) return
    chatByKey = { ...chatByKey, [key]: addToolRow(chatByKey[key] ?? [], step) }
  }
  function finishTurn(key: string, fallback: string) {
    const started = turnStartedAtByKey[key] ?? 0
    const elapsed = started > 0 ? Math.max(0, Date.now() - started) : 0
    chatByKey = {
      ...chatByKey,
      [key]: finishTurnRows(chatByKey[key] ?? [], fallback, !!turnGotTextByKey[key], elapsed),
    }
  }

  // Open a file the agent touched in this Observatory's editor. Tool inputs carry
  // absolute worktree paths; the editor wants repo-relative.
  function openAgentFile(path: string, line?: number, endLine?: number, startCol?: number, endCol?: number) {
    if (!ws) return
    let rel = path
    const root = ws.worktreePath
    if (root && rel.startsWith(root)) rel = rel.slice(root.length).replace(/^\//, '')
    if (!rel) return
    if (isPRHost) prDiffJump.set({ file: rel, line: line ?? 0 })
    else observatoryJump.set({ path: rel, line, endLine, startCol, endCol, workspaceId: ws.id })
  }

  /** AgentChat instance — ⌘L inserts @file chips via insertFileMention. */
  let agentChat = $state<AgentChat | undefined>(undefined)

  function addEditorMention(path: string, startLine?: number, endLine?: number, startCol?: number, endCol?: number) {
    // focusAgentPanel awaits remount; give bind:this one more frame if needed.
    const go = () => agentChat?.insertFileMention(path, startLine, endLine, startCol, endCol)
    if (agentChat) go()
    else void tick().then(go)
  }

  /** Paint the New-workspace opening prompt immediately — don't wait for the
   *  worktree or Send RPC. send() upgrades this row to queued+pendingId while
   *  setupHold, or skips a second bubble on idle dispatch. */
  function paintOpeningUser(key: string, seed: { text: string; display?: string; attachments?: ChatAttachmentDraft[] }) {
    const drafts = seed.attachments ?? []
    const marked = composeMarkedText(seed.text || '', drafts).trim()
    const body = (seed.display || '').trim() || displayUserChatText(marked)
    if (!body && !drafts.length) return
    const live = chatByKey[key] ?? []
    if (live.some((m) => m.me && m.optimistic && m.body === body)) return
    pushMsg(key, {
      me: true,
      body,
      optimistic: true,
      attachments: drafts.map((d) => ({
        id: d.id, name: d.name, mime: d.mime, dataUrl: d.dataUrl,
      })),
    })
  }

  // An opening instruction from New-work ("review PR 123", a change request) is
  // seeded here — the chosen mode + model are applied to the first tab, then
  // any prompt is sent (or queued until setupHold clears) as the first chat
  // message. Do not consume the seed until pickersReady: send() no-ops while
  // the dock is restoring, and clearing the seed early would drop the opening.
  $effect(() => {
    const seed = ws ? $workspaceSeeds[ws.id] : undefined
    // Wait out prompt-action prepare too — send() queues while promptRunning, but
    // clearing the seed first would still drop an opening instruction if send
    // were gated; keep the seed until the composer is fully idle.
    if (!seed || !ws) return
    if (!ws.isMain && createFailed) {
      takeWorkspaceSeed(ws.id)
      return
    }
    // Show the opening bubble while git worktree + setup still run — chat stays
    // mounted, so the user can see their message instead of an empty dock.
    if ((seed.text || seed.attachments?.length) && activeCode) {
      paintOpeningUser(chatKey(ws.id, activeCode), seed)
    }
    if (composerBusy) return
    if (typeof seed.simplify === 'boolean' && activeCode && activeCode !== 'MA') {
      const key = chatKey(ws.id, activeCode)
      if (simplifyByKey[key] !== seed.simplify) {
        bumpSimplifyGen(key)
        simplifyByKey = { ...simplifyByKey, [key]: seed.simplify }
      }
      simplifyFetched.add(key)
    }
    if ((seed.mode || seed.model) && activeCode !== 'MA') {
      // Same Settings/catalogue gate as loadChatPickers — a mode-only seed
      // (e.g. Plan panel "Execute in chat") must not cache sonnet/{} and block hydrate.
      if (!catalogReady || !$settings) return
      const key = chatKey(ws.id, activeCode)
      // Mode-only stubs ({ model: '' }) must not block the seed — same ?.model
      // convention as pickersReady / loadChatPickers.
      if (!pickersByKey[key]?.model) {
        const mode = normalizeChatMode(seed.mode)
        const d = resolvedDefaultForMode(mode, $settings, catalogModels)
        const ref = seed.model || d.model
        bumpPickerGen(key)
        pickersByKey = {
          ...pickersByKey,
          [key]: {
            model: ref,
            flags: normalizeFlags(
              flagsForModel(catalogModels, ref),
              seed.flags ?? d.flags,
            ),
            mode,
          },
        }
      }
    }
    if (seed.text || seed.attachments?.length) {
      if (activeCode !== 'MA' && !pickersReady) return
      if (!takeWorkspaceSeed(ws.id)) return
      const openingDisplay = (seed.display ?? '').trim()
      chatInput = seed.text
      chatAttachments = seed.attachments ?? []
      send(openingDisplay ? { displayText: openingDisplay } : undefined)
      return
    }
    takeWorkspaceSeed(ws.id)
  })

  // Findings for Fix-findings CTAs and update_finding chat cards (lookup by id).
  let chatFindings = $state<FindingRef[]>([])
  $effect(() => {
    const id = ws?.id
    if (!id) {
      chatFindings = []
      return
    }
    let cancelled = false
    const refresh = () => {
      ObservatoryService.ListFindings(id)
        .then((fs) => {
          if (!cancelled) chatFindings = fs ?? []
        })
        .catch(() => {
          if (!cancelled) chatFindings = []
        })
    }
    refresh()
    const off = onFindingsUpdated((wsId) => {
      if (wsId === id) refresh()
    })
    return () => {
      cancelled = true
      off()
    }
  })

  function tabHasUnsentDraft(id: string, code: string): boolean {
    const key = chatKey(id, code)
    const live = boundWsId === id && activeCode === code
    const text = live ? chatInput : (inputByKey[key] ?? '')
    if (text.trim()) return true
    const atts = live ? chatAttachments : (attachmentsByKey[key] ?? [])
    return atts.length > 0
  }

  function pickIdleTab(mode: ChatMode, group = '') {
    if (!ws) return { action: 'create' as const }
    const id = ws.id
    return pickPromptTab({
      tabs: tabsByWs[id] ?? [],
      activeCode,
      mode,
      group: group || undefined,
      isBusy: (code) => chatKeyBusy(chatKey(id, code)),
      tabMode: (tab) => tabModeFor(tab),
      started: (tab) => chatHasStarted(tab.title, chatByKey[chatKey(id, tab.code)]),
      drafted: (tab) => tabHasUnsentDraft(id, tab.code),
      conductorLocked: (code) => conductorTabLocked(code),
    })
  }

  async function promptModelsReady(id: string): Promise<boolean> {
    if (!$settings) {
      pushToast('error', 'Models are still loading — try again in a moment')
      return false
    }
    if (!catalogModels.length) {
      try {
        const models = await loadModelCatalog()
        if (models?.length) catalogModels = models
      } catch { /* toast below */ }
      if (!catalogModels.length || boundWsId !== id) {
        pushToast('error', 'Models are still loading — try again in a moment')
        return false
      }
    }
    return true
  }

  /** Prefer current idle tab in mode; else newest idle in mode; else new tab.
   *  group: nearest tab already in that slot, else unused in mode, else create. */
  async function ensureIdleTabForMode(mode: ChatMode, group = ''): Promise<boolean> {
    if (!ws) return false
    if (mode === 'loop' && isMain) {
      pushToast('error', 'The main working copy cannot run a loop')
      return false
    }
    if (mode === 'loop' && isPRHost) {
      pushToast('error', 'PR chat cannot run a loop')
      return false
    }
    const id = ws.id
    chatOpen = true
    // Preflight catalog/settings before creating a tab so a hydrate failure
    // does not leave a stray empty chat.
    if (!await promptModelsReady(id)) return false
    const pick = pickIdleTab(mode, group)
    let createdCode = ''
    if (pick.action === 'select') {
      selectChatTab(pick.code)
      await tick()
    } else if (pick.action === 'create') {
      const opened = await newChatTab()
      if (!opened) {
        pushToast('error', `Could not open a chat tab for ${mode} mode`)
        return false
      }
      createdCode = opened
      await tick()
    }
    const code = createdCode || activeCode
    if (code === 'MA') {
      pushToast('error', `Open a chat tab for ${mode} mode`)
      return false
    }
    // Hydrate before flipping mode. Otherwise onChatModeChange sees the coder
    // fallback + empty flags and keeps that model instead of Review/Plan/Ask
    // Settings defaults (slash /review creating a new tab). Bail when hydrate
    // fails (Settings/catalog not ready) so we don't persist the hardcoded
    // sonnet fallback over a stored selection.
    const hydrated = await loadChatPickers(id, code)
    // Only close a tab this call actually opened (createdCode), on the workspace
    // we created it in — never the user's pre-existing active tab.
    const abortCreated = () => {
      if (createdCode) void closeChatTab(createdCode, id)
    }
    // onChatModeChange targets the *current* active tab — abort if the user
    // switched away during hydrate so we don't clobber another tab. newChatTab
    // already set activeCode to the created tab before returning.
    if (boundWsId !== id || activeCode !== code) {
      abortCreated()
      pushToast('error', 'Chat tab changed — try the command again')
      return false
    }
    if (!hydrated) {
      abortCreated()
      pushToast('error', 'Models are still loading — try again in a moment')
      return false
    }
    const dock = onChatModeChange(mode)
    if (group) stampPromptGroup(id, code, group, dock)
    return boundWsId === id && activeCode === code
  }

  function stampPromptGroup(id: string, code: string, group: string, after?: Promise<void>) {
    const tabs = tabsByWs[id] ?? []
    if (tabs.some((t) => t.code === code)) {
      tabsByWs = {
        ...tabsByWs,
        [id]: tabs.map((t) => t.code === code ? { ...t, promptGroup: group } as WorkspaceChatTab : t),
      }
    }
    void (after ?? Promise.resolve())
      .then(() => WorkspacesService.RememberWorkspaceChatPromptGroup(id, code, group))
      .catch(() => {})
  }

  type HeldPromptSend = {
    holdId: string
    workspaceId: string
    actionId: string
    slash: string
    prompt: string
    display: string
    mode: ChatMode | ''
    tabGroup: string
    loopID: string
    freshSession: boolean
  }
  let heldPromptSends = $state<HeldPromptSend[]>([])

  function discardHeldPrompt(holdId: string) {
    heldPromptSends = heldPromptSends.filter((h) => h.holdId !== holdId)
  }

  function holdPreparedPrompt(held: HeldPromptSend) {
    if (!heldPromptSends.some((h) => h.holdId === held.holdId)) {
      heldPromptSends = [...heldPromptSends, held]
    }
    pushToast('error', `${held.display} is ready — open a chat tab and retry`, {
      action: { label: 'Retry', fn: () => { void flushHeldPromptSend(held.holdId) } },
      onDismiss: () => discardHeldPrompt(held.holdId),
      replace: `held-prompt-send:${held.holdId}`,
      ttlMs: 0,
    })
  }

  function dispatchPreparedPrompt(
    routed: { id: string; code: string; key: string; mode: ChatMode; model: string; flags: Record<string, string> },
    prompt: string,
    display: string,
    freshSession: boolean,
    alt = false,
  ) {
    const { id, code, key, mode, model, flags } = routed
    promptTargetKey = key
    if (boundWsId === id && activeCode === code) chatInput = ''
    inputByKey = { ...inputByKey, [key]: '' }
    const pending: PendingChatSend = {
      id: uuidv7(),
      marked: prompt,
      plain: display,
      drafts: [],
      mode,
      model,
      flags,
      displayText: display,
      promptAction: true,
      freshSession,
    }
    const pinIfVisible = () => {
      if (chatKey(boundWsId, activeCode) === key) agentChat?.scrollToBottom()
    }
    pinIfVisible()
    const queued = pendingByKey[key] ?? []
    if (chatKeyBusy(key) || setupHold) {
      if (!setupHold && !freshSession && steerByDefault !== alt && chatSteerFollowUp(true, pickersByKey[key]?.mode || mode, false, code)) {
        pushMsg(key, { me: true, body: display, optimistic: true, pendingId: pending.id })
        dispatchSteer(id, code, key, pending)
        return
      }
      pushMsg(key, { me: true, body: display, queued: true, pendingId: pending.id })
      pendingByKey = { ...pendingByKey, [key]: [...queued, pending] }
      return
    }
    pushMsg(key, { me: true, body: display, optimistic: true })
    dispatchSend(id, code, key, pending)
  }

  async function flushHeldPromptSend(holdId: string) {
    const held = heldPromptSends.find((h) => h.holdId === holdId)
    if (!held) return
    const restore = () => holdPreparedPrompt(held)
    if (!ws || held.workspaceId !== ws.id || promptRunning) {
      restore()
      return
    }
    if (!promptEnabled(held.actionId)) {
      pushToast('error', `${held.display} is disabled in Settings → Bundles`)
      restore()
      return
    }
    if (wsRunActive(ws) && (held.actionId === 'recur.chat.open-pr' || held.actionId === 'recur.chat.open-draft-pr')) {
      pushToast('error', 'A loop is running — wait until it pauses, then open a PR from the titlebar or /open-pr.')
      restore()
      return
    }
    promptRunning = true
    promptTargetKey = chatKey(ws.id, activeCode)
    promptAbort?.abort()
    promptAbort = new AbortController()
    const signal = promptAbort.signal
    try {
      if (held.mode) {
        if (!await ensureIdleTabForMode(held.mode, held.tabGroup)) {
          restore()
          return
        }
      } else if (!await ensureCurrentCodingTab()) {
        restore()
        return
      }
      if (signal.aborted) {
        restore()
        return
      }
      const id = ws.id
      const code = activeCode
      if (!code || code === 'MA' || !pickersReady) {
        restore()
        return
      }
      promptTargetKey = chatKey(id, code)
      if (held.mode === 'loop' && held.loopID) {
        if (loopPickersPinned) {
          pushToast('error', 'This tab is already running a loop')
          restore()
          return
        }
        const pinned = await persistLoopPolicy(held.loopID, dockPolicyId)
        if (!pinned || signal.aborted) {
          restore()
          return
        }
      }
      const key = chatKey(id, code)
      const p = pickersByKey[key]
      discardHeldPrompt(held.holdId)
      dispatchPreparedPrompt(
        {
          id,
          code,
          key,
          mode: held.mode || chatMode,
          model: p?.model || model,
          flags: { ...(p?.flags ?? flags) },
        },
        held.prompt,
        held.display,
        held.freshSession,
      )
    } catch (err) {
      restore()
      const msg = promptActionErrorMessage(err)
      if (msg !== 'prompt cancelled') pushToast('error', msg)
    } finally {
      const flushKey = promptTargetKey
      promptRunning = false
      promptTargetKey = ''
      promptAbort = null
      if (flushKey) tryFlushPending(flushKey)
    }
  }

  async function ensureCurrentCodingTab(): Promise<boolean> {
    if (!ws) return false
    chatOpen = true
    if (activeCode === 'MA') {
      pushToast('error', 'Open a coding chat tab first')
      return false
    }
    return true
  }

  async function invokePromptAction(slash: string, args = '', alt = false) {
    if (!ws || promptRunning) return
    const slashName = slash.replace(/^\//, '').trim()
    // Mark busy before any await — pickPrompt clears the /query synchronously,
    // and the idle "Message the agent…" placeholder must not flash while we
    // route/hydrate/run the script before dispatchSend sets `sending`.
    // Lock the invoke tab immediately; retarget after mode routing if needed.
    promptRunning = true
    promptTargetKey = chatKey(ws.id, activeCode)
    promptAbort?.abort()
    promptAbort = new AbortController()
    const signal = promptAbort.signal
    try {
      const list = await listPromptActionsUnfiltered()
      if (signal.aborted) return
      const info = findPromptBySlash(slash, list)
      if (!info) {
        pushToast('error', `Unknown prompt /${slash.replace(/^\//, '')}`)
        return
      }
      if (wsRunActive(ws) && (info.id === 'recur.chat.open-pr' || info.id === 'recur.chat.open-draft-pr')) {
        pushToast('error', 'A loop is running — wait until it pauses, then open a PR from the titlebar or /open-pr.')
        return
      }
      if (!promptEnabled(info.id)) {
        pushToast('error', `${info.name} is disabled in Settings → Bundles`)
        return
      }
      // Review prompts stamp/read the target tab; delay other fresh_session
      // routing until we know the script will send (empty /open-pr must not spawn a tab).
      const routeFirst = !info.freshSession || normalizeChatMode(info.mode) === 'review'

      const sendMode = (): ChatMode =>
        info.mode ? normalizeChatMode(info.mode) : chatMode

      const snapshotRouted = () => {
        const id = ws.id
        const code = activeCode
        if (!code || code === 'MA') return null
        const key = chatKey(id, code)
        const p = pickersByKey[key]
        return {
          id,
          code,
          key,
          mode: sendMode(),
          model: p?.model || model,
          flags: { ...(p?.flags ?? flags) },
        }
      }

      const route = async (): Promise<boolean> => {
        if (info.mode) {
          const ok = await ensureIdleTabForMode(normalizeChatMode(info.mode), info.tabGroup?.trim() ?? '')
          if (!ok) return false
        } else {
          const ok = await ensureCurrentCodingTab()
          if (!ok) return false
        }
        if (signal.aborted) return false
        promptTargetKey = chatKey(ws.id, activeCode)
        if (activeCode !== 'MA' && !pickersReady) return false
        const loopID = info.loop?.trim()
        if (normalizeChatMode(info.mode) === 'loop' && loopID) {
          if (loopPickersPinned) {
            pushToast('error', 'This tab is already running a loop')
            return false
          }
          const pinned = await persistLoopPolicy(loopID, dockPolicyId)
          if (!pinned || signal.aborted) return false
        }
        return true
      }

      let routed: ReturnType<typeof snapshotRouted> = null
      if (routeFirst) {
        if (!await route()) return
        routed = snapshotRouted()
        if (!routed) return
      } else {
        // Non-mutating route checks before a side-effecting shebang. Do not
        // create a tab yet — empty /open-pr must not spawn one. If deferred
        // create later fails, holdPreparedPrompt keeps a retryable send.
        const mode = info.mode ? normalizeChatMode(info.mode) : ''
        if (mode === 'loop' && isMain) {
          pushToast('error', 'The main working copy cannot run a loop')
          return
        }
        if (mode === 'loop' && isPRHost) {
          pushToast('error', 'PR chat cannot run a loop')
          return
        }
        if (!await promptModelsReady(ws.id)) return
        if (!info.mode) {
          if (!await ensureCurrentCodingTab()) return
        } else {
          const pick = pickIdleTab(mode || 'agent', info.tabGroup?.trim() ?? '')
          const code = pick.action === 'select' ? pick.code
            : pick.action === 'current' ? activeCode
            : ''
          if (code && code !== 'MA') {
            const hydrated = await loadChatPickers(ws.id, code)
            if (!hydrated) {
              pushToast('error', 'Models are still loading — try again in a moment')
              return
            }
          }
        }
      }

      let editor = observatory?.getFocusedEditorContext() ?? null
      if (editor) {
        try {
          editor = { ...editor, clipboard: await readText() }
        } catch { /* keep editor without clipboard */ }
      }
      if (signal.aborted) return
      const ctx = buildPromptContext(
        { id: ws.id, projectId: ws.projectId, worktreePath: ws.worktreePath, port: ws.port },
        editor,
        chatSurface,
        args,
        paramsFromArgs(info, args),
      )
      ctx.chatCode = routed?.code ?? activeCode

      const result = await runPromptAction(info, ctx, signal)
      if (signal.aborted) return
      if (result.issue) void linkWorkspaceIssue(ws.id, result.issue)
      const display = result.display || info.name
      if (!result.prompt.trim()) {
        pushToast('success', display)
        return
      }

      const prepared: HeldPromptSend = {
        holdId: uuidv7(),
        workspaceId: ws.id,
        actionId: info.id,
        slash: (info.slash || slashName).replace(/^\//, '').trim(),
        prompt: result.prompt,
        display,
        mode: info.mode ? normalizeChatMode(info.mode) : '',
        tabGroup: info.tabGroup?.trim() ?? '',
        loopID: info.loop?.trim() ?? '',
        freshSession: result.freshSession ?? !!info.freshSession,
      }
      if (!routeFirst) {
        if (!await route()) {
          holdPreparedPrompt(prepared)
          return
        }
        routed = snapshotRouted()
      }
      if (!routed) {
        holdPreparedPrompt(prepared)
        return
      }

      dispatchPreparedPrompt(routed, prepared.prompt, prepared.display, prepared.freshSession, alt)
    } catch (err) {
      const msg = promptActionErrorMessage(err)
      if (msg !== 'prompt cancelled') pushToast('error', msg)
    } finally {
      const flushKey = promptTargetKey
      promptRunning = false
      promptTargetKey = ''
      promptAbort = null
      // Follow-ups queued only because promptBusyFor(key) have no turn-end to
      // flush them when the prompt aborts/fails without dispatchSend. No-op when
      // a turn is already running (chatKeyBusy) — that path flushes on idle.
      if (flushKey) tryFlushPending(flushKey)
    }
  }

  function fixFindings() {
    if (sending || promptRunning || !ws) return
    void invokePromptAction('fix-findings')
  }

  $effect(() => {
    const pending = $promptActionRequest
    if (!ws || !pending || pending.workspaceId !== ws.id) return
    if (promptRunning) return
    const taken = takePromptActionRequest(ws.id)
    if (taken) void invokePromptAction(taken.slash, taken.args)
  })

  function clearHoldPending(key: string) {
    if (!holdPendingByKey[key]) return
    const { [key]: _, ...rest } = holdPendingByKey
    holdPendingByKey = rest
  }

  async function answerLoopEscalation(choice: string, text: string): Promise<boolean> {
    if (!ws || !liveEsc) return false
    const id = ws.id
    const key = chatKey(id, conductorChatCode() || 'MA')
    const body = formatAskUserDisplayText(choice, text)
    const held = liveEsc
    escLoadGen++
    liveEsc = null
    const list = chatByKey[key] ?? []
    const last = list[list.length - 1]
    if (!(last?.me && last.body === body)) {
      pushMsg(key, { me: true, body, optimistic: true, ts: Date.now() })
    }
    const ok = await answerEscalation(id, choice, text, held.agentCode ?? '')
    if (!ok) {
      liveEsc = held
      chatByKey = {
        ...chatByKey,
        [key]: (chatByKey[key] ?? []).filter((m) => !(m.optimistic && m.me && m.body === body)),
      }
      return false
    }
    return true
  }

  /** alt (⌘⏎) swaps steer/queue for a follow-up sent while the tab is busy. */
  function send(opts?: { displayText?: string; alt?: boolean }) {
    if (!ws) return
    // A failed setup keeps the tree but must not dispatch into a half-provisioned
    // checkout. While git/setup are still running, queue the send (same as a
    // busy tab) so the bubble is visible and flushes when setupHold clears.
    if (createFailed) return
    // Wait for dock restore on coding tabs — sending while pickers aren't ready
    // used chatMode's agent fallback and could overwrite a persisted Review mode.
    if (activeCode !== 'MA' && !pickersReady) return
    const drafts = chatAttachments
    const marked = composeMarkedText(chatInput, drafts).trim()
    // plain keeps inline @path mentions (agent-visible) but drops image markers.
    const plain = stripAttachmentMarkers(marked).trim()
    const id = ws.id
    const code = activeCode
    const key = chatKey(id, code)
    // Treat prompt-action prepare as busy on the target tab only so "⏎ to queue"
    // queues there instead of racing dispatchSend; other tabs stay free to send.
    const busy = chatKeyBusy(key) || promptBusyFor(key)
    const held = pendingByKey[key] ?? []

    // After Stop, Enter with an empty composer releases the held follow-ups.
    if (!plain && !drafts.length) {
      if (busy || setupHold || !held.length) return
      clearHoldPending(key)
      flushPendingNow(key)
      return
    }

    // Pending ask_user for this tab: composer send is the answer (text and/or
    // images). Do not also SendWorkspaceMessage — AnswerAskUser resumes with it.
    const pendingAsk = $userQuestions.find(
      (q) => q.workspaceId === id && q.agentCode === code,
    )
    if (pendingAsk && (plain || drafts.length)) {
      const wireAtts = toSendAttachments(drafts)
      const savedInput = chatInput
      const savedAtts = drafts.slice()
      chatInput = ''
      chatAttachments = []
      inputByKey = { ...inputByKey, [key]: '' }
      attachmentsByKey = { ...attachmentsByKey, [key]: [] }
      void answerQuestion(pendingAsk.id, '', plain, wireAtts).then((ok) => {
        if (ok) return
        // Question gone server-side — put the draft back on the answering tab.
        // Only touch the live composer when that tab is still bound (tab switches
        // rebind chatInput); always write the per-key slots.
        const liveHere = inputBoundKey === key
        const curInput = liveHere ? chatInput : (inputByKey[key] ?? '')
        const curAtts = liveHere ? chatAttachments : (attachmentsByKey[key] ?? [])
        const curPlain = stripAttachmentMarkers(curInput).trim()
        const restored = [savedInput, curPlain].filter(Boolean).join('\n\n')
        const seen = new Set(curAtts.map((a) => a.id))
        const restoredAtts = [
          ...savedAtts.filter((a) => !seen.has(a.id)),
          ...curAtts,
        ]
        inputByKey = { ...inputByKey, [key]: restored }
        attachmentsByKey = { ...attachmentsByKey, [key]: restoredAtts }
        if (liveHere) {
          chatInput = restored
          chatAttachments = restoredAtts
        }
      })
      return
    }

    // Open loop escalation on the conductor tab: composer send is the answer,
    // not a chat message (AnswerEscalation resumes the paused loop).
    if (onConductorTab && liveEsc?.status === 'open' && plain) {
      const savedInput = chatInput
      const savedAtts = drafts.slice()
      chatInput = ''
      chatAttachments = []
      inputByKey = { ...inputByKey, [key]: '' }
      attachmentsByKey = { ...attachmentsByKey, [key]: [] }
      void answerLoopEscalation('', plain).then((ok) => {
        if (ok) return
        const liveHere = inputBoundKey === key
        const curInput = liveHere ? chatInput : (inputByKey[key] ?? '')
        const curAtts = liveHere ? chatAttachments : (attachmentsByKey[key] ?? [])
        const curPlain = stripAttachmentMarkers(curInput).trim()
        const restored = [savedInput, curPlain].filter(Boolean).join('\n\n')
        const seen = new Set(curAtts.map((a) => a.id))
        const restoredAtts = [
          ...savedAtts.filter((a) => !seen.has(a.id)),
          ...curAtts,
        ]
        inputByKey = { ...inputByKey, [key]: restored }
        attachmentsByKey = { ...attachmentsByKey, [key]: restoredAtts }
        if (liveHere) {
          chatInput = restored
          chatAttachments = restoredAtts
        }
      })
      return
    }

    const openingLabel = (opts?.displayText ?? '').trim()
    const pending: PendingChatSend = {
      id: uuidv7(),
      marked,
      plain,
      drafts: drafts.slice(),
      mode: chatMode,
      model,
      flags: { ...flags },
      displayText: openingLabel || undefined,
    }
    freezeSimplify(key)
    chatInput = ''
    chatAttachments = []
    inputByKey = { ...inputByKey, [key]: '' }
    attachmentsByKey = { ...attachmentsByKey, [key]: [] }
    if (rejectedSteersByKey[key]) {
      const { [key]: _rej, ...restRej } = rejectedSteersByKey
      rejectedSteersByKey = restRej
    }
    const bubble = pendingSendBubbleBody(openingLabel, marked)
    if (busy || setupHold) {
      if (!setupHold && chatKeyBusy(key) && steerByDefault !== !!opts?.alt && chatSteerFollowUp(true, pending.mode, pending.drafts.length > 0, code)) {
        const display = pendingSendBubbleBody(pending.displayText, marked)
        pushMsg(key, {
          me: true,
          body: display,
          optimistic: true,
          pendingId: pending.id,
          attachments: drafts.map((d) => ({
            id: d.id, name: d.name, mime: d.mime, dataUrl: d.dataUrl,
          })),
        })
        dispatchSteer(id, code, key, pending)
        return
      }
      if (setupHold) {
        const nameFrom = openingLabel || marked
        void (code === 'MA' ? Promise.resolve() : persistChatDock(id, code, undefined, pending))
          .then(() => WorkspacesService.ApplyProvisionalChatTitles(id, code, nameFrom))
          .catch(() => {})
      }
      const row = {
        me: true,
        body: bubble,
        queued: true,
        pendingId: pending.id,
        attachments: drafts.map((d) => ({
          id: d.id, name: d.name, mime: d.mime, dataUrl: d.dataUrl,
        })),
      }
      // Seed may have already painted this body without a pendingId — upgrade
      // it so hydrate can reattach via pendingBubbles instead of duplicating.
      const live = chatByKey[key] ?? []
      const idx = live.findIndex((m) => m.me && m.optimistic && !m.pendingId && m.body === bubble)
      if (idx >= 0) {
        const next = live.slice()
        next[idx] = { ...live[idx], ...row }
        chatByKey = { ...chatByKey, [key]: next }
      } else {
        pushMsg(key, row)
      }
      pendingByKey = { ...pendingByKey, [key]: [...held, pending] }
      return
    }

    // Idle send: enqueue behind any held follow-ups, then flush one batch
    // (prompt-action turns never merge with normal sends).
    clearHoldPending(key)
    if (held.length) {
      pendingByKey = { ...pendingByKey, [key]: [...held, pending] }
      flushPendingNow(key)
      return
    }

    const live = chatByKey[key] ?? []
    if (!live.some((m) => m.me && m.optimistic && m.body === bubble)) {
      pushMsg(key, {
        me: true,
        body: bubble,
        optimistic: true,
        attachments: drafts.map((d) => ({
          id: d.id, name: d.name, mime: d.mime, dataUrl: d.dataUrl,
        })),
      })
    }
    dispatchSend(id, code, key, pending)
  }

  function retryChat() {
    if (!ws) return
    if (setupHold) return
    if (activeCode !== 'MA' && !pickersReady) return
    const id = ws.id
    const code = activeCode
    const key = chatKey(id, code)
    if (chatKeyBusy(key) || promptBusyFor(key)) return
    const msgs = chatByKey[key] ?? []
    if (!msgs.some((m) => m.me && !m.queued)) return
    // Engine replays the last stored wire prompt (and images). The bubble body
    // is display_text — a prompt-action label or ask_user answer — so do not
    // send it as the retry payload. Loop on a PA* tab means "engage", which
    // Retry must not do — send agent so the loop-tab lock still applies.
    dispatchSend(id, code, key, {
      id: uuidv7(),
      marked: CHAT_RETRY_DISPLAY,
      plain: '',
      drafts: [],
      mode: chatMode === 'loop' ? 'agent' : chatMode,
      model,
      flags: { ...flags },
      displayText: CHAT_RETRY_DISPLAY,
    })
  }

  function removeQueued(pendingId: string) {
    if (!pendingId || !ws) return
    const key = chatKey(ws.id, activeCode)
    const q = pendingByKey[key] ?? []
    if (!q.some((p) => p.id === pendingId)) return
    pendingByKey = { ...pendingByKey, [key]: q.filter((p) => p.id !== pendingId) }
    chatByKey = {
      ...chatByKey,
      [key]: (chatByKey[key] ?? []).filter((m) => m.pendingId !== pendingId),
    }
  }

  // Fold every queued follow-up into a single next user turn once this tab is idle.
  // Skipped after Stop until the user explicitly sends again.
  function tryFlushPending(key: string) {
    if (!queueOwner || holdPendingByKey[key]) return
    flushPendingNow(key)
  }

  function flushPendingNow(key: string) {
    if (!queueOwner || createFailed || setupHold || chatKeyBusy(key) || promptBusyFor(key)) return
    const q = pendingByKey[key]
    if (!q?.length) return
    const { batch, rest } = takeFlushBatch(q)
    if (!batch.length) return
    pendingByKey = { ...pendingByKey, [key]: rest }
    const pendingIds = new Set(batch.map((p) => p.id))
    const merged = mergePendingSends(batch)
    // Replace the individual queued bubbles with one combined user row — that is
    // what the engine will persist for this turn.
    const body = pendingSendBubbleBody(merged.displayText, merged.marked)
    chatByKey = {
      ...chatByKey,
      [key]: [
        ...(chatByKey[key] ?? []).filter((m) => !m.pendingId || !pendingIds.has(m.pendingId)),
        {
          me: true,
          body,
          optimistic: true,
          attachments: merged.drafts.map((d) => ({
            id: d.id, name: d.name, mime: d.mime, dataUrl: d.dataUrl,
          })),
        },
      ],
    }
    const tab = key.indexOf('\t')
    if (tab < 0) return
    dispatchSend(key.slice(0, tab), key.slice(tab + 1), key, merged)
  }

  $effect(() => {
    if (setupHold || !boundWsId) return
    const prefix = `${boundWsId}\t`
    for (const k of Object.keys(pendingByKey)) {
      if (k.startsWith(prefix)) tryFlushPending(k)
    }
  })

  /** Inject a follow-up into a live turn. Must not bump sendGen or clear
   *  sendingByKey — the original dispatchSend still owns that turn. */
  function dispatchSteer(id: string, code: string, key: string, pending: PendingChatSend) {
    const job = { ...pending, seq: pending.seq ?? ++steerClock }
    const wireAtts = toSendAttachments(job.drafts)
    const retrySend = job.displayText === CHAT_RETRY_DISPLAY || job.marked === CHAT_RETRY_DISPLAY
    const wire = retrySend ? job.marked : attachChatSurface(job.marked, chatSurface)
    const display = pendingSendBubbleBody(job.displayText, job.marked)
    WorkspacesService.SendWorkspaceMessage(
      id, code, wire, job.model, job.flags, wireAtts, job.mode,
      job.displayText || '',
      !!job.freshSession,
    ).then((reply) => {
      if (isChatInjectedReply(reply)) return
      const text = (reply || '').trim() || 'Done.'
      if (isChatStoppedReply(text)) {
        holdPendingByKey = { ...holdPendingByKey, [key]: true }
        dropRejectedSteer(key, job, display)
        return
      }
      finishTurn(key, text)
    }).catch(() => {
      dropRejectedSteer(key, job, display)
      pushToast('error', 'Could not steer the agent — try again')
    })
  }

  function dropRejectedSteer(key: string, pending: PendingChatSend, display: string) {
    chatByKey = { ...chatByKey, [key]: dropOptimisticUser(chatByKey[key] ?? [], display, pending.id) }
    const prev = rejectedSteersByKey[key] ?? []
    const next = [...prev.filter((p) => p.id !== pending.id), pending]
      .sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0))
    rejectedSteersByKey = { ...rejectedSteersByKey, [key]: next }
    restoreSteerDraft(key)
  }

  function restoreSteerDraft(key: string) {
    const fails = rejectedSteersByKey[key] ?? []
    const liveHere = inputBoundKey === key
    const curInput = liveHere ? chatInput : (inputByKey[key] ?? '')
    const curAtts = liveHere ? chatAttachments : (attachmentsByKey[key] ?? [])
    const curPlain = stripAttachmentMarkers(curInput).trim()
    const restored = mergeRejectedSteerDraft(fails.map((p) => p.marked), curPlain)
    const restoredAtts = mergeRejectedSteerAttachments(fails.map((p) => p.drafts), curAtts)
    inputByKey = { ...inputByKey, [key]: restored }
    attachmentsByKey = { ...attachmentsByKey, [key]: restoredAtts }
    if (liveHere) {
      chatInput = restored
      chatAttachments = restoredAtts
    }
  }

  /** Prompt-action items flush alone; consecutive normal sends may still merge. */
  function takeFlushBatch(queue: PendingChatSend[]): { batch: PendingChatSend[]; rest: PendingChatSend[] } {
    if (!queue.length) return { batch: [], rest: [] }
    if (queue[0].promptAction) {
      return { batch: [queue[0]], rest: queue.slice(1) }
    }
    let i = 1
    while (i < queue.length && !queue[i].promptAction) i++
    return { batch: queue.slice(0, i), rest: queue.slice(i) }
  }

  // Latest mode/model/flags win; text + attachments concatenate in queue order.
  // Only synthesize displayText when an entry actually has one (prompt-action
  // batches). Normals must keep displayText '' so attachment markers in `marked`
  // stay aligned with Go's display_text layout check.
  function mergePendingSends(queue: PendingChatSend[]): PendingChatSend {
    const last = queue[queue.length - 1]
    const hasDisplay = queue.some((p) => !!p.displayText)
    const displayParts = hasDisplay
      ? queue.map((p) => (p.displayText || p.plain || '').trim()).filter(Boolean)
      : []
    return {
      id: last.id,
      marked: queue.map((p) => p.marked).filter(Boolean).join('\n\n'),
      plain: queue.map((p) => p.plain).filter(Boolean).join('\n\n'),
      drafts: queue.flatMap((p) => p.drafts),
      mode: last.mode,
      model: last.model,
      flags: last.flags,
      displayText: hasDisplay ? displayParts.join('\n\n') : '',
      promptAction: queue.length === 1 ? queue[0].promptAction : undefined,
      freshSession: queue.length === 1 ? queue[0].freshSession : undefined,
    }
  }

  function dispatchSend(id: string, code: string, key: string, pending: PendingChatSend) {
    const gen = (sendGenByKey[key] ?? 0) + 1
    sendGenByKey = { ...sendGenByKey, [key]: gen }
    sendingByKey = { ...sendingByKey, [key]: true }
    if (!liveModelByKey[key]) {
      liveModelByKey = { ...liveModelByKey, [key]: pending.model }
    }
    if (key in hoistSendGenByKey) hoistSendGenByKey = omitChatKey(hoistSendGenByKey, key)
    turnStartedAtByKey = { ...turnStartedAtByKey, [key]: Date.now() }
    turnGotTextByKey = { ...turnGotTextByKey, [key]: false }
    // Optimistic provisional tab title (backend also paints + LLM-upgrades).
    {
      const tabs = tabsByWs[id] ?? []
      const tab = tabs.find((t) => t.code === code)
      if (tab && (!tab.title || tab.title === 'New chat')) {
        const label = pending.displayText === CHAT_RETRY_DISPLAY
          ? pending.plain
          : (pending.displayText || pending.plain)
        const titleFrom = displayUserChatText(label)
        const title =
          titleFrom.split('\n').map((l) => l.trim()).find(Boolean)?.slice(0, 28) ||
          (pending.drafts.length ? 'Image' : 'New chat')
        tabsByWs = {
          ...tabsByWs,
          [id]: tabs.map((t) => (t.code === code ? { ...t, title } : t)),
        }
      }
    }
    const agentId = `${id}-${code}`
    const wireAtts = toSendAttachments(pending.drafts)
    const retrySend = pending.displayText === CHAT_RETRY_DISPLAY || pending.marked === CHAT_RETRY_DISPLAY
    const wire = retrySend ? pending.marked : attachChatSurface(pending.marked, chatSurface)
    let injectedReply = false
    const simplify = freezeSimplify(key)
    // One unified path: the backend routes the MA tab to the Maestro
    // implementation (attachments included); mode shapes regular tabs.
    void persistChatDock(id, code, simplify, {
      model: pending.model, flags: pending.flags, mode: pending.mode,
    })
    const live = liveChatTarget(id, code)
    void WorkspacesService.SendWorkspaceMessage(
      live.id, live.code, wire, pending.model, pending.flags, wireAtts, pending.mode,
      pending.displayText || '',
      !!pending.freshSession,
    )
      .then((reply) => {
        if (isChatInjectedReply(reply)) {
          injectedReply = true
          return
        }
        const text = (reply || '').trim()
        // Stop may race ahead of stopChat's hold flag; treat the reply as authoritative.
        if (isChatStoppedReply(text)) {
          holdPendingByKey = { ...holdPendingByKey, [key]: true }
        }
        // Idle can flush the queue before this reply lands; the newer send
        // reset turnGotText, so painting here would repeat this turn's text.
        const cur = liveChatTarget(id, code)
        if (sendGenByKey[key] !== gen && sendGenByKey[cur.key] !== gen) return
        // Loop engage has no conductor LLM turn — don't paint a fake "Done."
        finishTurn(key, text || (pending.mode === 'loop' ? '' : 'Done.'))
      })
      .catch(() => {
        // Binding/RPC can fail while the engine turn is still running (sidebar
        // stays lit). Don't paint a fake error — idle + resync owns the ending.
        if ($agentsRunning.has(agentId)) return
        finishTurn(key, 'The agent hit an error — try again.')
      })
      .finally(() => {
        const live = liveChatTarget(id, code)
        clearHoistSending(key, gen)
        clearHoistSending(live.key, gen)
        // Loop engage is keyed to this send's mode — run even if a newer dispatch
        // already owns the busy flag (idle may have drained the queue early).
        if (pending.mode === 'loop' && live.code !== 'MA' && pending.displayText !== CHAT_RETRY_DISPLAY) void onLoopEngaged(live.id, live.code)
        if (sendGenByKey[key] !== gen && sendGenByKey[live.key] !== gen) return
        if (injectedReply) return
        sendingByKey = { ...sendingByKey, [key]: false, [live.key]: false }
        if (!$agentsRunning.has(agentId) && liveModelByKey[key]) {
          const { [key]: _live, ...restLive } = liveModelByKey
          liveModelByKey = restLive
        }
        // Keep turnStartedAt while the engine is still working (binding timeout /
        // remount). Clearing it here made the elapsed clock restart at 0:00 on
        // every workspace/panel switch. onAgent idle drops it below.
        // Skip stamp-clear / resync only when a newer turn is already open
        // after a footer — a finishTurn reply with no session.turn_end must
        // still release the stamp so the follow-up queue can flush.
        const list = chatByKey[live.key] ?? chatByKey[key] ?? []
        const started = turnStartedAtByKey[live.key] ?? turnStartedAtByKey[key] ?? 0
        const skipReset = shouldSkipIdleTranscriptReset(
          list,
          started,
          !!staleIdleArmedByKey[live.key] || !!staleIdleArmedByKey[key],
        )
        if (!$agentsRunning.has(agentId) && !$agentsRunning.has(`${live.id}-${live.code}`) && !skipReset) {
          const { [key]: _started, [live.key]: _liveStarted, ...restStarted } = turnStartedAtByKey
          turnStartedAtByKey = restStarted
        }
        if (!skipReset) {
          // Live deltas can scramble under load; the session store is the authority.
          // Mid-turn this merges history with the live tail; onAgent idle retries.
          resyncChat(live.id, live.code)
        }
        if (live.code !== 'MA') refreshTabTitles(live.id)
        tryFlushPending(live.key)
      })
  }

  // onLoopEngaged refreshes run state after a Loop-mode send. The tab you
  // sent from stays the conductor chat (mode=loop).
  async function onLoopEngaged(id: string, _code: string) {
    const [list, fresh] = await Promise.all([
      WorkspacesService.ListWorkspaceChats(id).catch(() => null),
      WorkspacesService.ListWorkspaces().catch(() => null),
    ])
    if (fresh) setWorkspacesFromList(fresh)
    if (list?.length) tabsByWs = { ...tabsByWs, [id]: list }
  }

  // stopChat interrupts this tab's in-flight chat turn and, on a loop
  // cockpit, the specialists too — the walk parks idle so talking to the
  // conductor (reenter / steer / send-back) continues. Queued follow-ups
  // are pulled back into the composer (not auto-sent).
  function stopChat() {
    // Abort the prompt only when Stop is on its target tab. Other tabs stay
    // unlocked (composerBusy is scoped), so this usually only runs on-target;
    // if a turn is also streaming there, cancel that turn after the abort.
    if (promptBusyHere) {
      promptAbort?.abort()
      if (!sending && !loopWalkLive) return
    }
    if (!ws || (!sending && !loopWalkLive)) return
    const owner = (liveEsc?.agentCode ?? '').trim()
    const mine = owner ? owner === activeCode : activeCode === conductorChatCode()
    if (liveEsc && mine) {
      escLoadGen++
      liveEsc = null
    }
    const key = chatKey(ws.id, activeCode)
    // Hold before cancel resolves — idle can fire before the queue is cleared /
    // the stopped reply lands.
    holdPendingByKey = { ...holdPendingByKey, [key]: true }
    const held = pendingByKey[key] ?? []
    if (held.length) {
      const merged = mergePendingSends(held)
      const pendingIds = new Set(held.map((p) => p.id))
      pendingByKey = { ...pendingByKey, [key]: [] }
      chatByKey = {
        ...chatByKey,
        [key]: (chatByKey[key] ?? []).filter((m) => !m.pendingId || !pendingIds.has(m.pendingId)),
      }
      const draftPlain = stripAttachmentMarkers(chatInput).trim()
      // Inline @paths stay in the restored text (they are the referents).
      chatInput = [merged.plain, draftPlain].filter(Boolean).join('\n\n')
      chatAttachments = [...merged.drafts, ...chatAttachments]
      inputByKey = { ...inputByKey, [key]: chatInput }
      attachmentsByKey = { ...attachmentsByKey, [key]: chatAttachments }
    }
    void WorkspacesService.CancelWorkspaceMessage(ws.id, activeCode).catch(() => {})
  }

  function resyncChat(id: string, code: string) {
    const key = chatKey(id, code)
    const gen = transcriptGenByKey[key] ?? 0
    WorkspacesService.WorkspaceChatItems(id, code)
      .then((items) => {
        if ((transcriptGenByKey[key] ?? 0) !== gen) return
        applyTranscript(key, items)
      })
      .catch(() => {})
  }

  // Live session stream: route by agent code into the matching chat-tab bucket
  // (active or background). Payloads are cumulative snapshots + seq.
  // Thinking arrives as text or a token estimate (redacted).
  $effect(() => onSessionDelta((id, code, evt) => {
    if (!acceptsChatCode(id, code)) return
    applyLiveStream(chatKey(id, code), 'text', evt.text, evt)
  }))
  $effect(() => onSessionThinking((id, code, evt) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    if (liveTurnClosed(key)) return
    if (evt.text || evt.replace || evt.done) {
      applyLiveStream(key, 'thinking', evt.text, evt)
    } else {
      chatByKey = { ...chatByKey, [key]: noteThinkingTokens(chatByKey[key] ?? [], evt.tokens, evt.uuid, evt.parentId) }
    }
  }))
  $effect(() => onSessionStep((id, code, step) => {
    if (!acceptsChatCode(id, code)) return
    addTool(chatKey(id, code), step)
  }))
  $effect(() => onSessionToolResult((id, code, toolId, result, isError, extras) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    chatByKey = { ...chatByKey, [key]: attachToolResult(chatByKey[key] ?? [], toolId, result, isError, extras) }
  }))
  $effect(() => onSessionToolInput((id, code, toolId, extras) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    chatByKey = { ...chatByKey, [key]: patchToolInput(chatByKey[key] ?? [], toolId, extras) }
  }))
  $effect(() => onSessionContextUsage((id, code, usage) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    const prev = contextUsageByKey[key]
    contextUsageByKey = {
      ...contextUsageByKey,
      [key]: {
        ...usage,
        compacting: false,
        compacted: !!prev?.compacted,
      },
    }
  }))
  $effect(() => onSessionCompact((id, code, evt) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    if (liveTurnClosed(key)) return
    chatByKey = { ...chatByKey, [key]: applyCompactRow(chatByKey[key] ?? [], evt) }
    const prev = contextUsageByKey[key]
    if (!prev) return
    const compacting = evt.status === 'started'
    const next = compacting
      ? { ...prev, compacting: true }
      : contextUsageAfterCompact(prev)
    if (!next) return
    contextUsageByKey = { ...contextUsageByKey, [key]: next }
  }))
  $effect(() => onSessionNotice((id, code, text) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    if (liveTurnClosed(key)) return
    chatByKey = { ...chatByKey, [key]: applyNoticeRow(chatByKey[key] ?? [], text) }
  }))
  $effect(() => onSessionFallback((id, code, evt) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    if (liveTurnClosed(key)) return
    chatByKey = { ...chatByKey, [key]: applyFallbackRow(chatByKey[key] ?? [], evt) }
  }))
  $effect(() => onSessionUser((id, code, text) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    chatByKey = { ...chatByKey, [key]: addUser(chatByKey[key] ?? [], text) }
  }))
  $effect(() => onSessionTurnEnd((id, code, turn) => {
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    chatByKey = { ...chatByKey, [key]: endTurnRow(dropOpenCompact(chatByKey[key] ?? []), turn.durationMs) }
    const usage = contextUsageFromTurn(turn)
    const prev = contextUsageByKey[key]
    if (usage) {
      contextUsageByKey = {
        ...contextUsageByKey,
        [key]: { ...usage, compacting: false, compacted: !!prev?.compacted },
      }
    } else if (prev?.compacting) {
      contextUsageByKey = { ...contextUsageByKey, [key]: { ...prev, compacting: false } }
    }
  }))

  // Engine status is the durable busy signal (sidebar + chat footer). When a turn
  // ends — including after a Send RPC that finished early — resync the transcript
  // and drain any follow-ups queued while it was running.
  $effect(() => onAgent((id, evt) => {
    if (evt.kind !== 'updated') return
    if (!evt.agentId.startsWith(id + '-')) return
    const code = evt.agentId.slice(id.length + 1)
    if (!acceptsChatCode(id, code)) return
    const key = chatKey(id, code)
    // Engine-driven turns (autonomous MA, background tabs, reload) never call
    // dispatchSend — stamp here so dropLiveAfterFooter can tell them from leftover.
    // Also replace a stamp that still belongs to a closed footer (lost idle);
    // leaving it in place makes dropLiveAfterFooter discard the whole next turn.
    if (isAgentRunningStatus(evt.status)) {
      const list = chatByKey[key] ?? []
      const started = turnStartedAtByKey[key] ?? 0
      if (shouldRefreshTurnStamp(list, started)) {
        if (started > 0 && currentTurnHasEnded(list, started)) {
          staleIdleArmedByKey = { ...staleIdleArmedByKey, [key]: true }
        }
        turnStartedAtByKey = { ...turnStartedAtByKey, [key]: Date.now() }
      }
      return
    }
    if (evt.status !== 'idle') return
    // Engine idle is authoritative: drop sticky RPC busy (binding may lag) so the
    // composer can drain the follow-up queue. A late .finally from the prior send
    // no-ops via sendGenByKey.
    // Directing/working for the next turn can reorder past this idle. Skip only
    // stamp-clear and resync on the first idle after a stamp refresh over a
    // footer — still drop sending and tryFlushPending. A later idle settles
    // (abort with no rows, or a stream that never got session.turn_end).
    const list = chatByKey[key] ?? []
    const started = turnStartedAtByKey[key] ?? 0
    const armed = !!staleIdleArmedByKey[key]
    const skipReset = shouldSkipIdleTranscriptReset(list, started, armed)
    if (armed) {
      const { [key]: _armed, ...restArmed } = staleIdleArmedByKey
      staleIdleArmedByKey = restArmed
    }
    if (sendingByKey[key]) {
      sendingByKey = { ...sendingByKey, [key]: false }
    }
    if (!skipReset) {
      if (turnStartedAtByKey[key]) {
        const { [key]: _started, ...restStarted } = turnStartedAtByKey
        turnStartedAtByKey = restStarted
      }
      if (liveModelByKey[key]) {
        const { [key]: _live, ...restLive } = liveModelByKey
        liveModelByKey = restLive
      }
      const prevUsage = contextUsageByKey[key]
      if (prevUsage?.compacting) {
        contextUsageByKey = { ...contextUsageByKey, [key]: { ...prevUsage, compacting: false } }
      }
      chatByKey = { ...chatByKey, [key]: dropOpenCompact(chatByKey[key] ?? []) }
      resyncChat(id, code)
    }
    if (code !== 'MA') refreshTabTitles(id)
    tryFlushPending(key)
  }))

  // The header (name/branch + Merge / Remove) is folded into the app titlebar
  // (App.svelte) in this mode, so this view is just the Observatory + chat.
</script>

<div class="wsv" bind:this={rootEl}>
  {#if ws}
    {#if createFailed && !hasRun && ws.worktreePath}
      {@const lastErr = ws.lastError ?? ''}
      {@const errNl = lastErr.indexOf('\n')}
      {@const errMulti = errNl >= 0}
      <div class="wsv-strip" role="status">
        <span class="wsv-state">{setupFailed ? 'Setup failed' : "Couldn't create"}</span>
        {#if lastErr && (!errMulti || !setupErrOpen)}
          <pre class="wsv-creating-err clip">{errMulti ? lastErr.slice(0, errNl) : lastErr}</pre>
        {/if}
        {#if errMulti}
          <button type="button" class="tbtn" onclick={() => (setupErrOpen = !setupErrOpen)}>
            {setupErrOpen ? 'Hide output' : 'Show output'}
          </button>
        {/if}
        <span class="wsv-strip-sp"></span>
        <div class="wsv-creating-acts">
          {#if setupFailed}
            <button type="button" class="tbtn" disabled={creating} onclick={retryWorkspaceSetup}>Retry setup</button>
            <button type="button" class="tbtn" onclick={() => openProject(ws.projectId)}>Edit setup command</button>
          {/if}
          <button type="button" class="tbtn" onclick={() => void removeWorkspace(ws.id)}>Remove</button>
        </div>
        {#if lastErr && errMulti && setupErrOpen}
          <pre class="wsv-creating-err block">{lastErr}</pre>
        {/if}
      </div>
    {/if}
    {#if chatTabDrag.active && dragTab}
      {@const meta = chatModeMeta(tabModeFor(dragTab))}
      {@const tabBusy = !!boundWsId && chatKeyBusy(chatKey(boundWsId, dragTab.code))}
      <div
        class="tab-chip-float"
        bind:this={chatTabFloatEl}
        style="left:{chatTabDrag.x}px;top:{chatTabDrag.y}px;width:{chatTabDrag.w}px;height:{chatTabDrag.h}px"
      >
        <TabChip
          class="wsv-chat-tab"
          floating
          label={dragTab.title || 'New chat'}
          icon={meta.icon}
          iconTint={meta.tint}
          active={activeCode === dragTab.code}
          busy={tabBusy}
          unseen={!!dragTab.unseen && activeCode !== dragTab.code}
          labelKey={`${boundWsId}:${dragTab.code}`}
        />
      </div>
    {/if}
    <div class="wsv-obs">
      <Observatory
        bind:this={observatory}
        projectId={ws.projectId}
        workspaceId={ws.id}
        {shellActive}
        loopScoped
        leftOpen={chatOpen}
        onToggleLeft={(o) => (chatOpen = o)}
        leftTitle="Agent"
        agentTabCount={chatTabs.length}
        onAgentNewTab={newChatTab}
        onAgentCloseTab={closeActiveChatTab}
        onAgentRotateTab={rotateChatTab}
        onFixFindings={fixFindings}
        onAddEditorMention={addEditorMention}
        mainBlocked={wsMainBlocked(ws)}
      >
        {#snippet mainOverlay()}
          <div class="wsv-creating" role="status" aria-live="polite">
            {#if ws.lastError}
              <Icon name="circle-alert" />
              <div class="wsv-creating-copy">
                <b>Couldn’t create the workspace</b>
                <pre class="wsv-creating-err">{ws.lastError}</pre>
                <span>{setupFailed
                  ? "The project's setup command exited non-zero. Fix it in the project settings, then remove this workspace and create it again."
                  : 'Remove this workspace and create it again.'}</span>
                <div class="wsv-creating-acts">
                  {#if setupFailed}
                    <button type="button" class="tbtn" onclick={() => openProject(ws.projectId)}>Edit setup command</button>
                  {/if}
                  <button type="button" class="tbtn" onclick={() => void removeWorkspace(ws.id)}>Remove workspace</button>
                </div>
              </div>
            {:else if ws.phase === 'waiting for a slot'}
              <Icon name="loader" class="spin" />
              <div class="wsv-creating-copy">
                <b>Waiting for a loop slot…</b>
                <span>Every loop slot is busy; this starts when one frees.</span>
              </div>
            {:else}
              <Icon name="loader" class="spin" />
              <div class="wsv-creating-copy">
                <b>Creating workspace…</b>
                <span>Setting up the worktree.</span>
              </div>
            {/if}
          </div>
        {/snippet}
        {#snippet planHead()}
          <TabStrip class="wsv-plan-tabs" label="Plan" activeKey={planTab}>
            <TabChip
              class="wsv-plan-tab"
              label="Details"
              icon="notebook"
              id="plan-tab-details"
              controls="plan-page"
              active={planTab === 'details'}
              lit
              onclick={() => (planTab = 'details')}
            />
            <TabChip
              class="wsv-plan-tab"
              label="Attachments"
              icon="grid"
              id="plan-tab-attachments"
              controls="plan-page"
              badge={planAttCount}
              active={planTab === 'attachments'}
              lit
              onclick={() => (planTab = 'attachments')}
            />
            <TabChip
              class="wsv-plan-tab"
              label="Todos"
              icon="list-todo"
              id="plan-tab-todos"
              controls="plan-page"
              badge={planTodosBadge}
              active={planTab === 'todos'}
              lit
              onclick={() => (planTab = 'todos')}
            />
            {#snippet trailing()}
              {#if !planEditing}
                {#if planNeedsApprove}
                  <button
                    type="button"
                    class="wsv-plan-approve"
                    title="Approve this plan"
                    onclick={approvePlanFromHead}
                  >Approve</button>
                {:else if ws.plan?.approved}
                  <button
                    type="button"
                    class="wsv-plan-act"
                    title="Hand the plan to an Agent-mode chat"
                    onclick={executePlanFromHead}
                  >Execute in chat</button>
                  {#if !hasRun}
                    <button
                      type="button"
                      class="wsv-plan-approve"
                      title="Engage the autopilot with this plan"
                      onclick={runPlanFromHead}
                    >Run as loop</button>
                  {/if}
                {/if}
              {/if}
            {/snippet}
          </TabStrip>
        {/snippet}
        {#snippet planPanel(selectedArtifactId)}
          <PlanPanel
            bind:this={planPanelCmp}
            workspaceId={ws.id}
            sessions={sessionFeed.sessions}
            bind:tab={planTab}
            bind:editing={planEditing}
            {selectedArtifactId}
            onOpenArtifact={(a) => { void observatory?.openArtifact(a) }}
          />
        {/snippet}
        {#snippet leftHead(lit: boolean)}
          <TabStrip class="wsv-chat-tabs" label="Agent chats" activeKey={activeCode} restoreScroll={chatTabScrollRestore} bind:stripEl={chatTabStripEl}>
            {#each chatTabs as tab (`${boundWsId}:${tab.code}`)}
              {@const meta = chatModeMeta(tabModeFor(tab))}
              {@const tabBusy = !!boundWsId && chatKeyBusy(chatKey(boundWsId, tab.code))}
              {@const floating = chatTabDrag.active && chatTabDrag.fromKey === tab.code}
              {#if floating}
                <div class="tab-chip-ph" style="width:{chatTabDrag.w}px" role="presentation" aria-hidden="true"></div>
              {:else}
                <TabChip
                  class="wsv-chat-tab"
                  tabKey={tab.code}
                  reorderable={chatTabs.length > 1}
                  label={tab.title || 'New chat'}
                  title={`${tab.title || 'New chat'} — ${meta.label} mode${tabBusy ? ' — running' : ''}${tab.unseen && activeCode !== tab.code ? ' — new response' : ''}`}
                  icon={meta.icon}
                  iconTint={meta.tint}
                  active={activeCode === tab.code}
                  {lit}
                  busy={tabBusy}
                  unseen={!!tab.unseen && activeCode !== tab.code}
                  animateLabel
                  labelKey={`${boundWsId}:${tab.code}`}
                  closeLabel={`Close ${tab.title || 'chat'}`}
                  onclick={() => {
                    if (chatTabDrag.suppressClick) {
                      chatTabDrag.suppressClick = false
                      return
                    }
                    selectChatTab(tab.code)
                  }}
                  onclose={() => closeChatTab(tab.code)}
                  onpointerdown={(e) => chatTabDrag.onPointerDown(e, tab.code)}
                  onkeydown={(e) => chatTabDrag.onKeydown(e, tab.code)}
                />
              {/if}
            {/each}
            {#snippet trailing()}
              {#if closedTabs.length}
                <div class="wsv-recents">
                  <button
                    type="button"
                    class="tab-strip-act"
                    class:on={recentsOpen}
                    title="Recent closed chats"
                    aria-label="Recent closed chats"
                    aria-expanded={recentsOpen}
                    aria-haspopup="menu"
                    onclick={() => (recentsOpen = !recentsOpen)}
                  >
                    <Icon name="clock" />
                  </button>
                  {#if recentsOpen}
                    <div class="wsv-recents-menu" role="menu">
                      {#each closedTabs as t (t.code)}
                        {@const age = formatRelativeAge(t.updatedAt)}
                        {@const title = t.title || NEW_CHAT_TITLE}
                        {@const meta = chatModeMeta(tabModeFor(t))}
                        {@const delArmed = armedDeleteCode === t.code}
                        <div
                          class="wsv-recents-row"
                          class:armed={delArmed}
                          role="none"
                          onpointerleave={() => { if (armedDeleteCode === t.code) clearArmedDelete() }}
                        >
                          <button
                            type="button"
                            class="wsv-recents-item"
                            role="menuitem"
                            title={title}
                            onclick={() => reopenClosedChat(t.code)}
                          >
                            <span class="wsv-recents-ico" style:color={meta.tint} aria-hidden="true">
                              <Icon name={meta.icon} />
                            </span>
                            <span class="wsv-recents-title">{title}</span>
                          </button>
                          <span class="wsv-recents-slot" role="none">
                            {#if age}
                              <span class="wsv-recents-age" title={age.full}>{age.rel}</span>
                            {/if}
                            <button
                              type="button"
                              class="wsv-recents-del"
                              class:armed={delArmed}
                              role="menuitem"
                              title={delArmed ? 'Confirm delete' : 'Delete permanently'}
                              aria-label={delArmed ? `Confirm delete ${title}` : `Delete ${title} permanently`}
                              onpointerdown={(e) => e.stopPropagation()}
                              onkeydown={(e) => { if (e.repeat) e.preventDefault() }}
                              onclick={(e) => {
                                e.stopPropagation()
                                onDeleteClosedChat(t.code)
                              }}
                            >
                              <Icon name="trash" />
                            </button>
                          </span>
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}
              <button
                type="button"
                class="tab-strip-act wsv-chat-new"
                title="New chat (⌘T)"
                aria-label="New chat"
                onclick={newChatTab}
              >
                <Icon name="plus" />
              </button>
            {/snippet}
          </TabStrip>
        {/snippet}
        {#snippet leftPanel(presentationActive: boolean)}
          <AgentChat
            bind:this={agentChat}
            messages={chat}
            bind:input={chatInput}
            bind:attachments={chatAttachments}
            sending={composerBusy}
            {steerFollowUp}
            {canSteer}
            {shellActive}
            {presentationActive}
            {turnStartedAt}
            statusText={setupStatus || waitingSubagents}
            onSend={(alt) => send({ alt })}
            onStop={stopChat}
            canStop={loopWalkLive}
            onRetry={retryChat}
            onRemoveQueued={removeQueued}
            {canFlushQueue}
            onCycleMode={cycleChatModeFromComposer}
            compact
            resetKey={activeKey}
            emptyText={activeCode === 'MA'
              ? 'Ask where this loop stands, or steer it — the Maestro can read the worktree, edit the plan, re-run review, and dispatch specialists; rails hold at execution.'
              : 'Ask the agent, or tell it what to change — the mode selector below decides whether it may edit.'}
            placeholder={composerBusy
              ? (steerFollowUp
                ? 'Steer the agent… (@ files, / skills, ⏎ steers, ⌘⏎ queues)'
                : canSteer
                  ? 'Add a follow-up… (@ files, / skills, ⏎ queues, ⌘⏎ steers)'
                  : 'Add a follow-up… (@ files, / skills, ⏎ to queue)')
              : createFailed
                ? (setupFailed && ws.worktreePath
                  ? 'Setup failed — retry setup or remove this workspace'
                  : 'Couldn’t create — remove this workspace to try again')
              : setupHold
                ? 'Message the agent… (@ files, / skills, ⏎ queues until setup finishes)'
              : activeCode === 'MA'
                ? 'Ask where things stand, or steer the loop… (@ files, / skills)'
                : 'Message the agent… (@ files, / skills, ⏎ to send)'}
            {contextUsage}
            onOpenFile={openAgentFile}
            onOpenArtifact={(a) => { void observatory?.openArtifact(a) }}
            rootPath={ws.worktreePath}
            projectId={ws.projectId}
            workspaceId={ws.id}
            agentCode={activeCode}
            escalation={onConductorTab ? liveEsc : null}
            onAnswerEscalation={answerLoopEscalation}
            findings={chatFindings}
            onFork={forkChatTab}
            onForkToWorkspace={forkChatToWorkspace}
            onTurnDiff={showTurnDiff}
            onReset={activeCode === 'MA' || loopPickersPinned ? undefined : resetChatTo}
            onPromptAction={invokePromptAction}
            promptBusy={promptRunning}
          >
            {#snippet sessionCard(log)}
              <SessionCard
                {presentationActive}
                code={log.code}
                name={log.name}
                messages={log.messages}
                live={log.live}
                rootPath={ws.worktreePath}
                workspaceId={ws.id}
                onOpenFile={openAgentFile}
                findings={chatFindings}
              />
            {/snippet}
            {#snippet toolbar()}
              {#if activeCode === 'MA'}
                <Select
                  ghost up
                  class="wsv-mode"
                  label="Chat mode"
                  options={chatModeOptions()}
                  value="loop"
                  disabled={composerBusy}
                  onchange={onChatModeChange}
                />
                {#if showLoopPickers}
                  <LoopPolicyPickers
                    loopId={dockLoopId}
                    policyId={dockPolicyId}
                    {loops}
                    {policies}
                    pinned={loopPickersPinned}
                    onLoopChange={(id) => persistLoopPolicy(id, dockPolicyId)}
                    onPolicyChange={(id) => persistLoopPolicy(dockLoopId, id)}
                  />
                {/if}
              {:else if pickersReady}
                <Select
                  ghost up
                  class="wsv-mode"
                  label="Chat mode"
                  options={chatModeOptions(isMain || isPRHost, isPRHost)}
                  value={chatMode}
                  disabled={composerBusy}
                  onchange={onChatModeChange}
                />
                {#if showLoopPickers}
                  <LoopPolicyPickers
                    loopId={dockLoopId}
                    policyId={dockPolicyId}
                    {loops}
                    {policies}
                    pinned={loopPickersPinned}
                    onLoopChange={(id) => persistLoopPolicy(id, dockPolicyId)}
                    onPolicyChange={(id) => persistLoopPolicy(dockLoopId, id)}
                  />
                {:else}
                <ModelPickers
                  ghost up
                  models={catalogModels}
                  {model}
                  {flags}
                  disabled={composerBusy}
                  onModelChange={onChatModelChange}
                  onFlagsChange={onChatFlagsChange}
                />
                {/if}
              {:else}
                <div class="model-pickers model-pickers-pending" aria-busy="true" aria-label="Loading models">
                  <span class="mpp-skel"></span>
                  <span class="mpp-skel mpp-skel-sm"></span>
                </div>
              {/if}
              <button
                type="button"
                class="wsv-simplify"
                class:on={simplifyOn}
                aria-pressed={simplifyOn}
                aria-label="Simplify mode"
                title={simplifyOn ? 'Simplify on' : 'Simplify off'}
                disabled={composerBusy || !$settings}
                onclick={() => toggleSimplify()}
              >
                <Icon name="feather" />
                <span class="wsv-simplify-label">Simplify</span>
              </button>
            {/snippet}
          </AgentChat>
        {/snippet}
      </Observatory>
    </div>
  {/if}
</div>

<style>
  .wsv { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .wsv-obs { flex: 1; min-height: 0; display: flex; overflow: hidden; }
  .wsv-creating {
    flex: 1; width: 100%; min-height: 0; display: flex; flex-direction: column;
    align-items: center; justify-content: center; gap: 14px;
    padding: 32px 24px; text-align: center; color: var(--muted);
  }
  .wsv-creating :global(svg) { width: 22px; height: 22px; color: var(--amber); }
  .wsv-creating-copy { display: flex; flex-direction: column; gap: 6px; max-width: 36rem; min-width: 0; }
  .wsv-creating-copy b { color: var(--text); font-weight: 600; font-size: 13px; }
  .wsv-creating-copy span { font-size: 12px; line-height: 1.45; color: var(--faint); }
  .wsv-creating-err {
    margin: 0;
    max-height: 12rem;
    overflow: auto;
    text-align: left;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.45;
    color: var(--text);
  }
  .wsv-creating-err.clip {
    max-height: none;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
    flex: 1 1 8rem;
  }
  .wsv-creating-err.block { flex: 1 1 100%; }
  .wsv-creating-acts { display: flex; flex-wrap: wrap; gap: 8px; justify-content: center; }
  .wsv-strip { flex-wrap: wrap; }
  .wsv-strip .wsv-creating-acts { justify-content: flex-end; }
  /* Tab strip layout lives on .obs-side-head.tabbed (styles/_observatory.scss) — same stretch /
     underline geometry as the file toolbar and terminal chrome. */
  :global(.wsv-chat-new) { color: var(--muted); }
  :global(.wsv-plan-approve) {
    display: inline-flex;
    align-items: center;
    height: 22px;
    padding: 0 8px;
    border: 1px solid var(--emerald);
    border-radius: 5px;
    background: var(--emerald);
    color: #0A0C10;
    font: inherit;
    font-size: 11px;
    font-weight: 550;
    cursor: pointer;
  }
  :global(.wsv-plan-approve:hover) {
    background: color-mix(in oklab, var(--emerald) 88%, white);
    border-color: color-mix(in oklab, var(--emerald) 88%, white);
  }
  :global(.wsv-plan-act) {
    display: inline-flex;
    align-items: center;
    height: 22px;
    padding: 0 8px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--muted);
    font: inherit;
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
  }
  :global(.wsv-plan-act:hover) {
    color: var(--text);
    background: var(--pane-2);
  }
  /* Reserve dock space while the catalogue loads — avoids a raw-ref flash. */
  .model-pickers-pending {
    display: inline-flex; align-items: center; gap: 6px;
    height: 24px; padding: 0 6px; min-width: 118px;
  }
  .mpp-skel {
    display: block; height: 10px; width: 72px; border-radius: 4px;
    background: var(--pane-2);
  }
  .mpp-skel-sm { width: 44px; opacity: .75; }
</style>
