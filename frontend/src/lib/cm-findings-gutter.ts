/**
 * Slim CodeMirror gutter for open reviewer findings on the current file.
 * Severity icons match Findings rows; click opens the finding; hover = title.
 */
import {
  RangeSet, RangeSetBuilder, StateEffect, StateField,
  type Extension, type Text,
} from '@codemirror/state'
import { EditorView, GutterMarker, gutter } from '@codemirror/view'
import { findingSevIcon } from './findings-ui'
import { icons, type IconDef } from './icons'

export type EditorFindingMark = {
  id: string
  line: number
  endLine?: number
  severity: string
  title: string
}

type LineHit = {
  id: string
  severity: string
  title: string
  count: number
}

function sevRank(s: string): number {
  switch ((s || '').toLowerCase()) {
    case 'critical': return 4
    case 'high': return 3
    case 'medium': return 2
    case 'low': return 1
    default: return 0
  }
}

function normSev(s: string): string {
  const v = (s || '').toLowerCase()
  if (v === 'critical' || v === 'high' || v === 'medium' || v === 'low' || v === 'info') return v
  return 'info'
}

/** Build the same SVG Icon.svelte would for a severity glyph (gutter has no Svelte). */
function sevIconSVG(severity: string): SVGSVGElement {
  const def: IconDef = icons[findingSevIcon(severity)]
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
  svg.setAttribute('viewBox', def.viewBox ?? '0 0 24 24')
  svg.setAttribute('aria-hidden', 'true')
  if (def.filled) {
    svg.setAttribute('fill', 'currentColor')
  } else {
    svg.setAttribute('fill', 'none')
    svg.setAttribute('stroke', 'currentColor')
    svg.setAttribute('stroke-width', String(def.strokeWidth ?? 2))
    svg.setAttribute('stroke-linecap', 'round')
    svg.setAttribute('stroke-linejoin', 'round')
  }
  svg.innerHTML = def.body
  return svg
}

class FindingGutterMarker extends GutterMarker {
  constructor(
    readonly severity: string,
    readonly title: string,
    readonly findingId: string,
  ) {
    super()
  }

  eq(other: FindingGutterMarker) {
    return (
      other instanceof FindingGutterMarker
      && other.severity === this.severity
      && other.title === this.title
      && other.findingId === this.findingId
    )
  }

  toDOM() {
    const el = document.createElement('span')
    el.className = `cm-finding-mark cm-finding-mark-${normSev(this.severity)}`
    el.title = this.title
    el.setAttribute('aria-label', this.title)
    el.appendChild(sevIconSVG(this.severity))
    return el
  }
}

const spacer = new FindingGutterMarker('info', '', '')

class ZeroWidthSpacer extends GutterMarker {
  elementClass = 'cm-gutter-spacer-0'
  toDOM() {
    const el = document.createElement('div')
    el.style.width = '0'
    el.style.minWidth = '0'
    el.style.padding = '0'
    return el
  }
}
const collapsedSpacer = new ZeroWidthSpacer()

function hitsByLine(doc: Text, marks: readonly EditorFindingMark[]): Map<number, LineHit> {
  const byLine = new Map<number, LineHit>()
  for (const m of marks) {
    if (!m.id || m.line < 1) continue
    const start = Math.min(m.line, doc.lines)
    const end = m.endLine && m.endLine > m.line
      ? Math.min(m.endLine, doc.lines)
      : start
    for (let ln = start; ln <= end; ln++) {
      const cur = byLine.get(ln)
      if (!cur) {
        byLine.set(ln, {
          id: m.id,
          severity: m.severity || 'info',
          title: m.title || 'Finding',
          count: 1,
        })
        continue
      }
      cur.count++
      if (sevRank(m.severity) > sevRank(cur.severity)) {
        cur.id = m.id
        cur.severity = m.severity || 'info'
        cur.title = m.title || 'Finding'
      }
    }
  }
  for (const hit of byLine.values()) {
    if (hit.count > 1) hit.title = `${hit.title} (+${hit.count - 1} more)`
  }
  return byLine
}

