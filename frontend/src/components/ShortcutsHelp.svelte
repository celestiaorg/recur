<script lang="ts">
  import { shortcutsOpen } from '../lib/stores'
  import { SHORTCUTS, SHORTCUT_GROUPS, type ShortcutEntry } from '../lib/shortcuts'
  import { listBoundCommands } from '../lib/commands'
  import Icon from './Icon.svelte'
  import Kbd from './Kbd.svelte'

  let q = $state('')
  let inputEl = $state<HTMLInputElement | null>(null)
  let bundleEntries = $state<ShortcutEntry[]>([])

  const bundleGroups = $derived.by(() => {
    const seen = new Set<string>()
    const out: string[] = []
    for (const s of bundleEntries) {
      if (!seen.has(s.group)) {
        seen.add(s.group)
        out.push(s.group)
      }
    }
    return out.sort((a, b) => a.localeCompare(b))
  })

  const groups = $derived.by(() => {
    const builtIn = new Set<string>(SHORTCUT_GROUPS)
    const extra = bundleGroups.filter((g) => !builtIn.has(g))
    return [...SHORTCUT_GROUPS, ...extra]
  })

  const allEntries = $derived<ShortcutEntry[]>([...SHORTCUTS, ...bundleEntries])

  const flat = $derived.by(() => {
    const query = q.trim().toLowerCase()
    const match = allEntries.filter(
      (s) =>
        !query ||
        s.label.toLowerCase().includes(query) ||
        s.group.toLowerCase().includes(query) ||
        s.keys.toLowerCase().includes(query) ||
        (s.terms ?? []).some((t) => t.toLowerCase().includes(query)),
    )
    return groups.flatMap((g) => match.filter((s) => s.group === g))
  })

  $effect(() => {
    if ($shortcutsOpen) {
      q = ''
      setTimeout(() => inputEl?.focus(), 0)
      void listBoundCommands()
        .then((cmds) => {
          bundleEntries = cmds.map((c) => ({
            group: c.category,
            label: c.label,
            keys: c.keyEquivalentDisplay!,
            terms: [
              'bundle',
              'command',
              c.category,
              c.keyEquivalent ?? '',
              c.keyEquivalentRaw ?? '',
            ],
          }))
        })
        .catch(() => {
          bundleEntries = []
        })
    }
  })

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      shortcutsOpen.set(false)
    }
  }

  function groupItems(g: string) {
    return flat.filter((s) => s.group === g)
  }
</script>

<div
  class="palette-bg"
  class:open={$shortcutsOpen}
  onclick={(e) => { if (e.target === e.currentTarget) shortcutsOpen.set(false) }}
  role="presentation"
>
  <div class="palette shortcuts-panel" role="dialog" aria-label="Keyboard shortcuts" aria-modal="true">
    <div class="pal-in">
      <Icon name="search" />
      <input
        bind:this={inputEl}
        bind:value={q}
        onkeydown={onKey}
        placeholder="Search keyboard shortcuts…"
        autocomplete="off"
        spellcheck="false"
      />
      <span class="esc">esc</span>
    </div>
    <div class="pal-list">
      {#if flat.length === 0}
        <div class="pal-empty">No matches</div>
      {:else}
        {#each groups as g}
          {@const items = groupItems(g)}
          {#if items.length}
            <div class="pal-g">{g}</div>
            {#each items as s (s.group + s.label + s.keys)}
              <div class="sc-row">
                <span class="pl">{s.label}</span>
                <span class="sc-keys">
                  {#each s.keys.split(/\s+/) as chord}
                    <Kbd class="pk" text={chord} />
                  {/each}
                </span>
              </div>
            {/each}
          {/if}
        {/each}
      {/if}
    </div>
    <div class="pal-foot">
      <span><Kbd class="k" text="⌘⇧/" />shortcuts</span>
      <span><Kbd class="k" text="esc" />close</span>
    </div>
  </div>
</div>
