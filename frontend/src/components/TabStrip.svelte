<script lang="ts">
  // Shared chrome tab bar: a horizontally scrolling tab row with pinned trailing
  // (and optional leading) actions that stay in view when the tabs overflow.
  //
  // Used by the Observatory file strip and the terminal dock — anywhere else that
  // grows a tab list should reuse this rather than reimplementing the overflow
  // split. The scroll container is the ONLY child that gets overflow-x:auto; the
  // trailing slot sits outside it so buttons never scroll off-screen.
  //
  // Pass `activeKey` (the id/path of the selected tab). The strip keeps that
  // chip in view across three moments that used to each need a hand-rolled
  // scroll call: the key changing, the chip first appearing in the DOM (a newly
  // opened file — selectedPath is set before ensureOpenTab adds the chip), and
  // the chip growing (a terminal tab renaming from "zsh" to a longer OSC title).
  //
  // Workspace / view restore is different: `restoreScroll` puts the strip back
  // where the user left it instead of chasing the active chip. A user-driven
  // scroll (including a WebKit-mapped vertical wheel on this overflow-x-only
  // strip) pins that offset; a chip click does not. Opening a file still
  // chases — bump `revealNonce` when the key does not change. Chase eases the
  // strip's own scrollLeft (WKWebView will not animate Element.scrollTo on
  // overflow strips, and Element.scrollIntoView yanks the editor). Restore snaps.
  import { tick, type Snippet } from 'svelte'
  import { snapStripScroll, scrollChildIntoStrip, stripScrollIsProgrammatic } from '../lib/tab-strip-scroll'

  let {
    label,
    chrome = false,
    class: className = '',
    activeKey = undefined as string | number | undefined,
    restoreScroll = undefined as { left: number; id: number } | undefined,
    revealNonce = 0,
    stripEl = $bindable<HTMLDivElement | undefined>(undefined),
    children,
    trailing,
    leading,
  }: {
    /** Accessible name for the tablist. */
    label: string
    /** Standalone header chrome (height / surface / bottom border) — terminal dock. */
    chrome?: boolean
    class?: string
    /** Selected tab id/path. Changing it scrolls that chip into view if needed. */
    activeKey?: string | number | undefined
    /**
     * Keep-alive restore: apply this scrollLeft and do not chase the active
     * chip until the user opens/selects a file (`activeKey` or `revealNonce`).
     */
    restoreScroll?: { left: number; id: number } | undefined
    /** Bump to reveal the active chip even when `activeKey` is unchanged. */
    revealNonce?: number
    /** The scrolling tab row — exposed for callers that need the DOM node. */
    stripEl?: HTMLDivElement | undefined
    /** Tab chips (and anything else that should scroll with them). */
    children: Snippet
    /** Pinned right-side actions; never scrolls with the tabs. */
    trailing?: Snippet
    leading?: Snippet
  } = $props()

  let appliedRestoreId = -1
  let lastRevealKey: string | number | undefined
  let lastRevealNonce = 0
  let holdRestore = false
  let userPinned = false

  $effect(() => {
    const strip = stripEl
    if (!strip) return
    const pin = () => {
      if (stripScrollIsProgrammatic(strip)) return
      userPinned = true
    }
    strip.addEventListener('scroll', pin, { passive: true })
    return () => {
      strip.removeEventListener('scroll', pin)
    }
  })

  $effect(() => {
    const key = activeKey
    if (key === undefined || key === '') return
    const strip = stripEl
    if (!strip) return
    const restore = restoreScroll
    const nonce = revealNonce
    const userReveal = nonce !== lastRevealNonce
    lastRevealNonce = nonce

    // One-shot keep-alive: pin the saved offset while this workspace's chips
    // land. Opening a file (new key or revealNonce) cancels the pin.
    if (restore && restore.id !== appliedRestoreId && !userReveal) {
      appliedRestoreId = restore.id
      holdRestore = true
      userPinned = true
      lastRevealKey = key
      const apply = () => { snapStripScroll(strip, restore.left) }
      apply()
      void tick().then(apply)
      const mo = new MutationObserver(apply)
      mo.observe(strip, { childList: true, subtree: true })
      let frames = 0
      let raf = 0
      const settle = () => {
        apply()
        frames += 1
        if (frames < 3) raf = requestAnimationFrame(settle)
        else mo.disconnect()
      }
      raf = requestAnimationFrame(settle)
      return () => {
        mo.disconnect()
        cancelAnimationFrame(raf)
      }
    }

    if ((holdRestore || userPinned) && !userReveal && key === lastRevealKey) return
    holdRestore = false
    userPinned = false
    lastRevealKey = key

    let observed: HTMLElement | undefined
    const scroll = (el: HTMLElement) => {
      if (holdRestore || userPinned) return
      if (document.body.classList.contains('dragging-tabs')) return
      scrollChildIntoStrip(strip, el, 'smooth')
    }
    const ro = new ResizeObserver(() => {
      if (observed) scroll(observed)
    })

    const attach = () => {
      const active = strip.querySelector<HTMLElement>('.tab-chip.on')
      if (!active) return
      if (active !== observed) {
        if (observed) ro.unobserve(observed)
        observed = active
        ro.observe(active)
      }
      scroll(active)
    }

    void tick().then(attach)
    requestAnimationFrame(attach)
    // Chip may land after activeKey (openPath sets selectedPath, then await, then
    // ensureOpenTab). Watch the strip so the first appearance still scrolls.
    const mo = new MutationObserver(() => {
      if (document.body.classList.contains('dragging-tabs')) return
      void tick().then(attach)
    })
    mo.observe(strip, { childList: true, subtree: true })

    return () => {
      mo.disconnect()
      ro.disconnect()
    }
  })
