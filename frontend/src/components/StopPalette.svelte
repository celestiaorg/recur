<script lang="ts">
  /**
   * Stop-run picker (⌘⇧R / titlebar chevron when several commands are going).
   * Same overlay chrome as ⌘R / ⌘O: filter, ↑↓, 1–9, Enter to stop.
   */
  import { stopMenuOpen, selectedWorkspaceId, workspaces, projects } from '../lib/stores'
  import {
    runningRuns, runSessionBy, requestStopRun, requestStopAll, workspaceRuns,
  } from '../lib/run-dev'
  import { paletteRowDigit } from '../lib/shortcut'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  type Row = { id: string; name: string; cmd: string; all: boolean }

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let hoverGate = createHoverGate()

  const running = $derived(runningRuns($selectedWorkspaceId, $runSessionBy))
  const cmds = $derived(new Map(
    workspaceRuns($selectedWorkspaceId, $workspaces, $projects).map((r) => [r.name, r.cmd]),
  ))
  const all = $derived.by((): Row[] => {
    const rows: Row[] = running.map((r) => ({
      id: r.name,
      name: r.name,
      cmd: cmds.get(r.name) ?? '',
      all: false,
    }))
    if (running.length > 1) {
      rows.push({
        id: '__all__',
        name: 'all',
        cmd: running.map((r) => r.name).join(', '),
        all: true,
      })
    }
    return rows
  })
  const flat = $derived.by(() => {
    const query = q.trim().toLowerCase()
    if (!query) return all
    return all.filter(
      (r) =>
        r.name.toLowerCase().includes(query) ||
        r.cmd.toLowerCase().includes(query) ||
        `stop ${r.name}`.toLowerCase().includes(query),
    )
  })

  $effect(() => {
    if ($stopMenuOpen) {
      q = ''
      sel = 0
      resetHoverGate(hoverGate)
      setTimeout(() => inputEl?.focus(), 0)
    }
  })

  $effect(() => {
    if ($stopMenuOpen && running.length === 0) stopMenuOpen.set(false)
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

  function pick(i: number) {
    const r = flat[i]
    if (!r) return
    stopMenuOpen.set(false)
    if (r.all) requestStopAll($selectedWorkspaceId)
    else requestStopRun($selectedWorkspaceId, r.name)
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (e.metaKey || e.ctrlKey || e.altKey) return
      e.preventDefault()
      sel = e.key === 'ArrowDown' ? Math.min(flat.length - 1, sel + 1) : Math.max(0, sel - 1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      pick(sel)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      stopMenuOpen.set(false)
    } else if (!q.trim()) {
      const n = paletteRowDigit(e, { shiftDigits: true })
      if (n !== null && n - 1 < flat.length) {
        e.preventDefault()
        pick(n - 1)
      }
    }
  }
</script>

<div
  class="palette-bg"
  class:open={$stopMenuOpen}
  onclick={(e) => {
    if (e.target === e.currentTarget) stopMenuOpen.set(false)
  }}
  role="presentation"
>
  <div class="palette cmd-palette open-in-palette stop-palette" role="dialog" aria-modal="true" aria-label="Stop">
    <div class="pal-in">
      <Icon name="square-fill" />
      <input
        bind:this={inputEl}
        bind:value={q}
        onkeydown={onKey}
        placeholder="Stop…"
        aria-label="Filter running commands"
        autocomplete="off"
        spellcheck="false"
      />
      <span class="esc">⌘⇧R</span>
    </div>
    <div class="pal-list" bind:this={listEl}>
      {#if flat.length === 0}
        <div class="pal-empty">{q.trim() ? 'No matching processes' : 'Nothing running'}</div>
      {:else}
        {#each flat as r, idx (r.id)}
          <div
            class="pal-row"
            class:sel={idx === sel}
            role="button"
            tabindex="-1"
            onmousemove={(e) => { if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) sel = idx }}
            onclick={() => pick(idx)}
            onkeydown={(e) => e.key === 'Enter' && pick(idx)}
          >
            <span class="pl">Stop {r.name}</span>
            {#if r.cmd}
              <span class="psub">{r.cmd}</span>
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
      <span><Kbd class="k" text="1–9" />stop</span>
      <span><Kbd class="k" text="↵" />stop</span>
      <span><Kbd class="k" text="esc" />close</span>
    </div>
  </div>
</div>
