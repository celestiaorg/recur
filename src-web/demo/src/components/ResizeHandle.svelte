<script lang="ts">
  import { get } from 'svelte/store'
  import {
    panelWidths, setPanelWidthLive, type PanelKey,
  } from '../lib/stores'

  let {
    panel,
    /**
     * Which edge of the panel this handle sits on: 'end' grows when dragging
     * right (left panels) and 'start' grows when dragging left (right panels).
     */
    edge = 'end',
  }: {
    panel: PanelKey
    edge?: 'start' | 'end'
  } = $props()

  let dragging = $state(false)

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return
    e.preventDefault()
    const el = e.currentTarget as HTMLElement
    el.setPointerCapture(e.pointerId)
    dragging = true
    document.body.classList.add('resizing-panels')
    const startX = e.clientX
    const startW = get(panelWidths)[panel]

    // Growing an Observatory dock shrinks the code column, so stop once code
    // would fall below a usable minimum (~400px, or 30% on a small window).
    let runtimeMax = Infinity
    const body = el.closest('.obs-body') as HTMLElement | null
    const main = body?.querySelector('.obs-main') as HTMLElement | null
    if (body && main) {
      const codeFloor = Math.min(Math.round(body.clientWidth * 0.3), 400)
      runtimeMax = Math.max(startW, startW + main.clientWidth - codeFloor)
    }

    // One width update per frame: pointermove can outpace the compositor.
    let pendingX = startX
    let raf = 0
    const apply = () => {
      raf = 0
      const dx = pendingX - startX
      const want = edge === 'end' ? startW + dx : startW - dx
      setPanelWidthLive(panel, Math.min(want, runtimeMax))
    }
    const move = (ev: PointerEvent) => {
      pendingX = ev.clientX
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
      document.body.classList.remove('resizing-panels')
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  function onKeyDown(e: KeyboardEvent) {
    const step = e.shiftKey ? 24 : 8
    let delta = 0
    if (e.key === 'ArrowLeft') delta = edge === 'end' ? -step : step
    else if (e.key === 'ArrowRight') delta = edge === 'end' ? step : -step
    else return
    e.preventDefault()
    setPanelWidthLive(panel, get(panelWidths)[panel] + delta)
  }
</script>

<button
  type="button"
  class="vsplit"
  class:edge-start={edge === 'start'}
  class:edge-end={edge === 'end'}
  class:dragging
  aria-label="Resize panel"
  title="Drag to resize"
  onpointerdown={onPointerDown}
  onkeydown={onKeyDown}
></button>
