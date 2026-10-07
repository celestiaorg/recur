<script lang="ts">
  import type { Snippet } from 'svelte'
  import Icon from './Icon.svelte'
  import type { IconName } from '../lib/icons'
  import ProviderIcon from './ProviderIcon.svelte'
  import { prepareFuzzy, fuzzyFilterPrepared } from '../lib/fuzzy'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { clampMenuLeft, menuFixedBox, menuPreferUp, portalMenuMods, portalToBody, watchMenuViewport } from '../lib/menu-space'

  export type SelectOption = {
    value: string
    label: string
    /** Optional group heading. Consecutive options sharing a group render under
     *  one non-selectable header (e.g. one section per model provider). */
    group?: string
    /** Provider slug for brand icon (collapsed trigger + group header). */
    provider?: string
    /** App icon for the option row AND the closed trigger (e.g. chat modes). */
    icon?: IconName
    /** Accent color for the icon + label — at-a-glance identity (chat modes). */
    tint?: string
    /** One-line description rendered after the label in the menu row. */
    hint?: string
  }

  let {
    value = $bindable(''),
    options = [],
    disabled = false,
    label = '',
    size = 'md',
    /** Inline style for the closed-button value (e.g. preview font). */
    valueStyle = '',
    /** Per-option inline style (e.g. each font rendered in its own face). */
    optionStyle,
    /** Extra class on the root (e.g. wider font picker). */
    class: className = '',
    /** Borderless inline trigger (composer footer controls). */
    ghost = false,
    /** Force the menu above the trigger. Otherwise it opens toward the
     *  side with more room (settings rows near the window bottom). */
    up = false,
    /** Show a filter field and fuzzy-match options (long lists like branches). */
    searchable = false,
    /** Toggle several options; the menu stays open. Empty `values` means none
     *  selected (callers treat that as “all” for filters). */
    multiple = false,
    values = $bindable<string[]>([]),
    /** Closed-button text when nothing is selected. */
    placeholder = '',
    onchange,
    onvalues,
    /** Optional override for the closed-button icon. When omitted, the selected
     *  option's provider mark is shown (if any). */
    icon,
  }: {
    value?: string
    options?: SelectOption[]
    disabled?: boolean
    label?: string
    size?: 'md' | 'sm'
    valueStyle?: string
    optionStyle?: (opt: SelectOption) => string
    class?: string
    ghost?: boolean
    up?: boolean
    searchable?: boolean
    multiple?: boolean
    values?: string[]
    placeholder?: string
    onchange?: (value: string) => void
    onvalues?: (values: string[]) => void
    icon?: Snippet
  } = $props()

  let open = $state(false)
  let active = $state(-1)
  let query = $state('')
  let root: HTMLDivElement | undefined = $state()
  let btnEl: HTMLButtonElement | undefined = $state()
  let listEl: HTMLDivElement | undefined = $state()
  let menuEl: HTMLDivElement | undefined = $state()
  let searchEl: HTMLInputElement | undefined = $state()
  // Remaining viewport px for the open menu (html/body overflow is hidden).
  let spacePx = $state(0)
  let menuTop = $state<number | undefined>(undefined)
  let menuBottom = $state<number | undefined>(undefined)
  let menuLeft = $state(0)
  let menuWidth = $state(0)
  // Ignore hover selection until the pointer actually moves — opening (or
  // keyboard-scrolling) under a stationary cursor fires a synthetic
  // pointerenter on whatever row is beneath it.
  let hoverGate = createHoverGate()

  const selectedSet = $derived(new Set(multiple ? values : [value]))
  const chosen = $derived(options.filter((o) => selectedSet.has(o.value)))
  const selected = $derived(chosen[0])
  const display = $derived.by(() => {
    if (!multiple) return selected?.label ?? value ?? ''
    if (!chosen.length) return ''
    return chosen.map((o) => o.label).join(', ')
  })
  const shown = $derived(display || placeholder || '—')
  const selectedProvider = $derived(selected?.provider ?? '')
  const hinted = $derived(options.some((o) => !!o.hint))

  const prepared = $derived(prepareFuzzy(options, (o) => o.label))
  const filtered = $derived.by(() => {
    if (!searchable || !query.trim()) return options
    return fuzzyFilterPrepared(prepared, query, options.length).map((h) => h.item)
  })
  // Content identity — not array identity — so a parent rebuilding `options`
  // with the same values (WorkspaceBaseSelect on workspace.updated) does not
  // re-gate hover.
  const filteredKey = $derived(filtered.map((o) => o.value).join('\0'))

  // Precompute where a group header starts, so the menu can render headings
  // without breaking the flat index that keyboard navigation relies on.
  const rows = $derived(
    filtered.map((o, i) => ({
      opt: o,
      i,
      heading: o.group && o.group !== filtered[i - 1]?.group ? o.group : '',
      headingProvider: o.group && o.group !== filtered[i - 1]?.group ? (o.provider ?? '') : '',
    })),
  )

  const uid = `sel-${Math.random().toString(36).slice(2, 10)}`
  const listId = `${uid}-list`
  function optId(i: number) {
    return `${uid}-opt-${i}`
  }

  function close(restoreFocus = false) {
    open = false
    active = -1
    query = ''
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
      minWidth: portalMenuMods(root, { hinted }).minWidth,
    })
    spacePx = box.spacePx
    menuTop = box.top
    menuBottom = box.bottom
    menuWidth = box.width
    const painted = menuEl?.getBoundingClientRect().width ?? 0
    menuLeft = clampMenuLeft(box.left, Math.max(box.width, painted))
  }

  function openMenu() {
    fitMenu()
    open = true
    query = ''
    active = Math.max(0, options.findIndex((o) => selectedSet.has(o.value)))
    resetHoverGate(hoverGate)
    queueMicrotask(() => {
      if (searchable) {
        searchEl?.focus()
        return
      }
      scrollActive()
    })
  }

  function toggle() {
    if (disabled || !options.length) return
    if (open) close()
    else openMenu()
  }

  function pick(v: string) {
    if (multiple) {
      const cur = values ?? []
      values = cur.includes(v) ? cur.filter((x) => x !== v) : [...cur, v]
      onvalues?.(values)
      return
    }
    value = v
    onchange?.(v)
    close(true)
  }

  function moveActive(delta: number) {
    if (!filtered.length) {
      active = -1
      return
    }
    // Disarm hover so scroll-under-pointer synthetic events cannot snap
    // active back to the row under a stationary cursor.
    resetHoverGate(hoverGate)
    const next = active < 0 ? (delta > 0 ? 0 : filtered.length - 1) : active + delta
    active = Math.max(0, Math.min(filtered.length - 1, next))
    scrollActive()
  }

  function jumpActive(i: number) {
    resetHoverGate(hoverGate)
    active = i
    scrollActive()
  }

  function onOptPointerMove(e: PointerEvent, i: number) {
    if (hoverGateAllows(hoverGate, e.clientX, e.clientY)) active = i
  }

  function onOptPointerEnter(i: number) {
    if (hoverGate.armed) active = i
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
    // Searchable menus put focus in the filter field; the trigger only needs Escape.
    if (searchable) {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        close(true)
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
      if (active >= 0 && filtered[active]) pick(filtered[active].value)
      return
    }
    if (e.key === 'Home') {
      e.preventDefault()
      if (filtered.length) jumpActive(0)
      return
    }
    if (e.key === 'End') {
      e.preventDefault()
      if (filtered.length) jumpActive(filtered.length - 1)
    }
  }

  function onSearchKey(e: KeyboardEvent) {
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
    if (e.key === 'Enter') {
      e.preventDefault()
      if (active >= 0 && filtered[active]) pick(filtered[active].value)
      return
    }
    if (e.key === 'Home' && !query) {
      e.preventDefault()
      if (filtered.length) jumpActive(0)
      return
    }
    if (e.key === 'End' && !query) {
      e.preventDefault()
      if (filtered.length) jumpActive(filtered.length - 1)
    }
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
      // NewTask preventDefault + cycleDialogTab first; restoring here would
      // steal that focus. Elsewhere, put focus back on the trigger before the
      // search input unmounts so Tab continues from this control, not <body>.
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
    return watchMenuViewport(fitMenu, menuEl)
  })

  $effect(() => {
    if (!open || !searchable) return
    void query
    if (!filtered.length) {
      active = -1
      return
    }
    if (active < 0 || active >= filtered.length) active = 0
  })

  // Reset only when the menu opens or its rows change — do not read `active`.
  // Hover writes `active` on every row crossing; putting reset in the clamp
  // effect above would disarm the gate after each highlight (QuickOpen / @-menu
  // keep the same split).
  $effect(() => {
    if (!open || !searchable) return
    void query
    void filteredKey
    resetHoverGate(hoverGate)
  })
