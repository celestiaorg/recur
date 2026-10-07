/**
 * Map a CodeMirror selection to an @file mention span for ⌘L.
 *
 * Whole-line selections become `:12` / `:12-40`. A selection that starts or
 * ends mid-line becomes a column span (`:12:5-12:18` or `:12:5-13:20`) so the
 * composer chip and the agent wrap can name the highlighted text, not just
 * the line.
 */
import { type EditorState, type Text } from '@codemirror/state'
import { posAtLineCol } from './editor-view-state'

/** 1-based inclusive editor span for an @file chip. */
export type MentionSelection = {
  startLine: number
  endLine: number
  /** 1-based. Set with endCol when the selection is not whole-line(s). */
  startCol?: number
  endCol?: number
}

/**
 * 1-based line/column span for a non-empty selection.
 * Empty / collapsed caret → null (caller attaches the whole file).
 * A selection ending at column 0 of a line does not include that line.
 * Whole-line coverage omits columns so chips stay `@path:12` / `@path:12-40`.
 */
export function mentionSelectionFromState(state: EditorState): MentionSelection | null {
  const sel = state.selection.main
  if (sel.empty) return null
  const from = Math.min(sel.from, sel.to)
  const to = Math.max(sel.from, sel.to)
  const start = state.doc.lineAt(from)
  let end = state.doc.lineAt(to)
  if (to > from && end.from === to) {
    end = state.doc.line(Math.max(start.number, end.number - 1))
  }
  const coversFullLines = from === start.from && to >= end.to
  if (coversFullLines) {
    return { startLine: start.number, endLine: end.number }
  }
  const lastPos = Math.min(Math.max(from, to - 1), Math.max(end.from, end.to - 1))
  return {
    startLine: start.number,
    endLine: end.number,
    startCol: from - start.from + 1,
    endCol: Math.max(1, lastPos - end.from + 1),
  }
}

/** Document offsets for scrolling/selecting a mention loc (exclusive `to`). */
export function mentionRangeFromLoc(
  doc: Text,
  loc: { startLine: number; endLine?: number; startCol?: number; endCol?: number },
): { from: number; to: number } | null {
  if (loc.startLine < 1 || doc.lines < 1) return null
  const startLn = Math.min(loc.startLine, doc.lines)
  const endLn = loc.endLine != null && loc.endLine > startLn
    ? Math.min(loc.endLine, doc.lines)
    : startLn
  const startCol = loc.startCol ?? 0
  const endCol = loc.endCol ?? 0
  if (startCol > 0 && endCol > 0) {
    const from = posAtLineCol(doc, startLn, startCol)
    const endLine = doc.line(endLn)
    let to = posAtLineCol(doc, endLn, endCol)
    if (to < endLine.to) to += 1
    if (to < from) to = from
    return { from, to }
  }
  return { from: doc.line(startLn).from, to: doc.line(endLn).to }
}
