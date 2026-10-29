/** How many full DiffFile bodies may be mounted at once. */
export const REVIEW_HYDRATE_MAX = 8
/** Extra files around the selected path, even if they are off-screen. */
export const REVIEW_HYDRATE_OVERSCAN = 2
/** Prefetch band above/below the scrollport, in px. */
export const REVIEW_VIEW_PAD_PX = 900

const HEAD_PX = 38
export const DIFF_LINE_PX = 20
/** Extra rows above/below the viewport. Wrapping + comments make estimates drift. */
export const DIFF_LINE_OVERSCAN = 60
/** Skip line windowing when the file is this small — the `{#each}` is cheap. */
export const DIFF_LINE_WINDOW_MIN = 80
/** Guess when `lines` is missing (add/del stats only). */
const STAT_LINE_CAP = 500
const FILE_GAP_PX = 14

export function estimateDiffHeight(
  file: { lines?: readonly unknown[] | null; diff?: { add?: number; del?: number } | null },
  collapsed: boolean,
): number {
  if (collapsed) return HEAD_PX
  const n = file.lines?.length
    ?? Math.min((file.diff?.add ?? 0) + (file.diff?.del ?? 0) + 4, STAT_LINE_CAP)
  return HEAD_PX + Math.max(n, 1) * DIFF_LINE_PX + FILE_GAP_PX
}

export type LineWin = { start: number; end: number }

/** Sum of slot heights in `[start, end)`. Missing entries use `fallback`. */
export function slotPad(
  heights: ArrayLike<number | undefined> | undefined,
  start: number,
  end: number,
  fallback = DIFF_LINE_PX,
): number {
  const lo = Math.max(0, start)
  const hi = Math.max(lo, end)
  if (!heights) return (hi - lo) * fallback
  let n = 0
  for (let i = lo; i < hi; i++) n += heights[i] || fallback
  return n
}

/**
 * Walk `.df-lines` children: each slot is an `.hline` plus following
 * non-hline siblings (threads, findings, composer). Returns whether `into` changed.
 */
export function readSlotHeights(
  nodes: ArrayLike<{ classList: { contains: (name: string) => boolean }; offsetHeight: number }>,
  start: number,
  into: number[],
): boolean {
  let changed = false
  let slot = start
  let i = 0
  const n = nodes.length
  while (i < n) {
    let h = nodes[i].offsetHeight
    i++
    while (i < n && !nodes[i].classList.contains('hline')) {
      h += nodes[i].offsetHeight
      i++
    }
    if (h > 0 && into[slot] !== h) {
      into[slot] = h
      changed = true
    }
    slot++
  }
  return changed
}

/** Slice of rows overlapping the scrollport (+ overscan). `heights` is sparse. */
export function lineWindow(
  count: number,
  bodyTop: number,
  scrollTop: number,
  viewH: number,
  linePx = DIFF_LINE_PX,
  overscan = DIFF_LINE_OVERSCAN,
  heights?: ArrayLike<number | undefined>,
): LineWin {
  if (count <= 0) return { start: 0, end: 0 }
  if (count <= DIFF_LINE_WINDOW_MIN) return { start: 0, end: count }
  const pad = overscan * linePx
  const a = scrollTop - pad
  const b = scrollTop + viewH + pad
  const bodyBot = bodyTop + slotPad(heights, 0, count, linePx)
  if (bodyBot <= a) return { start: Math.max(0, count - overscan), end: count }
  if (bodyTop >= b) return { start: 0, end: Math.min(count, overscan) }
  let y = bodyTop
  let start = 0
  for (let i = 0; i < count; i++) {
    const next = y + ((heights && heights[i]) || linePx)
    if (next > a) { start = i; break }
    y = next
  }
  y = bodyTop + slotPad(heights, 0, start, linePx)
  let end = count
  for (let i = start; i < count; i++) {
    y += (heights && heights[i]) || linePx
    if (y >= b) { end = i + 1; break }
  }
  return { start, end: Math.max(start, end) }
}

export function coverIndex(win: LineWin, i: number, count: number): LineWin {
  if (i < 0 || i >= count) return win
  return { start: Math.min(win.start, i), end: Math.max(win.end, i + 1) }
}

/** Slice of `overscan` rows around `i` — jumps must not span the whole file. */
export function windowAround(i: number, count: number, overscan = DIFF_LINE_OVERSCAN): LineWin {
  if (count <= 0) return { start: 0, end: 0 }
  if (i < 0 || i >= count) return { start: 0, end: Math.min(count, overscan) }
  return { start: Math.max(0, i - overscan), end: Math.min(count, i + overscan + 1) }
}

/** Viewport slice, unless a new highlight jump needs `slot` mounted. */
export function jumpLineWindow(
  view: LineWin,
  count: number,
  highlightLine: number,
  lastJump: number,
  slot: number,
): LineWin {
  if (highlightLine > 0 && lastJump !== highlightLine && slot >= 0) {
    return windowAround(slot, count)
  }
  return view
}

