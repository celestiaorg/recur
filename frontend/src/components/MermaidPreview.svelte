<script lang="ts">
  // Whole-file Mermaid preview with fit / zoom / pan.
  // Labels are SVG text (htmlLabels: false). Zoom changes the SVG's
  // width/height with viewBox held still so paths and labels scale together
  // as vectors — CSS scale() blurs, and stretching a foreignObject HTML
  // label clips the glyphs.
  import Icon from './Icon.svelte'
  import { renderMermaidSvg, unclipMermaidSvg } from '../lib/mermaid'
  import { previewCamera, type PreviewScroll } from '../lib/preview-scroll'

  let {
    content = '',
    path = '',
    remember,
    peek,
  }: {
    content?: string
    path?: string
    remember?: (path: string, pos: PreviewScroll) => void
    peek?: (path: string) => PreviewScroll | undefined
  } = $props()

  const MIN_SCALE = 0.1
  const MAX_SCALE = 8
  const ZOOM_STEP = 1.25
  const PAD = 40

  let viewport: HTMLElement | undefined = $state()
  let host: HTMLElement | undefined = $state()
  let error = $state('')
  let pending = $state(false)

  let scale = $state(1)
  let tx = $state(0)
  let ty = $state(0)
  let naturalW = $state(0)
  let naturalH = $state(0)
  let userAdjusted = $state(false)

  let dragging = $state(false)
  let dragOriginX = 0
  let dragOriginY = 0
  let dragStartTx = 0
  let dragStartTy = 0

  const pct = $derived(`${Math.round(scale * 100)}%`)
  const transform = $derived(`translate(${tx}px, ${ty}px)`)

  function clampScale(s: number): number {
    return Math.min(MAX_SCALE, Math.max(MIN_SCALE, s))
  }

  function attrPx(svg: SVGSVGElement, name: 'width' | 'height'): number {
    const a = svg.getAttribute(name)
    if (!a || a.endsWith('%')) return 0
    const n = parseFloat(a)
    return Number.isFinite(n) && n > 0 ? n : 0
  }

  function readNaturalSize(svg: SVGSVGElement): { w: number; h: number } | null {
    let w = attrPx(svg, 'width')
    let h = attrPx(svg, 'height')
    const vb = svg.viewBox?.baseVal
    if (vb && vb.width > 0 && vb.height > 0) {
      if (w < 1) w = vb.width
      if (h < 1) h = vb.height
    }
    if (w < 1 || h < 1) {
      try {
        const b = svg.getBBox()
        w = b.width
        h = b.height
      } catch {
        /* not in DOM yet */
      }
    }
    if (w < 1 || h < 1) return null
    return { w, h }
  }

  function ensureViewBox(svg: SVGSVGElement, w: number, h: number) {
    if (!svg.getAttribute('viewBox')) {
      svg.setAttribute('viewBox', `0 0 ${w} ${h}`)
    }
  }

  function applyDisplaySize() {
    const svg = host?.querySelector('svg')
    if (!svg || naturalW < 1 || naturalH < 1) return
    const w = naturalW * scale
    const h = naturalH * scale
    svg.removeAttribute('style')
    svg.setAttribute('width', String(w))
    svg.setAttribute('height', String(h))
    svg.style.width = `${w}px`
    svg.style.height = `${h}px`
    svg.style.maxWidth = 'none'
    svg.style.overflow = 'visible'
  }

  function captureNatural() {
    const svg = host?.querySelector('svg')
    if (!svg) return false
    // viewBox is the stable intrinsic size; width/height are display-only and
    // get overwritten by zoom, so never treat them as natural after first paint.
    const vb = svg.viewBox?.baseVal
    if (vb && vb.width > 0 && vb.height > 0) {
      naturalW = vb.width
      naturalH = vb.height
      return true
    }
    svg.style.width = ''
    svg.style.height = ''
    const box = readNaturalSize(svg)
    if (!box) return false
    naturalW = box.w
    naturalH = box.h
    ensureViewBox(svg, box.w, box.h)
    return true
  }

  function fit(force = false) {
    if (!viewport || !host) return
    if (userAdjusted && !force) return
    if (naturalW < 1 && !captureNatural()) return
    const vr = viewport.getBoundingClientRect()
    if (vr.width < 8 || vr.height < 8) return
    const next = clampScale(
      Math.min((vr.width - PAD) / naturalW, (vr.height - PAD) / naturalH),
    )
    scale = next
    applyDisplaySize()
    tx = (vr.width - naturalW * next) / 2
    ty = (vr.height - naturalH * next) / 2
    userAdjusted = false
  }

  function zoomAt(factor: number, cx: number, cy: number) {
    const next = clampScale(scale * factor)
    if (next === scale) return
    tx = cx - ((cx - tx) / scale) * next
    ty = cy - ((cy - ty) / scale) * next
    scale = next
    applyDisplaySize()
    userAdjusted = true
  }

  function zoomBy(factor: number) {
    if (!viewport) return
    const vr = viewport.getBoundingClientRect()
    zoomAt(factor, vr.width / 2, vr.height / 2)
  }

  function resetView() {
    userAdjusted = false
    if (path) remember?.(path, { top: 0, left: 0 })
    fit(true)
  }

  $effect(() => {
    const src = content
    const cam = path ? previewCamera(peek?.(path)) : null
    // Clear before the host guard — an error branch must not self-lock when
    // content changes (host stays mounted; see template overlay below).
    error = ''
    userAdjusted = false
    scale = 1
    tx = 0
    ty = 0
    naturalW = 0
    naturalH = 0
    const el = host
    if (!el) {
      pending = !!src.trim()
      return
    }
    let cancelled = false
    el.replaceChildren()
    if (!src.trim()) {
      pending = false
      return () => { cancelled = true }
    }
    pending = true
    void renderMermaidSvg(src).then(
      (svg) => {
        if (cancelled || !el.isConnected) return
        pending = false
        el.innerHTML = svg
        unclipMermaidSvg(el)
        requestAnimationFrame(() => {
          if (cancelled) return
          captureNatural()
          applyDisplaySize()
          if (cam) {
            scale = clampScale(cam.zoom)
            tx = cam.left
            ty = cam.top
            userAdjusted = true
            applyDisplaySize()
          } else {
            fit(true)
          }
        })
      },
      (err) => {
        if (cancelled || !el.isConnected) return
        pending = false
        error = err instanceof Error ? err.message : 'Mermaid render failed'
      },
    )
    return () => { cancelled = true }
  })

  $effect(() => {
    if (!path || !userAdjusted) return
    remember?.(path, { top: ty, left: tx, zoom: scale })
  })

  $effect(() => {
    const vp = viewport
    if (!vp || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => fit(false))
    ro.observe(vp)
    return () => ro.disconnect()
  })

  $effect(() => {
    const vp = viewport
    if (!vp) return
    const onWheel = (e: WheelEvent) => {
      if (pending || error || !content.trim()) return
      e.preventDefault()
      const vr = vp.getBoundingClientRect()
      const factor = e.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP
      zoomAt(factor, e.clientX - vr.left, e.clientY - vr.top)
    }
    const onReveal = (e: Event) => {
      const d = (e as CustomEvent<{ x: number; y: number }>).detail
      if (!d || !Number.isFinite(d.x) || !Number.isFinite(d.y)) return
      tx = d.x
      ty = d.y
      userAdjusted = true
    }
    vp.addEventListener('wheel', onWheel, { passive: false })
    vp.addEventListener('preview-find-reveal', onReveal)
    return () => {
      vp.removeEventListener('wheel', onWheel)
      vp.removeEventListener('preview-find-reveal', onReveal)
    }
  })

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0 || pending || error || !content.trim()) return
    dragging = true
    dragOriginX = e.clientX
    dragOriginY = e.clientY
    dragStartTx = tx
    dragStartTy = ty
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }

  function onPointerMove(e: PointerEvent) {
    if (!dragging) return
    tx = dragStartTx + (e.clientX - dragOriginX)
    ty = dragStartTy + (e.clientY - dragOriginY)
    userAdjusted = true
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragging) return
    dragging = false
    try {
      ;(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId)
    } catch {
      /* already released */
    }
  }

  function onDblClick() {
    if (pending || error || !content.trim()) return
    resetView()
  }
