/**
 * CodeMirror change sets → line splices for the TextMate engine document.
 */
import type { ChangeDesc, Text } from '@codemirror/state'

export type LineReplace = {
  /** 1-based first affected line (same index in the old and new documents). */
  fromLine: number
  /** How many old lines the splice removes. */
  oldLineCount: number
  /** Replacement lines (LF-split, no trailing newline on each). */
  newLines: string[]
}

/** Narrowed line-replace covering the lines whose text differs, or null if none. */
export function lineReplaceFromChanges(
  oldDoc: Text,
  newDoc: Text,
  changes: ChangeDesc,
): LineReplace | null {
  let fromA = Infinity
  let toA = -1
  let fromB = Infinity
  let toB = -1
  changes.iterChangedRanges((a0, a1, b0, b1) => {
    if (a0 < fromA) fromA = a0
    if (a1 > toA) toA = a1
    if (b0 < fromB) fromB = b0
    if (b1 > toB) toB = b1
  })
  if (fromA === Infinity) return null

  let oldFromLine = oldDoc.lineAt(Math.min(fromA, oldDoc.length)).number
  let oldToLine = oldDoc.lineAt(Math.min(Math.max(toA, fromA), oldDoc.length)).number
  let newFromLine = newDoc.lineAt(Math.min(fromB, newDoc.length)).number
  let newToLine = newDoc.lineAt(Math.min(Math.max(toB, fromB), newDoc.length)).number

  // A live refresh replaces the whole document; narrow it to the lines whose
  // text actually differs so the engine keeps tokens for the rest.
  while (
    oldFromLine <= oldToLine &&
    newFromLine <= newToLine &&
    oldDoc.line(oldFromLine).text === newDoc.line(newFromLine).text
  ) {
    oldFromLine++
    newFromLine++
  }
  while (
    oldToLine >= oldFromLine &&
    newToLine >= newFromLine &&
    oldDoc.line(oldToLine).text === newDoc.line(newToLine).text
  ) {
    oldToLine--
    newToLine--
  }
  if (oldToLine < oldFromLine && newToLine < newFromLine) return null

  const newLines: string[] = []
  for (let n = newFromLine; n <= newToLine; n++) newLines.push(newDoc.line(n).text)

  return {
    fromLine: oldFromLine,
    oldLineCount: oldToLine - oldFromLine + 1,
    newLines,
  }
}

/** Shift a line→value map through a line splice (keys are 1-based). */
export function spliceLineMap<T>(
  map: Map<number, T>,
  fromLine: number,
  oldLineCount: number,
  newLineCount: number,
): Map<number, T> {
  const next = new Map<number, T>()
  const delta = newLineCount - oldLineCount
  const oldEnd = fromLine + oldLineCount
  for (const [n, value] of map) {
    if (n < fromLine) next.set(n, value)
    else if (n >= oldEnd) next.set(n + delta, value)
  }
  return next
}