</script>

<div class="tab-strip {className}" class:chrome role="presentation">
  {#if leading}
    <div class="tab-strip-leading">
      {@render leading()}
    </div>
  {/if}
  <div
    class="tab-strip-scroll"
    role="tablist"
    aria-label={label}
    bind:this={stripEl}
  >
    {@render children()}
  </div>
  {#if trailing}
    <div class="tab-strip-trailing">
      {@render trailing()}
    </div>
  {/if}
</div>

<style>
  .tab-strip {
    display: flex;
    align-items: stretch;
    gap: 6px;
    padding: 0 4px 0 8px;
    min-width: 0;
    min-height: 0;
    overflow: visible;
  }
  .tab-strip.chrome {
    flex: none;
    height: var(--obs-chrome-h, 40px);
    background: var(--pane);
    border-bottom: 1px solid var(--line);
    overflow: hidden;
  }

  .tab-strip-scroll {
    flex: 1;
    min-width: 0;
    align-self: stretch;
    display: flex;
    align-items: stretch;
    gap: 2px;
    padding: 0;
    overflow-x: auto;
    overflow-y: hidden;
    flex-wrap: nowrap;
    scrollbar-width: thin;
    scrollbar-color: var(--line) transparent;
  }
  .tab-strip.chrome .tab-strip-scroll {
    min-width: 80px;
  }
  .tab-strip-scroll::-webkit-scrollbar { height: 4px; }
  .tab-strip-scroll::-webkit-scrollbar-thumb { background: var(--line); border-radius: 2px; }
  .tab-strip-scroll::-webkit-scrollbar-track { background: transparent; }

  .tab-strip-leading,
  .tab-strip-trailing {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
    align-self: center;
  }

  /* Square icon actions that ride the strip (+, clear, maximize, …). */
  :global(.tab-strip-act) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--faint);
  }
  :global(.tab-strip-act:hover:not(:disabled)) {
    color: var(--text);
    background: var(--pane-2);
  }
  :global(.tab-strip-act:disabled) {
    color: var(--faint);
    opacity: .5;
    cursor: default;
  }
  :global(.tab-strip-act.on),
  :global(.tab-strip-act[aria-pressed="true"]) {
    color: var(--amber);
    background: var(--amber-d);
  }
  :global(.tab-strip-act svg) {
    display: block;
    width: 15px;
    height: 15px;
  }

  :global(.tab-chip-ph) {
    flex: none;
    height: 100%;
    box-sizing: border-box;
    background: var(--pane-2);
    outline: 1px dashed var(--line);
    outline-offset: -2px;
    pointer-events: none;
  }
  :global(.tab-chip-float) {
    position: fixed;
    z-index: 400;
    pointer-events: none;
    display: flex;
    align-items: stretch;
  }
  :global(.tab-chip-float .tab-chip) {
    height: 100%;
    max-width: none;
  }
  :global(body.dragging-tabs) {
    cursor: grabbing !important;
    user-select: none;
  }
  :global(body.dragging-tabs *) {
    cursor: grabbing !important;
  }
</style>