</script>

<div class="file-preview file-preview-mermaid">
  {#if !content.trim()}
    <div class="file-preview-empty">Empty Mermaid</div>
  {:else}
    {#if !error}
      <div class="mmd-toolbar" role="toolbar" aria-label="Mermaid zoom">
        <button type="button" class="rbtn mmd-tb-btn" title="Zoom out" aria-label="Zoom out" onclick={() => zoomBy(1 / ZOOM_STEP)} disabled={scale <= MIN_SCALE}>
          <Icon name="minus" />
        </button>
        <button type="button" class="rbtn mmd-tb-pct" title="Fit to view" aria-label="Fit to view ({pct})" onclick={resetView}>
          {pct}
        </button>
        <button type="button" class="rbtn mmd-tb-btn" title="Zoom in" aria-label="Zoom in" onclick={() => zoomBy(ZOOM_STEP)} disabled={scale >= MAX_SCALE}>
          <Icon name="plus" />
        </button>
        <button type="button" class="rbtn mmd-tb-btn" title="Fit to view" aria-label="Fit to view" onclick={resetView}>
          <Icon name="maximize" />
        </button>
      </div>
    {/if}
    {#if pending}
      <div class="file-preview-empty mmd-pending">Rendering…</div>
    {:else if error}
      <div class="file-preview-empty file-preview-error mmd-pending">{error}</div>
    {/if}
    <!-- Keep host mounted through errors so content edits can re-render. -->
    <div
      class="mmd-viewport"
      class:is-dragging={dragging}
      class:is-pending={pending || !!error}
      bind:this={viewport}
      role="application"
      aria-label={path || 'Mermaid preview'}
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerUp}
      onpointercancel={onPointerUp}
      ondblclick={onDblClick}
    >
      <div class="mmd-stage" style:transform>
        <div class="mmd-host" bind:this={host}></div>
      </div>
    </div>
  {/if}
</div>
