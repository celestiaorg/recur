/**
 * Insertion-order LRU with a resident-byte budget.
 *
 * Warm workspace caches stay discrete (different keys, put-filters, and
 * eviction side-effects) but share this map: Map insertion order is LRU,
 * `sizeOf` is the accounting, and the newest entry is always kept even when
 * it alone exceeds `maxBytes` (same rule as Go's file-index hub).
 */

/** JS strings are UTF-16. Header is a budgeting fudge, not V8 accounting. */
const STR_HDR = 24
const OBJ_HDR = 32
const MAP_ENTRY = 40

/** Per-kind heap budget for observatory / chat / CodeMirror parks. */
export const WARM_CACHE_MAX_BYTES = 64 * 1024 * 1024
/** Tiny chrome (land, tab strip, transcript UI). */
export const CHROME_CACHE_MAX_BYTES = 4 * 1024 * 1024

export function stringBytes(s: string): number {
  return STR_HDR + s.length * 2
}

/**
 * Approximate JS-heap for JSON-like values. Cycle-safe. CodeMirror `Text` /
 * `EditorState` are sized from `doc.length` so we never walk a syntax tree.
 */
export function estimateBytes(value: unknown, seen?: Set<object>): number {
  if (value == null) return 0
  const t = typeof value
  if (t === 'string') return stringBytes(value as string)
  if (t === 'number' || t === 'boolean') return 8
  if (t === 'bigint') return 16
  if (t !== 'object') return 0
  const obj = value as object
  if (seen?.has(obj)) return 0
  const vis = seen ?? new Set<object>()
  vis.add(obj)

  if (ArrayBuffer.isView(value)) return value.byteLength + OBJ_HDR
  if (value instanceof ArrayBuffer) return value.byteLength + OBJ_HDR

  const rec = value as { length?: number; lines?: number; doc?: { length?: number }; sliceString?: unknown }
  if (typeof rec.length === 'number' && typeof rec.lines === 'number' && typeof rec.sliceString === 'function') {
    return STR_HDR + rec.length * 2
  }
  if (rec.doc && typeof rec.doc.length === 'number' && 'facet' in rec) {
    return OBJ_HDR + rec.doc.length * 16
  }

  if (value instanceof Map) {
    let n = OBJ_HDR + value.size * MAP_ENTRY
    for (const [k, v] of value) n += estimateBytes(k, vis) + estimateBytes(v, vis)
    return n
  }
  if (value instanceof Set) {
    let n = OBJ_HDR + value.size * MAP_ENTRY
    for (const v of value) n += estimateBytes(v, vis)
    return n
  }
  if (Array.isArray(value)) {
    let n = OBJ_HDR + value.length * 8
    for (const v of value) n += estimateBytes(v, vis)
    return n
  }
  let n = OBJ_HDR
  for (const k of Object.keys(value)) {
    n += stringBytes(k) + estimateBytes((value as Record<string, unknown>)[k], vis)
  }
  return n
}

export type ByteLruOpts<V> = {
  maxBytes: number
  sizeOf: (value: V) => number
  /** Skip the insert (and drop any previous value) when sizeOf exceeds this. */
  entryMaxBytes?: number
}

type Slot<V> = { value: V; bytes: number }

export class ByteLru<V> {
  private readonly map = new Map<string, Slot<V>>()
  private total = 0
  private _maxBytes: number
  private readonly sizeOf: (value: V) => number
  private readonly entryMaxBytes: number

  constructor(opts: ByteLruOpts<V>) {
    this._maxBytes = opts.maxBytes
    this.sizeOf = opts.sizeOf
    this.entryMaxBytes = opts.entryMaxBytes ?? 0
  }

  get maxBytes(): number { return this._maxBytes }
  set maxBytes(n: number) {
    this._maxBytes = n
    this.evict('')
  }

  get size(): number { return this.map.size }
  get bytes(): number { return this.total }

  has(key: string): boolean { return this.map.has(key) }

  keys(): IterableIterator<string> { return this.map.keys() }

  peek(key: string): V | undefined { return this.map.get(key)?.value }

  /** LRU-promoting read. */
  get(key: string): V | undefined {
    const slot = this.map.get(key)
    if (!slot) return undefined
    this.map.delete(key)
    this.map.set(key, slot)
    return slot.value
  }

  /**
   * Insert `value` as newest. Returns entries dropped to fit the budget
   * (not including a same-key replace). Newest is never dropped.
   */
  set(key: string, value: V): { key: string; value: V }[] {
    if (!key) return []
    const bytes = Math.max(0, Math.ceil(this.sizeOf(value)))
    this.drop(key)
    if (this.entryMaxBytes > 0 && bytes > this.entryMaxBytes) return []
    this.map.set(key, { value, bytes })
    this.total += bytes
    return this.evict(key)
  }

  /** Remove and return, transferring ownership — no LRU promote. */
  take(key: string): V | undefined { return this.drop(key)?.value }

  delete(key: string): boolean { return !!this.drop(key) }

  /**
   * Re-measure a mutated value without changing LRU order. `pruneTabs`
   * deletes inner entries in place; promoting those sessions would lie.
   */
  refresh(key: string): void {
    const slot = this.map.get(key)
    if (!slot) return
    this.total -= slot.bytes
    slot.bytes = Math.max(0, Math.ceil(this.sizeOf(slot.value)))
    this.total += slot.bytes
    if (this.total < 0) this.total = 0
  }

  prune(keep: (key: string, value: V) => boolean): V[] {
    const dropped: V[] = []
    for (const [key, slot] of [...this.map]) {
      if (keep(key, slot.value)) continue
      this.map.delete(key)
      this.total -= slot.bytes
      dropped.push(slot.value)
    }
    if (this.total < 0) this.total = 0
    return dropped
  }

  clear(): void {
    this.map.clear()
    this.total = 0
  }

  private drop(key: string): { key: string; value: V } | undefined {
    const slot = this.map.get(key)
    if (!slot) return undefined
    this.map.delete(key)
    this.total -= slot.bytes
    if (this.total < 0) this.total = 0
    return { key, value: slot.value }
  }

  private evict(keepKey: string): { key: string; value: V }[] {
    const dropped: { key: string; value: V }[] = []
    if (this._maxBytes <= 0) return dropped
    while (this.total > this._maxBytes && this.map.size > 1) {
      const oldest = this.map.keys().next().value
      if (oldest === undefined || oldest === keepKey) break
      const gone = this.drop(oldest)
      if (gone) dropped.push(gone)
      else break
    }
    return dropped
  }
}
