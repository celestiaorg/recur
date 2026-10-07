<script lang="ts">
  import { paletteOpen, shortcutsOpen, commandsOpen, openInOpen, workspaces, prs, projects, go, openWorkspace, openPR, openProject, openNewTask, openNewProject, selectedWorkspaceId, view, openSyntaxModePicker, openSettings, pushToast, scheduleEditor, type SettingsSection } from '../lib/stores'
  import { createBlankWorkspace } from '../lib/new-workspace'
  import { pickAndOpenPath } from '../lib/open-path'
  import { workspaceDevCmd, runSessionBy, isAnyRunRunning, openRunMenu, openStopMenu } from '../lib/run-dev'
  import { wsMainBlocked } from '../lib/modes'
  import { copySelectedWorkspacePath } from '../lib/workspace-path'
  import { currentEditorFocus } from '../lib/commands'
  import { prIconName } from '../lib/colors'
  import { bridgeErrorMessage } from '../lib/api'
  import * as BundleService from '../../bindings/recur/internal/app/bundleservice'
  import Icon, { type IconName } from './Icon.svelte'
  import ProjectPic from './ProjectPic.svelte'
  import Kbd from './Kbd.svelte'

  type Cmd = {
    g: string; label: string; psub?: string; kb?: string
    dot?: string; pic?: string; lc?: string; icon?: IconName
    svg?: string
    // keys: extra searchable terms beyond label/psub/group (Phase 4.2) — e.g. a
    // loop's id/projectId/branch, a PR's number, a project's repo path.
    keys?: string[]
    run: () => void
  }

  const groupColor: Record<string, string> = {
    needs: 'var(--coral)', active: 'var(--amber)', ready: 'var(--emerald)',
  }

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let searchHits = $state.raw<Cmd[]>([])
  let searchGroupNames = $state.raw<string[]>([])
  let searchGen = 0
  const palOrder = $derived(['Actions', 'Go to', ...searchGroupNames, 'Loops', 'Pull requests', 'Projects'])

  function pickSearchHit(insert: string, action: string): void {
    const a = (action ?? '').trim()
    if (a.startsWith('settings:')) {
      openSettings(a.slice('settings:'.length) as SettingsSection)
      return
    }
    const ref = insert.trim()
    if (!ref) return
    void (async () => {
      try {
        const res = await BundleService.ExpandGoal(ref)
        const text = res?.prompt?.trim() || ref
        openNewTask('', { text, display: res?.display ?? '' })
      } catch (e) {
        pushToast('error', bridgeErrorMessage(e) || 'Could not expand search hit')
      }
    })()
  }

  const canDevAction = $derived(
    $view === 'detail' && !!$selectedWorkspaceId &&
      !wsMainBlocked($workspaces.find((w) => w.id === $selectedWorkspaceId)) &&
      !!workspaceDevCmd($selectedWorkspaceId, $workspaces, $projects),
  )
  const anyRunRunning = $derived(canDevAction && isAnyRunRunning($selectedWorkspaceId, $runSessionBy))

  const editorFileOpen = $derived.by(() => {
    void $paletteOpen
    return !!currentEditorFocus().path
  })

  const all = $derived<Cmd[]>([
    { g: 'Actions', icon: 'plus', label: 'New workspace', kb: '⌘N', run: () => openNewTask() },
    { g: 'Actions', icon: 'house-plus', label: 'New project', run: () => openNewProject() },
    { g: 'Actions', icon: 'clock-plus', label: 'New schedule', run: () => scheduleEditor.set({}) },
    { g: 'Actions', icon: 'plus', label: 'New blank workspace', kb: '⇧⌘N', run: () => { void createBlankWorkspace() } },
    { g: 'Actions', icon: 'folder', label: 'Open file or folder', kb: '⌥⌘O', run: () => { void pickAndOpenPath() } },
    ...($view === 'detail' && $selectedWorkspaceId &&
      !wsMainBlocked($workspaces.find((w) => w.id === $selectedWorkspaceId))
      ? [
          { g: 'Actions', icon: 'queue-pop-out' as const, label: 'Open in…', kb: '⌘O', run: () => { paletteOpen.set(false); openInOpen.set(true) } } as Cmd,
          { g: 'Actions', icon: 'copy' as const, label: 'Copy path', run: () => { void copySelectedWorkspacePath() } } as Cmd,
        ]
      : []),
    ...(editorFileOpen
      ? [{
          g: 'Actions', icon: 'code' as const, label: 'Change language mode',
          run: () => { paletteOpen.set(false); openSyntaxModePicker() },
        } as Cmd]
      : []),
    {
      g: 'Actions', icon: 'command', label: 'Run command / snippets…', kb: '⌘⇧P',
      run: () => { shortcutsOpen.set(false); commandsOpen.set(true) },
    },
    { g: 'Actions', icon: 'help', label: 'Keyboard shortcuts', kb: '⌘⇧/', run: () => { paletteOpen.set(false); shortcutsOpen.set(true) } },
    ...(canDevAction
      ? [{
          g: 'Actions',
          icon: 'play' as IconName,
          label: 'Run…',
          kb: '⌘R',
          run: () => { paletteOpen.set(false); openRunMenu($selectedWorkspaceId) },
        } as Cmd]
      : []),
    ...(anyRunRunning
      ? [{
          g: 'Actions',
          icon: 'square-fill' as IconName,
          label: 'Stop run…',
          kb: '⌘⇧R',
          run: () => { paletteOpen.set(false); openStopMenu($selectedWorkspaceId) },
        } as Cmd]
      : []),
    { g: 'Go to', icon: 'inbox', label: 'Inbox', run: () => go('briefing') },
    { g: 'Go to', icon: 'git-pull-request', label: 'Pull requests', run: () => go('pr') },
    { g: 'Go to', icon: 'clock', label: 'Schedules', run: () => go('schedules') },
    { g: 'Go to', icon: 'settings', label: 'Settings · policy', run: () => go('settings') },
    { g: 'Go to', icon: 'help', label: 'Get started', run: () => go('welcome') },
    ...$workspaces.filter((l) => !l.archived && !(l.hostPrId ?? '').trim()).map((l) => ({
      g: 'Loops', dot: groupColor[l.group], label: l.goal, psub: l.id,
      keys: [l.goal, l.id, l.short, l.projectId, l.branch], run: () => openWorkspace(l.id),
    })),
    ...$prs.map((p) => ({
      g: 'Pull requests', icon: prIconName(p), label: p.title, psub: '#' + p.number,
      keys: [p.title, '#' + p.number, String(p.number), p.workspaceId, p.branch], run: () => openPR(p.id),
    })),
    ...$projects.map((p) => ({
      g: 'Projects', pic: p.pic, lc: p.color, label: p.name,
      keys: [p.name, p.repoPath ?? ''], run: () => openProject(p.id),
    })),
  ])

  const flat = $derived.by(() => {
    const query = q.trim().toLowerCase()
    const match = all.filter(
      (c) =>
        !query ||
        c.label.toLowerCase().includes(query) ||
        (c.psub ?? '').toLowerCase().includes(query) ||
        c.g.toLowerCase().includes(query) ||
        (c.keys ?? []).some((k) => k.toLowerCase().includes(query)),
    )
    // order by palOrder so the flat index matches the rendered order.
    return palOrder.flatMap((g) => {
      if (searchGroupNames.includes(g)) return searchHits.filter((c) => c.g === g)
      return match.filter((c) => c.g === g)
    })
  })

  $effect(() => {
    if (!$paletteOpen || q.trim().length < 2) {
      searchGen++
      searchHits = []
      searchGroupNames = []
      return
    }
    const query = q.trim()
    const gen = ++searchGen
    const t = setTimeout(() => {
      void BundleService.SearchPalette(query)
        .then((res) => {
          if (gen !== searchGen) return
          const groups = res?.groups ?? []
          const rows: Cmd[] = []
          const names: string[] = []
          for (const g of groups) {
            const hits = g.hits ?? []
            if (!hits.length) continue
            names.push(g.name)
            for (const h of hits) {
              const insert = (h.insert ?? '').trim()
              const action = (h.action ?? '').trim()
              rows.push({
                g: g.name,
                icon: 'hash',
                svg: g.iconSvg || undefined,
                label: h.label || insert,
                psub: (h.psub ?? '').trim() || insert,
                run: () => pickSearchHit(insert, action),
              })
            }
          }
          searchHits = rows
          searchGroupNames = names
        })
        .catch(() => {
          if (gen !== searchGen) return
          searchHits = []
          searchGroupNames = []
        })
    }, 200)
    return () => { clearTimeout(t) }
  })

  $effect(() => {
    if ($paletteOpen) {
      q = ''
      sel = 0
      setTimeout(() => inputEl?.focus(), 0)
    }
  })
  $effect(() => {
    // keep selection in range as the list changes
    if (sel >= flat.length) sel = Math.max(0, flat.length - 1)
  })

  $effect(() => {
    void sel
    void flat
    queueMicrotask(() => {
      listEl?.querySelector<HTMLElement>('.pal-row.sel')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function run(i: number) {
    const c = flat[i]
    if (!c) return
    paletteOpen.set(false)
    c.run()
  }
  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (e.key === 'ArrowDown') { e.preventDefault(); sel = Math.min(flat.length - 1, sel + 1) }
    else if (e.key === 'ArrowUp') { e.preventDefault(); sel = Math.max(0, sel - 1) }
    else if (e.key === 'Enter') { e.preventDefault(); run(sel) }
    else if (e.key === 'Escape') { paletteOpen.set(false) }
  }
  // group → its rendered items + their flat index
  function groupItems(g: string) {
    const out: { c: Cmd; idx: number }[] = []
    flat.forEach((c, idx) => { if (c.g === g) out.push({ c, idx }) })
    return out
  }
</script>

<div class="palette-bg" class:open={$paletteOpen} onclick={(e) => { if (e.target === e.currentTarget) paletteOpen.set(false) }} role="presentation">
  <div class="palette">
    <div class="pal-in">
      <Icon name="search" />
      <input bind:this={inputEl} bind:value={q} onkeydown={onKey} placeholder="Search workspaces, PRs, or projects…" autocomplete="off" spellcheck="false" />
      <span class="esc">esc</span>
    </div>
    <div class="pal-list" bind:this={listEl}>
      {#if flat.length === 0}
        <div class="pal-empty">No matches</div>
      {:else}
        {#each palOrder as g}
          {@const items = groupItems(g)}
          {#if items.length}
            <div class="pal-g">{g}</div>
            {#each items as { c, idx } (idx)}
              <div class="pal-row" class:sel={idx === sel} role="button" tabindex="-1"
                onmousemove={() => (sel = idx)} onclick={() => run(idx)} onkeydown={(e) => e.key === 'Enter' && run(idx)}>
                <span class="pic">
                  {#if c.dot}<span class="dot" style="background:{c.dot}"></span>
                  {:else if c.pic}<ProjectPic pic={c.pic} color={c.lc ?? ''} name={c.label} />
                  {:else if c.icon}<Icon name={c.icon} svg={c.svg || ''} />{/if}
                </span>
                <span class="pl">{c.label}</span>
                {#if c.psub}<span class="psub">{c.psub}</span>{/if}
                {#if c.kb}<Kbd class="pk" text={c.kb} />{/if}
              </div>
            {/each}
          {/if}
        {/each}
      {/if}
    </div>
    <div class="pal-foot"><span><Kbd class="k" text="↑↓" />navigate</span><span><Kbd class="k" text="↵" />open</span><span><Kbd class="k" text="esc" />close</span></div>
  </div>
</div>
