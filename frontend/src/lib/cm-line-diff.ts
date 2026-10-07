// Line-oriented diff for Observatory MergeEditor.
//
// @codemirror/merge defaults to character Myers with scanLimit: 500. On a
// medium/large file that has been substantially edited (e.g. agent work vs
// spin-off base), the limiter collapses the whole remainder into one Change —
// so Diff paints nearly the entire file as a single hunk even when `git diff`
// reports many tight hunks. Git diffs lines; we do the same.
//
// Strategy: map each unique line to one BMP private-use code unit, run CM's
// own `diff` on the encoded strings (line Myers), then expand offsets back to
// character ranges in the original documents.
//
// Always pass scanLimit/timeout into the inner `diff` — unrestricted Myers on
// a large, highly divergent file freezes the UI (main-thread O(n·d)).

import { Change, diff, presentableDiff } from '@codemirror/merge'

const MAX_UNIQUE_LINES = 0xf8ff - 0xe000 + 1 // BMP private-use block

/** Depth budget for line-encoded Myers. Higher than CM's default 500 so we
 *  don't collapse medium refactors; capped so worst-case stays interactive. */
const LINE_DIFF_SCAN_LIMIT = 4000
/** Wall-clock abort for the inner diff (ms). */
const LINE_DIFF_TIMEOUT_MS = 80

/** Per-hunk char refine for highlightChanges (word marks inside line chunks). */
const WORD_DIFF_SCAN_LIMIT = 500
const WORD_DIFF_TIMEOUT_MS = 40
/** Skip refine when both sides of a hunk are huge — keep the line Change. */
const WORD_DIFF_MAX_CHARS = 20_000

export type LineSplit = { lines: string[]; starts: number[] }

/** Split on `\n`, recording each line's start offset in the original string. */
export function splitLines(s: string): LineSplit {
  const lines = s.split('\n')
  const starts: number[] = []
  let pos = 0
  for (let i = 0; i < lines.length; i++) {
    starts.push(pos)
    pos += lines[i].length + (i < lines.length - 1 ? 1 : 0)
  }
  return { lines, starts }
}

export function lineIndexToOffset(split: LineSplit, lineIndex: number, docLen: number): number {
  if (lineIndex < 0) return 0
  if (lineIndex >= split.starts.length) return docLen
  return split.starts[lineIndex]
}

/** Expand line-index Changes (from encoded Myers) to character offsets. */
export function toCharChanges(
  raw: readonly Change[],
  A: LineSplit,
  B: LineSplit,
  aLen: number,
  bLen: number,
): readonly Change[] {
  return raw.map(
    (ch) =>
      new Change(
        lineIndexToOffset(A, ch.fromA, aLen),
        lineIndexToOffset(A, ch.toA, aLen),
        lineIndexToOffset(B, ch.fromB, bLen),
        lineIndexToOffset(B, ch.toB, bLen),
      ),
  )
}

/** Whole-document replace — last-resort when we can't afford a real diff. */
function wholeFileChange(aLen: number, bLen: number): readonly Change[] {
  return [new Change(0, aLen, 0, bLen)]
}

/** Inclusive line containing `offset`; `docLen` maps to `lines.length`. */
export function lineIndexAt(split: LineSplit, offset: number, docLen: number): number {
  if (offset <= 0) return 0
  if (offset >= docLen) return split.lines.length
  const { starts } = split
  let lo = 0
  let hi = starts.length - 1
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (starts[mid]! <= offset) lo = mid
    else hi = mid - 1
  }
  return lo
}

/** Exclusive end line: a boundary at the start of line i does not include i. */
export function exclusiveLineAt(split: LineSplit, offset: number, docLen: number): number {
  if (offset <= 0) return 0
  if (offset >= docLen) return split.lines.length
  const i = lineIndexAt(split, offset, docLen)
  if (split.starts[i] === offset) return i
  return i + 1
}

/** Line-index hunk (0-based, exclusive ends) — same bounds git would use. */
export type LineHunk = {
  fromA: number
  toA: number
  fromB: number
  toB: number
}

