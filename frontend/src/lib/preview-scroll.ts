/** Per-path scroll for FilePreview. CodeMirror parks its own view; this covers
 *  markdown / SVG overflow, HTML iframe scroll (postMessage), and mermaid pan/zoom
 *  when the surface remounts. Observatory copies these into fileViews so they
 *  survive session restore. */

import type { FileViewState } from './editor-view-state'
import { PREVIEW_IFRAME_RESTORE, PREVIEW_IFRAME_SCROLL } from './preview'

export type PreviewScroll = { top: number; left: number; zoom?: number }

export function previewCamera(s: PreviewScroll | undefined): { top: number; left: number; zoom: number } | null {
  if (!s) return null
  const zoom = s.zoom != null && s.zoom > 0 ? s.zoom : 0
  if (s.top === 0 && s.left === 0 && zoom === 0) return null
  return { top: s.top, left: s.left, zoom: zoom || 1 }
}

function markPreviewOffset(el: HTMLElement, top: number, left: number): void {
  el.dataset.previewTop = String(top)
  el.dataset.previewLeft = String(left)
}

function previewRestoreSettled(el: HTMLElement, s: PreviewScroll): boolean {
  return Math.abs(el.scrollTop - s.top) < 1 && Math.abs(el.scrollLeft - s.left) < 1
}

