/**
 * Slim CodeMirror gutter for SCM-style add / modify / delete hints.
 * Same baseline as Observatory Diff (FileAtBase), painted like VS Code's
 * editorGutter.added/modified/deleted decorations.
 */
import {
  RangeSet, RangeSetBuilder, StateEffect, StateField,
  type Extension, type Text,
} from '@codemirror/state'
import { EditorView, GutterMarker, ViewPlugin, gutter, type ViewUpdate } from '@codemirror/view'
import { lineHunks } from './cm-line-diff'
import { normalizeEditorNewlines } from './editor-dirty'

export type DiffHintKind = 'add' | 'mod' | 'del'

export type DiffHintMark = {
  line: number
  kind: DiffHintKind
}

const TITLES: Record<DiffHintKind, string> = {
  add: 'Added versus base',
  mod: 'Modified versus base',
  del: 'Deleted lines versus base',
}

/** Idle recompute after edits: marks remap with the change set in the meantime. */
const DIFF_RECOMPUTE_MS = 80

function setKind(byLine: Map<number, DiffHintKind>, line: number, kind: DiffHintKind, maxLine: number) {
  if (line < 1 || line > maxLine) return
  const prev = byLine.get(line)
  if (!prev) byLine.set(line, kind)
  else if (prev !== kind) byLine.set(line, 'mod')
}

/**
 * Classify line hunks into gutter marks on the current document:
 * green bar (added lines), --git-m bar (modified lines), red bar
 * (pure deletions, on the line that now sits after the gap).
 */
export function diffMarksFromDocs(base: string, current: string): DiffHintMark[] {
  const a = normalizeEditorNewlines(base)
  const b = normalizeEditorNewlines(current)
  if (a === b) return []
  const bLineCount = Math.max(b.split('\n').length, 1)
  // Empty base is a new/untracked file: every current line is an addition.
  // Don't run Myers: an empty string is one blank line and would match a
  // trailing newline, painting the file as a replace instead of an add.
  if (!a) {
    return Array.from({ length: bLineCount }, (_, i) => ({ line: i + 1, kind: 'add' as const }))
  }
  if (!b) return [{ line: 1, kind: 'del' }]
  const hunks = lineHunks(a, b)
  const byLine = new Map<number, DiffHintKind>()
  for (const h of hunks) {
    const insert = h.fromA === h.toA
    const del = h.fromB === h.toB
    if (insert && !del) {
      for (let i = h.fromB; i < h.toB; i++) setKind(byLine, i + 1, 'add', bLineCount)
      continue
    }
    if (del && !insert) {
      setKind(byLine, Math.min(Math.max(h.fromB + 1, 1), bLineCount), 'del', bLineCount)
      continue
    }
    for (let i = h.fromB; i < h.toB; i++) setKind(byLine, i + 1, 'mod', bLineCount)
  }
  return [...byLine.entries()]
    .sort((x, y) => x[0] - y[0])
    .map(([line, kind]) => ({ line, kind }))
}

class DiffHintMarker extends GutterMarker {
  constructor(readonly kind: DiffHintKind) {
    super()
  }

  eq(other: DiffHintMarker) {
    return other instanceof DiffHintMarker && other.kind === this.kind
  }

  toDOM() {
    const el = document.createElement('span')
    el.className = `cm-diff-hint cm-diff-hint-${this.kind}`
    el.title = TITLES[this.kind]
    el.setAttribute('aria-label', TITLES[this.kind])
    return el
  }
}

function buildSet(doc: Text, base: string | null): RangeSet<DiffHintMarker> {
  if (base == null) return RangeSet.empty
  const marks = diffMarksFromDocs(base, doc.toString())
  if (!marks.length) return RangeSet.empty
  const builder = new RangeSetBuilder<DiffHintMarker>()
  for (const m of marks) {
    if (m.line < 1 || m.line > doc.lines) continue
    const line = doc.line(m.line)
    builder.add(line.from, line.from, new DiffHintMarker(m.kind))
  }
  return builder.finish()
}

const setDiffBase = StateEffect.define<string | null>()
const recomputeDiffMarks = StateEffect.define<null>()

const diffBaseField = StateField.define<string | null>({
  create() {
    return null
  },
  update(base, tr) {
    for (const e of tr.effects) {
      if (e.is(setDiffBase)) return e.value
    }
    return base
  },
})

const diffMarkersField = StateField.define<RangeSet<DiffHintMarker>>({
  create(state) {
    return buildSet(state.doc, state.field(diffBaseField))
  },
  update(_set, tr) {
    let next: string | null | undefined
    let recompute = false
    for (const e of tr.effects) {
      if (e.is(setDiffBase)) next = e.value
      if (e.is(recomputeDiffMarks)) recompute = true
    }
    if (next !== undefined) return buildSet(tr.state.doc, next)
    if (recompute) return buildSet(tr.state.doc, tr.state.field(diffBaseField))
    // Remap through the edit so bars stay attached to lines; full Myers waits
    // for the idle plugin so typing is not on the transaction critical path.
    if (tr.docChanged) return _set.map(tr.changes)
    return _set
  },
})

/** Apply (or clear) the git-base document used for gutter hints. */
export function setEditorDiffBase(view: EditorView, base: string | null) {
  const cur = view.state.field(diffBaseField, false)
  if (cur === undefined) return
  if (cur === base) return
  view.dispatch({ effects: setDiffBase.of(base) })
}

function diffRecomputePlugin() {
  return ViewPlugin.fromClass(class {
    timer = 0
    constructor(readonly view: EditorView) {}
    update(u: ViewUpdate) {
      if (!u.docChanged) return
      if (u.state.field(diffBaseField, false) == null) return
      window.clearTimeout(this.timer)
      this.timer = window.setTimeout(() => {
        this.timer = 0
        if (this.view.state.field(diffBaseField, false) == null) return
        this.view.dispatch({ effects: recomputeDiffMarks.of(null) })
      }, DIFF_RECOMPUTE_MS)
    }
    destroy() {
      window.clearTimeout(this.timer)
    }
  })
}

export function diffGutterExtension(): Extension {
  return [
    diffBaseField,
    diffMarkersField,
    diffRecomputePlugin(),
    gutter({
      class: 'cm-diff-gutter',
      markers: (v) => v.state.field(diffMarkersField),
    }),
    EditorView.baseTheme({
      // Fixed width: CM only mounts visible markers, so an unsized column
      // grows/shrinks as hunks enter the viewport and shoves the code.
      '.cm-diff-gutter': {
        width: '3px',
        minWidth: '3px',
      },
      '.cm-diff-gutter .cm-gutterElement': {
        display: 'flex',
        alignItems: 'stretch',
        justifyContent: 'flex-end',
        padding: '0',
      },
      '.cm-diff-hint': {
        display: 'block',
        boxSizing: 'border-box',
        width: '2px',
        alignSelf: 'stretch',
      },
      '.cm-diff-hint-add': {
        backgroundColor: 'var(--cm-gutter-added, var(--git-a))',
      },
      '.cm-diff-hint-mod': {
        backgroundColor: 'var(--git-m)',
      },
      '.cm-diff-hint-del': {
        backgroundColor: 'var(--cm-gutter-deleted, var(--git-d))',
      },
    }),
  ]
}
