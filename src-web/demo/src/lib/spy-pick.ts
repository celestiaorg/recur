/** Ranges of stacked `.cfile` blocks, tops relative to the scroll root. */
export type SpyRange = { path: string; top: number; bottom: number }

/**
 * File whose range contains `scrollTop`, else the last file whose top is at or
 * above the viewport. Empty when no card has laid out yet (all 0×0) — guessing
 * would pick the last path and the review hydrator would chase selectedPath.
 */
export function fileAtScrollTop(files: readonly SpyRange[], scrollTop: number): string {
  if (!files.length) return ''
  let lo = 0
  let hi = files.length - 1
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (files[mid].top <= scrollTop) lo = mid
    else hi = mid - 1
  }
  // 0×0 cards (read before the first reflow) all sit at the origin; the search
  // would pick the last path and the review hydrator would chase selectedPath.
  while (lo > 0 && files[lo].bottom <= files[lo].top) lo--
  if (files[lo].bottom <= files[lo].top) return ''
  return files[lo].path
}

/** Viewport offset of the file at `scrollTop`, to restore after a layout shift. */
export type ScrollAnchor = { path: string; rel: number }

/** Stick to the document end when the user is already this close to it. */
export const VIEWPORT_END_PX = 48

export type ViewportPin = { atEnd: boolean; anchor: ScrollAnchor | null }

export function captureScrollAnchor(files: readonly SpyRange[], scrollTop: number): ScrollAnchor | null {
  const path = fileAtScrollTop(files, scrollTop)
  if (!path) return null
  for (const f of files) {
    if (f.path === path) return { path, rel: f.top - scrollTop }
  }
  return null
}

export function restoreScrollTop(files: readonly SpyRange[], anchor: ScrollAnchor): number | null {
  for (const f of files) {
    if (f.path === anchor.path) return f.top - anchor.rel
  }
  return null
}

export function captureViewportPin(
  files: readonly SpyRange[],
  scrollTop: number,
  viewH: number,
  scrollHeight: number,
): ViewportPin {
  return {
    atEnd: scrollTop + viewH >= scrollHeight - VIEWPORT_END_PX,
    anchor: captureScrollAnchor(files, scrollTop),
  }
}

/** Put the viewport back after a hydration swap. `atEnd` uses the live
 *  scrollHeight so growing the last file does not leave the user stranded.
 *  Skip when the viewport has moved since capture — the user kept scrolling. */
export function applyViewportPin(
  root: HTMLElement,
  files: readonly SpyRange[],
  pin: ViewportPin,
  capturedTop?: number,
): void {
  if (capturedTop != null && root.scrollTop !== capturedTop) return
  if (pin.atEnd) {
    root.scrollTop = Math.max(0, root.scrollHeight - root.clientHeight)
    return
  }
  if (!pin.anchor) return
  const top = restoreScrollTop(files, pin.anchor)
  if (top != null) root.scrollTop = top
}

/**
 * Adjacent path in `paths` (document order). `null` at the ends, or when
 * `paths` is empty. An unknown `current` steps to the first (dir +1) or last
 * (dir −1) entry.
 */
export function stepPath(paths: readonly string[], current: string, dir: -1 | 1): string | null {
  if (!paths.length) return null
  const i = paths.indexOf(current)
  if (i < 0) return dir === 1 ? paths[0] : paths[paths.length - 1]
  const next = i + dir
  if (next < 0 || next >= paths.length) return null
  return paths[next]
}

/**
 * Top-most currently intersecting file (smallest `top`).
 *
 * IntersectionObserver callbacks are a **delta** — only entries that crossed a
 * threshold. Feeding that list here picks a next file the moment its header
 * enters the top band, then `fileAtScrollTop` snaps back 150ms later. Pass a
 * complete snapshot, or use `fileAtScrollTop` instead.
 */
export function topmostIntersecting(
  entries: readonly { path: string; top: number; intersecting: boolean }[],
): string {
  let best = ''
  let bestTop = Infinity
  for (const e of entries) {
    if (!e.intersecting || !e.path) continue
    if (e.top < bestTop) {
      bestTop = e.top
      best = e.path
    }
  }
  return best
}
