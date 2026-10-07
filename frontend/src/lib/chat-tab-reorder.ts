/** Pure helpers for tab-strip drag reorder (chat, files, terminals). */

export const CHAT_TAB_DRAG_THRESHOLD = 4

/** Move `fromIndex` so it lands at `insertAt` in the list-without-that-item. */
export function reorderByInsert<T>(list: T[], fromIndex: number, insertAt: number): T[] {
  if (fromIndex < 0 || fromIndex >= list.length) return list
  const item = list[fromIndex]
  const others = list.filter((_, i) => i !== fromIndex)
  const at = Math.max(0, Math.min(insertAt, others.length))
  const next = others.slice()
  next.splice(at, 0, item)
  return next
}

/** First index in `mids` whose center is to the right of `clientX`; else append. */
export function insertIndexFromPointerX(mids: readonly number[], clientX: number): number {
  for (let i = 0; i < mids.length; i++) {
    if (clientX < mids[i]) return i
  }
  return mids.length
}

/** Restore `snapshot` order for keys that are still in `current`; keep new keys. */
export function restoreStripOrder<T>(
  current: T[],
  snapshot: T[],
  keyOf: (item: T) => string,
): T[] {
  const live = new Map(current.map((t) => [keyOf(t), t]))
  const out: T[] = []
  const seen = new Set<string>()
  for (const t of snapshot) {
    const k = keyOf(t)
    const item = live.get(k)
    if (!item || seen.has(k)) continue
    out.push(item)
    seen.add(k)
  }
  for (const t of current) {
    const k = keyOf(t)
    if (seen.has(k)) continue
    out.push(t)
    seen.add(k)
  }
  return out
}

/**
 * Apply a saved strip order. Unknown/closed codes are skipped. Loop still
 * leads when it is open but missing from the save; other new tabs append.
 */
export function applyChatTabOrder<T extends { code: string }>(tabs: T[], order: string[]): T[] {
  if (!order.length || tabs.length < 2) return tabs
  const byCode = new Map(tabs.map((t) => [t.code, t]))
  const inOrder = new Set(order)
  const out: T[] = []
  const seen = new Set<string>()
  const loop = byCode.get('MA')
  if (loop && !inOrder.has('MA')) {
    out.push(loop)
    seen.add('MA')
  }
  for (const code of order) {
    const t = byCode.get(code)
    if (!t || seen.has(code)) continue
    out.push(t)
    seen.add(code)
  }
  for (const t of tabs) {
    if (!seen.has(t.code)) out.push(t)
  }
  return out
}