function buildSet(doc: Text, marks: readonly EditorFindingMark[]): RangeSet<FindingGutterMarker> {
  if (!marks.length) return RangeSet.empty
  const byLine = hitsByLine(doc, marks)
  if (!byLine.size) return RangeSet.empty
  const lines = [...byLine.keys()].sort((a, b) => a - b)
  const builder = new RangeSetBuilder<FindingGutterMarker>()
  for (const ln of lines) {
    const hit = byLine.get(ln)!
    const line = doc.line(ln)
    builder.add(line.from, line.from, new FindingGutterMarker(hit.severity, hit.title, hit.id))
  }
  return builder.finish()
}

const setFindingsMarks = StateEffect.define<readonly EditorFindingMark[]>()

const findingsSourceField = StateField.define<readonly EditorFindingMark[]>({
  create() {
    return []
  },
  update(marks, tr) {
    for (const e of tr.effects) {
      if (e.is(setFindingsMarks)) return e.value
    }
    return marks
  },
})

const findingsMarkersField = StateField.define<RangeSet<FindingGutterMarker>>({
  create(state) {
    return buildSet(state.doc, state.field(findingsSourceField))
  },
  update(_set, tr) {
    let next: readonly EditorFindingMark[] | null = null
    for (const e of tr.effects) {
      if (e.is(setFindingsMarks)) next = e.value
    }
    if (next) return buildSet(tr.state.doc, next)
    if (tr.docChanged) return buildSet(tr.state.doc, tr.state.field(findingsSourceField))
    return _set
  },
})

/** Apply a new set of finding marks to a live editor view. */
export function setEditorFindings(view: EditorView, marks: readonly EditorFindingMark[]) {
  view.dispatch({ effects: setFindingsMarks.of(marks) })
}

/**
 * Findings gutter. Update marks with {@link setEditorFindings}.
 * `onOpen` is read from a ref so the parent can keep a stable extension.
 */
export function findingsGutterExtension(
  onOpenRef: { current: ((findingId: string) => void) | undefined },
): Extension {
  return [
    findingsSourceField,
    findingsMarkersField,
    gutter({
      class: 'cm-findings-gutter',
      markers: (v) => v.state.field(findingsMarkersField),
      initialSpacer: () => collapsedSpacer,
      updateSpacer: (_spacer, update) =>
        update.state.field(findingsMarkersField).size > 0 ? spacer : collapsedSpacer,
      domEventHandlers: {
        mousedown(view, line, event) {
          let id = ''
          view.state.field(findingsMarkersField).between(line.from, line.from, (_f, _t, value) => {
            id = value.findingId
          })
          if (!id) return false
          event.preventDefault()
          onOpenRef.current?.(id)
          return true
        },
      },
    }),
    EditorView.baseTheme({
      '.cm-findings-gutter': {
        minWidth: '0',
      },
      '.cm-findings-gutter .cm-gutterElement': {
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: '0 1px 0 4px',
      },
      '.cm-findings-gutter .cm-gutterElement.cm-gutter-spacer-0, .cm-findings-gutter:not(:has(.cm-finding-mark)) .cm-gutterElement:empty': {
        padding: '0',
        width: '0',
        minWidth: '0',
      },
      '.cm-finding-mark': {
        display: 'flex',
        width: '12px',
        height: '12px',
        color: 'var(--muted)',
        cursor: 'pointer',
      },
      '.cm-finding-mark svg': {
        width: '12px',
        height: '12px',
        display: 'block',
      },
      '.cm-finding-mark-critical, .cm-finding-mark-high': {
        color: 'var(--coral)',
      },
      '.cm-finding-mark-medium': {
        color: 'var(--amber)',
      },
      '.cm-finding-mark-low, .cm-finding-mark-info': {
        color: 'var(--muted)',
      },
    }),
  ]
}
