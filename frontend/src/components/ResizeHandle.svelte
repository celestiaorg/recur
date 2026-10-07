<script lang="ts">
  import { get } from 'svelte/store'
  import {
    panelWidths, setPanelWidth, setPanelWidthLive, persistPanelWidths, type PanelKey,
  } from '../lib/stores'

  let {
    panel,
    /**
     * Which edge of the panel this handle sits on. For a horizontal drag (axis 'x')
     * 'end' grows when dragging right (left panels) and 'start' grows when dragging
     * left (right panels); for a vertical drag (axis 'y') 'start' grows when
     * dragging up, which is what a bottom dock wants.
     */
    edge = 'end',
    /** 'x' resizes a panel's width, 'y' its height (the terminal dock). */
    axis = 'x',
  }: {
    panel: PanelKey
    edge?: 'start' | 'end'
    axis?: 'x' | 'y'
  } = $props()

  let dragging = $state(false)

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return
    e.preventDefault()
    const el = e.currentTarget as HTMLElement
    el.setPointerCapture(e.pointerId)
    dragging = true
    document.body.classList.add('resizing-panels')
    if (axis === 'y') document.body.classList.add('resizing-rows')
    const startX = axis === 'y' ? e.clientY : e.clientX
    const startW = get(panelWidths)[panel]

    // Layout-aware ceiling for the Observatory docks: growing a dock shrinks the
    // code column (the grid's 1fr middle, .obs-main) by the same amount, so stop the
    // drag once the code column would fall below a usable minimum — relative to the
    // CODE-EDITOR view (left + code + right, .obs-body), not the whole window. The
    // floor is a modest ~400px (or 30% on a small window, whichever is smaller), so
    // a dock can grow generously by shrinking code down to that minimum WITHOUT
    // having to shrink the other dock first; only when code is already near the
    // floor do the two docks compete. The PR-detail agent dock is portaled out of
    // .obs-body, so the same rule is applied against #v-prdetail / .prd-col.
    let runtimeMax = Infinity
    const body = el.closest('.obs-body') as HTMLElement | null
    const main = body?.querySelector('.obs-main') as HTMLElement | null
    if (axis === 'y') {
      // The terminal dock grows upward inside .obs-main, so its ceiling is measured
      // against the editor's HEIGHT — same "leave the code a usable minimum" rule,
      // just rotated. 160px is roughly the point below which the editor stops being
      // an editor.
      if (main) {
        const editorFloor = Math.min(Math.round(main.clientHeight * 0.25), 160)
        runtimeMax = Math.max(startW, main.clientHeight - editorFloor)
      }
    } else if (body && main) {
      const codeFloor = Math.min(Math.round(body.clientWidth * 0.3), 400)
      runtimeMax = Math.max(startW, startW + main.clientWidth - codeFloor)
    } else {
      const pr = el.closest('#v-prdetail') as HTMLElement | null
      const prCol = pr?.querySelector('.prd-col') as HTMLElement | null
      if (pr && prCol) {
        const floor = Math.min(Math.round(pr.clientWidth * 0.4), 360)
        runtimeMax = Math.max(startW, startW + prCol.clientWidth - floor)
      }
    }

    // One width update per frame, persisted once on release — pointermove can
    // outpace the compositor, and each call rewrote every panel CSS var and
    // re-serialised the width map into localStorage.
    let pendingX = startX
    let raf = 0
    const apply = () => {
      raf = 0
      const dx = pendingX - startX
      const want = edge === 'end' ? startW + dx : startW - dx
      setPanelWidthLive(panel, Math.min(want, runtimeMax))
    }
    const move = (ev: PointerEvent) => {
      pendingX = axis === 'y' ? ev.clientY : ev.clientX
      if (!raf) raf = requestAnimationFrame(apply)
    }
    const up = (ev: PointerEvent) => {
      try { el.releasePointerCapture(ev.pointerId) } catch { /* already released */ }
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      if (raf) {
        cancelAnimationFrame(raf)
        apply()
      }
      dragging = false
      document.body.classList.remove('resizing-panels', 'resizing-rows')
      persistPanelWidths()
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  function onKeyDown(e: KeyboardEvent) {
    const step = e.shiftKey ? 24 : 8
    const [less, more] = axis === 'y' ? ['ArrowUp', 'ArrowDown'] : ['ArrowLeft', 'ArrowRight']
    let delta = 0
    if (e.key === less) delta = edge === 'end' ? -step : step
    else if (e.key === more) delta = edge === 'end' ? step : -step
    else return
    e.preventDefault()
    setPanelWidth(panel, get(panelWidths)[panel] + delta)
  }
</script>

<button
  type="button"
  class:vsplit={axis === 'x'}
  class:hsplit={axis === 'y'}
  class:edge-start={edge === 'start'}
  class:edge-end={edge === 'end'}
  class:dragging
  aria-label="Resize panel"
  title="Drag to resize"
  onpointerdown={onPointerDown}
  onkeydown={onKeyDown}
></button>
