import { RangeSetBuilder, type Extension } from '@codemirror/state'
import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from '@codemirror/view'

/** Full-width line backgrounds for unified-diff markup (token bg only hugs text). */
function lineClass(text: string): string | null {
  if (text.startsWith('+')) return 'cm-diff-ins'
  if (text.startsWith('-')) return 'cm-diff-del'
  if (text.startsWith('@@')) return 'cm-diff-meta'
  return null
}

const diffLinePlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet

    constructor(view: EditorView) {
      this.decorations = this.build(view)
    }

    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) this.decorations = this.build(u.view)
    }

    build(view: EditorView): DecorationSet {
      const b = new RangeSetBuilder<Decoration>()
      for (const { from, to } of view.visibleRanges) {
        let pos = from
        while (pos <= to) {
          const line = view.state.doc.lineAt(pos)
          const cls = lineClass(line.text)
          if (cls) b.add(line.from, line.from, Decoration.line({ class: cls }))
          pos = line.to + 1
        }
      }
      return b.finish()
    }
  },
  { decorations: (v) => v.decorations },
)

const diffLineTheme = EditorView.baseTheme({
  '.cm-diff-ins': { backgroundColor: 'rgba(63, 185, 80, 0.14)' },
  '.cm-diff-del': { backgroundColor: 'rgba(224, 101, 95, 0.14)' },
  '.cm-diff-meta': { backgroundColor: 'rgba(210, 168, 255, 0.10)' },
})

/** Pair with the legacy diff stream language. */
export const diffLineHighlight: Extension = [diffLinePlugin, diffLineTheme]
