// Suppress intra-line word marks when they cover an entire line: the line wash
// is enough. Chunk.changes must still span those lines (hunk bounds), so we
// can't drop them from the diff; we tag the line and clear the mark fill in CSS.

import { getChunks } from '@codemirror/merge'
import { RangeSetBuilder, type Extension } from '@codemirror/state'
import {
  Decoration,
  ViewPlugin,
  type DecorationSet,
  type EditorView,
  type ViewUpdate,
} from '@codemirror/view'

const fullLineChange = Decoration.line({ class: 'cm-fullLineChange' })

function intervalsCover(lineFrom: number, lineTo: number, intervals: [number, number][]): boolean {
  const len = lineTo - lineFrom
  if (len <= 0) return false
  intervals.sort((x, y) => x[0] - y[0])
  let at = lineFrom
  for (const [a, b] of intervals) {
    if (a > at) return false
    if (b > at) at = b
    if (at >= lineTo) return true
  }
  return at >= lineTo
}

/** True when chunk.changes cover every character of [lineFrom, lineTo) on side B. */
export function lineFullyCoveredByChanges(
  chunkFromB: number,
  changes: readonly { fromB: number; toB: number }[],
  lineFrom: number,
  lineTo: number,
): boolean {
  const intervals: [number, number][] = []
  for (const c of changes) {
    const from = chunkFromB + c.fromB
    const to = chunkFromB + c.toB
    const a = Math.max(from, lineFrom)
    const b = Math.min(to, lineTo)
    if (a < b) intervals.push([a, b])
  }
  return intervalsCover(lineFrom, lineTo, intervals)
}

function computeFullLineDeco(view: EditorView): DecorationSet {
  const info = getChunks(view.state)
  if (!info) return Decoration.none
  const doc = view.state.doc
  const builder = new RangeSetBuilder<Decoration>()
  const { from: viewFrom, to: viewTo } = view.viewport

  for (const chunk of info.chunks) {
    if (chunk.fromB >= viewTo || chunk.toB <= viewFrom) continue
    if (chunk.fromB === chunk.toB) continue

    let pos = chunk.fromB
    while (pos < chunk.toB) {
      const line = doc.lineAt(Math.min(pos, doc.length))
      if (line.from >= chunk.toB) break
      if (line.to >= viewFrom && line.from <= viewTo) {
        if (lineFullyCoveredByChanges(chunk.fromB, chunk.changes, line.from, line.to)) {
          builder.add(line.from, line.from, fullLineChange)
        }
      }
      pos = line.to + 1
    }
  }
  return builder.finish()
}

/** Line class `cm-fullLineChange` when word marks would paint the whole line. */
export function suppressFullLineWordMarks(): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet
      constructor(view: EditorView) {
        this.decorations = computeFullLineDeco(view)
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged || getChunks(u.state) !== getChunks(u.startState)) {
          this.decorations = computeFullLineDeco(u.view)
        }
      }
    },
    { decorations: (v) => v.decorations },
  )
}
