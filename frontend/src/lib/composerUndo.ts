/**
 * Undo history for the contenteditable composer. WebKit's native stack merges
 * all consecutive typing into one step and misses our direct DOM edits (chips,
 * Shift+Enter), so the composer keeps its own wire-text snapshots instead.
 */

type Att = { id: string }
export type UndoSnap<A extends Att> = { text: string; atts: readonly A[]; caret: number }

/** Typing after this much idle time starts a new undo step. */
export const UNDO_PAUSE_MS = 700
const MAX_STEPS = 200

export class ComposerUndo<A extends Att> {
  private stack: UndoSnap<A>[] = []
  private i = -1
  private lastAt = 0
  /** +1 / -1 while the top step is single-char typing / deleting, else 0. */
  private kind = 0

  constructor(private pauseMs = UNDO_PAUSE_MS) {}

  /** External content change: drop history unless the content is unchanged (or `force`). */
  reset(s: UndoSnap<A>, force = false) {
    const top = this.stack[this.i]
    if (!force && top && top.text === s.text && sameIds(top.atts, s.atts)) return
    this.stack = [s]
    this.i = 0
    this.lastAt = 0
    this.kind = 0
  }

  /**
   * Record the state after an edit. Single-char edits in the same direction,
   * at the caret, on the same line, within the pause window merge into the
   * current step.
   */
  record(s: UndoSnap<A>, now = Date.now()) {
    const top = this.stack[this.i]
    if (!top) return this.reset(s)
    const sameAtts = sameIds(top.atts, s.atts)
    if (top.text === s.text && sameAtts) {
      this.stack[this.i] = s
      return
    }
    this.stack.length = this.i + 1
    const delta = s.text.length - top.text.length
    const sameLines = lineCount(s.text) === lineCount(top.text)
    const kind = Math.abs(delta) === 1 && sameAtts && sameLines ? delta : 0
    const merge = kind !== 0
      && kind === this.kind
      && now - this.lastAt < this.pauseMs
      && Math.abs(s.caret - top.caret) <= 1
    if (merge) {
      this.stack[this.i] = s
    } else {
      this.stack.push(s)
      if (this.stack.length > MAX_STEPS) this.stack.shift()
      this.i = this.stack.length - 1
    }
    this.kind = kind
    this.lastAt = now
  }

  undo(): UndoSnap<A> | null {
    if (this.i <= 0) return null
    const from = this.stack[this.i]
    this.i--
    return this.restore(from, this.stack[this.i])
  }

  redo(): UndoSnap<A> | null {
    if (this.i >= this.stack.length - 1) return null
    const from = this.stack[this.i]
    this.i++
    return this.restore(from, this.stack[this.i])
  }

  private restore(from: UndoSnap<A>, to: UndoSnap<A>): UndoSnap<A> {
    this.lastAt = 0
    this.kind = 0
    return { text: to.text, atts: to.atts, caret: changeEnd(from.text, to.text) }
  }
}

function lineCount(text: string): number {
  return text.split('\n').length
}

function sameIds(a: readonly Att[], b: readonly Att[]): boolean {
  return a.length === b.length && a.every((x, i) => x.id === b[i].id)
}

/** Offset in `to` just past the region that differs from `from`. */
export function changeEnd(from: string, to: string): number {
  let p = 0
  const n = Math.min(from.length, to.length)
  while (p < n && from[p] === to[p]) p++
  let s = 0
  while (s < n - p && from[from.length - 1 - s] === to[to.length - 1 - s]) s++
  return to.length - s
}