</script>

<div class="sel {className}" class:open class:disabled class:sm={size === 'sm'} class:ghost class:up class:searchable class:multiple data-sel-id={uid} bind:this={root}>
  <button
    type="button"
    class="sel-btn"
    bind:this={btnEl}
    {disabled}
    aria-label={label}
    title={shown}
    role="combobox"
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-controls={open ? listId : undefined}
    aria-activedescendant={!searchable && open && active >= 0 ? optId(active) : undefined}
    onclick={toggle}
    onkeydown={onKey}
  >
    {#if icon}
      <span class="sel-ico">{@render icon()}</span>
    {:else if selected?.icon}
      <span class="sel-ico" style:color={selected.tint || undefined}><Icon name={selected.icon} /></span>
    {:else if selectedProvider}
      <span class="sel-ico"><ProviderIcon provider={selectedProvider} /></span>
    {/if}
    <span
      class="sel-val"
      style={valueStyle || undefined}
      style:color={selected?.tint || undefined}
    >{shown}</span>
    {#if size === 'sm'}
      <Icon class="sel-chev" name="chevrons-up-down" />
    {:else}
      <Icon class="sel-chev" name="chevron-down" strokeWidth={2.2} />
    {/if}
  </button>
  {#if open}
    <div
      class="sel-menu {portalMenuMods(root, { hinted }).className}"
      class:has-search={searchable}
      data-sel-id={uid}
      {@attach portalToBody}
      bind:this={menuEl}
      style:--sel-space="{spacePx}px"
      style:top={menuTop != null ? `${menuTop}px` : 'auto'}
      style:bottom={menuBottom != null ? `${menuBottom}px` : 'auto'}
      style:left="{menuLeft}px"
      style:width="{menuWidth}px"
    >
      {#if searchable}
        <div class="sel-search">
          <Icon name="search" />
          <input
            bind:this={searchEl}
            bind:value={query}
            type="text"
            role="searchbox"
            aria-label="Filter {label}"
            aria-controls={listId}
            aria-activedescendant={active >= 0 ? optId(active) : undefined}
            placeholder="Search…"
            spellcheck="false"
            autocomplete="off"
            onkeydown={onSearchKey}
          />
        </div>
      {/if}
      <div class="sel-list" role="listbox" id={listId} aria-label={label} aria-multiselectable={multiple || undefined} bind:this={listEl} tabindex="-1">
        {#each rows as row (row.opt.value)}
          {@const o = row.opt}
          {@const i = row.i}
          {@const on = selectedSet.has(o.value)}
          {#if row.heading}
            <div class="sel-group" role="presentation">
              {#if row.headingProvider}
                <span class="sel-group-ico"><ProviderIcon provider={row.headingProvider} /></span>
              {/if}
              <span>{row.heading}</span>
            </div>
          {/if}
          <button
            type="button"
            class="sel-opt"
            class:on
            role="option"
            id={optId(i)}
            tabindex="-1"
            aria-selected={on}
            data-active={i === active}
            style={optionStyle?.(o) || undefined}
            onpointermove={(e) => onOptPointerMove(e, i)}
            onpointerenter={() => onOptPointerEnter(i)}
            onclick={() => pick(o.value)}
          >
            {#if on}
              <Icon class="sel-check" name="check" strokeWidth={2.4} />
            {:else}
              <span class="sel-check-slot" aria-hidden="true"></span>
            {/if}
            {#if o.icon}
              <span class="sel-opt-ico" style:color={o.tint || undefined}><Icon name={o.icon} /></span>
            {/if}
            <span class="sel-opt-lbl">{o.label}</span>
            {#if o.hint}
              <span class="sel-opt-hint">{o.hint}</span>
            {/if}
          </button>
        {:else}
          <div class="sel-empty">{query.trim() ? 'No matches' : 'No options'}</div>
        {/each}
      </div>
    </div>
  {/if}
</div>
