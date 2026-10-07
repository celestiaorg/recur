/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import {
  applyPreviewScrollToFileViews, attachPreviewScroll, createPreviewScrollStore,
  hydratePreviewScroll, peekPreviewScroll, previewCamera, previewScrollFields,
  rememberPreviewScroll, restorePreviewScroll,
} from './preview-scroll'
import type { FileViewState } from './editor-view-state'
import { restoreHotShellGeometry, stashHotShellGeometry } from './editor-view-state'

function vs(extra: Partial<FileViewState> = {}): FileViewState {
  return { viewMode: 'preview', line: 1, col: 1, scrollTop: 0, scrollLeft: 0, ...extra }
}

function scroller(top = 0, left = 0) {
  const el = document.createElement('div')
  Object.defineProperties(el, {
    scrollTop: { configurable: true, writable: true, value: top },
    scrollLeft: { configurable: true, writable: true, value: left },
  })
  return el
}

function boundedScroller(opts: {
  top?: number
  left?: number
  scrollHeight: number
  clientHeight: number
  scrollWidth?: number
  clientWidth?: number
}) {
  const el = document.createElement('div')
  let top = opts.top ?? 0
  let left = opts.left ?? 0
  let scrollHeight = opts.scrollHeight
  let scrollWidth = opts.scrollWidth ?? opts.clientWidth ?? 0
  const clientHeight = opts.clientHeight
  const clientWidth = opts.clientWidth ?? 0
  const clamp = (v: number, max: number) => Math.max(0, Math.min(max, v))
  Object.defineProperties(el, {
    scrollHeight: { configurable: true, get: () => scrollHeight, set: (v: number) => { scrollHeight = v } },
    scrollWidth: { configurable: true, get: () => scrollWidth, set: (v: number) => { scrollWidth = v } },
    clientHeight: { configurable: true, get: () => clientHeight },
    clientWidth: { configurable: true, get: () => clientWidth },
    scrollTop: {
      configurable: true,
      get: () => top,
      set: (v: number) => { top = clamp(v, Math.max(0, scrollHeight - clientHeight)) },
    },
    scrollLeft: {
      configurable: true,
      get: () => left,
      set: (v: number) => { left = clamp(v, Math.max(0, scrollWidth - clientWidth)) },
    },
  })
  return el
}

