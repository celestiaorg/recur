/**
 * Idle-time background tokenization sweep (VS Code's DefaultBackgroundTokenizer,
 * textModelTokens.ts:445-559 — requestIdleCallback outer budget, ~1.5ms inner
 * slices). Advances each document's accurate watermark to EOF, correcting
 * heuristic viewport guesses on the way, and notifies listeners with the
 * changed line range so views repaint only when visible tokens moved.
 */
import type { TmEngine } from './tm-engine'

export type TmFillListener = (changed: { from: number; to: number } | null, done: boolean) => void

type IdleHandle = { cancel: () => void }

/** requestIdleCallback with a setTimeout fallback (fixed ~4ms budget). */
function runWhenIdle(fn: (timeRemaining: () => number) => void): IdleHandle {
  const g = globalThis as {
    requestIdleCallback?: (cb: (d: { timeRemaining(): number }) => void, o?: { timeout: number }) => number
    cancelIdleCallback?: (id: number) => void
  }
  if (typeof g.requestIdleCallback === 'function') {
    const id = g.requestIdleCallback((d) => fn(() => d.timeRemaining()), { timeout: 500 })
    return { cancel: () => g.cancelIdleCallback?.(id) }
  }
  const id = setTimeout(() => {
    const end = performance.now() + 4
    fn(() => end - performance.now())
  }, 32)
  return { cancel: () => clearTimeout(id) }
}

const SLICE_MS = 1.5

type FillState = {
  listeners: Set<TmFillListener>
  handle: IdleHandle | null
  running: boolean
}

const fills = new Map<string, FillState>()

/**
 * Keep `docId` sweeping while at least one subscriber holds on. Returns an
 * unsubscribe; the sweep pauses when the last subscriber leaves and stops
 * when the document is fully tokenized.
 */
export function tmEnsureFill(engine: TmEngine, docId: string, listener: TmFillListener): () => void {
  let st = fills.get(docId)
  if (!st) {
    st = { listeners: new Set(), handle: null, running: false }
    fills.set(docId, st)
  }
  st.listeners.add(listener)
  schedule(engine, docId)
  return () => {
    const cur = fills.get(docId)
    if (!cur) return
    cur.listeners.delete(listener)
    if (cur.listeners.size === 0) {
      cur.handle?.cancel()
      fills.delete(docId)
    }
  }
}

function schedule(engine: TmEngine, docId: string): void {
  const st = fills.get(docId)
  if (!st || st.handle || st.running) return
  st.handle = runWhenIdle((timeRemaining) => {
    const cur = fills.get(docId)
    if (!cur) return
    cur.handle = null
    cur.running = true
    let changedFrom: number | null = null
    let changedTo = 0
    let done = false
    try {
      let lastFilled = -1
      do {
        const p = engine.fillSlice(docId, SLICE_MS)
        if (p.changedFrom !== null) {
          changedFrom = changedFrom === null ? p.changedFrom : Math.min(changedFrom, p.changedFrom)
          changedTo = Math.max(changedTo, p.changedTo)
        }
        done = p.done
        // No progress (grammar still loading) — back off to the next idle slot.
        if (done || p.filled === lastFilled) break
        lastFilled = p.filled
      } while (timeRemaining() > SLICE_MS)
    } finally {
      cur.running = false
    }
    const changed = changedFrom === null ? null : { from: changedFrom, to: changedTo }
    for (const fn of [...cur.listeners]) fn(changed, done)
    if (done) fills.delete(docId)
    else schedule(engine, docId)
  })
}

/** Tests: drop all pending sweeps. */
export function _resetTmFillForTests(): void {
  for (const st of fills.values()) st.handle?.cancel()
  fills.clear()
}
