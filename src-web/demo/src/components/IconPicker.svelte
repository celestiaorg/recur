<script lang="ts">
  import { prepareFuzzy, fuzzyFilterPrepared } from '../lib/fuzzy'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { lucideIcons, lucideSuggested } from '../lib/lucide-icons'
  import { clampMenuLeft, menuFixedBox, menuPreferUp, portalToBody, watchMenuViewport } from '../lib/menu-space'
  import LucideIcon from './LucideIcon.svelte'
  import Icon from './Icon.svelte'

  const COLS = 8
  const SEARCH_LIMIT = 80

  let {
    value = $bindable(''),
    disabled = false,
    onchange,
  }: {
    value?: string
    disabled?: boolean
    onchange?: (value: string) => void
  } = $props()

  let open = $state(false)
  let active = $state(0)
  let query = $state('')
  let root: HTMLDivElement | undefined = $state()
  let btnEl: HTMLButtonElement | undefined = $state()
  let menuEl: HTMLDivElement | undefined = $state()
  let listEl: HTMLDivElement | undefined = $state()
  let searchEl: HTMLInputElement | undefined = $state()
  let spacePx = $state(0)
  let menuTop = $state<number | undefined>(undefined)
  let menuBottom = $state<number | undefined>(undefined)
  let menuLeft = $state(0)
  let menuWidth = $state(300)
  let hoverGate = createHoverGate()

  const uid = `ico-${Math.random().toString(36).slice(2, 10)}`
  const listId = `${uid}-list`
  function optId(i: number) {
    return `${uid}-opt-${i}`
  }
  const prepared = $derived(prepareFuzzy([...lucideIcons], (e) => `${e.name} ${e.aliases.join(' ')}`))
  const shown = $derived.by(() => {
    const q = query.trim()
    if (!q) {
      const cur = lucideIcons.find((e) => e.name === value || e.aliases.includes(value))
      if (cur && !lucideSuggested.includes(cur)) return [cur, ...lucideSuggested]
      return [...lucideSuggested]
    }
    return fuzzyFilterPrepared(prepared, q, SEARCH_LIMIT).map((h) => h.item)
  })
  const shownKey = $derived(shown.map((e) => e.name).join('\0'))

  function close(restoreFocus = false) {
    open = false
    active = 0
    query = ''
    if (restoreFocus) btnEl?.focus()
  }

  function fitMenu() {
    if (!btnEl) return
    const rect = btnEl.getBoundingClientRect()
    const box = menuFixedBox(rect, { minWidth: 300, up: menuPreferUp(rect) })
    spacePx = box.spacePx
    menuTop = box.top
    menuBottom = box.bottom
    menuWidth = Math.max(box.width, 300)
    const painted = menuEl?.getBoundingClientRect().width ?? 0
    menuLeft = clampMenuLeft(box.left, Math.max(menuWidth, painted))
  }

  function openMenu() {
    fitMenu()
    open = true
    query = ''
    const i = shown.findIndex((e) => e.name === value || e.aliases.includes(value))
    active = Math.max(0, i)
    resetHoverGate(hoverGate)
    queueMicrotask(() => searchEl?.focus())
  }

  function toggle() {
    if (disabled) return
    if (open) close()
    else openMenu()
  }

  function pick(name: string) {
    value = name
    onchange?.(name)
    close(true)
  }

  function scrollActive() {
    queueMicrotask(() => listEl?.querySelector<HTMLElement>('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }))
  }

  function jump(i: number) {
    if (!shown.length) {
      active = 0
      return
    }
    resetHoverGate(hoverGate)
    active = Math.max(0, Math.min(shown.length - 1, i))
    scrollActive()
  }

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close(true)
      return
    }
    if (e.key === 'ArrowRight') { e.preventDefault(); jump(active + 1); return }
    if (e.key === 'ArrowLeft') { e.preventDefault(); jump(active - 1); return }
    if (e.key === 'ArrowDown') { e.preventDefault(); jump(active + COLS); return }
    if (e.key === 'ArrowUp') { e.preventDefault(); jump(active - COLS); return }
    if (e.key === 'Enter') {
      e.preventDefault()
      const hit = shown[active]
      if (hit) pick(hit.name)
      return
    }
    if (e.key === 'Home') { e.preventDefault(); jump(0); return }
    if (e.key === 'End') { e.preventDefault(); jump(shown.length - 1) }
  }

  function onDocPointer(e: PointerEvent) {
    if (!open || !root) return
    const t = e.target as Node
    if (root.contains(t) || menuEl?.contains(t)) return
    close()
  }

  $effect(() => {
    if (!open) return
    const onTab = (e: KeyboardEvent) => {
      if (e.key !== 'Tab') return
      close(!e.defaultPrevented)
    }
    window.addEventListener('keydown', onTab, true)
    document.addEventListener('pointerdown', onDocPointer)
    return () => {
      window.removeEventListener('keydown', onTab, true)
      document.removeEventListener('pointerdown', onDocPointer)
    }
  })

  $effect(() => {
    if (!open || !btnEl) return
    fitMenu()
    return watchMenuViewport(fitMenu, menuEl)
  })

  $effect(() => {
    if (!open) return
    void query
    void shownKey
    if (!shown.length) {
      active = 0
      return
    }
    if (active >= shown.length) active = 0
    resetHoverGate(hoverGate)
  })
</script>

<div class="ico-pick sel sm" class:open class:disabled data-sel-id={uid} bind:this={root}>
  <button
    type="button"
    class="sel-btn"
    bind:this={btnEl}
    {disabled}
    aria-label="Lucide icon"
    aria-haspopup="listbox"
    aria-expanded={open}
    onclick={toggle}
  >
    <span class="sel-ico"><LucideIcon name={value || 'box'} /></span>
    <span class="sel-val">{value || 'box'}</span>
    <Icon class="sel-chev" name="chevron-down" strokeWidth={2.2} />
  </button>
  {#if open}
    <div
      class="ico-menu"
      data-sel-id={uid}
      {@attach portalToBody}
      bind:this={menuEl}
      style:--sel-space="{spacePx}px"
      style:top={menuTop != null ? `${menuTop}px` : 'auto'}
      style:bottom={menuBottom != null ? `${menuBottom}px` : 'auto'}
      style:left="{menuLeft}px"
      style:width="{menuWidth}px"
    >
      <div class="sel-search">
        <Icon name="search" />
        <input
          bind:this={searchEl}
          bind:value={query}
          type="text"
          role="searchbox"
          aria-label="Filter Lucide icons"
          aria-controls={listId}
          aria-activedescendant={shown.length && active >= 0 ? optId(active) : undefined}
          placeholder="Search icons…"
          spellcheck="false"
          autocomplete="off"
          onkeydown={onSearchKey}
        />
      </div>
      <div class="ico-grid" id={listId} role="listbox" aria-label="Lucide icons" bind:this={listEl}>
        {#each shown as entry, i (entry.name)}
          <button
            type="button"
            class="ico-cell"
            class:on={entry.name === value || entry.aliases.includes(value)}
            id={optId(i)}
            role="option"
            tabindex="-1"
            title={entry.name}
            aria-label={entry.name}
            aria-selected={entry.name === value || entry.aliases.includes(value)}
            data-active={i === active}
            onpointermove={(e) => { if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) active = i }}
            onclick={() => pick(entry.name)}
          >
            <LucideIcon name={entry.name} />
          </button>
        {:else}
          <div class="sel-empty">No matches</div>
        {/each}
      </div>
    </div>
  {/if}
</div>