/** Expand `win` to include `i` only when that row already sits near the slice. */
export function coverNear(win: LineWin, i: number, count: number, maxGap = DIFF_LINE_OVERSCAN): LineWin {
  if (i < 0 || i >= count) return win
  if (i + 1 + maxGap < win.start || i - maxGap > win.end) return win
  return coverIndex(win, i, count)
}

/** Restore the pane after a line-window slice change. Skip when the pane has
 *  moved since capture — a highlight jump or the user kept scrolling. */
export function applySlotPin(
  pane: { scrollTop: number },
  el: { isConnected: boolean; getBoundingClientRect(): { top: number } } | null | undefined,
  pinY: number | null,
  capturedTop: number,
): void {
  if (pinY == null || !el?.isConnected) return
  if (pane.scrollTop !== capturedTop) return
  const drift = el.getBoundingClientRect().top - pinY
  if (Math.abs(drift) > 0.5) pane.scrollTop += drift
}

/** min-height for a stacked `.cfile`. Live DiffFile cards size themselves
 * (including collapse). Collapsed stubs are header-only; expanded stubs keep
 * the last measured body so scroll position stays stable. */
export function reservedCardHeight(
  file: { lines?: readonly unknown[] | null; diff?: { add?: number; del?: number } | null },
  opts: { live: boolean; collapsed: boolean; measured?: number },
): number | undefined {
  if (opts.live) return undefined
  if (opts.collapsed) return HEAD_PX
  const m = opts.measured ?? 0
  return m > 0 ? m : estimateDiffHeight(file, false)
}

/**
 * Paths whose DiffFile body should mount. Selected file + neighbors first,
 * then the scrollport (visible before prefetch pad). Not distance-sorted, or a
 * stale spy drops the viewport.
 */
export function pickHydrated(
  paths: readonly string[],
  selected: string,
  inView: readonly string[],
  max = REVIEW_HYDRATE_MAX,
  overscan = REVIEW_HYDRATE_OVERSCAN,
): Set<string> {
  if (!paths.length || max <= 0) return new Set()
  const idx = new Map<string, number>()
  for (let i = 0; i < paths.length; i++) idx.set(paths[i], i)
  const out = new Set<string>()
  const add = (p: string | undefined) => {
    if (!p || out.size >= max || !idx.has(p)) return
    out.add(p)
  }
  const sel = idx.get(selected) ?? 0
  for (let d = 0; d <= overscan; d++) {
    add(paths[sel - d])
    add(paths[sel + d])
  }
  for (const p of inView) add(p)
  if (!out.size) out.add(paths[0])
  return out
}

/** Cache a hydrated card's offsetHeight so a later stub keeps the same min-height. */
export function recordStubHeight(
  el: HTMLElement,
  path: string,
  heights: Map<string, number>,
  onLiveResize?: () => void,
): () => void {
  const ro = new ResizeObserver(() => {
    if (el.classList.contains('stub')) return
    if (el.querySelector('.df-head.collapsed')) return
    const h = el.offsetHeight
    if (h <= 0) return
    const prev = heights.get(path)
    heights.set(path, h)
    if (onLiveResize && prev !== h) onLiveResize()
  })
  ro.observe(el)
  return () => ro.disconnect()
}

/** True when a stacked card's top has scrolled past the scrollport, so its
 * sticky header is pinned. Uses the same offsetTop/scrollTop space as inViewPaths. */
export function headerIsStuck(cardTop: number, cardBottom: number, scrollTop: number): boolean {
  return cardTop < scrollTop && cardBottom > scrollTop
}

export function syncStuckHeaders(cards: Iterable<HTMLElement>, scrollTop: number): void {
  for (const card of cards) {
    const head = card.querySelector('.df-head')
    if (!(head instanceof HTMLElement)) continue
    const top = card.offsetTop
    head.classList.toggle('stuck', headerIsStuck(top, top + card.offsetHeight, scrollTop))
  }
}

/** Visible cards first, then the prefetch pad, both in document order. */
export function inViewPaths(
  ranges: readonly { path: string; top: number; bottom: number }[],
  scrollTop: number,
  viewH: number,
  pad = REVIEW_VIEW_PAD_PX,
): string[] {
  const visA = scrollTop
  const visB = scrollTop + viewH
  const a = scrollTop - pad
  const b = scrollTop + viewH + pad
  const visible: string[] = []
  const prefetch: string[] = []
  for (const r of ranges) {
    if (r.bottom > visA && r.top < visB) visible.push(r.path)
    else if (r.bottom > a && r.top < b) prefetch.push(r.path)
  }
  return visible.concat(prefetch)
}

export function samePathList(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false
  return true
}

export function samePathSet(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false
  for (const p of a) if (!b.has(p)) return false
  return true
}
