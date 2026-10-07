<script lang="ts">
  // The Observatory's bottom terminal drawer: a tab strip over one TerminalPane
  // per live shell.
  //
  // It sits under the editor rather than in the movable panel rail because a
  // terminal needs columns — the side docks are ~260px, which is a third of the
  // 80 a shell expects. Sharing the editor's full width is also what every other
  // tool does, so ⌃` lands where muscle memory says it should.
  //
  // Shells belong to the workspace, not to this component: mounting adopts the
  // sessions already running (ListTerminals) instead of spawning new ones, so
  // closing the drawer and reopening it — or leaving and coming back — picks up
  // exactly where it left off. After an app restart there are no live PTYs;
  // RestoreWorkspaceTerminals respawns the last tabs (cwd, title, scrollback).
  import { onMount, onDestroy, tick } from 'svelte'
  import TerminalPane from './TerminalPane.svelte'
  import TabStrip from './TabStrip.svelte'
  import TabChip from './TabChip.svelte'
  import Icon from './Icon.svelte'
  import ResizeHandle from './ResizeHandle.svelte'
  import { call, callOk, bridgeErrorMessage } from '../lib/api'
  import { TabStripDrag } from '../lib/tab-strip-drag.svelte'
  import { isCmdOrCtrl, isLetterKey } from '../lib/shortcut'
  import { PREVIEW_FIND_CAP, previewFindCountLabel } from '../lib/preview-find'
  import {
    setTerminalDockOpen, terminalDockMaximizedBy, setTerminalDockMaximized,
    projects, workspaces, workspaceTerminals, terminalTabsEqual, terminalFocus,
    pushToast,
    terminalPrefs, theme as appTheme,
  } from '../lib/stores'
  import { terminalSurfaceBg, terminalFrosts, terminalThemeCatalog } from '../lib/terminal'
  import {
    runDevRequest, takeRunDevRequest, requestStopRun, waitForSessionStop,
    workspaceDevCmd, workspaceRun, workspaceRuns, runSessionBy, runTabBy,
    setRunSession, setRunTab, clearRunSession, clearRunTabBySessionId,
    isRunRunning, runTabId, runNameForSession, markRunStarting, markRunStarted,
    armRunBusyWatch,
  } from '../lib/run-dev'
  import { runLabel } from '../lib/run-commands'
  import {
    OpenTerminal, ListTerminals, CloseTerminal, WriteTerminal, InterruptTerminal,
    SetTerminalTitle, SetActiveTerminal, RestoreWorkspaceTerminals, ReorderTerminals, DetachTerminal,
  } from '../../bindings/recur/internal/app/terminalservice'
  import { discardParkedTerminal, releaseParkedTerminal } from '../lib/caches/term-park'

  let {
    workspaceId,
    lit = false,
    overlay = false,
    beside = false,
    gpu = true,
    onTabsChange,
  }: {
    workspaceId: string
    lit?: boolean
    /** True while the dock is FLIPped over the Observatory — panes frost through. */
    overlay?: boolean
    /** Editor and terminal share a row instead of stacking. */
    beside?: boolean
    /** Parked hot shells must not hold a WebGL context (WKWebView cap). */
    gpu?: boolean
    /** Tab count for menubar enable/disable (Previous/Next Tab, …). */
    onTabsChange?: (count: number) => void
  } = $props()

  const maximized = $derived(!!$terminalDockMaximizedBy[workspaceId])
  const termBg = $derived.by(() => {
    void $terminalThemeCatalog
    void $appTheme
    return terminalSurfaceBg($terminalPrefs.theme)
  })
  const frosts = $derived.by(() => {
    void $terminalThemeCatalog
    void $appTheme
    return terminalFrosts($terminalPrefs.theme)
  })

  type Tab = { id: string; title: string; exited: boolean; busy?: boolean }

  let tabs = $state<Tab[]>([])
  let activeId = $state('')
  let loading = $state(true)
  let termTabStripEl = $state<HTMLDivElement | undefined>(undefined)
  let termTabFloatEl = $state<HTMLDivElement | undefined>(undefined)
  let termDragOwnerId = ''
  const termTabDrag = new TabStripDrag<Tab>({
    getStrip: () => termTabStripEl,
    getFloat: () => termTabFloatEl,
    getItems: () => tabs,
    setItems: (next) => { tabs = next },
    keyOf: (t) => t.id,
    canStart: () => tabs.length > 1,
    bindOwner: () => {
      const id = workspaceId
      termDragOwnerId = id
      return {
        getItems: () => tabs,
        setItems: (next) => { tabs = next },
      }
    },
    onCommit: async (current, snapshot) => {
      const id = termDragOwnerId
      if (!id) return
      const ok = await callOk(
        ReorderTerminals(id, current.map((t) => t.id)),
        'Could not reorder terminals',
      )
      if (!ok) termTabDrag.rollback(snapshot)
    },
  })
  $effect(() => {
    workspaceId
    termTabDrag.cancel()
  })
  onDestroy(() => termTabDrag.cancel())

  $effect(() => {
    onTabsChange?.(tabs.length)
  })

  $effect(() => {
    const list = tabs.map((t) => ({
      id: t.id, title: t.title, exited: t.exited, active: t.id === activeId,
    }))
    workspaceTerminals.update((m) => {
      const prev = m[workspaceId] ?? []
      if (terminalTabsEqual(prev, list)) return m
      return { ...m, [workspaceId]: list }
    })
  })

  $effect(() => {
    const req = $terminalFocus
    if (!req || req.workspaceId !== workspaceId || !req.id) return
    if (!tabs.some((t) => t.id === req.id)) return
    setTerminalDockOpen(workspaceId, true)
    selectTab(req.id)
    terminalFocus.set(null)
  })
  // $state so `bind:this` into a keyed property is tracked — a plain object leaves
  // the Clear / focus actions reading a stale ref after a tab is added or removed.
  let panes = $state<Record<string, TerminalPane | undefined>>({})
  let findOpen = $state(false)
  let findQuery = $state('')
  let findCase = $state(false)
  let findRegexp = $state(false)
  let findWord = $state(false)
  let findTotal = $state(0)
  let findIndex = $state(0)
  let findCapped = $state(false)
  let findValid = $state(true)
  let findInput: HTMLInputElement | undefined = $state()
  let searchedId = ''
  const findCount = $derived(
    !findQuery || !findValid ? '' : previewFindCountLabel(findIndex, findTotal, findCapped),
  )

  // Sizing the first PTY from the dock's real geometry avoids the shell printing
  // its first prompt at 80x24 and then reflowing when the pane measures itself.
  let dockEl: HTMLElement | undefined
  let dockBody: HTMLDivElement | undefined
  function guessSize(): { cols: number; rows: number } {
    const w = dockBody?.clientWidth ?? 0
    const h = dockBody?.clientHeight ?? 0
    // Approximate cell metrics — the pane pushes exact ones the moment it fits.
    const cols = w > 0 ? Math.max(20, Math.floor((w - 16) / 8)) : 80
    const rows = h > 0 ? Math.max(4, Math.floor((h - 8) / 17)) : 24
    return { cols, rows }
  }

  function adopt(list: { id: string; title?: string; exited?: boolean; active?: boolean; busy?: boolean }[]) {
    tabs = list.map((t) => ({
      id: t.id, title: t.title || 'zsh', exited: !!t.exited, busy: !!t.busy,
    }))
    const focused = list.find((t) => t.active && !t.exited)
    const live = tabs.find((t) => !t.exited)
    activeId = focused?.id || (live ?? tabs[0]).id
    void SetActiveTerminal(workspaceId, activeId).catch(() => {})
  }

  async function spawn(title?: string, opts?: { takeFocus?: boolean }): Promise<string | null> {
    // Hold keyboard focus on the dock while the new pane mounts — otherwise
    // hiding the previous pane blurs its xterm textarea and ⌘W (and a quick
    // second ⌘T) would miss the dock listener until the new shell is ready.
    // Skip on the onMount fallback: a workspace-switch remount that finds no
    // shells must not park keys here, or the pane's dock-owns-focus gate fires.
    if (opts?.takeFocus !== false) dockEl?.focus({ preventScroll: true })
    const { cols, rows } = guessSize()
    const info = await call(OpenTerminal(workspaceId, cols, rows), 'Failed to open terminal')
    if (!info) return null
    tabs = [...tabs, { id: info.id, title: title || info.title || 'zsh', exited: false }]
    activeId = info.id
    void SetActiveTerminal(workspaceId, info.id).catch(() => {})
    return info.id
  }

  const hasRuns = $derived(!!workspaceDevCmd(workspaceId, $workspaces, $projects))
  const dockRun = $derived.by(() => {
    const fromTab = runNameForSession(workspaceId, activeId, $runSessionBy, $runTabBy)
    const name = fromTab || workspaceRun(workspaceId, $workspaces, $projects)?.name || 'dev'
    const running = isRunRunning(workspaceId, name, $runSessionBy)
    return { name, running }
  })
  const dockRunTitle = $derived(dockRun.running
    ? `${runLabel('Stop', dockRun.name)} (⌘⇧R)`
    : `${runLabel('Run', dockRun.name)} (⌘R)`)
  const dockRunAria = $derived(dockRun.running
    ? runLabel('Stop', dockRun.name)
    : runLabel('Run', dockRun.name))

  async function dropRunTab(id: string) {
    clearRunTabBySessionId(workspaceId, id)
    const leftover = discardParkedTerminal(id)
    if (leftover) {
      releaseParkedTerminal(leftover)
      DetachTerminal(id).catch(() => {})
    }
    const idx = tabs.findIndex((t) => t.id === id)
    tabs = tabs.filter((t) => t.id !== id)
    delete panes[id]
    if (activeId === id) {
      const next = tabs[Math.max(0, idx - 1)] ?? tabs[0]
      activeId = next?.id ?? ''
      void SetActiveTerminal(workspaceId, activeId).catch(() => {})
    }
    await callOk(CloseTerminal(id), 'Failed to close terminal')
  }

  /** Type the command into a dedicated login shell; the tab stays usable after it finishes. */
  async function spawnAndRunDev(cmd: string, name = 'dev') {
    const prevId = runTabId(workspaceId, name)
    const prev = tabs.find((t) => t.id === prevId)
    let id = ''
    if (prev && !prev.exited) {
      id = prev.id
      selectTab(id)
    } else {
      if (prev?.exited) await dropRunTab(prev.id)
      const spawned = await spawn(name)
      if (!spawned) return
      id = spawned
      setRunTab(workspaceId, name, id)
      await new Promise((r) => setTimeout(r, 450))
      if (!tabs.some((t) => t.id === id && !t.exited)) return
    }
    markRunStarting(id)
    try {
      await waitForSessionStop(id)
      try {
        await InterruptTerminal(id)
      } catch (err) {
        pushToast('error', `Could not stop the running process: ${bridgeErrorMessage(err)}`)
        return
      }
      await new Promise((r) => setTimeout(r, 50))
      setRunSession(workspaceId, name, id)
      const line = cmd.endsWith('\n') ? cmd : `${cmd}\n`
      if (!(await callOk(WriteTerminal(id, line), 'Failed to start run'))) {
        clearRunSession(workspaceId, name, id)
        return
      }
      armRunBusyWatch(id)
    } finally {
      window.setTimeout(() => markRunStarted(id), 80)
    }
    panes[id]?.focus()
  }

  // Titlebar / palette "Run dev" → open dock (if needed) and launch dev_cmd.
  $effect(() => {
    const req = $runDevRequest
    if (!req || req.workspaceId !== workspaceId) return
    void (async () => {
      // Wait for adopt/spawn on mount so we don't race the initial shell.
      for (let i = 0; i < 40 && loading; i++) {
        await new Promise((r) => setTimeout(r, 25))
      }
      const req = takeRunDevRequest(workspaceId)
      if (!req) return
      await spawnAndRunDev(req.cmd, req.name)
    })()
  })

  function runOrStopFromDock() {
    const name = dockRun.name
    if (dockRun.running) {
      requestStopRun(workspaceId, name)
      return
    }
    const run = workspaceRuns(workspaceId, $workspaces, $projects).find((r) => r.name === name)
      ?? workspaceRun(workspaceId, $workspaces, $projects)
    if (!run) return
    void spawnAndRunDev(run.cmd, run.name)
  }

  async function closeTab(id: string) {
    const leftover = discardParkedTerminal(id)
    if (leftover) {
      releaseParkedTerminal(leftover)
      DetachTerminal(id).catch(() => {})
    }
    const wasActive = activeId === id
    const idx = tabs.findIndex((t) => t.id === id)
    clearRunSession(workspaceId, runNameForSession(workspaceId, id) || '', id)
    clearRunTabBySessionId(workspaceId, id)
    // Same blur race as spawn: removing the focused pane parks focus on <body>
    // until the next pane's $effect runs — hold it on the dock so a rapid second
    // ⌘W / ⌘T still hits our listener.
    if (tabs.length > 1) dockEl?.focus({ preventScroll: true })
    tabs = tabs.filter((t) => t.id !== id)
    delete panes[id]
    if (wasActive) {
      const next = tabs[Math.min(idx, tabs.length - 1)]?.id ?? ''
      if (next) selectTab(next)
      else {
        activeId = ''
        void SetActiveTerminal(workspaceId, '').catch(() => {})
      }
    }
    await callOk(CloseTerminal(id), 'Failed to close terminal')
    // An empty dock has nothing to show, so closing the last tab closes the drawer
    // rather than leaving a blank strip behind.
    if (tabs.length === 0) setTerminalDockOpen(workspaceId, false)
  }

  onMount(async () => {
    const existing = await call(ListTerminals(workspaceId), 'Failed to list terminals')
    const usable = (existing ?? []).filter((t) => !!t?.id)
    if (usable.length) {
      adopt(usable)
    } else {
      await tick()
      const { cols, rows } = guessSize()
      const restored = await call(RestoreWorkspaceTerminals(workspaceId, cols, rows), 'Failed to restore terminals')
      const restoredUsable = (restored ?? []).filter((t) => !!t?.id)
      if (restoredUsable.length) adopt(restoredUsable)
      else await spawn(undefined, { takeFocus: false })
    }
    const names = new Set(workspaceRuns(workspaceId, $workspaces, $projects).map((r) => r.name))
    const byName = new Map<string, Tab[]>()
    for (const t of tabs) {
      if (!names.has(t.title)) continue
      const list = byName.get(t.title) ?? []
      list.push(t)
      byName.set(t.title, list)
    }
    for (const [name, matches] of byName) {
      if (matches.length !== 1) continue
      const t = matches[0]!
      setRunTab(workspaceId, name, t.id)
      if (t.busy && !t.exited) setRunSession(workspaceId, name, t.id)
    }
    loading = false
  })

  // ⌘T / ⌘W / ⌥⌘←→ match every other Mac terminal: new tab, close active tab,
  // and rotate through tabs — but only while focus is inside the dock, otherwise
  // they'd steal the bindings from the rest of the app. Capture phase so we run
  // before xterm's own capture listener, which would otherwise swallow the ⌃
  // variants (⌘ combos are already declined by TerminalPane).
  function selectTab(id: string) {
    activeId = id
    panes[id]?.focus()
    void SetActiveTerminal(workspaceId, id).catch(() => {})
  }

  function onFindResults(index: number, total: number, valid: boolean) {
    findValid = valid
    findIndex = index
    findTotal = total
    findCapped = total >= PREVIEW_FIND_CAP
  }

  async function openFind() {
    findOpen = true
    await tick()
    findInput?.focus({ preventScroll: true })
    findInput?.select()
  }

  function closeFind() {
    if (!findOpen) return
    findOpen = false
    panes[searchedId]?.clearSearch()
    searchedId = ''
    findTotal = 0
    findIndex = 0
    findCapped = false
    findValid = true
    panes[activeId]?.focus()
  }

  function stepFind(dir: 1 | -1) {
    if (!findOpen) return
    const pane = panes[activeId]
    if (!pane || !findQuery) return
    if (dir > 0) pane.findNext(findQuery, { caseSensitive: findCase, regexp: findRegexp, wholeWord: findWord })
    else pane.findPrevious(findQuery, { caseSensitive: findCase, regexp: findRegexp, wholeWord: findWord })
  }

  $effect(() => {
    void findQuery
    void findCase
    void findRegexp
    void findWord
    void findOpen
    const pane = panes[activeId]
    if (!findOpen) return
    if (searchedId !== activeId) panes[searchedId]?.clearSearch()
    searchedId = activeId
    if (!pane || !findQuery) {
      findTotal = 0
      findIndex = 0
      findCapped = false
      findValid = true
      pane?.clearSearch()
      return
    }
    pane.findNext(findQuery, {
      caseSensitive: findCase, regexp: findRegexp, wholeWord: findWord, incremental: true,
    })
  })

  function onFindInputKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault()
      stepFind(e.shiftKey ? -1 : 1)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      closeFind()
    }
  }

  function rotateTab(dir: -1 | 1) {
    if (tabs.length < 2) return
    const i = tabs.findIndex((t) => t.id === activeId)
    const next = tabs[(i < 0 ? 0 : i + dir + tabs.length) % tabs.length]
    selectTab(next.id)
  }

  function onDockKey(e: KeyboardEvent) {
    const mod = isCmdOrCtrl(e)
    const t = e.target instanceof HTMLElement ? e.target : null
    const inFind = !!t?.closest('.cm-recur-search')

    if (e.key === 'Escape' && findOpen) {
      e.preventDefault()
      e.stopPropagation()
      closeFind()
      return
    }
    if (mod && !e.altKey && !e.shiftKey && isLetterKey(e, 'f')) {
      e.preventDefault()
      e.stopPropagation()
      void openFind()
      return
    }
    if (findOpen && mod && !e.altKey && isLetterKey(e, 'g')) {
      e.preventDefault()
      e.stopPropagation()
      stepFind(e.shiftKey ? -1 : 1)
      return
    }
    if (inFind && mod && e.altKey && !e.shiftKey && isLetterKey(e, 'c')) {
      e.preventDefault()
      findCase = !findCase
      return
    }
    if (inFind && mod && e.altKey && !e.shiftKey && isLetterKey(e, 'r')) {
      e.preventDefault()
      findRegexp = !findRegexp
      return
    }
    if (inFind && mod && e.altKey && !e.shiftKey && isLetterKey(e, 'w')) {
      e.preventDefault()
      findWord = !findWord
      return
    }

    if (!mod || e.shiftKey) return
    const key = e.key.toLowerCase()

    // ⌥⌘← / ⌥⌘→ — previous / next tab (wraps).
    if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      e.preventDefault()
      e.stopPropagation()
      rotateTab(e.key === 'ArrowLeft' ? -1 : 1)
      return
    }
    if (e.altKey) return

    if (key === 't') {
      e.preventDefault()
      e.stopPropagation()
      void spawn()
      return
    }
    if (key === 'w' && activeId) {
      e.preventDefault()
      e.stopPropagation()
      void closeTab(activeId)
    }
  }

  onMount(() => {
    const el = dockEl
    if (!el) return
    el.addEventListener('keydown', onDockKey, true)
    return () => el.removeEventListener('keydown', onDockKey, true)
  })

  /** Focus the active shell (or the dock chrome while a pane is still mounting). */
  export function focus() {
    if (activeId && panes[activeId]) {
      panes[activeId]?.focus()
      return
    }
    dockEl?.focus({ preventScroll: true })
  }

  export function newTab() { void spawn() }

  export function closeActiveTab() {
    if (activeId) void closeTab(activeId)
  }

  export function rotateActiveTab(dir: -1 | 1) { rotateTab(dir) }

  function onTitle(id: string, title: string) {
    // Dedicated run tabs keep the command name; shells emit an OSC title on every prompt.
    if (runNameForSession(workspaceId, id, $runSessionBy, $runTabBy)) return
    const tab = tabs.find((t) => t.id === id)
    if (!tab || !title || tab.title === title) return
    tabs = tabs.map((t) => (t.id === id ? { ...t, title } : t))
    void SetTerminalTitle(id, title).catch(() => {})
  }

  function onExit(id: string, code: number) {
    // The process is gone but its final screen (a stack trace, a failed test) is
    // still worth reading, so the tab stays until the user dismisses it — unless
    // the backend already dropped the tombstone (cap per workspace).
    clearRunSession(workspaceId, runNameForSession(workspaceId, id) || '', id)
    tabs = tabs.map((t) => (t.id === id ? { ...t, title: `${t.title} — exited (${code})`, exited: true } : t))
    void ListTerminals(workspaceId).then((list) => {
      if (!list) return
      const keep = new Set(list.map((t) => t.id))
      const next = tabs.filter((t) => keep.has(t.id))
      if (next.length === tabs.length) return
      for (const t of tabs) {
        if (keep.has(t.id)) continue
        const leftover = discardParkedTerminal(t.id)
        if (leftover) {
          releaseParkedTerminal(leftover)
          DetachTerminal(t.id).catch(() => {})
        }
      }
      const droppedActive = activeId !== '' && !keep.has(activeId)
      tabs = next
      if (!droppedActive) return
      const live = next.find((t) => !t.exited)
      activeId = (live ?? next[0])?.id ?? ''
      void SetActiveTerminal(workspaceId, activeId).catch(() => {})
    }).catch(() => {})
  }

  function onTabKeydown(e: KeyboardEvent, id: string) {
    termTabDrag.onKeydown(e, id)
    if (e.defaultPrevented) return
    if (e.key !== 'Backspace' && e.key !== 'Delete') return
    e.preventDefault()
    void closeTab(id)
  }