describe('preview scroll', () => {
  it('round-trips scroll for a path and resets an unseen path', () => {
    const a = scroller(240, 12)
    rememberPreviewScroll('notes.md', a)
    const b = scroller(99, 4)
    restorePreviewScroll('notes.md', b)
    expect(b.scrollTop).toBe(240)
    expect(b.scrollLeft).toBe(12)
    restorePreviewScroll('other.md', b)
    expect(b.scrollTop).toBe(0)
    expect(b.scrollLeft).toBe(0)
  })

  it('saves on detach and restores on the next attach', () => {
    const el = scroller()
    const stop = attachPreviewScroll('readme.md')(el)
    el.scrollTop = 180
    el.dispatchEvent(new Event('scroll'))
    stop?.()
    el.scrollTop = 0
    attachPreviewScroll('readme.md')(el)
    expect(el.scrollTop).toBe(180)
  })

  it('does not keep another file\'s offset on the same element', () => {
    const el = scroller()
    const stopA = attachPreviewScroll('a.md')(el)
    el.scrollTop = 300
    el.dispatchEvent(new Event('scroll'))
    stopA?.()
    attachPreviewScroll('b.md')(el)
    expect(el.scrollTop).toBe(0)
  })

  it('hydrates the live map from fileViews and copies it back for persist', () => {
    hydratePreviewScroll({
      'doc.md': vs({ previewScrollTop: 420 }),
    })
    expect(previewScrollFields('doc.md')).toEqual({ previewScrollTop: 420 })
    const views = applyPreviewScrollToFileViews({
      'doc.md': vs({ line: 2, scrollTop: 10 }),
    }, ['doc.md'])
    expect(views['doc.md']?.previewScrollTop).toBe(420)
    expect(views['doc.md']?.line).toBe(2)
  })

  it('clears previous preview offsets when the live offset is zero', () => {
    hydratePreviewScroll({
      'doc.md': vs({ previewScrollTop: 420, previewScrollLeft: 16 }),
    })
    rememberPreviewScroll('doc.md', scroller(0, 0))
    const views = applyPreviewScrollToFileViews({
      'doc.md': vs({ previewScrollTop: 420, previewScrollLeft: 16 }),
    }, ['doc.md'])
    expect(views['doc.md']?.previewScrollTop).toBeUndefined()
    expect(views['doc.md']?.previewScrollLeft).toBeUndefined()
  })

  it('restores after the requested document has loaded against real scroll bounds', () => {
    hydratePreviewScroll({})
    rememberPreviewScroll('long.md', scroller(400, 0))
    const el = boundedScroller({ scrollHeight: 80, clientHeight: 80, clientWidth: 40, scrollWidth: 40 })
    const early = attachPreviewScroll('long.md', 'short.md')(el)
    el.scrollTop = 400
    expect(el.scrollTop).toBe(0)
    expect(peekPreviewScroll('long.md')).toEqual({ top: 400, left: 0 })
    early?.()
    ;(el as unknown as { scrollHeight: number }).scrollHeight = 800
    attachPreviewScroll('long.md', 'long.md')(el)
    expect(el.scrollTop).toBe(400)
  })

  it('does not persist a clamp-to-zero over a restore the scroller cannot hold yet', () => {
    hydratePreviewScroll({})
    rememberPreviewScroll('long.md', scroller(400, 0))
    const el = boundedScroller({ scrollHeight: 80, clientHeight: 80, clientWidth: 40, scrollWidth: 40 })
    const stop = attachPreviewScroll('long.md')(el)
    expect(el.scrollTop).toBe(0)
    el.dispatchEvent(new Event('scroll'))
    stop?.()
    expect(peekPreviewScroll('long.md')).toEqual({ top: 400, left: 0 })
  })

  it('saves user scrolling after the scroll range shrinks below the restored offset', () => {
    hydratePreviewScroll({})
    rememberPreviewScroll('long.md', scroller(400, 0))
    const el = boundedScroller({ scrollHeight: 800, clientHeight: 80, clientWidth: 40, scrollWidth: 40 })
    const stop = attachPreviewScroll('long.md')(el)
    expect(el.scrollTop).toBe(400)
    ;(el as unknown as { scrollHeight: number }).scrollHeight = 280
    el.scrollTop = 50
    el.dispatchEvent(new Event('scroll'))
    expect(peekPreviewScroll('long.md')).toEqual({ top: 50, left: 0 })
    expect(el.dataset.previewTop).toBe('50')
    stop?.()
    expect(peekPreviewScroll('long.md')).toEqual({ top: 50, left: 0 })
    el.scrollTop = 0
    attachPreviewScroll('long.md')(el)
    expect(el.scrollTop).toBe(50)
  })

  it('lets user scrolling cancel a restore the scroller still cannot hold', () => {
    hydratePreviewScroll({})
    rememberPreviewScroll('long.md', scroller(400, 0))
    const el = boundedScroller({ scrollHeight: 80, clientHeight: 80, clientWidth: 40, scrollWidth: 40 })
    attachPreviewScroll('long.md')(el)
    expect(el.scrollTop).toBe(0)
    ;(el as unknown as { scrollHeight: number }).scrollHeight = 280
    el.scrollTop = 50
    el.dispatchEvent(new Event('scroll'))
    expect(peekPreviewScroll('long.md')).toEqual({ top: 50, left: 0 })
    expect(el.dataset.previewTop).toBe('50')
  })

  it('does not let an unreachable horizontal offset block a vertical user save', () => {
    hydratePreviewScroll({})
    rememberPreviewScroll('wide.md', scroller(400, 300))
    const el = boundedScroller({
      scrollHeight: 800, clientHeight: 80, scrollWidth: 120, clientWidth: 40,
    })
    attachPreviewScroll('wide.md')(el)
    expect(el.scrollTop).toBe(400)
    expect(el.scrollLeft).toBe(80)
    el.scrollTop = 50
    el.dispatchEvent(new Event('scroll'))
    expect(peekPreviewScroll('wide.md')).toEqual({ top: 50, left: 80 })
  })

  it('re-applies restore once content growth can hold the saved offset', () => {
    const Prev = globalThis.ResizeObserver
    const cbs: Array<() => void> = []
    globalThis.ResizeObserver = class {
      constructor(cb: () => void) { cbs.push(cb) }
      observe() {}
      disconnect() {}
      unobserve() {}
    } as unknown as typeof ResizeObserver
    try {
      hydratePreviewScroll({})
      rememberPreviewScroll('long.md', scroller(400, 0))
      const el = boundedScroller({ scrollHeight: 80, clientHeight: 80, clientWidth: 40, scrollWidth: 40 })
      attachPreviewScroll('long.md')(el)
      expect(el.scrollTop).toBe(0)
      ;(el as unknown as { scrollHeight: number }).scrollHeight = 800
      for (const cb of cbs) cb()
      expect(el.scrollTop).toBe(400)
    } finally {
      globalThis.ResizeObserver = Prev
    }
  })

  it('does not remember overflow zeroed by a parked hot-shell move', () => {
    hydratePreviewScroll({})
    const park = document.createElement('div')
    park.id = 'wsv-park'
    document.body.appendChild(park)
    rememberPreviewScroll('readme.md', scroller(240))
    const el = scroller(240)
    park.appendChild(el)
    const stop = attachPreviewScroll('readme.md')(el)
    el.scrollTop = 0
    el.dispatchEvent(new Event('scroll'))
    stop?.()
    expect(peekPreviewScroll('readme.md')).toEqual({ top: 240, left: 0 })
    park.remove()
  })

  it('restores the incoming workspace on a same-path switch and ignores outgoing teardown', () => {
    hydratePreviewScroll({ 'README.md': vs({ previewScrollTop: 100 }) }, 'wsA')
    const el = scroller(100)
    const stopA = attachPreviewScroll('README.md', 'README.md', 'wsA')(el)
    hydratePreviewScroll({ 'README.md': vs({ previewScrollTop: 50 }) }, 'wsB')
    stopA?.()
    expect(peekPreviewScroll('README.md')).toEqual({ top: 50, left: 0 })
    const incoming = scroller(100)
    attachPreviewScroll('README.md', 'README.md', 'wsB')(incoming)
    expect(incoming.scrollTop).toBe(50)
  })

  it('keeps separate maps for two mounted owners through an A→B→A return', () => {
    const a = createPreviewScrollStore()
    const b = createPreviewScrollStore()
    a.hydratePreviewScroll({ 'README.md': vs({ previewScrollTop: 100 }) }, 'wsA')
    const elA = scroller(100)
    const stopA = a.attachPreviewScroll('README.md', 'README.md', 'wsA')(elA)
    b.hydratePreviewScroll({ 'README.md': vs({ previewScrollTop: 50 }) }, 'wsB')
    elA.scrollTop = 300
    elA.dispatchEvent(new Event('scroll'))
    const viewsA = a.applyPreviewScrollToFileViews({
      'README.md': vs({ previewScrollTop: 100 }),
    }, ['README.md'])
    expect(viewsA['README.md']?.previewScrollTop).toBe(300)
    expect(b.peekPreviewScroll('README.md')).toEqual({ top: 50, left: 0 })
    const viewsAAgain = a.applyPreviewScrollToFileViews({
      'README.md': vs({ previewScrollTop: 100 }),
    }, ['README.md'])
    expect(viewsAAgain['README.md']?.previewScrollTop).toBe(300)
    stopA?.()
  })

  it('round-trips mermaid camera including zoom', () => {
    hydratePreviewScroll({
      'flow.mmd': vs({ previewScrollTop: 40, previewScrollLeft: 12, previewZoom: 2 }),
    })
    expect(previewCamera(peekPreviewScroll('flow.mmd'))).toEqual({ top: 40, left: 12, zoom: 2 })
    rememberPreviewScroll('flow.mmd', { top: 8, left: 3, zoom: 1.25 })
    const views = applyPreviewScrollToFileViews({
      'flow.mmd': vs({ previewScrollTop: 40, previewZoom: 2 }),
    }, ['flow.mmd'])
    expect(views['flow.mmd']?.previewScrollTop).toBe(8)
    expect(views['flow.mmd']?.previewScrollLeft).toBe(3)
    expect(views['flow.mmd']?.previewZoom).toBe(1.25)
  })

  it('round-trips mermaid camera with negative pan offsets', () => {
    hydratePreviewScroll({})
    rememberPreviewScroll('flow.mmd', { top: -120, left: -80, zoom: 2 })
    const views = applyPreviewScrollToFileViews({
      'flow.mmd': vs(),
    }, ['flow.mmd'])
    expect(views['flow.mmd']?.previewScrollTop).toBe(-120)
    expect(views['flow.mmd']?.previewScrollLeft).toBe(-80)
    expect(views['flow.mmd']?.previewZoom).toBe(2)
    hydratePreviewScroll(views)
    expect(previewCamera(peekPreviewScroll('flow.mmd'))).toEqual({ top: -120, left: -80, zoom: 2 })
  })

  it('restores html iframe scroll via postMessage', () => {
    hydratePreviewScroll({ 'a.html': vs({ previewScrollTop: 90, previewScrollLeft: 4 }) })
    const wrap = document.createElement('div')
    const iframe = document.createElement('iframe')
    wrap.appendChild(iframe)
    document.body.appendChild(wrap)
    const posts: unknown[] = []
    const cw = iframe.contentWindow!
    cw.postMessage = ((data: unknown) => { posts.push(data) }) as typeof cw.postMessage
    const stop = attachPreviewScroll('a.html')(wrap)
    expect(posts).toContainEqual({ type: 'recur-preview-restore', top: 90, left: 4 })
    window.dispatchEvent(new MessageEvent('message', {
      data: { type: 'recur-preview-scroll', top: 120, left: 8 },
      source: cw,
    }))
    expect(peekPreviewScroll('a.html')).toEqual({ top: 120, left: 8 })
    stop?.()
    wrap.remove()
  })

  it('ignores iframe paint-at-zero until a live offset arrives', () => {
    hydratePreviewScroll({ 'a.html': vs({ previewScrollTop: 90, previewScrollLeft: 4 }) })
    const wrap = document.createElement('div')
    const iframe = document.createElement('iframe')
    wrap.appendChild(iframe)
    document.body.appendChild(wrap)
    const cw = iframe.contentWindow!
    cw.postMessage = (() => {}) as typeof cw.postMessage
    const stop = attachPreviewScroll('a.html')(wrap)
    window.dispatchEvent(new MessageEvent('message', {
      data: { type: 'recur-preview-scroll', top: 0, left: 0 },
      source: cw,
    }))
    expect(peekPreviewScroll('a.html')).toEqual({ top: 90, left: 4 })
    window.dispatchEvent(new MessageEvent('message', {
      data: { type: 'recur-preview-scroll', top: 90, left: 4 },
      source: cw,
    }))
    window.dispatchEvent(new MessageEvent('message', {
      data: { type: 'recur-preview-scroll', top: 0, left: 0 },
      source: cw,
    }))
    expect(peekPreviewScroll('a.html')).toEqual({ top: 0, left: 0 })
    stop?.()
    wrap.remove()
  })

  it('resets iframe wrapper offsets when switching html files before a workspace stash', () => {
    hydratePreviewScroll({ 'a.html': vs({ previewScrollTop: 90, previewScrollLeft: 4 }) })
    const shell = document.createElement('div')
    shell.innerHTML = `
      <div class="obs-editor-pane"><div class="obs-surface">
        <div class="file-preview"><iframe></iframe></div>
      </div></div>
    `
    document.body.appendChild(shell)
    const wrap = shell.querySelector<HTMLElement>('.file-preview')!
    Object.defineProperty(wrap, 'clientHeight', { configurable: true, value: 400 })
    const iframe = wrap.querySelector('iframe')!
    const posts: unknown[] = []
    const cw = iframe.contentWindow!
    cw.postMessage = ((data: unknown) => { posts.push(data) }) as typeof cw.postMessage
    const stopA = attachPreviewScroll('a.html')(wrap)
    expect(wrap.dataset.previewTop).toBe('90')
    expect(wrap.dataset.previewLeft).toBe('4')
    stopA?.()
    posts.length = 0
    const stopB = attachPreviewScroll('b.html')(wrap)
    expect(posts).toContainEqual({ type: 'recur-preview-restore', top: 0, left: 0 })
    expect(wrap.dataset.previewTop).toBe('0')
    expect(wrap.dataset.previewLeft).toBe('0')
    stashHotShellGeometry(shell)
    posts.length = 0
    restoreHotShellGeometry(shell, { defer: false })
    expect(posts).toContainEqual({ type: 'recur-preview-restore', top: 0, left: 0 })
    expect(posts).not.toContainEqual({ type: 'recur-preview-restore', top: 90, left: 4 })
    stopB?.()
    shell.remove()
  })
})