export function createPreviewScrollStore() {
  const saved = new Map<string, PreviewScroll>()
  /** Workspace whose offsets currently occupy `saved`. Attach teardown after a
   *  switch must not write the outgoing surface into the incoming map. */
  let boundWorkspace = ''

  function rememberPreviewScroll(
    path: string,
    pos: Pick<HTMLElement, 'scrollTop' | 'scrollLeft'> | PreviewScroll,
  ): void {
    if (!path) return
    if ('scrollTop' in pos) {
      saved.set(path, { top: pos.scrollTop, left: pos.scrollLeft })
      return
    }
    saved.set(path, { top: pos.top, left: pos.left, zoom: pos.zoom })
  }

  function restorePreviewScroll(path: string, el: HTMLElement): void {
    if (!path) return
    const s = saved.get(path)
    el.scrollTop = s?.top ?? 0
    el.scrollLeft = s?.left ?? 0
  }

  function peekPreviewScroll(path: string): PreviewScroll | undefined {
    return saved.get(path)
  }

  function previewScrollFields(
    path: string,
    prev?: Pick<FileViewState, 'previewScrollTop' | 'previewScrollLeft' | 'previewZoom'>,
  ): Pick<FileViewState, 'previewScrollTop' | 'previewScrollLeft' | 'previewZoom'> {
    const s = saved.get(path)
    const top = s ? s.top : prev?.previewScrollTop ?? 0
    const left = s ? s.left : prev?.previewScrollLeft ?? 0
    const zoom = s ? (s.zoom ?? 0) : prev?.previewZoom ?? 0
    return {
      ...(top !== 0 ? { previewScrollTop: top } : {}),
      ...(left !== 0 ? { previewScrollLeft: left } : {}),
      ...(zoom > 0 ? { previewZoom: zoom } : {}),
    }
  }

  function hydratePreviewScroll(
    views: Record<string, FileViewState>,
    workspace = '',
  ): void {
    saved.clear()
    boundWorkspace = workspace
    for (const [path, vs] of Object.entries(views)) {
      const top = vs.previewScrollTop ?? 0
      const left = vs.previewScrollLeft ?? 0
      const zoom = vs.previewZoom
      if (top !== 0 || left !== 0 || (zoom != null && zoom > 0)) {
        saved.set(path, { top, left, zoom })
      }
    }
  }

  function applyPreviewScrollToFileViews(
    views: Record<string, FileViewState>,
    keepPaths: Iterable<string>,
  ): Record<string, FileViewState> {
    let out: Record<string, FileViewState> | undefined
    for (const path of keepPaths) {
      const s = saved.get(path)
      if (!s) continue
      const prev = views[path]
      const next: FileViewState = {
        viewMode: prev?.viewMode ?? 'preview' as const,
        line: prev?.line ?? 1,
        col: prev?.col ?? 1,
        scrollTop: prev?.scrollTop ?? 0,
        scrollLeft: prev?.scrollLeft ?? 0,
        ...prev,
      }
      delete next.previewScrollTop
      delete next.previewScrollLeft
      delete next.previewZoom
      Object.assign(next, previewScrollFields(path, prev))
      if (
        prev
        && prev.previewScrollTop === next.previewScrollTop
        && prev.previewScrollLeft === next.previewScrollLeft
        && prev.previewZoom === next.previewZoom
      ) continue
      out ??= { ...views }
      out[path] = next
    }
    return out ?? views
  }

  function canSave(workspace: string, el: HTMLElement): boolean {
    if (workspace && boundWorkspace && workspace !== boundWorkspace) return false
    // Park reparent zeros overflow; do not persist that 0 over the live offset.
    if (el.closest('#wsv-park')) return false
    return true
  }

  function attachIframeScroll(
    path: string,
    workspace: string,
    el: HTMLElement,
    iframe: HTMLIFrameElement,
  ) {
    const restore = () => {
      const s = saved.get(path)
      const top = s?.top ?? 0
      const left = s?.left ?? 0
      markPreviewOffset(el, top, left)
      iframe.contentWindow?.postMessage({
        type: PREVIEW_IFRAME_RESTORE,
        top,
        left,
      }, '*')
    }
    // srcdoc paints at 0 before the restore message is handled; that 0 must
    // not replace the offset we are still trying to apply.
    const want = saved.get(path)
    let seenLive = !(want && (want.top !== 0 || want.left !== 0))
    const onMsg = (e: MessageEvent) => {
      if (e.source !== iframe.contentWindow) return
      const d = e.data
      if (!d || d.type !== PREVIEW_IFRAME_SCROLL) return
      if (!canSave(workspace, el)) return
      const top = Number(d.top) || 0
      const left = Number(d.left) || 0
      if (!seenLive && top === 0 && left === 0) return
      seenLive = true
      rememberPreviewScroll(path, { top, left })
      markPreviewOffset(el, top, left)
    }
    window.addEventListener('message', onMsg)
    iframe.addEventListener('load', restore)
    restore()
    const id = requestAnimationFrame(restore)
    return () => {
      cancelAnimationFrame(id)
      window.removeEventListener('message', onMsg)
      iframe.removeEventListener('load', restore)
    }
  }

  function attachPreviewScroll(path: string, loadedPath = path, workspace = '') {
    return (el: HTMLElement) => {
      if (!path || loadedPath !== path) return
      const iframe = el.querySelector(':scope > iframe')
      if (iframe instanceof HTMLIFrameElement) return attachIframeScroll(path, workspace, el, iframe)
      const want = saved.get(path)
      let pending = !!(want && (want.top !== 0 || want.left !== 0))
      let restoring = false
      let appliedTop = 0
      let appliedLeft = 0
      const applyRestore = () => {
        restoring = true
        restorePreviewScroll(path, el)
        const s = saved.get(path)
        markPreviewOffset(el, s?.top ?? 0, s?.left ?? 0)
        appliedTop = el.scrollTop
        appliedLeft = el.scrollLeft
        restoring = false
        if (!s || previewRestoreSettled(el, s)) pending = false
      }
      const tryRestore = () => {
        if (!pending) return
        applyRestore()
      }
      applyRestore()
      const id = requestAnimationFrame(tryRestore)
      const save = () => {
        if (!canSave(workspace, el) || restoring) return
        // Restore-induced clamp (short first paint, mermaid/images still growing)
        // must not persist over the offset we have not applied yet.
        if (pending && Math.abs(el.scrollTop - appliedTop) < 1 && Math.abs(el.scrollLeft - appliedLeft) < 1) return
        pending = false
        rememberPreviewScroll(path, el)
        markPreviewOffset(el, el.scrollTop, el.scrollLeft)
      }
      el.addEventListener('scroll', save, { passive: true })
      let ro: ResizeObserver | undefined
      if (typeof ResizeObserver !== 'undefined') {
        ro = new ResizeObserver(tryRestore)
        ro.observe(el)
        for (const child of el.children) ro.observe(child)
      }
      return () => {
        cancelAnimationFrame(id)
        ro?.disconnect()
        save()
        el.removeEventListener('scroll', save)
      }
    }
  }

  return {
    rememberPreviewScroll,
    restorePreviewScroll,
    peekPreviewScroll,
    previewScrollFields,
    hydratePreviewScroll,
    applyPreviewScrollToFileViews,
    attachPreviewScroll,
  }
}

export type PreviewScrollStore = ReturnType<typeof createPreviewScrollStore>

const live = createPreviewScrollStore()
export const rememberPreviewScroll = live.rememberPreviewScroll
export const restorePreviewScroll = live.restorePreviewScroll
export const peekPreviewScroll = live.peekPreviewScroll
export const previewScrollFields = live.previewScrollFields
export const hydratePreviewScroll = live.hydratePreviewScroll
export const applyPreviewScrollToFileViews = live.applyPreviewScrollToFileViews
export const attachPreviewScroll = live.attachPreviewScroll
