<script lang="ts">
  import { getDocument, GlobalWorkerOptions, RenderingCancelledException } from 'pdfjs-dist'
  import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist'
  import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url'
  import Icon from './Icon.svelte'
  import { clampPage, discardPageCanvas, fitWidthScale, MAX_SCALE, MIN_SCALE, pageAtScroll, pageOriginInScroller, pdfjsAssetDirs, zoomStep } from '../lib/pdf-preview'

  GlobalWorkerOptions.workerSrc = workerUrl

  const PAGE_GAP = 12
  const VIEW_PAD = 12

  let { src = '', path = '' }: { src?: string; path?: string } = $props()

  let viewport: HTMLElement | undefined = $state()
  let error = $state('')
  let pending = $state(false)
  let page = $state(1)
  let pageDraft = $state('1')
  let pageCount = $state(0)
  let scale = $state(1)
  let pdf = $state.raw<PDFDocumentProxy | null>(null)
  let pageWidthPx = 0
  let pageSizes: { n: number; w: number; h: number }[] = $state([])

  const pct = $derived(`${Math.round(scale * 100)}%`)
  const ready = $derived(pageCount > 0 && !error)

  function setPage(n: number) {
    const next = clampPage(n, pageCount)
    page = next
    pageDraft = String(next)
    viewport?.querySelector<HTMLElement>(`[data-page="${next}"]`)?.scrollIntoView({ block: 'start' })
  }

  function commitPage() {
    setPage(Number(pageDraft))
  }

  function fitWidth() {
    const inner = Math.max(0, (viewport?.clientWidth ?? 0) - VIEW_PAD * 2)
    scale = fitWidthScale(pageWidthPx, inner)
  }

  function syncPageFromScroll() {
    if (!viewport || pageSizes.length === 0) return
    const origin = pageOriginInScroller(viewport, viewport.querySelector<HTMLElement>('[data-page]'), VIEW_PAD)
    const heights = pageSizes.map((p) => p.h * scale)
    const next = pageAtScroll(
      viewport.scrollTop - origin,
      heights,
      PAGE_GAP,
      viewport.scrollHeight - viewport.clientHeight - origin,
      page,
    )
    if (next === page) return
    page = next
    pageDraft = String(next)
  }

  function observePage(n: number, s: number, doc: PDFDocumentProxy | null) {
    return (el: HTMLElement) => {
      const root = viewport
      if (!root || !doc) return
      const canvas = el.querySelector('canvas')
      if (!(canvas instanceof HTMLCanvasElement)) return

      let task: RenderTask | null = null
      let cancelled = false
      let visible = false
      let painted = false
      let inflight = false

      const render = async () => {
        if (cancelled || painted || inflight || !visible) return
        inflight = true
        let failed = false
        try {
          const pg = await doc.getPage(n)
          if (cancelled || !visible) return
          const base = pg.getViewport({ scale: 1 })
          const slot = pageSizes[n - 1]
          if (slot && (slot.w !== base.width || slot.h !== base.height)) {
            pageSizes[n - 1] = { n, w: base.width, h: base.height }
          }
          const dpr = window.devicePixelRatio || 1
          const vp = pg.getViewport({ scale: s })
          canvas.width = Math.floor(vp.width * dpr)
          canvas.height = Math.floor(vp.height * dpr)
          const ctx = canvas.getContext('2d')
          if (!ctx) {
            failed = true
            return
          }
          const transform = dpr === 1 ? undefined : [dpr, 0, 0, dpr, 0, 0]
          task = pg.render({ canvas, canvasContext: ctx, viewport: vp, transform })
          if (cancelled || !visible) {
            task.cancel()
            return
          }
          await task.promise
          painted = true
        } catch (e) {
          if (cancelled || e instanceof RenderingCancelledException) return
          failed = true
          error = e instanceof Error ? e.message : 'PDF render failed'
        } finally {
          inflight = false
          task = null
          if (cancelled || !visible) discardPageCanvas(canvas)
          else if (!painted && !failed) void render()
        }
      }

      const io = new IntersectionObserver((entries) => {
        for (const e of entries) {
          visible = e.isIntersecting
          if (visible) void render()
          else {
            task?.cancel()
            painted = false
            discardPageCanvas(canvas)
          }
        }
      }, { root })
      io.observe(el)
      return () => {
        cancelled = true
        task?.cancel()
        io.disconnect()
        discardPageCanvas(canvas)
      }
    }
  }

  $effect(() => {
    const url = src
    let cancelled = false
    let loadingTask: ReturnType<typeof getDocument> | null = null

    pdf = null
    pageCount = 0
    page = 1
    pageDraft = '1'
    pageWidthPx = 0
    pageSizes = []
    error = ''
    if (!url) {
      pending = false
      return
    }
    pending = true
    const ac = new AbortController()

    void (async () => {
      try {
        const res = await fetch(url, { signal: ac.signal })
        if (!res.ok) throw new Error(`PDF fetch failed (${res.status})`)
        const data = new Uint8Array(await res.arrayBuffer())
        if (cancelled) return
        loadingTask = getDocument({
          data,
          isEvalSupported: false,
          isOffscreenCanvasSupported: false,
          ...pdfjsAssetDirs(document.baseURI, import.meta.env.BASE_URL),
        } as Parameters<typeof getDocument>[0])
        const loaded = await loadingTask.promise
        if (cancelled) {
          void loadingTask.destroy()
          return
        }
        const first = await loaded.getPage(1)
        if (cancelled) {
          void loadingTask.destroy()
          return
        }
        const base = first.getViewport({ scale: 1 })
        pageWidthPx = base.width
        // simplify: page-1 size is the placeholder for every slot; first fetch of a later page updates that slot if it differs
        pageSizes = Array.from({ length: loaded.numPages }, (_, i) => ({
          n: i + 1,
          w: base.width,
          h: base.height,
        }))
        pdf = loaded
        pageCount = loaded.numPages
        scale = fitWidthScale(pageWidthPx, Math.max(0, (viewport?.clientWidth ?? 0) - VIEW_PAD * 2))
        pending = false
      } catch (e) {
        if (cancelled || (e instanceof DOMException && e.name === 'AbortError')) return
        pending = false
        error = e instanceof Error ? e.message : 'PDF load failed'
      }
    })()

    return () => {
      cancelled = true
      ac.abort()
      void loadingTask?.destroy()
    }
  })

  function onKey(e: KeyboardEvent) {
    if (pending || error || pageCount < 1) return
    if (!(e.metaKey || e.ctrlKey)) return
    if (e.key === '+' || e.key === '=' || e.code === 'Equal' || e.code === 'NumpadAdd') {
      e.preventDefault()
      scale = zoomStep(scale, 1)
    } else if (e.key === '-' || e.code === 'Minus' || e.code === 'NumpadSubtract') {
      e.preventDefault()
      scale = zoomStep(scale, -1)
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="obs-pdf-preview"
  role="region"
  aria-label={path || 'PDF preview'}
  onkeydown={onKey}
>
  {#if ready}
    <div class="obs-pdf-toolbar" role="toolbar" aria-label="PDF page and zoom">
      <div class="obs-pdf-tb-group">
        <button type="button" class="obs-icon-btn" title="Previous page" aria-label="Previous page" onclick={() => setPage(page - 1)} disabled={page <= 1}>
          <Icon name="chevron-left" />
        </button>
        <input
          class="obs-pdf-page"
          value={pageDraft}
          inputmode="numeric"
          spellcheck="false"
          autocomplete="off"
          aria-label="Page number"
          oninput={(e) => { pageDraft = e.currentTarget.value }}
          onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); commitPage() } }}
          onblur={commitPage}
        />
        <span class="obs-pdf-of">of {pageCount}</span>
        <button type="button" class="obs-icon-btn" title="Next page" aria-label="Next page" onclick={() => setPage(page + 1)} disabled={page >= pageCount}>
          <Icon name="chevron-right" />
        </button>
      </div>
      <div class="obs-pdf-tb-group">
        <button type="button" class="obs-icon-btn" title="Zoom out" aria-label="Zoom out" onclick={() => { scale = zoomStep(scale, -1) }} disabled={scale <= MIN_SCALE}>
          <Icon name="minus" />
        </button>
        <button type="button" class="obs-pdf-tb-pct" title="Fit to width" aria-label="Fit to width ({pct})" onclick={fitWidth}>
          {pct}
        </button>
        <button type="button" class="obs-icon-btn" title="Zoom in" aria-label="Zoom in" onclick={() => { scale = zoomStep(scale, 1) }} disabled={scale >= MAX_SCALE}>
          <Icon name="plus" />
        </button>
        <button type="button" class="obs-icon-btn" title="Fit to width" aria-label="Fit to width" onclick={fitWidth}>
          <Icon name="maximize" />
        </button>
      </div>
    </div>
  {/if}
  {#if pending}
    <div class="file-preview-empty obs-pdf-pending">Loading…</div>
  {:else if error}
    <div class="file-preview-empty file-preview-error obs-pdf-pending">{error}</div>
  {/if}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="obs-pdf-viewport"
    class:is-pending={pending || !!error}
    style:--obs-pdf-gap="{PAGE_GAP}px"
    tabindex="0"
    bind:this={viewport}
    onscroll={syncPageFromScroll}
  >
    <div class="obs-pdf-pages">
      {#each pageSizes as p (p.n)}
        <div
          class="obs-pdf-page-slot"
          data-page={p.n}
          style:width="{p.w * scale}px"
          style:height="{p.h * scale}px"
          {@attach observePage(p.n, scale, pdf)}
        >
          <canvas></canvas>
        </div>
      {/each}
    </div>
  </div>
</div>
