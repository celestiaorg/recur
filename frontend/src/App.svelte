<script lang="ts">
  import { onMount } from 'svelte'
  import { get } from 'svelte/store'
  import { Events, System } from '@wailsio/runtime'
  import { loadInitial, wireEvents, WorkspacesService, PRsService, call, upsertWorkspace, onWorkspaceFs, onAppFocused, onGithubBranchRules } from './lib/api'
  import { applyOpenPathResult, pickAndOpenPath } from './lib/open-path'
  import { wireFileIndexEvents } from './lib/file-index'
  import { maybeRunBridgeBench } from './lib/bridge-bench'
  import {
    crumb, paletteOpen, commandsOpen, newTaskOpen, newProjectOpen, shortcutsOpen, projects, projectFormId, view, go, openNewTask, openPR, openWorkspace, openProject, enableShellPersist, pushToast, prReviewModal,
    workspaces, selectedWorkspaceId, selectedPRId, prs, settings, railCollapsed, toggleRail, toggleTerminalDock,
    toggleTerminalDockMaximized, toggleShortcuts, toggleCommandsPalette, toggleGoPalette, toggleOpenIn,
    cycleWorkspace,
    cycleEditorTheme,
    toastEditorTheme,
    openInOpen, runMenuOpen, stopMenuOpen, workspacesRunning, permissionRequests, theme, editorTheme, setTheme, setEditorTheme,
    terminalPrefs, setTerminalPrefs,
    workspaceScanPending,
    syntaxModeOpen,
    instanceLabel,
  } from './lib/stores'
  import { keybindingMenu } from './lib/keybinding-menu'
  import { chatUIOccluded } from './lib/chat-occlusion'
  import { requestRunDev, workspaceDevCmd, workspaceRun, runSessionBy, runningRuns, isAnyRunRunning, openRunMenu, openStopMenu, requestStopRun } from './lib/run-dev'
  import { runLabel } from './lib/run-commands'
  import { requestPromptAction } from './lib/promptActions'
  import { flushPendingFixChecks } from './lib/pr-fix-checks'
  import { isBackquote, isCmdOrCtrl, isLetterKey } from './lib/shortcut'
  import { closeRailPeek, railPeek } from './lib/rail-peek'
  import { createBlankWorkspace } from './lib/new-workspace'
  import { checkForUpdates, wireUpdaterEvents } from './lib/updater'
  import { onMenuAction, wireMenuEvents, pushMenuThemes, pushMenuThemeState, setMenuState, terminalIdFromMenu } from './lib/menu'
  import { applyShellRestore, loadShellState, resolveShellRestore } from './lib/shell-state'
  import { loadScheduleExpanded } from './lib/schedule-expanded'
  import { copySelectedWorkspacePath } from './lib/workspace-path'
  import { isTitlebarZoomTarget } from './lib/titlebar'
  import { initZoom, zoomChordAction, zoomIn, zoomOut, zoomReset } from './lib/zoom'
  import { wsMainBlocked, wsRunActive, workspaceCanRemove, workspaceShowsRemove } from './lib/modes'
  import { removeWorkspace } from './lib/remove-workspace'
  import { prForWorkspace, prIconName, prLifecycle } from './lib/colors'
  import {
    EMPTY_WORKSPACE_LAND, getWorkspaceLand, putWorkspaceLand, workspaceLandFrom,
  } from './lib/caches/workspace-land-cache'
  import { allEditorThemesForPicker } from './lib/cm-themes'
  import ArmedButton from './components/ArmedButton.svelte'
  import PRTitlebarActions from './components/PRTitlebarActions.svelte'
  import PRListTitlebar from './components/PRListTitlebar.svelte'
  import InboxTitlebar from './components/InboxTitlebar.svelte'
  import SchedulesTitlebar from './components/SchedulesTitlebar.svelte'
  import PRReviewModal from './components/PRReviewModal.svelte'
  import EmojiSuggest from './components/EmojiSuggest.svelte'
  import UserMentionSuggest from './components/UserMentionSuggest.svelte'
  import DecodeText from './components/DecodeText.svelte'
  import Icon from './components/Icon.svelte'
  import WorkspaceBaseSelect from './components/WorkspaceBaseSelect.svelte'
  import WorkspaceIssueButton from './components/WorkspaceIssueButton.svelte'

  import Sidebar from './components/Sidebar.svelte'
  import CommandPalette from './components/CommandPalette.svelte'
  import RunCommandPalette from './components/RunCommandPalette.svelte'
  import OpenInPalette from './components/OpenInPalette.svelte'
  import RunPalette from './components/RunPalette.svelte'
  import StopPalette from './components/StopPalette.svelte'
  import SyntaxPalette from './components/SyntaxPalette.svelte'
  import KeybindingMenu from './components/KeybindingMenu.svelte'
  import ShortcutsHelp from './components/ShortcutsHelp.svelte'
  import Toasts from './components/Toasts.svelte'
  import TitleTooltip from './components/TitleTooltip.svelte'
  import PermissionPrompts from './components/PermissionPrompts.svelte'
  import QuestionPrompts from './components/QuestionPrompts.svelte'

  import WorkspaceDetail from './views/WorkspaceDetail.svelte'
  import NewTask from './views/NewTask.svelte'
  import NewProject from './views/NewProject.svelte'
  import Settings from './views/Settings.svelte'
  import Project from './views/Project.svelte'
  import ScheduledPrompts from './components/ScheduledPrompts.svelte'
  import ScheduleEditor from './components/ScheduleEditor.svelte'
  import PRList from './views/PRList.svelte'
  import Issues from './components/Issues.svelte'
  import PRDetail from './views/PRDetail.svelte'
  import Briefing from './views/Briefing.svelte'
  import Welcome from './views/Welcome.svelte'

  const selectedLoop = $derived($workspaces.find((l) => l.id === $selectedWorkspaceId))
  const titlebarProject = $derived($projects.find((p) => p.id === $projectFormId))
  const titlebarPR = $derived($prs.find((p) => p.id === $selectedPRId))
  const showLoopBack = $derived($view === 'detail')
  const showPRBack = $derived($view === 'prdetail')
  // Wails sets the OS only after the page loads, so keep the macOS spacer until it says otherwise.
  const offMac = () => System.IsLinux() || System.IsWindows()
  let trafficClear = $state(!offMac())
  window.addEventListener('wails:runtime-config-ready', () => (trafficClear = !offMac()), { once: true })
  let pendingPRCreate = $state<{ prId: string; wsId: string } | null>(null)
  const creatingThisPR = $derived(!!titlebarPR?.id && pendingPRCreate?.prId === titlebarPR.id)

  $effect(() => {
    const pending = pendingPRCreate
    if (!pending) return
    const pr = $prs.find((p) => p.id === pending.prId)
    if ((pr?.workspaceId ?? '').trim()) {
      pendingPRCreate = null
      flushPendingFixChecks()
      return
    }
    if (!pending.wsId) return
    const ws = $workspaces.find((w) => w.id === pending.wsId)
    if (!ws || ws.lastError || ws.phase === 'failed') {
      pendingPRCreate = null
    }
  })

  $effect(() => {
    void $prs
    void $workspaces
    flushPendingFixChecks()
  })

  async function createPRWorkspace() {
    const id = titlebarPR?.id
    if (!id || pendingPRCreate?.prId === id) return
    pendingPRCreate = { prId: id, wsId: '' }
    const ws = await call(PRsService.CreateWorkspaceFromPR(id), 'Could not create the workspace')
    if (!ws) {
      if (pendingPRCreate?.prId === id) pendingPRCreate = null
      return
    }
    upsertWorkspace(ws)
    workspaces.update((arr) =>
      arr.map((w) => (w.id === ws.id ? { ...w, ...ws, hostPrId: ws.hostPrId ?? '' } : w)),
    )
    const ready = !!(ws.worktreePath ?? '').trim() && !(ws.hostPrId ?? '').trim()
    if (ready) {
      prs.update((arr) => arr.map((p) => (p.id === id ? { ...p, workspaceId: ws.id, projectId: p.projectId || ws.projectId } : p)))
      pushToast('success', 'Workspace created')
      if (pendingPRCreate?.prId === id) pendingPRCreate = null
    } else {
      pendingPRCreate = { prId: id, wsId: ws.id }
      pushToast('info', 'Creating workspace…')
    }
  }
  // A hand-driven workspace (no run engaged) folds its header into the titlebar:
  // Commit / Update / Merge + Remove live on the right.
  // GitHub-opt-in projects show Open PR (split: ready / draft) until a
  // GitHub PR exists, then View PR always, plus a separate Pull / Push /
  // Force push button when that action is available. A Local fallback keeps
  // Open PR so gh create can be retried, and still shows View PR for the
  // Local row — including after a loop settles at pr_open.
  // Land chrome (Commit / Update / Merge / Open PR) while no loop is driving
  // the worktree. A paused converge-reviews run is idle — the human can open
  // a PR themselves. pr_open stays on PR chrome (the loop already opened one).
  const isWorkspaceMode = $derived(
    $view === 'detail' && !!selectedLoop && !selectedLoop.isMain && !(selectedLoop.hostPrId ?? '').trim()
      && selectedLoop.state !== 'merged'
      && selectedLoop.state !== 'pr_open'
      && !wsRunActive(selectedLoop),
  )
  // The main working copy also folds its header into the titlebar, but has no
  // Remove affordance (it tracks the project's real checkout).
  const isMainMode = $derived($view === 'detail' && !!selectedLoop && !!selectedLoop.isMain)
  // Remove lives in the titlebar for any removable-kind workspace. Running
  // loops and in-flight agent turns keep the button visible but disabled
  // (engine refuses until they settle). Main / PR hosts never show it.
  const showRemove = $derived(
    $view === 'detail' && !!selectedLoop && workspaceShowsRemove(selectedLoop),
  )
  const canRemove = $derived(
    $view === 'detail' && !!selectedLoop && workspaceCanRemove(selectedLoop, $workspacesRunning.has(selectedLoop.id)),
  )
  const canRunDev = $derived(
    $view === 'detail' && !!selectedLoop && !wsMainBlocked(selectedLoop) && !!workspaceDevCmd(selectedLoop.id, $workspaces, $projects),
  )
  const defaultRunChoice = $derived(selectedLoop ? workspaceRun(selectedLoop.id, $workspaces, $projects) : null)
  const runningChoices = $derived(selectedLoop ? runningRuns(selectedLoop.id, $runSessionBy) : [])
  const anyRunRunning = $derived(!!selectedLoop && isAnyRunRunning(selectedLoop.id, $runSessionBy))
  const titlebarRunLabel = $derived(runLabel('Run', defaultRunChoice?.name || 'dev'))
  const titlebarStopLabel = $derived(runningChoices.length === 1
    ? runLabel('Stop', runningChoices[0]!.name)
    : 'Stop')

  // Confirm copy for Remove when the worktree has uncommitted or unmerged work.
  let discardRisk = $state('')
  const removeConfirmLabel = $derived(
    discardRisk ? `Confirm remove — ${discardRisk}` : 'Confirm remove',
  )
  let discardRiskSeq = 0
  function refreshDiscardRisk() {
    const id = $selectedWorkspaceId
    if (!id || !canRemove) {
      discardRisk = ''
      return
    }
    const seq = ++discardRiskSeq
    void WorkspacesService.WorkspaceDiscardRisk(id)
      .then((risk) => {
        if (seq === discardRiskSeq) discardRisk = risk ?? ''
      })
      .catch(() => {
        if (seq === discardRiskSeq) discardRisk = ''
      })
  }
  $effect(() => {
    // Re-run when selection / visibility of Remove changes.
    void $selectedWorkspaceId
    void canRemove
    refreshDiscardRisk()
  })
  $effect(() => {
    const on = $view === 'detail' && !!selectedLoop && !wsMainBlocked(selectedLoop)
    setMenuState({ openIn: on, copyPath: on })
  })

  let land = $state({ ...EMPTY_WORKSPACE_LAND })
  let landSeq = 0
  let landClock = 0
  let landWorkspaceId = ''
  /** False until WorkspaceLandState / fetch settles for the on-screen workspace. */
  let landKnown = $state(false)
  let updatingBase = $state(false)
  let pullingPR = $state(false)
  let pushingRemote = $state(false)
  let rulesEvt: { projectId: string; remote: string; branch: string; requirePR: boolean; at: number } | null = null
  function landShortBranch() {
    if (!land.remote) return land.base
    const prefix = land.remote + '/'
    return land.base.startsWith(prefix) ? land.base.slice(prefix.length) : land.base
  }
  function landHitsRules(branch: string, remote?: string) {
    if (!land.remote) return false
    if (remote && land.remote !== remote) return false
    return landShortBranch() === branch
  }
  function applyRequirePR(projectId: string, branch: string, requirePR: boolean, remote?: string) {
    const id = get(selectedWorkspaceId)
    if (!id) return
    const ws = get(workspaces).find((l) => l.id === id)
    if (!ws || ws.projectId !== projectId) return
    rulesEvt = { projectId, remote: remote || land.remote, branch, requirePR, at: landClock }
    if (!landHitsRules(branch, remote)) return
    if (land.requirePR === requirePR) return
    land = { ...land, requirePR }
    putWorkspaceLand(id, land)
  }
  function applyLand(st: {
    behindBase?: number; aheadBase?: number; dirty?: boolean; unmerged?: boolean
    base?: string; remote?: string
    prId?: string; prNumber?: number; prSource?: string; prDraft?: boolean
    aheadRemote?: number; behindRemote?: number
    requirePR?: boolean
  } | null | undefined, started = 0) {
    land = workspaceLandFrom(st)
    if (rulesEvt && started > 0 && rulesEvt.at >= started) {
      const id = get(selectedWorkspaceId)
      const ws = id ? get(workspaces).find((l) => l.id === id) : undefined
      if (ws && ws.projectId === rulesEvt.projectId && landHitsRules(rulesEvt.branch, rulesEvt.remote || undefined)) {
        land = { ...land, requirePR: rulesEvt.requirePR }
      }
    }
    landKnown = true
    const id = $selectedWorkspaceId
    if (id) putWorkspaceLand(id, land)
  }
  function recallLand(id: string) {
    const cached = id ? getWorkspaceLand(id) : null
    land = cached ?? { ...EMPTY_WORKSPACE_LAND }
    landKnown = false
    rulesEvt = null
  }
  function refreshLandState() {
    const id = $selectedWorkspaceId
    if (!id || !(isWorkspaceMode || isMainMode) || wsMainBlocked(selectedLoop)) {
      return
    }
    const seq = ++landSeq
    const started = ++landClock
    void WorkspacesService.WorkspaceLandState(id)
      .then((st) => {
        if (seq === landSeq) applyLand(st, started)
      })
      .catch(() => {
        if (seq !== landSeq) return
        // Don't keep a recalled snapshot disabled on "Checking…" forever.
        land = { ...EMPTY_WORKSPACE_LAND }
        landKnown = true
      })
  }
  $effect.pre(() => {
    const id = $selectedWorkspaceId
    if (id === landWorkspaceId) return
    landWorkspaceId = id
    landSeq += 1
    recallLand(id)
  })
  $effect(() => {
    void $selectedWorkspaceId
    void isWorkspaceMode
    void isMainMode
    void selectedLoop?.diff
    void selectedLoop?.behindBase
    void selectedLoop?.state
    void wsMainBlocked(selectedLoop)
    if ($workspaceScanPending) {
      landSeq++
      return
    }
    refreshLandState()
  })
  // Fetch remotes after the workbench paints so a workspace click is not
  // coupled to git fetch. Main working copy fetches its tracking branch; every
  // other workspace fetches the merge base (and GitHub PR head). Keyboard cycle
  // already skips this via workspaceScanPending; clicks wait two frames as well.
  $effect(() => {
    if ($workspaceScanPending) return
    if (!isMainMode && !isWorkspaceMode) return
    const id = $selectedWorkspaceId
    if (!id) return
    const started = ++landClock
    let cancelled = false
    let inner = 0
    const outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => {
        if (cancelled) return
        void WorkspacesService.FetchWorkingCopyUpstream(id)
          .then((st) => {
            if (cancelled) return
            landSeq++
            applyLand(st, started)
          })
          .catch(() => {})
      })
    })
    return () => {
      cancelled = true
      cancelAnimationFrame(outer)
      if (inner) cancelAnimationFrame(inner)
    }
  })
  // Titlebar land is snapshotted on open. A clean `git pull` moves HEAD without
  // changing Diff/BehindBase on the workspace row, so the $effect above never
  // re-runs. Observatory already refreshes off workspace.fs_changed; the Update
  // / Pull / Push buttons must too. app.focused covers a pull in another app
  // then click-back (same trigger as the file-index refresh).
  const LAND_FS_DEBOUNCE_MS = 50
  let landFsTimer: ReturnType<typeof setTimeout> | undefined
  function scheduleLandRefreshFromDisk() {
    if (get(workspaceScanPending)) return
    if (landFsTimer) return
    landFsTimer = setTimeout(() => {
      landFsTimer = undefined
      if (get(workspaceScanPending)) return
      refreshLandState()
    }, LAND_FS_DEBOUNCE_MS)
  }
  $effect(() => {
    const unsubFs = onWorkspaceFs((changedId) => {
      if (changedId !== get(selectedWorkspaceId)) return
      scheduleLandRefreshFromDisk()
    })
    const unsubFocus = onAppFocused(() => {
      if (!get(selectedWorkspaceId)) return
      scheduleLandRefreshFromDisk()
    })
    const unsubRules = onGithubBranchRules((projectId, branch, requirePR, remote) => {
      applyRequirePR(projectId, branch, requirePR, remote)
    })
    return () => {
      unsubFs()
      unsubFocus()
      unsubRules()
      if (landFsTimer) {
        clearTimeout(landFsTimer)
        landFsTimer = undefined
      }
    }
  })
  // Hand-driven workspace: exclusive Commit → Update → Merge (one body of work).
  // Main working copy: Commit / Update / Push are independent — leftover
  // untracked or modified files must not hide Push or Update.
  const landOn = $derived((isWorkspaceMode || isMainMode) && !wsMainBlocked(selectedLoop))
  const showCommit = $derived(landOn && land.dirty && !land.unmerged)
  const showUpdate = $derived(
    isMainMode
      ? !!land.remote && (land.behindBase > 0 || land.unmerged)
      : isWorkspaceMode && !showCommit && (land.behindBase > 0 || land.unmerged),
  )
  const showPush = $derived(
    isMainMode && !!land.remote && land.aheadBase > 0 && land.behindBase === 0 && !land.unmerged,
  )
  const workspaceGitHubPR = $derived($prs.find((p) =>
    p.workspaceId === $selectedWorkspaceId && p.source === 'gh' && p.state !== 'Merged' && p.state !== 'Closed' && p.group !== 'merged'))
  const selectedProject = $derived($projects.find((p) => p.id === selectedLoop?.projectId))
  const githubPRs = $derived(!!selectedProject?.prOnGithub)
  const showMerge = $derived(isWorkspaceMode && !showCommit && !showUpdate && land.aheadBase > 0 && !workspaceGitHubPR && !(githubPRs && !!land.remote && land.requirePR !== false))
  const agentBusy = $derived($workspacesRunning.has($selectedWorkspaceId))
  const landLocked = $derived(agentBusy || updatingBase || pullingPR || pushingRemote)
  const landPending = $derived(landOn && !landKnown)
  const landBusy = $derived(landLocked || landPending)
  const landLockedTitle = $derived(agentBusy ? 'Wait until the agent finishes' : '')
  const landBusyTitle = $derived(landPending ? 'Checking git status…' : landLockedTitle)
  const storePR = $derived($selectedWorkspaceId ? prForWorkspace($prs, $selectedWorkspaceId) : undefined)
  const storeGitHubPR = $derived(storePR?.source === 'gh' ? storePR : undefined)
  const storeOpenPR = $derived(
    storeGitHubPR && prLifecycle(storeGitHubPR) !== 'merged' && prLifecycle(storeGitHubPR) !== 'closed'
      ? storeGitHubPR
      : undefined,
  )
  const viewPRId = $derived(storePR?.id || land.prId || '')
  const viewPRNumber = $derived(storePR?.number || land.prNumber || 0)
  const viewPRDraft = $derived(!!(land.prDraft || storeOpenPR?.draft))
  const hasViewablePR = $derived(!!viewPRId)
  const showViewWorkspacePR = $derived(
    $view === 'detail' && !!selectedLoop && !selectedLoop.isMain && hasViewablePR,
  )
  const openPRSource = $derived(land.prSource || storeOpenPR?.source || '')
  const githubPROpen = $derived(
    openPRSource === 'gh'
    && (!!storeOpenPR || (!!land.prId && prLifecycle(storeGitHubPR) !== 'merged' && prLifecycle(storeGitHubPR) !== 'closed')),
  )
  // First open (hand-driven) or retry after a Local fallback — including a
  // loop that settled at pr_open, where land chrome is hidden.
  const showOpenGitHubPR = $derived(
    githubPRs && !githubPROpen && prLifecycle(storeGitHubPR) !== 'merged'
    && $view === 'detail' && !!selectedLoop && !selectedLoop.isMain
    && !wsMainBlocked(selectedLoop)
    && selectedLoop.state !== 'merged'
    && !wsRunActive(selectedLoop)
    && (isWorkspaceMode || showViewWorkspacePR),
  )
  const prDiverged = $derived(githubPROpen && land.aheadRemote > 0 && land.behindRemote > 0)
  const canPullPR = $derived(githubPROpen && !land.dirty && !land.unmerged && land.behindRemote > 0)
  const canPushPR = $derived(githubPROpen && !land.dirty && !land.unmerged && land.aheadRemote > 0 && land.behindRemote === 0)
  const canForcePushPR = $derived(githubPROpen && !land.dirty && !land.unmerged && prDiverged)
  const mergeBaseLabel = $derived.by(() => {
    const b = (selectedLoop?.baseBranch || 'main').trim()
    const m = /^(origin|upstream)\/(.+)$/.exec(b)
    return m ? m[2] : (b || 'main')
  })
  const updateLabel = $derived.by(() => {
    const base = isMainMode ? (land.base || 'remote') : mergeBaseLabel
    if (updateMethod === 'rebase') return `Rebase onto ${base}`
    return isMainMode ? `Update from ${base}` : `Update with ${base}`
  })
  const mergeMethodLabels: Record<string, string> = {
    merge: 'Merge',
    squash: 'Squash and merge',
    rebase: 'Rebase and merge',
  }
  const mergeMethodKeys = ['merge', 'squash', 'rebase'] as const
  const updateMethodKeys = ['merge', 'rebase'] as const
  const updateMethodMenuLabels: Record<string, string> = {
    merge: 'Merge',
    rebase: 'Rebase',
  }
  // Project override → global default → merge (same resolution as PRDetail).
  const resolvedMergeMethod = $derived.by(() => {
    if (selectedProject?.mergeMethod && mergeMethodLabels[selectedProject.mergeMethod]) return selectedProject.mergeMethod
    if ($settings?.defaultMergeMethod && mergeMethodLabels[$settings.defaultMergeMethod]) return $settings.defaultMergeMethod
    return 'merge'
  })
  const resolvedUpdateMethod = $derived.by(() => {
    if (selectedProject?.updateBaseMethod === 'rebase' || selectedProject?.updateBaseMethod === 'merge') return selectedProject.updateBaseMethod
    if ($settings?.defaultUpdateBaseMethod === 'rebase' || $settings?.defaultUpdateBaseMethod === 'merge') return $settings.defaultUpdateBaseMethod
    return 'merge'
  })
  let mergeMethodOverride = $state<string | null>(null)
  const mergeMethod = $derived(mergeMethodOverride ?? resolvedMergeMethod)
  const mergeLabel = $derived(mergeMethodLabels[mergeMethod] ?? 'Merge')
  const mergeHint = $derived.by(() => {
    if (land.remote && land.base) return `${mergeLabel} to ${land.base} (push)`
    return `${mergeLabel} into ${mergeBaseLabel} on disk — you are the only gate`
  })
  let confirmingMerge = $state(false)
  let mergeMenuOpen = $state(false)
  let updateMethodOverride = $state<string | null>(null)
  const updateMethod = $derived(updateMethodOverride ?? resolvedUpdateMethod)
  let updateMenuOpen = $state(false)
  let openPRMenuOpen = $state(false)
  $effect(() => {
    void $selectedWorkspaceId
    confirmingMerge = false
    mergeMenuOpen = false
    mergeMethodOverride = null
    updateMenuOpen = false
    openPRMenuOpen = false
    updateMethodOverride = null
    runMenuOpen.set(false)
    stopMenuOpen.set(false)
  })
  $effect(() => {
    if (!landBusy) return
    confirmingMerge = false
    mergeMenuOpen = false
    updateMenuOpen = false
    openPRMenuOpen = false
    runMenuOpen.set(false)
    stopMenuOpen.set(false)
  })

  async function mergeWorkspace() {
    if (landBusy) return
    if (!confirmingMerge) {
      confirmingMerge = true
      mergeMenuOpen = false
      return
    }
    confirmingMerge = false
    mergeMenuOpen = false
    const merged = await call(WorkspacesService.MergeWorkspace($selectedWorkspaceId, mergeMethod), 'Could not merge')
    if (!merged) return
    workspaces.update((arr) => arr.map((l) => (l.id === merged.id ? merged : l)))
    if (land.remote && land.base) {
      pushToast('success', `${mergeLabel} to ${land.base}`)
    } else {
      pushToast('success', `${mergeLabel} into ${mergeBaseLabel}`)
    }
  }
  function pickMergeMethod(m: string) {
    mergeMethodOverride = m
    mergeMenuOpen = false
    confirmingMerge = false
  }
  function pickUpdateMethod(m: string) {
    updateMethodOverride = m
    updateMenuOpen = false
  }

  function requestCommit() {
    if (landBusy) return
    requestPromptAction($selectedWorkspaceId, 'commit')
  }

  function requestOpenPR(draft = false) {
    if (landBusy || agentBusy) return
    openPRMenuOpen = false
    requestPromptAction($selectedWorkspaceId, draft ? 'open-draft-pr' : 'open-pr')
  }

  function viewWorkspacePR() {
    if (viewPRId) openPR(viewPRId)
  }

  async function pullWorkspacePR() {
    if (landBusy || !canPullPR) return
    const id = $selectedWorkspaceId
    if (!id) return
    pullingPR = true
    try {
      const res = await call(WorkspacesService.PullWorkspacePR(id), 'Could not pull from the PR')
      if (!res) return
      if (res.workspace?.id) {
        workspaces.update((arr) => arr.map((l) => (l.id === res.workspace.id ? { ...l, ...res.workspace, behindBase: res.workspace.behindBase ?? 0 } : l)))
      }
      if (res.status === 'conflict') {
        requestPromptAction(id, 'update')
        return
      }
      if (res.status === 'updated') {
        pushToast('success', viewPRNumber ? `Pulled updates from PR #${viewPRNumber}` : 'Pulled updates from the PR')
      } else if (res.status === 'noop') {
        pushToast('info', viewPRNumber ? `Already up to date with PR #${viewPRNumber}` : 'Already up to date with the PR')
      }
    } finally {
      pullingPR = false
      refreshLandState()
    }
  }

  async function pushWorkspacePR(force = false) {
    if (landBusy || (force ? !canForcePushPR : !canPushPR)) return
    const id = $selectedWorkspaceId
    if (!id) return
    pushingRemote = true
    try {
      const prId = await call(WorkspacesService.OpenWorkspacePR(id, force), force ? 'Could not force-push to the PR' : 'Could not push to the PR')
      if (!prId) return
      if (force) {
        pushToast('success', viewPRNumber ? `Force-pushed to PR #${viewPRNumber}` : 'Force-pushed updates to the PR')
      } else {
        pushToast('success', viewPRNumber ? `Pushed to PR #${viewPRNumber}` : 'Pushed updates to the PR')
      }
    } finally {
      pushingRemote = false
      refreshLandState()
    }
  }

  async function updateWorkspace() {
    if (landBusy) return
    const id = $selectedWorkspaceId
    if (!id) return
    updatingBase = true
    try {
      const res = await call(
        isMainMode
          ? WorkspacesService.UpdateWorkingCopyFromRemote(id, updateMethod)
          : WorkspacesService.UpdateWorkspaceWithBase(id, updateMethod),
        'Could not update',
      )
      if (!res) return
      if (res.workspace?.id) {
        workspaces.update((arr) => arr.map((l) => (l.id === res.workspace.id ? { ...l, ...res.workspace, behindBase: res.workspace.behindBase ?? 0 } : l)))
      }
      const base = (res.base || mergeBaseLabel).trim() || 'main'
      if (res.status === 'conflict') {
        requestPromptAction(id, 'update')
        return
      }
      if (res.status === 'updated') {
        if (res.pushedPR) {
          const verb = res.forcePushed ? 'force-pushed' : 'pushed'
          pushToast('success', `Updated with latest ${base} and ${verb} to PR #${res.pushedPR}`)
        } else {
          pushToast('success', `Updated with latest ${base}`)
        }
      } else if (res.status === 'autostash') {
        pushToast('error', `Updated with latest ${base}, but uncommitted edits could not be reapplied — they are in git stash`)
      } else if (res.status === 'noop') {
        pushToast('info', `Already up to date with ${base}`)
      }
    } finally {
      updatingBase = false
      refreshLandState()
    }
  }

  async function pushWorkingCopy() {
    if (landBusy) return
    const id = $selectedWorkspaceId
    if (!id) return
    pushingRemote = true
    try {
      const pushed = await call(WorkspacesService.PushWorkingCopy(id), 'Could not push')
      if (!pushed) return
      if (pushed.id) {
        workspaces.update((arr) => arr.map((l) => (l.id === pushed.id ? pushed : l)))
      }
      pushToast('success', `Pushed to ${land.remote || 'remote'}`)
    } finally {
      pushingRemote = false
      refreshLandState()
    }
  }

  async function removeSelected() {
    const id = $selectedWorkspaceId
    if (!id || !canRemove) return
    await removeWorkspace(id)
  }

  // D-attention: modals/dialogs covering the chat mean a finish on the selected
  // tab should still raise unseen / dock badge (same as OS window unfocused).
  // App-chrome stores OR component-registered occluders (lightbox, Quick Open, …).
  // ask_user no longer occludes: off-tab is a short toast; on-tab is inline in the
  // transcript. permissionRequests still float and cover the chat.
  $effect(() => {
    const occluded =
      $newTaskOpen ||
      $newProjectOpen ||
      !!$prReviewModal ||
      $paletteOpen ||
      $commandsOpen ||
      $openInOpen ||
      $runMenuOpen ||
      $stopMenuOpen ||
      $syntaxModeOpen ||
      $shortcutsOpen ||
      $permissionRequests.length > 0 ||
      !!$keybindingMenu ||
      $chatUIOccluded
    void WorkspacesService.SetChatUIOccluded(occluded).catch(() => {})
  })

  // Native menubar Appearance checks follow the theme stores. Owned here so
  // stores.ts does not import menu.ts (circular).
  $effect(() => {
    void $theme
    void $editorTheme
    void $terminalPrefs.theme
    pushMenuThemeState()
  })

  function onTitlebarMouseDown(e: MouseEvent) {
    // detail===2 is the second click of a double-click. Prefer mousedown over
    // dblclick: -webkit-app-region:drag / Wails' invisible titlebar often
    // swallow the latter.
    if (e.button !== 0 || e.detail !== 2) return
    if (!isTitlebarZoomTarget(e.target, e.clientY)) return
    e.preventDefault()
    void Events.Emit('window.titlebarDblclick').catch(() => {})
  }

  onMount(() => {
    // Discover bundle TextMate grammars/themes early so the first editor open can paint.
    void import('./lib/tm-bootstrap')
      .then((m) => m.ensureTextMateBootstrap())
      .then(() => {
        // Editors that mounted before the bundle themes registered re-apply
        // themselves off the colour-map change — a store set() cannot signal
        // it, since Svelte does not notify when the value is unchanged.
        pushMenuThemes()
      })
      .catch(() => {})
    void import('./lib/terminal').then((m) => m.ensureTerminalThemes()).then(() => pushMenuThemes()).catch(() => {})
    // First-run routing: a brand-new / cleared app (no projects) opens on the Welcome
    // screen. Otherwise restore the last workspace/view; persist starts after that
    // so the in-memory Briefing default cannot clobber a saved shell on the way up.
    Promise.all([loadInitial(), loadShellState(), loadScheduleExpanded()])
      .then(([, saved]) => {
        if (get(projects).length === 0) {
          go('welcome')
        } else {
          applyShellRestore(resolveShellRestore(saved, {
            workspaceIds: new Set(get(workspaces).filter((w) => !w.archived).map((w) => w.id)),
            prIds: new Set(get(prs).map((p) => p.id)),
            projectIds: new Set(get(projects).map((p) => p.id)),
          }), { openWorkspace, openPR, openProject, go })
        }
        enableShellPersist()
        void Events.Emit('app.ready').catch(() => {})
      })
      .catch((err: any) => {
        enableShellPersist()
        pushToast('error', `Could not load Recur: ${err?.message ?? String(err)}`)
        void Events.Emit('app.ready').catch(() => {})
      })
    wireEvents()
    wireFileIndexEvents()
    wireMenuEvents()
    wireUpdaterEvents()
    Events.On('app.open', (e: any) => {
      applyOpenPathResult(e?.data)
    })
    void maybeRunBridgeBench()
    void initZoom()
    const unsubMenu = onMenuAction((action) => {
      if (action.startsWith('app-theme:')) {
        setTheme(action.slice('app-theme:'.length))
        return
      }
      if (action.startsWith('editor-theme:')) {
        const key = action.slice('editor-theme:'.length)
        setEditorTheme(key)
        const name = allEditorThemesForPicker().find((t) => t.key === key)?.name ?? key
        toastEditorTheme(name)
        return
      }
      if (action.startsWith('terminal-theme:')) {
        setTerminalPrefs({ theme: terminalIdFromMenu(action.slice('terminal-theme:'.length)) })
        return
      }
      switch (action) {
        case 'open-path':
          void pickAndOpenPath()
          return
        case 'new-loop':
          openNewTask()
          return
        case 'new-blank-workspace':
          void createBlankWorkspace()
          return
        case 'palette':
          toggleGoPalette()
          return
        case 'run-command':
          toggleCommandsPalette()
          return
        case 'open-in':
          if (!wsMainBlocked(get(workspaces).find((w) => w.id === get(selectedWorkspaceId)))) toggleOpenIn()
          return
        case 'copy-path':
          if (!wsMainBlocked(get(workspaces).find((w) => w.id === get(selectedWorkspaceId)))) void copySelectedWorkspacePath()
          return
        case 'shortcuts':
          toggleShortcuts()
          return
        case 'prev-workspace':
          cycleWorkspace(-1)
          return
        case 'next-workspace':
          cycleWorkspace(1)
          return
        case 'prev-editor-theme':
          toastEditorTheme(cycleEditorTheme(-1).name)
          return
        case 'next-editor-theme':
          toastEditorTheme(cycleEditorTheme(1).name)
          return
        case 'toggle-sidebar':
          toggleRail()
          return
        case 'settings':
          go('settings')
          return
        case 'check-updates':
          checkForUpdates()
          return
        case 'reload-bundles':
          void import('./lib/bundle-reload')
            .then((m) => m.reloadBundles())
            .then((skipped) => {
              if (skipped) pushToast('info', `Bundles reloaded, ${skipped} skipped (see Settings → Bundles)`)
              else pushToast('success', 'Bundles reloaded')
            })
            .catch((err) => pushToast('error', `Reload bundles failed: ${err instanceof Error ? err.message : String(err)}`))
          return
        case 'zoom-in':
          zoomIn()
          return
        case 'zoom-out':
          zoomOut()
          return
        case 'zoom-reset':
          zoomReset()
          return
        case 'toggle-terminal': {
          const wsId = get(view) === 'detail' ? get(selectedWorkspaceId) : ''
          if (wsId && !wsMainBlocked(get(workspaces).find((w) => w.id === wsId))) toggleTerminalDock(wsId)
          return
        }
        case 'maximize-terminal': {
          const wsId = get(view) === 'detail' ? get(selectedWorkspaceId) : ''
          if (wsId && !wsMainBlocked(get(workspaces).find((w) => w.id === wsId))) toggleTerminalDockMaximized(wsId)
          return
        }
        case 'run-dev': {
          const wsId = get(view) === 'detail' ? get(selectedWorkspaceId) : ''
          if (wsId && !wsMainBlocked(get(workspaces).find((w) => w.id === wsId))) openRunMenu(wsId)
          return
        }
        case 'stop-run': {
          const wsId = get(view) === 'detail' ? get(selectedWorkspaceId) : ''
          if (wsId && !wsMainBlocked(get(workspaces).find((w) => w.id === wsId))) openStopMenu(wsId)
          return
        }
      }
    })
    const onKey = (e: KeyboardEvent) => {
      const zoom = zoomChordAction(e)
      if (zoom) {
        e.preventDefault()
        zoom()
        return
      }
      if (isCmdOrCtrl(e) && e.key.toLowerCase() === 'k' && !e.shiftKey) {
        e.preventDefault()
        toggleGoPalette()
        return
      }
      // ⌘⇧P — Run Command (Recur Bundles). ⌘P remains Quick Open in Observatory.
      if (isCmdOrCtrl(e) && e.shiftKey && !e.altKey && e.code === 'KeyP') {
        e.preventDefault()
        toggleCommandsPalette()
        return
      }
      // ⌘⇧/ (⌘?) — keyboard shortcuts list. Matched on e.code because Shift
      // turns / into ?, so e.key alone is unreliable. Plain ⌘/ stays editor
      // toggle-comment.
      if (isCmdOrCtrl(e) && e.shiftKey && !e.altKey && (e.code === 'Slash' || e.key === '/' || e.key === '?')) {
        e.preventDefault()
        toggleShortcuts()
        return
      }
      // ⌘N — New workspace modal. ⇧⌘N — blank Agent workspace (no modal).
      // ⌥⌘N is Explorer New File; ignore alt so we don't steal it.
      if (isCmdOrCtrl(e) && !e.altKey && e.key.toLowerCase() === 'n') {
        e.preventDefault()
        if (e.shiftKey) void createBlankWorkspace()
        else openNewTask()
        return
      }
      if (isCmdOrCtrl(e) && !e.shiftKey && e.altKey && isLetterKey(e, 'o')) {
        e.preventDefault()
        void pickAndOpenPath()
        return
      }
      if (isCmdOrCtrl(e) && e.key.toLowerCase() === 'o' && !e.shiftKey && !e.altKey) {
        e.preventDefault()
        toggleOpenIn()
        return
      }
      if (isCmdOrCtrl(e) && e.key.toLowerCase() === 'b') {
        e.preventDefault()
        toggleRail()
        return
      }
      if (isCmdOrCtrl(e) && !e.shiftKey && !e.altKey && e.code === 'Comma') {
        e.preventDefault()
        go('settings')
        return
      }
      // ⌥⌘↑ / ⌥⌘↓ — previous / next workspace in sidebar order (wraps).
      // Same modifiers as tab cycling; vertical because the tree is vertical.
      // CmdOrCtrl (⌘ on Mac, Ctrl elsewhere) — not ctrl-as-cmd. Add-cursor is
      // ⌃⇧↑/↓ so it does not collide with this chord or VoiceOver.
      if (isCmdOrCtrl(e) && e.altKey && !e.shiftKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
        e.preventDefault()
        cycleWorkspace(e.key === 'ArrowDown' ? 1 : -1)
        return
      }
      // ⌥⌘[ / ⌥⌘] — previous / next code editor theme. Dark A–Z (Charcoal
      // first), then light A–Z (Paper first); wraps. Matched on e.code because
      // Option+[ is a dead-key / brace on some layouts.
      if (isCmdOrCtrl(e) && e.altKey && !e.shiftKey && (e.code === 'BracketLeft' || e.code === 'BracketRight')) {
        e.preventDefault()
        toastEditorTheme(cycleEditorTheme(e.code === 'BracketRight' ? 1 : -1).name)
        return
      }
      // ⌘R — open the Run palette. ⌘⇧R stops the only running command, or opens
      // the Stop menu when several are going. ⌥⌘R is toggle review / regexp /
      // Reveal in Finder (Observatory owns that chord).
      if (isCmdOrCtrl(e) && !e.altKey && e.code === 'KeyR') {
        const wsId = get(view) === 'detail' ? get(selectedWorkspaceId) : ''
        if (!wsId || !workspaceDevCmd(wsId)) return
        e.preventDefault()
        if (wsMainBlocked(get(workspaces).find((w) => w.id === wsId))) return
        if (e.shiftKey) openStopMenu(wsId)
        else openRunMenu(wsId)
        return
      }
      // ⌃` toggles the Observatory's terminal drawer, ⌃⇧` maximizes it to the full
      // editor column. Ctrl-only (not ⌘) to match every other editor. Matched on
      // e.code because Shift turns ` into ~, so e.key alone would miss the second
      // binding. TerminalPane declines both combos so they reach here instead of
      // being swallowed by the focused shell.
      if (e.ctrlKey && !e.metaKey && !e.altKey && isBackquote(e)) {
        // The dock is per-workspace, so the binding only means something while a
        // workspace is open — the toggles no-op on an empty id.
        const wsId = get(view) === 'detail' ? get(selectedWorkspaceId) : ''
        if (!wsId) return
        if (wsMainBlocked(get(workspaces).find((w) => w.id === wsId))) return
        e.preventDefault()
        if (e.shiftKey) toggleTerminalDockMaximized(wsId)
        else toggleTerminalDock(wsId)
        return
      }
      // Esc-to-back (Phase 4.5): skip while the palette / shortcuts / new-loop
      // modal is open (they handle their own Escape) or while typing in a field,
      // so Esc never eats in-progress input.
      if (e.key !== 'Escape' || e.defaultPrevented || get(paletteOpen) || get(commandsOpen) || get(openInOpen) || get(runMenuOpen) || get(stopMenuOpen) || get(syntaxModeOpen) || get(shortcutsOpen) || get(newTaskOpen) || get(newProjectOpen) || get(prReviewModal)) return
      const t = e.target as HTMLElement | null
      const tag = t?.tagName
      // Contenteditable composer (inline @ chips) counts as typing too.
      if (tag === 'TEXTAREA' || tag === 'INPUT' || tag === 'SELECT' || t?.isContentEditable) return
      // Close the collapsed-rail flyout without swallowing Escape for prompts
      // (PermissionPrompts / AgentChat) and without leaving the current view.
      if (get(railPeek)) {
        closeRailPeek()
        return
      }
      switch (get(view)) {
        case 'detail':
        case 'settings':
        case 'welcome':
          go('briefing')
          break
        case 'prdetail':
          go('pr')
          break
        case 'project':
          go(get(projects).length ? 'briefing' : 'welcome')
          break
      }
    }
    window.addEventListener('keydown', onKey)
    // Capture phase and non-passive so CodeMirror and xterm cannot swallow it.
    // Throttled so one trackpad gesture does not jump several steps.
    let lastWheelZoom = 0
    const onWheel = (e: WheelEvent) => {
      if (!isCmdOrCtrl(e) || e.deltaY === 0) return
      e.preventDefault()
      if (e.timeStamp - lastWheelZoom < 120) return
      lastWheelZoom = e.timeStamp
      if (e.deltaY < 0) zoomIn()
      else zoomOut()
    }
    window.addEventListener('wheel', onWheel, { capture: true, passive: false })
    return () => {
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('wheel', onWheel, { capture: true })
      unsubMenu()
    }
  })