</script>

<section class="term-dock" class:beside class:overlay class:term-solid={!frosts} aria-label="Terminal" tabindex="-1" bind:this={dockEl} style:--term-bg={termBg}>
  <!-- Sibling of the xterm canvas, not an ancestor: WKWebView paints WebGL black
       inside any backdrop-filter stacking context. -->
  <div class="term-dock-frost" aria-hidden="true"></div>
  {#if !beside}
    <ResizeHandle panel="terminalDock" edge="start" axis="y" />
  {/if}

  <TabStrip label="Terminal sessions" chrome class="term-tabs" activeKey={activeId} bind:stripEl={termTabStripEl}>
    {#each tabs as tab (tab.id)}
      {@const floating = termTabDrag.active && termTabDrag.fromKey === tab.id}
      {#if floating}
        <div class="tab-chip-ph" style="width:{termTabDrag.w}px" role="presentation" aria-hidden="true"></div>
      {:else}
        <TabChip
          class="term-tab"
          tabKey={tab.id}
          reorderable={tabs.length > 1}
          label={tab.title}
          active={activeId === tab.id}
          lit={lit}
          muted={tab.exited}
          closeLabel={`Close ${tab.title}`}
          onclick={() => {
            if (termTabDrag.suppressClick) {
              termTabDrag.suppressClick = false
              return
            }
            selectTab(tab.id)
          }}
          onkeydown={(e) => onTabKeydown(e, tab.id)}
          onclose={() => closeTab(tab.id)}
          onpointerdown={(e) => termTabDrag.onPointerDown(e, tab.id)}
        />
      {/if}
    {/each}
    {#snippet trailing()}
      {#if hasRuns}
        <button
          type="button"
          class="tab-strip-act term-act"
          class:on={dockRun.running}
          title={dockRunTitle}
          aria-label={dockRunAria}
          aria-pressed={dockRun.running}
          onclick={runOrStopFromDock}
        >
          <Icon name={dockRun.running ? 'square-fill' : 'play'} />
        </button>
      {/if}
      <button type="button" class="tab-strip-act term-act" title="New Tab (⌘T)" aria-label="New Tab" onclick={() => spawn()}>
        <Icon name="plus" />
      </button>
      <button
        type="button"
        class="tab-strip-act term-act"
        title="Clear terminal"
        aria-label="Clear terminal"
        disabled={!activeId}
        onclick={() => panes[activeId]?.clear()}
      >
        <Icon name="ban" />
      </button>
      <button
        type="button"
        class="tab-strip-act term-act"
        class:on={maximized}
        title={maximized ? 'Restore terminal size (⌃⇧`)' : 'Maximize terminal (⌃⇧`)'}
        aria-label={maximized ? 'Restore terminal size' : 'Maximize terminal'}
        aria-pressed={maximized}
        onclick={() => setTerminalDockMaximized(workspaceId, !maximized)}
      >
        <Icon name={maximized ? 'minimize' : 'maximize'} />
      </button>
      <button
        type="button"
        class="tab-strip-act term-act"
        title="Hide terminal (⌃`)"
        aria-label="Hide terminal"
        onclick={() => setTerminalDockOpen(workspaceId, false)}
      >
        <Icon name="panel-bottom" />
      </button>
    {/snippet}
  </TabStrip>
  {#if termTabDrag.active && termTabDrag.fromKey}
    {@const tab = tabs.find((t) => t.id === termTabDrag.fromKey)}
    {#if tab}
      <div
        class="tab-chip-float"
        bind:this={termTabFloatEl}
        style="left:{termTabDrag.x}px;top:{termTabDrag.y}px;width:{termTabDrag.w}px;height:{termTabDrag.h}px"
      >
        <TabChip
          class="term-tab"
          floating
          label={tab.title}
          active={activeId === tab.id}
          lit={lit}
          muted={tab.exited}
        />
      </div>
    {/if}
  {/if}

  <div class="term-dock-body">
    {#if findOpen}
      <div class="term-dock-find cm-recur-search">
        <div class="cm-recur-search-row">
          <span class="cm-recur-search-glyph" aria-hidden="true"><Icon name="search" /></span>
          <input
            bind:this={findInput}
            bind:value={findQuery}
            type="text"
            class="cm-recur-search-input"
            name="search"
            placeholder="Find"
            aria-label="Find"
            spellcheck="false"
            autocomplete="off"
            onkeydown={onFindInputKey}
          />
          <span
            class="cm-recur-search-count"
            class:none={!!findQuery && findValid && findTotal === 0}
            data-empty={!findQuery || !findValid ? '1' : '0'}
            aria-live="polite"
          >{findCount}</span>
          <div class="cm-recur-search-nav">
            <button type="button" class="obs-icon-btn" title="Previous match (⇧⌘G)" aria-label="Previous match" onclick={() => stepFind(-1)}>
              <Icon name="chevron-left" />
            </button>
            <button type="button" class="obs-icon-btn" title="Next match (⌘G)" aria-label="Next match" onclick={() => stepFind(1)}>
              <Icon name="chevron-right" />
            </button>
          </div>
          <div class="cm-recur-search-toggles">
            <button
              type="button"
              class="obs-icon-btn cm-recur-search-toggle"
              class:on={findCase}
              title="Match case (⌥⌘C)"
              aria-label="Match case"
              aria-pressed={findCase}
              onclick={() => { findCase = !findCase }}
            >
              <Icon name="case-sensitive" />
            </button>
            <button
              type="button"
              class="obs-icon-btn cm-recur-search-toggle"
              class:on={findRegexp}
              title="Regular expression (⌥⌘R)"
              aria-label="Regular expression"
              aria-pressed={findRegexp}
              onclick={() => { findRegexp = !findRegexp }}
            >
              <Icon name="regex" />
            </button>
            <button
              type="button"
              class="obs-icon-btn cm-recur-search-toggle"
              class:on={findWord}
              title="Whole word (⌥⌘W)"
              aria-label="Whole word"
              aria-pressed={findWord}
              onclick={() => { findWord = !findWord }}
            >
              <Icon name="whole-word" />
            </button>
          </div>
          <button type="button" class="obs-icon-btn cm-recur-search-close" title="Close" aria-label="Close find" onclick={() => closeFind()}>
            <Icon name="x" />
          </button>
        </div>
      </div>
    {/if}
    <div class="term-dock-panes" bind:this={dockBody}>
      {#if loading}
        <p class="term-empty">Starting shell…</p>
      {:else if tabs.length === 0}
        <p class="term-empty">No terminals. Press + to start one.</p>
      {/if}
      <!-- Every tab stays mounted: a background shell keeps streaming into a live
           screen, so switching tabs is instant and nothing is missed. -->
      {#each tabs as tab (tab.id)}
        <TerminalPane
          bind:this={panes[tab.id]}
          id={tab.id}
          {workspaceId}
          active={activeId === tab.id}
          {gpu}
          frosted={overlay && frosts}
          onTitle={(t) => onTitle(tab.id, t)}
          onExit={(c) => onExit(tab.id, c)}
          {onFindResults}
        />
      {/each}
    </div>
  </div>
</section>

<style>
  .term-dock {
    position: relative;
    flex: none;
    height: var(--terminal-dock-height, 260px);
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--line);
    background: var(--term-bg, var(--bg));
  }

  .term-dock.beside {
    min-width: 0;
    border-top: none;
    border-left: 1px solid var(--line);
  }

  .term-dock.beside:not(.overlay) {
    height: auto;
  }

  /* Programmatic focus target while a pane remounts — no visible ring. */
  .term-dock:focus { outline: none; }
  .term-dock:focus-visible { outline: none; }

  .term-dock-body {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  /* position:relative is load-bearing — hidden panes are absolutely positioned and
     need this as their containing block to keep a real measured size. */
  .term-dock-panes {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .term-empty {
    margin: 0;
    padding: 10px 12px;
    font-size: 11px;
    color: var(--faint);
  }
</style>
