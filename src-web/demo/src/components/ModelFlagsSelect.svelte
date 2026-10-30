<script lang="ts">
  import Icon from './Icon.svelte'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { clampMenuLeft, menuFixedBox, menuPreferUp, portalMenuMods, portalToBody, watchMenuViewport } from '../lib/menu-space'

  /** T3-style multi-dimension flags picker: summary trigger + sectioned menu
   *  that stays open across picks (one selection per dimension).
   *
   *  Collapsed trigger matches Select ghost (solo chat-dock model picker). */

  export type FlagOption = { value: string; label: string; default?: boolean }
  export type FlagDimension = { id: string; label: string; options: FlagOption[] }

  let {
    dimensions = [],
    value = {},
    disabled = false,
    label = 'Model flags',
    ghost = false,
    /** Open the menu above the trigger (chat dock). */
    up = false,
    onchange,
  }: {
    dimensions?: FlagDimension[]
    value?: Record<string, string>
    disabled?: boolean
    label?: string
    ghost?: boolean
    up?: boolean
    onchange?: (next: Record<string, string>) => void
  } = $props()

  let open = $state(false)
  let active = $state(-1)
  let root: HTMLDivElement | undefined = $state()
  let btnEl: HTMLButtonElement | undefined = $state()
  let listEl: HTMLDivElement | undefined = $state()
  // Remaining viewport px for the open menu (html/body overflow is hidden).
  let spacePx = $state(0)
  let menuTop = $state<number | undefined>(undefined)
  let menuBottom = $state<number | undefined>(undefined)
  let menuLeft = $state(0)
  let menuWidth = $state(0)
  // Ignore hover selection until the pointer actually moves: opening (or
  // keyboard-scrolling) under a stationary cursor fires a synthetic
  // pointerenter on whatever row is beneath it.
  let hoverGate = createHoverGate()

  const selected = $derived(resolveFlags(dimensions, value))

  type FlagRow = { dimId: string; value: string }
  const rows = $derived(
    dimensions.flatMap((d) => d.options.map((o): FlagRow => ({ dimId: d.id, value: o.value }))),
  )

  const uid = `flags-${Math.random().toString(36).slice(2, 10)}`
  const listId = `${uid}-list`
  function optId(i: number) {
    return `${uid}-opt-${i}`
  }

  const tokens = $derived(
    dimensions
      .map((d) => summaryToken(d, selected[d.id]))
      .filter((t): t is string => !!t),
  )

  const summary = $derived(tokens.join(' · ') || '-')

  /** Collapsed chip text for one dim. Bool dims (Fast/Thinking): show the
   *  dimension name when On, omit when Off. Other dims show the option label. */
  function summaryToken(d: FlagDimension, v: string | undefined): string {
    if (!v) return ''
    if (isBoolDim(d)) {
      return v === 'true' ? d.label : ''
    }
    const opt = d.options.find((o) => o.value === v)
    return opt?.label ?? v
  }

  function isBoolDim(d: FlagDimension): boolean {
    if (d.options.length !== 2) return false
    const vals = new Set(d.options.map((o) => o.value))
    return vals.has('true') && vals.has('false')
  }

  function resolveFlags(dims: FlagDimension[], cur: Record<string, string>): Record<string, string> {
    const out: Record<string, string> = {}
    for (const d of dims) {
      const allowed = new Set(d.options.map((o) => o.value))
      if (cur[d.id] && allowed.has(cur[d.id])) {
        out[d.id] = cur[d.id]
        continue
      }
      const def = d.options.find((o) => o.default) ?? d.options[0]
      if (def) out[d.id] = def.value
    }
    return out
  }

  function close(restoreFocus = false) {
    open = false
    active = -1
    if (restoreFocus) btnEl?.focus()
  }

  function scrollActive() {
    queueMicrotask(() => listEl?.querySelector<HTMLElement>('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }))
  }

  function fitMenu() {
    if (!btnEl) return
    const rect = btnEl.getBoundingClientRect()
    const box = menuFixedBox(rect, {
      up: up || menuPreferUp(rect),
      minWidth: portalMenuMods(root).minWidth,
    })
    spacePx = box.spacePx
    menuTop = box.top
    menuBottom = box.bottom
    menuWidth = box.width
    const painted = listEl?.getBoundingClientRect().width ?? 0
    menuLeft = clampMenuLeft(box.left, Math.max(box.width, painted))
  }

  function openMenu() {
    if (disabled || !dimensions.length) return
    fitMenu()
    open = true
    const i = rows.findIndex((r) => selected[r.dimId] === r.value)
    active = i >= 0 ? i : 0
    resetHoverGate(hoverGate)
    scrollActive()
  }

  function toggle() {
    if (disabled || !dimensions.length) return
    if (open) close()
    else openMenu()
  }

  function pick(dimId: string, optValue: string) {
    const next = { ...selected, [dimId]: optValue }
    onchange?.(next)
  }

  function moveActive(delta: number) {
    if (!rows.length) {
      active = -1
      return
    }
    resetHoverGate(hoverGate)
    const next = active < 0 ? (delta > 0 ? 0 : rows.length - 1) : active + delta
    active = Math.max(0, Math.min(rows.length - 1, next))
    scrollActive()
  }

  function jumpActive(i: number) {
    resetHoverGate(hoverGate)
    active = i
    scrollActive()
  }

  function onOptPointerMove(e: PointerEvent, i: number) {
    if (i >= 0 && hoverGateAllows(hoverGate, e.clientX, e.clientY)) active = i
  }

  function onOptPointerEnter(i: number) {
    if (i >= 0 && hoverGate.armed) active = i
  }

  function onKey(e: KeyboardEvent) {
    if (disabled) return
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        openMenu()
      }
      return
    }
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close(true)
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      moveActive(1)
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      moveActive(-1)
      return
    }
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      const row = rows[active]
      if (row) pick(row.dimId, row.value)
      return
    }
    if (e.key === 'Home') {
      e.preventDefault()
      if (rows.length) jumpActive(0)
      return
    }
    if (e.key === 'End') {
      e.preventDefault()
      if (rows.length) jumpActive(rows.length - 1)
    }
  }

  function onDocPointer(e: PointerEvent) {
    if (!open || !root) return
    const t = e.target as Node
    if (root.contains(t) || listEl?.contains(t)) return
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
    if (!open || !btnEl || !root) return
    fitMenu()
    return watchMenuViewport(fitMenu, listEl)
  })