</script>

<div class="win">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="titlebar" onmousedown={onTitlebarMouseDown}>
    <div class="tb-left">
      {#if trafficClear}<div class="tb-traffic-clear"></div>{/if}
      <button
        type="button"
        class="tb-rail"
        class:on={!$railCollapsed}
        onclick={toggleRail}
        title={$railCollapsed ? 'Show sidebar (⌘B)' : 'Hide sidebar (⌘B)'}
        aria-label={$railCollapsed ? 'Show sidebar' : 'Hide sidebar'}
        aria-pressed={!$railCollapsed}
      >
        <Icon name="panel-left" />
      </button>
      <div class="tb-div"></div>
      {#if showLoopBack}
        <button class="tb-back" onclick={() => go('briefing')}>← inbox</button>
        {#if selectedLoop}
          <!-- Live name (DecodeText) — crumb HTML is only refreshed on navigate.
               Loop / run-active workspaces use the same name [branch] as agent/ask. -->
          <span class="crumb">
            / <b>{selectedLoop.projectId}</b> /
            <span class="on"><DecodeText text={selectedLoop.short || selectedLoop.branch || selectedLoop.id} sourceKey={selectedLoop.id} /></span>
            {#if selectedLoop.branch}
              {' '}<span class="crumb-branch">{selectedLoop.branch}</span>
            {/if}
          </span>
        {:else}
          <span class="crumb">/ {@html $crumb}</span>
        {/if}
        {#if selectedLoop && !selectedLoop.isMain}
          <span class="crumb-arrow" aria-hidden="true">→</span>
          <WorkspaceBaseSelect workspace={selectedLoop} />
        {/if}
      {:else if showPRBack}
        <button class="tb-back" onclick={() => go('pr')}>← pull requests</button>
        <span class="crumb">/ <b class="on">#{titlebarPR?.number ?? $selectedPRId}</b></span>
      {:else}
        {#if $view === 'project'}
          <div class="crumb">
            {#if titlebarProject}
              <b>Project</b> / <span class="on">{titlebarProject.name}</span>
            {:else}
              <b class="on">New project</b>
            {/if}
          </div>
        {:else}
          <div class="crumb">{@html $crumb}</div>
        {/if}
        {#if $view === 'pr'}
          <PRListTitlebar part="counts" />
        {/if}
        {#if $view === 'briefing'}
          <InboxTitlebar part="counts" />
        {/if}
        {#if $view === 'schedules'}
          <SchedulesTitlebar part="counts" />
        {/if}
      {/if}
    </div>
    {#if $instanceLabel}
      <span
        class="tb-instance"
        title={`Dev instance ${$instanceLabel}`}
        aria-label={`Dev instance ${$instanceLabel}`}
      ><span class="tb-instance-k">dev</span><span class="tb-instance-v">{$instanceLabel}</span></span>
    {/if}
    <div class="tb-right">
      {#if $view === 'briefing'}
        <InboxTitlebar part="actions" />
      {/if}
      {#if showLoopBack && selectedLoop && !wsMainBlocked(selectedLoop)}
        <button
          type="button"
          class="tb-icon-btn tb-open-in"
          class:on={$openInOpen}
          onclick={toggleOpenIn}
          title="Open in… (⌘O)"
          aria-label="Open in"
          aria-haspopup="dialog"
          aria-expanded={$openInOpen}
        >
          <Icon name="queue-pop-out" />
        </button>
        <button
          type="button"
          class="tb-icon-btn tb-copy-path"
          onclick={() => void copySelectedWorkspacePath()}
          title="Copy path"
          aria-label="Copy path"
        >
          <Icon name="copy" />
        </button>
      {/if}
      {#if canRunDev}
        <div class="run-split" class:menu-open={$runMenuOpen}>
          <button
            class="tbtn"
            title={`${titlebarRunLabel} (click) · picker (⌘R)`}
            onclick={() => { runMenuOpen.set(false); requestRunDev($selectedWorkspaceId) }}
          >{titlebarRunLabel}</button>
          <button
            class="tbtn run-chev"
            class:on={$runMenuOpen}
            aria-label="Choose run command"
            aria-haspopup="dialog"
            aria-expanded={$runMenuOpen}
            onclick={() => { openRunMenu($selectedWorkspaceId); mergeMenuOpen = false; updateMenuOpen = false }}
          ><Icon name="chevron-down" /></button>
        </div>
        {#if anyRunRunning}
          <div class="run-split stop-split" class:menu-open={$stopMenuOpen}>
            <button
              class="tbtn"
              class:on={true}
              title={runningChoices.length === 1 ? `${titlebarStopLabel} (⌘⇧R)` : 'Stop (⌘⇧R)'}
              onclick={() => {
                if (runningChoices.length === 1) requestStopRun($selectedWorkspaceId, runningChoices[0]!.name)
                else openStopMenu($selectedWorkspaceId)
              }}
            >{titlebarStopLabel}</button>
            {#if runningChoices.length > 1}
              <button
                class="tbtn run-chev"
                class:on={true}
                aria-label="Choose process to stop"
                aria-haspopup="dialog"
                aria-expanded={$stopMenuOpen}
                onclick={() => { openStopMenu($selectedWorkspaceId); mergeMenuOpen = false; updateMenuOpen = false; openPRMenuOpen = false }}
              ><Icon name="chevron-down" /></button>
            {/if}
          </div>
        {/if}
      {/if}
      {#if isWorkspaceMode || isMainMode}
        {#if landOn}
          {#if showCommit}
            <button
              class="tbtn solid"
              disabled={landBusy}
              title={landBusyTitle || 'Commit uncommitted changes'}
              onclick={requestCommit}
            ><Icon name="git-commit" />Commit</button>
          {/if}
          {#if showUpdate}
            {@const remoteBase = isMainMode ? (land.base || 'remote') : mergeBaseLabel}
            <div class="update-split" class:menu-open={updateMenuOpen}>
              <button
                class="tbtn solid"
                disabled={landBusy}
                title={landBusyTitle || (land.unmerged ? `A merge or rebase is already in progress — finish or abort it before updating` : updateMethod === 'rebase' ? `Rebase onto latest ${remoteBase}` : `Update with latest ${remoteBase}`)}
                onclick={updateWorkspace}
              ><Icon name="arrow-up" class={updatingBase ? 'spin' : ''} />{updatingBase ? 'Updating…' : updateLabel}</button>
              <button
                class="tbtn solid update-chev"
                disabled={landBusy}
                aria-label="Choose update method"
                onclick={() => { updateMenuOpen = !updateMenuOpen; mergeMenuOpen = false; openPRMenuOpen = false; runMenuOpen.set(false); stopMenuOpen.set(false) }}
              ><Icon name="chevron-down" /></button>
              {#if updateMenuOpen}
                <div class="merge-menu" role="menu">
                  {#each updateMethodKeys as m}
                    <button class="merge-menu-item" class:on={updateMethod === m} role="menuitem" onclick={() => pickUpdateMethod(m)}><span class="mm-check">{updateMethod === m ? '✓' : ''}</span>{updateMethodMenuLabels[m]}</button>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
          {#if showPush}
            <button
              class="rbtn merge"
              disabled={landBusy}
              title={landBusyTitle || `Push to ${land.remote}`}
              onclick={pushWorkingCopy}
            ><Icon name="upload" />{pushingRemote ? 'Pushing…' : `Push to ${land.remote}`}</button>
          {/if}
          {#if showMerge}
            <div class="merge-split" class:menu-open={mergeMenuOpen}>
              <button
                class="rbtn merge"
                disabled={landBusy}
                title={landBusyTitle || mergeHint}
                onclick={mergeWorkspace}
              ><Icon name="git-pull-request" />{confirmingMerge ? `Confirm ${mergeLabel.toLowerCase()}` : mergeLabel}</button>
              <button
                class="rbtn merge-chev"
                disabled={landBusy}
                aria-label="Choose merge method"
                onclick={() => { mergeMenuOpen = !mergeMenuOpen; openPRMenuOpen = false; confirmingMerge = false; runMenuOpen.set(false); stopMenuOpen.set(false) }}
              ><Icon name="chevron-down" /></button>
              {#if mergeMenuOpen}
                <div class="merge-menu" role="menu">
                  {#each mergeMethodKeys as m}
                    <button class="merge-menu-item" class:on={mergeMethod === m} role="menuitem" onclick={() => pickMergeMethod(m)}><span class="mm-check">{mergeMethod === m ? '✓' : ''}</span>{mergeMethodLabels[m]}</button>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
          {#if isWorkspaceMode && githubPRs && githubPROpen}
              {#if canPullPR}
                <button
                  class="tbtn solid"
                  disabled={landBusy}
                  title={landBusyTitle || 'Bring GitHub commits into this workspace'}
                  onclick={() => void pullWorkspacePR()}
                ><Icon name="arrow-up" />{pullingPR ? 'Pulling…' : 'Pull updates'}</button>
              {/if}
              {#if canPushPR}
                <button
                  class="tbtn solid"
                  disabled={landBusy}
                  title={landBusyTitle || 'Push new commits to the open pull request'}
                  onclick={() => void pushWorkspacePR()}
                ><Icon name="upload" />{pushingRemote ? 'Pushing…' : 'Push updates'}</button>
              {/if}
              {#if canForcePushPR}
                <button
                  class="tbtn solid"
                  disabled={landBusy}
                  title={landBusyTitle || 'Overwrite the GitHub head with this workspace (e.g. after rebase onto main). Remote-only commits will be dropped.'}
                  onclick={() => void pushWorkspacePR(true)}
                ><Icon name="upload" />{pushingRemote ? 'Force-pushing…' : 'Force push'}</button>
              {/if}
          {/if}
        {/if}
      {/if}
      {#if showOpenGitHubPR}
              <div class="update-split" class:menu-open={openPRMenuOpen}>
                <button
                  class="tbtn"
                  disabled={landBusy || agentBusy}
                  title={landBusyTitle || (agentBusy ? 'Wait until the agent finishes' : 'Open a GitHub pull request')}
                  onclick={() => requestOpenPR()}
                >Open PR</button>
                <button
                  class="tbtn update-chev"
                  disabled={landBusy || agentBusy}
                  aria-label="Open as draft"
                  onclick={() => { openPRMenuOpen = !openPRMenuOpen; mergeMenuOpen = false; updateMenuOpen = false; runMenuOpen.set(false); stopMenuOpen.set(false) }}
                ><Icon name="chevron-down" /></button>
                {#if openPRMenuOpen}
                  <div class="merge-menu" role="menu">
                    <button class="merge-menu-item" class:on={true} role="menuitem" onclick={() => requestOpenPR()}>Open PR</button>
                    <button class="merge-menu-item" role="menuitem" onclick={() => requestOpenPR(true)}>Open draft PR</button>
                  </div>
                {/if}
              </div>
      {/if}
      {#if $view === 'detail' && selectedLoop && !selectedLoop.isMain}
        <WorkspaceIssueButton workspace={selectedLoop} />
      {/if}
      {#if showViewWorkspacePR}
        <button
          class="tbtn"
          title="View this workspace's pull request"
          onclick={viewWorkspacePR}
        ><Icon name={prIconName({ ...storePR, draft: viewPRDraft })} />{viewPRNumber ? (viewPRDraft ? `View draft #${viewPRNumber}` : `View PR #${viewPRNumber}`) : (viewPRDraft ? 'View draft' : 'View PR')}</button>
      {/if}
      {#if showPRBack}
        {#if titlebarPR?.workspaceId}
          <button
            type="button"
            class="tbtn"
            title="View this pull request's workspace"
            onclick={() => { const id = titlebarPR.workspaceId; if (id) openWorkspace(id) }}
          ><Icon name="box" />View workspace</button>
        {:else if titlebarPR && prLifecycle(titlebarPR) !== 'merged' && prLifecycle(titlebarPR) !== 'closed'}
          <button
            type="button"
            class="tbtn"
            title="Create a Recur workspace on this pull request"
            disabled={creatingThisPR}
            onclick={() => void createPRWorkspace()}
          ><Icon name="plus" />{creatingThisPR ? 'Creating…' : 'Create workspace'}</button>
        {/if}
        <PRTitlebarActions />
      {/if}
      {#if showRemove}
        <ArmedButton
          label="Remove"
          confirmLabel={removeConfirmLabel}
          disabled={!canRemove}
          title={canRemove ? undefined : 'Stop the workspace first'}
          onarm={refreshDiscardRisk}
          onconfirm={removeSelected}
        />
      {/if}
    </div>
  </div>

  <div class="body" class:rail-collapsed={$railCollapsed}>
    <Sidebar />
    <main class="main">
      <WorkspaceDetail />
      <Settings />
      <Project />
      <PRList />
      {#if $view === 'issues'}
        <section class="view on briefing" id="v-issues" aria-label="Issues">
          <Issues />
        </section>
      {/if}
      {#if $view === 'schedules'}
        <section class="view on briefing" id="v-schedules" aria-label="Schedules">
          <ScheduledPrompts />
        </section>
      {/if}
      <PRDetail />
      <Briefing />
      <Welcome />
    </main>
  </div>
</div>

<NewTask />
<ScheduleEditor />
<PRReviewModal />
<EmojiSuggest />
<UserMentionSuggest />
<NewProject />
<CommandPalette />
<RunCommandPalette />
<OpenInPalette />
<RunPalette />
<StopPalette />
<SyntaxPalette />
<KeybindingMenu />
<ShortcutsHelp />
<Toasts />
<TitleTooltip />
<PermissionPrompts />
<QuestionPrompts />
