<script lang="ts">
  /**
   * Disambiguation menu when several bundle commands or snippets share a chord
   * (or Tab trigger). Number keys 1–9 pick a row; Esc closes.
   */
  import { closeKeybindingMenu, keybindingMenu, type KeybindingMenuItem } from '../lib/keybinding-menu'

  let sel = $state(0)

  const items = $derived($keybindingMenu?.items ?? [])
  const open = $derived(!!$keybindingMenu)

  type Section = { title: string; rows: { item: KeybindingMenuItem; idx: number }[] }

  const sections = $derived.by(() => {
    const byCat = new Map<string, KeybindingMenuItem[]>()
    for (const it of items) {
      const list = byCat.get(it.category) ?? []
      list.push(it)
      byCat.set(it.category, list)
    }
    const cats = [...byCat.keys()].sort((a, b) => a.localeCompare(b))
    const out: Section[] = []
    let idx = 0
    for (const title of cats) {
      const rows = (byCat.get(title) ?? [])
        .slice()
        .sort((a, b) => (a.order !== b.order ? a.order - b.order : a.label.localeCompare(b.label)))
        .map((item) => {
          const row = { item, idx }
          idx++
          return row
        })
      if (rows.length) out.push({ title, rows })
    }
    return out
  })

  /** Display order (section rows) — number keys / Enter / click index into this. */
  const flat = $derived(sections.flatMap((s) => s.rows.map((i) => i.item)))

  $effect(() => {
    if (open) sel = 0
  })

  $effect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        closeKeybindingMenu()
        return
      }
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        if (e.metaKey || e.ctrlKey || e.altKey) return
        e.preventDefault()
        sel = e.key === 'ArrowDown'
          ? Math.min(flat.length - 1, sel + 1)
          : Math.max(0, sel - 1)
        return
      }
      if (e.key === 'Enter') {
        e.preventDefault()
        runAt(sel)
        return
      }
      const n = e.key >= '1' && e.key <= '9' ? Number(e.key) : -1
      if (n >= 1 && n <= Math.min(9, flat.length)) {
        e.preventDefault()
        e.stopPropagation()
        runAt(n - 1)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })

  function runAt(i: number) {
    const item = flat[i]
    const pick = $keybindingMenu?.onPick
    if (!item || !pick) return
    closeKeybindingMenu()
    pick(item)
  }
</script>

{#if open}
  <div
    class="kb-menu-bg"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget) closeKeybindingMenu()
    }}
  >
    <div class="kb-menu" role="menu" aria-label="Choose command">
      {#each sections as section}
        <div class="kb-g">{section.title}</div>
        {#each section.rows as { item, idx } (item.id)}
          <div
            class="kb-row"
            class:sel={idx === sel}
            role="menuitem"
            tabindex="-1"
            onmousemove={() => (sel = idx)}
            onclick={() => runAt(idx)}
            onkeydown={(e) => e.key === 'Enter' && runAt(idx)}
          >
            <span class="kb-label">{item.label}</span>
            {#if idx < 9}
              <span class="kb-num">{idx + 1}</span>
            {/if}
          </div>
        {/each}
      {/each}
    </div>
  </div>
{/if}

<style>
  .kb-menu-bg {
    position: fixed;
    inset: 0;
    z-index: 210;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(5, 7, 10, 0.35);
  }
  .kb-menu {
    min-width: 260px;
    max-width: min(420px, 92vw);
    max-height: 70vh;
    overflow: auto;
    background: var(--pane);
    border: 1px solid var(--line-bright);
    border-radius: 10px;
    box-shadow: 0 20px 60px -16px rgba(0, 0, 0, 0.7);
    padding: 6px 0;
  }
  .kb-g {
    padding: 8px 14px 3px;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .kb-g + .kb-g,
  .kb-row + .kb-g {
    border-top: 1px solid var(--line);
    margin-top: 4px;
    padding-top: 8px;
  }
  .kb-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 5px 14px;
    cursor: default;
    min-height: 26px;
  }
  .kb-row.sel {
    background: var(--sel, rgba(255, 255, 255, 0.08));
  }
  .kb-label {
    flex: 1;
    font-size: 13px;
    line-height: 1.3;
    color: var(--text);
  }
  .kb-num {
    flex: none;
    font-size: 11px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
    min-width: 1ch;
    text-align: right;
  }
</style>
