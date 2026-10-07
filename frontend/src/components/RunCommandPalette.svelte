<script lang="ts">
  /**
   * Recur Bundle command + snippet palette (⌘⇧P / status-bar Bundles).
   * Scoped to the focused file; snippets show their tab trigger (`ife⇥`).
   */
  import { commandsOpen, pushToast } from '../lib/stores'
  import {
    categories as commandCategories, currentEditorFocus, listCommands, runEditorCommand,
    type RegisteredCommand,
  } from '../lib/commands'
  import {
    listSnippets, runEditorSnippet, snippetTriggerHint, type RegisteredSnippet,
  } from '../lib/snippets'
  import {
    COMMAND_USAGE_RECENT_SIZE, rankedCommandIds, recordCommandUsage,
  } from '../lib/command-usage'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  type Row = {
    id: string
    category: string
    label: string
    keys: string[]
    kb?: string
    hint?: string
    run: () => void
  }

  const RECENT = 'Recent'

  let q = $state('')
  let sel = $state(0)
  let inputEl = $state<HTMLInputElement | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let cats = $state<string[]>([])
  let cmds = $state<RegisteredCommand[]>([])
  let snips = $state<RegisteredSnippet[]>([])
  /** Re-read on each open so new runs reshuffle Recent. */
  let recentIds = $state<string[]>([])

  const rows = $derived<Row[]>([
    ...cmds
      .filter((c) => c.triggers.includes('palette'))
      .map((c) => ({
        id: c.id,
        category: c.category,
        label: c.label,
        keys: [c.category, c.id, c.interpreter, c.keyEquivalentDisplay ?? '', c.keyEquivalent ?? ''],
        kb: c.keyEquivalentDisplay,
        run: () => {
          recordCommandUsage(c.id)
          void runEditorCommand(c.id).catch((err) => {
            pushToast('error', err instanceof Error ? err.message : String(err))
          })
        },
      })),
    ...snips.map((s) => ({
      id: s.id,
      category: s.bundleName || 'Snippets',
      label: s.label,
      keys: [s.bundleName ?? '', s.id, s.tabTrigger, s.keyEquivalentDisplay ?? '', s.keyEquivalent ?? ''],
      kb: s.keyEquivalentDisplay,
      hint: snippetTriggerHint(s.tabTrigger),
      run: () => {
        recordCommandUsage(s.id)
        try {
          runEditorSnippet(s)
        } catch (err) {
          pushToast('error', err instanceof Error ? err.message : String(err))
        }
      },
    })),
  ])

  const byId = $derived(new Map(rows.map((r) => [r.id, r])))

  function matches(r: Row, query: string): boolean {
    if (!query) return true
    return (
      r.label.toLowerCase().includes(query) ||
      r.category.toLowerCase().includes(query) ||
      r.keys.some((k) => k.toLowerCase().includes(query)) ||
      (r.hint ?? '').toLowerCase().includes(query)
    )
  }

  /** Visible recent ids (available for this file + match query), frecency order. */
  const visibleRecentIds = $derived.by(() => {
    const query = q.trim().toLowerCase()
    return recentIds.filter((id) => {
      const r = byId.get(id)
      return !!r && matches(r, query)
    })
  })

  const flat = $derived.by(() => {
    const query = q.trim().toLowerCase()
    const recentSet = new Set(visibleRecentIds)
    const recentRows = visibleRecentIds.map((id) => byId.get(id)!)
    const rest = cats.flatMap((g) =>
      rows.filter((r) => r.category === g && matches(r, query) && !recentSet.has(r.id)),
    )
    return [...recentRows, ...rest]
  })

  const displaySections = $derived.by(() => {
    const recentSet = new Set(visibleRecentIds)
    const out: { title: string; items: { r: Row; idx: number }[] }[] = []

    if (visibleRecentIds.length) {
      const items: { r: Row; idx: number }[] = []
      for (const id of visibleRecentIds) {
        const idx = flat.findIndex((r) => r.id === id)
        if (idx >= 0) items.push({ r: flat[idx], idx })
      }
      if (items.length) out.push({ title: RECENT, items })
    }

    for (const g of cats) {
      const items: { r: Row; idx: number }[] = []
      flat.forEach((r, idx) => {
        if (r.category === g && !recentSet.has(r.id)) items.push({ r, idx })
      })
      if (items.length) out.push({ title: g, items })
    }
    return out
  })

  $effect(() => {
    if ($commandsOpen) {
      q = ''
      sel = 0
      recentIds = rankedCommandIds(COMMAND_USAGE_RECENT_SIZE)
      setTimeout(() => inputEl?.focus(), 0)
      const { scope, path } = currentEditorFocus()
      void Promise.all([commandCategories(scope, path), listCommands(scope, path), listSnippets(scope, path)])
        .then(([c, list, snippets]) => {
          const snCats = [...new Set(snippets.map((s) => s.bundleName || 'Snippets'))]
            .filter((name) => !c.includes(name))
            .sort((a, b) => a.localeCompare(b))
          cats = [...c, ...snCats]
          cmds = list
          snips = snippets
        })
        .catch(() => {
          cats = []
          cmds = []
          snips = []
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

  function run(i: number) {
    const r = flat[i]
    if (r) {
      commandsOpen.set(false)
      r.run()
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      sel = Math.min(flat.length - 1, sel + 1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      sel = Math.max(0, sel - 1)
    } else if (e.key === 'Enter') {
      e.preventDefault()
      run(sel)
    } else if (e.key === 'Escape') {
      commandsOpen.set(false)
    }
  }
</script>

<div
  class="palette-bg"
  class:open={$commandsOpen}
  onclick={(e) => {
    if (e.target === e.currentTarget) commandsOpen.set(false)
  }}
  role="presentation"
>
  <div class="palette cmd-palette" role="dialog" aria-modal="true" aria-label="Run command">
    <div class="pal-in">
      <Icon name="command" />
      <input
        bind:this={inputEl}
        bind:value={q}
        onkeydown={onKey}
        placeholder="Type a command or snippet…"
        aria-label="Search commands and snippets"
        autocomplete="off"
        spellcheck="false"
      />
      <span class="esc">⌘⇧P</span>
    </div>
    <div class="pal-list" bind:this={listEl}>
      {#if flat.length === 0}
        <div class="pal-empty">{q.trim() ? 'No matching commands or snippets' : 'No commands or snippets for this file'}</div>
      {:else}
        {#each displaySections as section}
          <div class="pal-g">{section.title}</div>
          {#each section.items as { r, idx } (section.title + ':' + r.id)}
            <div
              class="pal-row"
              class:sel={idx === sel}
              role="button"
              tabindex="-1"
              onmousemove={() => (sel = idx)}
              onclick={() => run(idx)}
              onkeydown={(e) => e.key === 'Enter' && run(idx)}
            >
              <span class="pl">{r.label}</span>
              {#if section.title === RECENT}
                <span class="psub">{r.category}</span>
              {/if}
              {#if r.hint}
                <span class="ptrig">{r.hint}</span>
              {/if}
              {#if r.kb}
                <Kbd class="pk" text={r.kb} />
              {/if}
            </div>
          {/each}
        {/each}
      {/if}
    </div>
    <div class="pal-foot">
      <span><Kbd class="k" text="↑↓" />navigate</span>
      <span><Kbd class="k" text="↵" />run</span>
      <span><Kbd class="k" text="esc" />close</span>
    </div>
  </div>
</div>