</script>

<div class="sel flags-sel" class:open class:disabled class:ghost class:up data-sel-id={uid} bind:this={root}>
  <button
    type="button"
    class="sel-btn"
    bind:this={btnEl}
    {disabled}
    aria-label={label}
    role="combobox"
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-controls={open ? listId : undefined}
    aria-activedescendant={open && active >= 0 ? optId(active) : undefined}
    onclick={toggle}
    onkeydown={onKey}
  >
    <span class="sel-val">{summary}</span>
    <Icon class="sel-chev" name="chevron-down" strokeWidth={2.2} />
  </button>
  {#if open}
    <div
      class="sel-menu flags-menu {portalMenuMods(root).className}"
      role="listbox"
      id={listId}
      data-sel-id={uid}
      aria-label={label}
      tabindex="-1"
      {@attach portalToBody}
      bind:this={listEl}
      style:--sel-space="{spacePx}px"
      style:top={menuTop != null ? `${menuTop}px` : 'auto'}
      style:bottom={menuBottom != null ? `${menuBottom}px` : 'auto'}
      style:left="{menuLeft}px"
      style:width="{menuWidth}px"
    >
      {#each dimensions as dim (dim.id)}
        <div class="sel-group" role="presentation">{dim.label}</div>
        {#each dim.options as opt (opt.value)}
          {@const on = selected[dim.id] === opt.value}
          {@const i = rows.findIndex((r) => r.dimId === dim.id && r.value === opt.value)}
          <button
            type="button"
            class="sel-opt"
            class:on
            role="option"
            id={optId(i)}
            tabindex="-1"
            aria-selected={on}
            data-active={i === active}
            onpointermove={(e) => onOptPointerMove(e, i)}
            onpointerenter={() => onOptPointerEnter(i)}
            onclick={() => pick(dim.id, opt.value)}
          >
            {#if on}
              <Icon class="sel-check" name="check" strokeWidth={2.4} />
            {:else}
              <span class="sel-check-slot" aria-hidden="true"></span>
            {/if}
            <span class="sel-opt-lbl">{opt.label}{opt.default ? ' (default)' : ''}</span>
          </button>
        {/each}
      {/each}
    </div>
  {/if}
</div>