function encodeLineAlphabet(a: LineSplit, b: LineSplit): { ea: string; eb: string } | null {
  const map = new Map<string, string>()
  let next = 0
  const encode = (lines: string[]): string | null => {
    const parts = new Array<string>(lines.length)
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]
      let ch = map.get(line)
      if (ch === undefined) {
        if (next >= MAX_UNIQUE_LINES) return null
        ch = String.fromCharCode(0xe000 + next++)
        map.set(line, ch)
      }
      parts[i] = ch
    }
    return parts.join('')
  }
  const ea = encode(a.lines)
  const eb = encode(b.lines)
  if (ea == null || eb == null) return null
  return { ea, eb }
}

/**
 * Line Myers hunks (no word refine). Used by the code-view SCM gutter so a
 * one-token edit is still a modified *line*, not a mid-line insert+delete.
 */
export function lineHunks(a: string, b: string): LineHunk[] {
  if (a === b) return []
  const A = splitLines(a)
  const B = splitLines(b)
  const encoded = encodeLineAlphabet(A, B)
  const conf = { scanLimit: LINE_DIFF_SCAN_LIMIT, timeout: LINE_DIFF_TIMEOUT_MS }
  // Char Myers + scanLimit collapses large unique-line files into one
  // whole-document Change. The gutter would then paint every line modified.
  // Emit nothing rather than a false wall of bars (Diff pane has its own fallback).
  if (!encoded) return []
  try {
    return [...diff(encoded.ea, encoded.eb, conf)].map((ch) => ({
      fromA: ch.fromA,
      toA: ch.toA,
      fromB: ch.fromB,
      toB: ch.toB,
    }))
  } catch {
    return []
  }
}

/**
 * Word/char refine inside each line-level replace so `highlightChanges` can
 * mark tokens instead of whole lines. Pure inserts/deletes stay as-is.
 * Uses CM `presentableDiff` (word-boundary cleanup) with a tight budget —
 * never passes our override, so no recursion.
 */
export function refineForHighlights(a: string, b: string, lineChanges: readonly Change[]): readonly Change[] {
  const out: Change[] = []
  const conf = { scanLimit: WORD_DIFF_SCAN_LIMIT, timeout: WORD_DIFF_TIMEOUT_MS }
  for (const ch of lineChanges) {
    if (ch.fromA === ch.toA || ch.fromB === ch.toB) {
      out.push(ch)
      continue
    }
    const sa = a.slice(ch.fromA, ch.toA)
    const sb = b.slice(ch.fromB, ch.toB)
    if (sa.length + sb.length > WORD_DIFF_MAX_CHARS) {
      out.push(ch)
      continue
    }
    try {
      const inner = presentableDiff(sa, sb, conf)
      if (inner.length === 0) {
        out.push(ch)
        continue
      }
      for (const ic of inner) {
        out.push(
          new Change(
            ic.fromA + ch.fromA,
            ic.toA + ch.fromA,
            ic.fromB + ch.fromB,
            ic.toB + ch.fromB,
          ),
        )
      }
    } catch {
      out.push(ch)
    }
  }
  return out
}

/**
 * DiffConfig.override for unifiedMergeView — line Myers for hunk boundaries
 * (close to `git diff`), then char refine inside replaces for word highlights.
 */
export function lineOrientedDiff(a: string, b: string): readonly Change[] {
  if (a === b) return []

  const A = splitLines(a)
  const B = splitLines(b)
  const encoded = encodeLineAlphabet(A, B)
  const conf = { scanLimit: LINE_DIFF_SCAN_LIMIT, timeout: LINE_DIFF_TIMEOUT_MS }

  let lineChanges: readonly Change[]
  if (!encoded) {
    // Too many unique lines — bounded char Myers, never unbounded.
    try {
      lineChanges = diff(a, b, conf)
    } catch {
      return wholeFileChange(a.length, b.length)
    }
    // Already char-level; presentableDiff in Chunk.build will polish.
    return lineChanges
  }

  try {
    lineChanges = toCharChanges(diff(encoded.ea, encoded.eb, conf), A, B, a.length, b.length)
  } catch {
    return wholeFileChange(a.length, b.length)
  }
  return refineForHighlights(a, b, lineChanges)
}
