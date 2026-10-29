

/** Sidebar order for one project's task rows: each visible parent followed by
 *  its children (store order within each tier). A child whose parent is not
 *  in the list (archived, other project) renders as a root. */
export function nestWorkspaces<T extends { id: string; parentId?: string }>(items: readonly T[]): T[] {
  const ids = new Set(items.map((w) => w.id))
  const kids = new Map<string, T[]>()
  const roots: T[] = []
  for (const w of items) {
    const pid = (w.parentId ?? '').trim()
    if (pid && pid !== w.id && ids.has(pid)) {
      const list = kids.get(pid)
      if (list) list.push(w)
      else kids.set(pid, [w])
    } else {
      roots.push(w)
    }
  }
  const out: T[] = []
  for (const r of roots) {
    out.push(r)
    const list = kids.get(r.id)
    if (list) out.push(...list)
  }
  return out
}

/** True when this row should render nested under a parent in the same list. */
export function isNestedChild<T extends { id: string; parentId?: string }>(w: T, items: readonly T[]): boolean {
  const pid = (w.parentId ?? '').trim()
  return !!pid && pid !== w.id && items.some((o) => o.id === pid)
}

/**
 * Pixels to add to a scroller's scrollTop so a row sits in the visible band
 * (below sticky headers, above a bottom fade). Coordinates are viewport-space.
 */
export function scrollDeltaToReveal(
  rowTop: number,
  rowBottom: number,
  viewTop: number,
  viewBottom: number,
  topInset: number,
  bottomInset: number,
): number {
  const visibleTop = viewTop + topInset
  const visibleBottom = viewBottom - bottomInset
  if (visibleBottom <= visibleTop) return rowTop - visibleTop
  if (rowTop < visibleTop) return rowTop - visibleTop
  if (rowBottom > visibleBottom) return rowBottom - visibleBottom
  return 0
}
