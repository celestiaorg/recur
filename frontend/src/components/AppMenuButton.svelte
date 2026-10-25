<script lang="ts">
  import { get } from 'svelte/store'
  import type { MenuNode } from '../../bindings/recur/internal/app'
  import { loadMenuSpec, menuState, runMenuAction, themeCatalog } from '../lib/menu'
  import { resolveMenu, type MenuEntry } from '../lib/menu-spec'
  import Icon from './Icon.svelte'

  let open = $state(false)
  let spec = $state<MenuNode[]>([])
  let entries = $state<MenuEntry[]>([])
  let expanded = $state<Record<string, boolean>>({})
  let root = $state<HTMLDivElement>()
  let button = $state<HTMLButtonElement>()
  let panel = $state<HTMLDivElement>()

  void loadMenuSpec().then((s) => (spec = s)).catch(() => {})

  function rows(): HTMLButtonElement[] {
    return panel ? [...panel.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')] : []
  }

  async function show(focusFirst: boolean) {
    entries = resolveMenu(spec, get(menuState), themeCatalog())
    expanded = {}
    open = true
    if (!focusFirst) return
    await Promise.resolve()
    rows()[0]?.focus()
  }

  function close(refocus = false) {
    if (!open) return
    open = false
    if (refocus) button?.focus()
  }

  function activate(e: MenuEntry, path: string) {
    if (e.kind === 'submenu') {
      expanded = { ...expanded, [path]: !expanded[path] }
      return
    }
    close()
    runMenuAction(e)
  }

  function onPanelKey(ev: KeyboardEvent) {
    const list = rows()
    const at = list.indexOf(document.activeElement as HTMLButtonElement)
    const step = (d: number) => list[(at + d + list.length) % list.length]?.focus()
    const row = document.activeElement as HTMLElement | null
    switch (ev.key) {
      case 'ArrowDown': step(1); break
      case 'ArrowUp': step(-1); break
      case 'Home': list[0]?.focus(); break
      case 'End': list[list.length - 1]?.focus(); break
      case 'ArrowRight': {
        const path = row?.dataset.sub
        if (!path || expanded[path]) return
        expanded = { ...expanded, [path]: true }
        break
      }
      case 'ArrowLeft': {
        const path = row?.dataset.sub && expanded[row.dataset.sub] ? row.dataset.sub : row?.dataset.parent
        if (!path) return
        expanded = { ...expanded, [path]: false }
        panel?.querySelector<HTMLButtonElement>(`[data-sub="${path}"]`)?.focus()
        break
      }
      case 'Tab': close(); return
      default: return
    }
    ev.preventDefault()
  }

  function onButtonKey(ev: KeyboardEvent) {
    if (ev.key !== 'ArrowDown') return
    ev.preventDefault()
    if (open) rows()[0]?.focus()
    else void show(true)
  }

  // Alt pressed and released on its own opens the menu (focus-the-menu-bar
  // convention). Any other key or click in between cancels it.
  let altAlone = false
  function onWindowKeydown(ev: KeyboardEvent) {
    if (open && ev.key === 'Escape') {
      ev.preventDefault()
      close(true)
    }
    altAlone = ev.key === 'Alt' && !ev.repeat && !ev.ctrlKey && !ev.shiftKey && !ev.metaKey
  }
  function onWindowKeyup(ev: KeyboardEvent) {
    if (ev.key !== 'Alt' || !altAlone) return
    altAlone = false
    if (open) close(true)
    else void show(true)
  }
  function onWindowPointerdown(ev: PointerEvent) {
    altAlone = false
    if (open && root && !root.contains(ev.target as Node)) close()
  }
</script>

<svelte:window
  onkeydown={onWindowKeydown}
  onkeyup={onWindowKeyup}
  onpointerdown={onWindowPointerdown}
  onblur={() => { altAlone = false; close() }}
/>

{#snippet list(items: MenuEntry[], path: string)}
  {#each items as e, i (i)}
    {@const p = `${path}/${i}`}
    {#if e.kind === 'separator'}
      <div class="app-menu-sep" role="separator"></div>
    {:else if e.kind === 'submenu'}
      <button
        type="button"
        class="merge-menu-item app-menu-sub"
        role="menuitem"
        aria-haspopup="true"
        aria-expanded={!!expanded[p]}
        data-sub={p}
        data-parent={path || undefined}
        onclick={() => activate(e, p)}
      ><span class="mm-check">{expanded[p] ? '▾' : '▸'}</span>{e.label}</button>
      {#if expanded[p]}
        <div class="app-menu-group" role="group">{@render list(e.children, p)}</div>
      {/if}
    {:else}
      {@const radio = /^(app|editor|terminal)-theme:/.test(e.action)}
      <button
        type="button"
        class="merge-menu-item"
        class:on={e.checked}
        role={radio ? 'menuitemradio' : 'menuitem'}
        aria-checked={radio ? e.checked : undefined}
        disabled={e.disabled}
        data-parent={path || undefined}
        onclick={() => activate(e, p)}
      ><span class="mm-check">{e.checked ? '✓' : ''}</span>{e.label}{#if e.accel}<span class="app-menu-accel">{e.accel}</span>{/if}</button>
    {/if}
  {/each}
{/snippet}

<div class="tb-menu-wrap" bind:this={root}>
  <button
    type="button"
    class="tb-rail tb-menu"
    class:on={open}
    bind:this={button}
    aria-label="Application menu"
    aria-haspopup="menu"
    aria-expanded={open}
    title="Menu (Alt)"
    onclick={() => (open ? close() : void show(false))}
    onkeydown={onButtonKey}
  ><Icon name="menu" /></button>
  {#if open}
    <div class="merge-menu app-menu" role="menu" aria-label="Application menu" tabindex="-1" bind:this={panel} onkeydown={onPanelKey}>
      {@render list(entries, '')}
    </div>
  {/if}
</div>
