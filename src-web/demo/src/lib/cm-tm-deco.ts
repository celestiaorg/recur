/**
 * Build CM6 mark decorations from tokenizeLine2 metadata (viewport-scoped).
 */
import { RangeSetBuilder, type Range } from '@codemirror/state'
import { Decoration, type DecorationSet, type EditorView } from '@codemirror/view'
import { tmPaintTokenBackgrounds } from './cm-tm-paint'
import { getActiveEditorBackground, getActivePaneBackground, getActiveRootBackgrounds, getColorMap } from './tm-registry'
import {
  TM_FONT,
  compositeCssColor,
  decodeBackground,
  decodeFontStyle,
  decodeForeground,
  shouldPaintTokenBackground,
  tokenForegroundReadable,
} from './tm-theme'

export function classForMeta(
  meta: number,
  paintBackground: boolean,
  colorMap: string[],
  documentBgs: string[],
  paneBg = documentBgs[0] || '#1e1e1e',
): string {
  const fg = decodeForeground(meta)
  const bg = decodeBackground(meta)
  const fs = decodeFontStyle(meta)
  const parts: string[] = []
  const fgColor = fg > 0 ? colorMap[fg] : undefined
  const bgColor = bg > 0 ? colorMap[bg] : undefined
  const paintTokenBg =
    paintBackground && bg > 0 && shouldPaintTokenBackground(bgColor, documentBgs)
  const surface = paintTokenBg && bgColor ? compositeCssColor(bgColor, paneBg) : paneBg
  if (fg > 0 && tokenForegroundReadable(fgColor, surface)) {
    parts.push(`cm-tm-${fg}`)
  }
  if (paintTokenBg) {
    parts.push(`cm-tm-bg-${bg}`)
  }
  if (fs & TM_FONT.Italic) parts.push('cm-tm-italic')
  if (fs & TM_FONT.Bold) parts.push('cm-tm-bold')
  if (fs & TM_FONT.Underline) parts.push('cm-tm-underline')
  if (fs & TM_FONT.Strikethrough) parts.push('cm-tm-strike')
  return parts.join(' ')
}

/** Non-empty selection ranges — token washes must not cover CM’s behind-content selection. */
export function selectionRanges(view: EditorView): ReadonlyArray<{ from: number; to: number }> {
  const out: { from: number; to: number }[] = []
  for (const r of view.state.selection.ranges) {
    if (!r.empty) out.push({ from: r.from, to: r.to })
  }
  return out
}

function overlapsSelection(
  from: number,
  to: number,
  ranges: ReadonlyArray<{ from: number; to: number }>,
): boolean {
  for (const r of ranges) {
    if (from < r.to && to > r.from) return true
  }
  return false
}

function marksForLine(
  line: { from: number; text: string },
  tokens: ArrayLike<number>,
  paintBackground: boolean,
  classFor: (meta: number, paintBg: boolean) => string,
  sel: ReadonlyArray<{ from: number; to: number }>,
): Range<Decoration>[] {
  const text = line.text
  const out: Range<Decoration>[] = []
  // Clamp to the line: a token array is only guaranteed to match the text it
  // was produced from, and the paint path can hand us tokens for a previous
  // version of the line (an edit that shortened it). An unclamped offset
  // produces a mark past the line — or past the document on the last line —
  // which CodeMirror rejects when the view consumes the set, outside any
  // try/catch around the build.
  const len = text.length
  for (let i = 0; i < tokens.length; i += 2) {
    const start = Math.min(tokens[i], len)
    const meta = tokens[i + 1]
    const end = Math.min(i + 2 < tokens.length ? tokens[i + 2] : len, len)
    if (end <= start) continue
    const from = line.from + start
    const to = line.from + end
    const paintBg = paintBackground && !(sel.length > 0 && overlapsSelection(from, to, sel))
    const cls = classFor(meta, paintBg)
    out.push(Decoration.mark({ class: cls }).range(from, to))
  }
  return out
}

function paintContext(view: EditorView, scopeName = ''): {
  paintBackground: boolean
  classFor: (meta: number, paintBg: boolean) => string
  sel: ReadonlyArray<{ from: number; to: number }>
} {
  const paintBackground = view.state.facet(tmPaintTokenBackgrounds)
  const colorMap = getColorMap()
  const documentBgs = [getActiveEditorBackground(), ...getActiveRootBackgrounds()]
  const paneBg = getActivePaneBackground(scopeName)
  const memo = new Map<number, string>()
  const classFor = (meta: number, paintBg: boolean) => {
    const key = meta * 2 + (paintBg ? 1 : 0)
    let cls = memo.get(key)
    if (cls === undefined) {
      cls = classForMeta(meta, paintBg, colorMap, documentBgs, paneBg)
      memo.set(key, cls)
    }
    return cls
  }
  return {
    paintBackground,
    classFor,
    sel: paintBackground ? selectionRanges(view) : [],
  }
}

export function sameTokens(a: ArrayLike<number> | undefined, b: ArrayLike<number>): boolean {
  if (!a || a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

/** Full RangeSet for `fromLine..toLine` from cached tokens. */
export function buildDecorationsFromTokens(
  view: EditorView,
  tokensAt: (line: number) => ArrayLike<number> | undefined,
  fromLine: number,
  toLine: number,
  scopeName = '',
): DecorationSet {
  const ctx = paintContext(view, scopeName)
  const builder = new RangeSetBuilder<Decoration>()
  const last = Math.min(toLine, view.state.doc.lines)
  for (let n = Math.max(1, fromLine); n <= last; n++) {
    const tokens = tokensAt(n)
    if (!tokens) continue
    for (const r of marksForLine(view.state.doc.line(n), tokens, ctx.paintBackground, ctx.classFor, ctx.sel)) {
      builder.add(r.from, r.to, r.value)
    }
  }
  return builder.finish()
}

