<script lang="ts">
  /**
   * Named-run picker (⌘R / titlebar chevron). Same overlay chrome as ⌘O:
   * filter, ↑↓, 1–9, Enter to run, ⌘↩ to pin the default.
   */
  import { runMenuOpen, selectedWorkspaceId, workspaces, projects } from '../lib/stores'
  import { isCmdOrCtrl, paletteRowDigit } from '../lib/shortcut'
  import {
    requestRunDev, pinProjectRun, projectForWorkspace, workspaceRuns, workspaceRun,
    isRunRunning, runSessionBy,
  } from '../lib/run-dev'
  import { filterRuns, orderRuns } from '../lib/run-commands'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let hoverGate = createHoverGate()

  const pin = $derived(workspaceRun($selectedWorkspaceId, $workspaces, $projects)?.name ?? '')
  const all = $derived(orderRuns(
    workspaceRuns($selectedWorkspaceId, $workspaces, $projects),
    pin,
  ))
  const query = $derived(q.trim().toLowerCase())
  const flat = $derived(filterRuns(all, query))

  $effect(() => {
    if ($runMenuOpen) {
      q = ''
      sel = 0
      resetHoverGate(hoverGate)
      setTimeout(() => inputEl?.focus(), 0)
    }
  })

  $effect(() => {
    void query
    sel = 0
    resetHoverGate(hoverGate)
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

  async function makeDefault(name: string) {
    const proj = projectForWorkspace($selectedWorkspaceId)
    if (!proj || !name) return
    if (await pinProjectRun(proj, name)) sel = 0
  }

  function run(i: number) {
    const r = flat[i]
    if (!r) return
    runMenuOpen.set(false)
    requestRunDev($selectedWorkspaceId, r)
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (e.metaKey || e.ctrlKey || e.altKey) return
      e.preventDefault()
      sel = e.key === 'ArrowDown' ? Math.min(flat.length - 1, sel + 1) : Math.max(0, sel - 1)
    } else if (isCmdOrCtrl(e) && e.key === 'Enter') {
      e.preventDefault()
      const r = flat[sel]
      if (r) void makeDefault(r.name)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      run(sel)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      runMenuOpen.set(false)
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
  class:open={$runMenuOpen}
  onclick={(e) => {
    if (e.target === e.currentTarget) runMenuOpen.set(false)
  }}
  role="presentation"
>
  <div class="palette cmd-palette open-in-palette run-palette" role="dialog" aria-modal="true" aria-label="Run">
    <div class="pal-in">
      <Icon name="play" />
      <input
        bind:this={inputEl}
        bind:value={q}
        onkeydown={onKey}
        placeholder="Run…"
        aria-label="Filter run commands"
        autocomplete="off"
        spellcheck="false"
      />
      <span class="esc">⌘R</span>
    </div>
    <div class="pal-list" bind:this={listEl}>
      {#if flat.length === 0}
        <div class="pal-empty">{query ? 'No matching commands' : 'No run commands'}</div>
      {:else}
        {#each flat as r, idx (r.name)}
          <div
            class="pal-row"
            class:sel={idx === sel}
            role="button"
            tabindex="-1"
            onmousemove={(e) => { if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) sel = idx }}
            onclick={() => run(idx)}
            onkeydown={(e) => e.key === 'Enter' && run(idx)}
          >
            <span class="pl">{r.name}</span>
            <span class="psub">{r.cmd}</span>
            {#if r.name === pin}
              <span class="psub">default</span>
            {/if}
            {#if isRunRunning($selectedWorkspaceId, r.name, $runSessionBy)}
              <span class="psub">running</span>
            {/if}
            <button
              type="button"
              class="open-in-pin"
              class:on={r.name === pin}
              title={r.name === pin ? 'Default' : 'Set as default'}
              aria-label={r.name === pin ? 'Default' : 'Set as default'}
              aria-pressed={r.name === pin}
              onmousedown={(e) => e.preventDefault()}
              onclick={(e) => {
                e.stopPropagation()
                void makeDefault(r.name)
              }}
            >
              <Icon name="pin" />
            </button>
            {#if !query && idx < 9}
              <span class="pk">{idx + 1}</span>
            {/if}
          </div>
        {/each}
      {/if}
    </div>
    <div class="pal-foot">
      <span><Kbd class="k" text="↑↓" />navigate</span>
      <span><Kbd class="k" text="1–9" />run</span>
      <span><Kbd class="k" text="↵" />run</span>
      <span><Kbd class="k" text="⌘↩" />default</span>
      <span><Kbd class="k" text="esc" />close</span>
    </div>
  </div>
</div>
