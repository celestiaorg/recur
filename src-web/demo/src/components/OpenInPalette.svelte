<script lang="ts">
  /**
   * Workspace "Open in" picker (⌘O / File → Open In… / titlebar).
   * Same overlay chrome as ⌘K / ⌘⇧P; number keys 1–9 pick a row when the
   * filter is empty.
   */
  import { openInOpen, selectedWorkspaceId } from '../lib/stores'
  import { WorkspacesService, callOk } from '../lib/api'
  import { copySelectedWorkspacePath } from '../lib/workspace-path'
  import { isCmdOrCtrl, paletteRowDigit } from '../lib/shortcut'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { openInDefault, orderOpenInTargets, setOpenInDefault } from '../lib/open-in-pref'
  import Icon, { type IconName } from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  type Row = {
    id: string
    label: string
    icon: IconName
    iconURL?: string
    run: () => void
  }

  const ICONS: Record<string, IconName> = {
    finder: 'folder',
    cursor: 'code',
    vscode: 'code',
    zed: 'code',
    xcode: 'code',
    ghostty: 'terminal',
    iterm: 'terminal',
    terminal: 'terminal',
    warp: 'terminal',
    kaleidoscope: 'file-diff',
    sublime: 'code',
    'copy-path': 'copy',
  }

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let apps = $state<{ id: string; name: string; iconURL?: string }[]>([])
  let ready = $state(false)
  let hoverGate = createHoverGate()

  function iconFor(id: string): IconName {
    return ICONS[id] ?? 'box'
  }

  function openApp(id: string) {
    const workspaceID = $selectedWorkspaceId
    if (!workspaceID) return
    void callOk(WorkspacesService.OpenWorkspaceIn(workspaceID, id), 'Open failed')
  }

  const all = $derived<Row[]>([
    ...orderOpenInTargets(apps, $openInDefault).map((a) => ({
      id: a.id,
      label: a.name,
      icon: iconFor(a.id),
      iconURL: a.iconURL,
      run: () => openApp(a.id),
    })),
    {
      id: 'copy-path',
      label: 'Copy path',
      icon: 'copy' as const,
      run: () => { void copySelectedWorkspacePath() },
    },
  ])

  const flat = $derived.by(() => {
    const query = q.trim().toLowerCase()
    if (!query) return all
    return all.filter(
      (r) => r.label.toLowerCase().includes(query) || r.id.toLowerCase().includes(query),
    )
  })

  $effect(() => {
    if ($openInOpen) {
      q = ''
      sel = 0
      ready = false
      resetHoverGate(hoverGate)
      setTimeout(() => inputEl?.focus(), 0)
      void WorkspacesService.ListOpenInTargets()
        .then((list) => {
          apps = (list ?? []).map((t) => ({
            id: t.id,
            name: t.name,
            iconURL: t.icon || undefined,
          }))
        })
        .catch(() => {
          apps = [{ id: 'finder', name: 'Finder' }]
        })
        .finally(() => {
          ready = true
        })
    }
  })

  $effect(() => {
    if (sel >= flat.length) sel = Math.max(0, flat.length - 1)
  })

  $effect(() => {
    void sel
    void flat
    queueMicrotask(() => {
      listEl?.querySelector<HTMLElement>('.pal-row.sel')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function makeDefault(id: string) {
    if (!id || id === 'copy-path') return
    setOpenInDefault(id)
    sel = 0
  }

  function run(i: number) {
    const r = flat[i]
    if (!r) return
    openInOpen.set(false)
    r.run()
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (e.metaKey || e.ctrlKey || e.altKey) return
      e.preventDefault()
      sel = e.key === 'ArrowDown' ? Math.min(flat.length - 1, sel + 1) : Math.max(0, sel - 1)
    } else if (isCmdOrCtrl(e) && e.key === 'Enter') {
      e.preventDefault()
      const r = flat[sel]
      if (r) makeDefault(r.id)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      run(sel)
    } else if (e.key === 'Escape') {
      openInOpen.set(false)
    } else if (!q.trim()) {
      const n = paletteRowDigit(e)
      if (n !== null && n - 1 < flat.length) {
        e.preventDefault()
        run(n - 1)
      }
    }
  }
</script>

<div
  class="palette-bg"
  class:open={$openInOpen}
  onclick={(e) => {
    if (e.target === e.currentTarget) openInOpen.set(false)
  }}
  role="presentation"
>
  <div class="palette cmd-palette open-in-palette" role="dialog" aria-modal="true" aria-label="Open in">
    <div class="pal-in">
      <Icon name="queue-pop-out" />
      <input
        bind:this={inputEl}
        bind:value={q}
        onkeydown={onKey}
        placeholder="Open workspace in…"
        aria-label="Filter apps"
        autocomplete="off"
        spellcheck="false"
      />
      <span class="esc">⌘O</span>
    </div>
    <div class="pal-list" bind:this={listEl}>
      {#if !ready}
        <div class="pal-empty">Looking for apps…</div>
      {:else if flat.length === 0}
        <div class="pal-empty">{q.trim() ? 'No matching apps' : 'No apps found'}</div>
      {:else}
        {#each flat as r, idx (r.id)}
          <div
            class="pal-row"
            class:sel={idx === sel}
            role="button"
            tabindex="-1"
            onmousemove={(e) => { if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) sel = idx }}
            onclick={() => run(idx)}
            onkeydown={(e) => e.key === 'Enter' && run(idx)}
          >
            <span class="pic" class:os={!!r.iconURL}>
              {#if r.iconURL}
                <img src={r.iconURL} alt="" draggable="false" />
              {:else}
                <Icon name={r.icon} />
              {/if}
            </span>
            <span class="pl">{r.label}</span>
            {#if r.id === $openInDefault}
              <span class="psub">default</span>
            {/if}
            {#if r.id !== 'copy-path'}
              <button
                type="button"
                class="open-in-pin"
                class:on={r.id === $openInDefault}
                title={r.id === $openInDefault ? 'Default' : 'Set as default'}
                aria-label={r.id === $openInDefault ? 'Default' : 'Set as default'}
                aria-pressed={r.id === $openInDefault}
                onmousedown={(e) => e.preventDefault()}
                onclick={(e) => {
                  e.stopPropagation()
                  makeDefault(r.id)
                }}
              >
                <Icon name="pin" />
              </button>
            {/if}
            {#if !q.trim() && idx < 9}
              <span class="pk">{idx + 1}</span>
            {/if}
          </div>
        {/each}
      {/if}
    </div>
    <div class="pal-foot">
      <span><Kbd class="k" text="↑↓" />navigate</span>
      <span><Kbd class="k" text="1–9" />open</span>
      <span><Kbd class="k" text="↵" />open</span>
      <span><Kbd class="k" text="⌘↩" />default</span>
      <span><Kbd class="k" text="esc" />close</span>
    </div>
  </div>
</div>
