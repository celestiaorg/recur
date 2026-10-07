/**
 * Lint gutter using Recur sev-* icons (same pack as Findings), driven by
 * @codemirror/lint diagnostics from LSP serverDiagnostics / setDiagnostics.
 */
import {
  RangeSet, RangeSetBuilder, StateField, type Extension, type Text,
} from '@codemirror/state'
import { EditorView, GutterMarker, gutter } from '@codemirror/view'
import { setDiagnosticsEffect, type Diagnostic } from '@codemirror/lint'
import { icons, type IconDef, type IconName } from './icons'

function lintSevIcon(severity: string): IconName {
  switch (severity) {
    case 'error': return 'sev-high'
    case 'warning': return 'sev-medium'
    case 'info': return 'sev-info'
    default: return 'sev-low'
  }
}

function sevRank(s: string): number {
  switch (s) {
    case 'error': return 4
    case 'warning': return 3
    case 'info': return 2
    case 'hint': return 1
    default: return 0
  }
}

function sevIconSVG(severity: string): SVGSVGElement {
  const def: IconDef = icons[lintSevIcon(severity)]
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

type LineHit = { severity: string; title: string; count: number }

class LintSevMarker extends GutterMarker {
  constructor(
    readonly severity: string,
    readonly title: string,
  ) {
    super()
  }

  eq(other: LintSevMarker) {
    return (
      other instanceof LintSevMarker
      && other.severity === this.severity
      && other.title === this.title
    )
  }

  toDOM() {
    const el = document.createElement('span')
    el.className = `cm-lint-sev cm-lint-sev-${this.severity}`
    el.title = this.title
    el.setAttribute('aria-label', this.title)
    el.appendChild(sevIconSVG(this.severity))
    return el
  }
}

const spacer = new LintSevMarker('hint', '')

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

function hitsByLine(doc: Text, diagnostics: readonly Diagnostic[]): Map<number, LineHit> {
  const byLine = new Map<number, LineHit>()
  for (const d of diagnostics) {
    if (d.from < 0 || d.from > doc.length) continue
    const ln = doc.lineAt(d.from).number
    const sev = d.severity || 'hint'
    const msg = (d.message || 'Issue').replace(/\s+/g, ' ').trim()
    const cur = byLine.get(ln)
    if (!cur) {
      byLine.set(ln, { severity: sev, title: msg, count: 1 })
      continue
    }
    cur.count++
    if (sevRank(sev) > sevRank(cur.severity)) {
      cur.severity = sev
      cur.title = msg
    }
  }
  for (const hit of byLine.values()) {
    if (hit.count > 1) hit.title = `${hit.title} (+${hit.count - 1} more)`
  }
  return byLine
}

function buildSet(doc: Text, diagnostics: readonly Diagnostic[]): RangeSet<LintSevMarker> {
  if (!diagnostics.length) return RangeSet.empty
  const byLine = hitsByLine(doc, diagnostics)
  if (!byLine.size) return RangeSet.empty
  const lines = [...byLine.keys()].sort((a, b) => a - b)
  const builder = new RangeSetBuilder<LintSevMarker>()
  for (const ln of lines) {
    const hit = byLine.get(ln)!
    const line = doc.line(ln)
    builder.add(line.from, line.from, new LintSevMarker(hit.severity, hit.title))
  }
  return builder.finish()
}

const lintDiagSource = StateField.define<readonly Diagnostic[]>({
  create() {
    return []
  },
  update(diags, tr) {
    for (const e of tr.effects) {
      if (e.is(setDiagnosticsEffect)) return e.value ?? []
    }
    if (tr.docChanged) {
      return diags.map((d) => ({
        ...d,
        from: tr.changes.mapPos(d.from, 1),
        to: tr.changes.mapPos(d.to, -1),
      }))
    }
    return diags
  },
})

const lintSevMarkers = StateField.define<RangeSet<LintSevMarker>>({
  create(state) {
    return buildSet(state.doc, state.field(lintDiagSource))
  },
  update(_set, tr) {
    let next: readonly Diagnostic[] | null = null
    for (const e of tr.effects) {
      if (e.is(setDiagnosticsEffect)) next = e.value ?? []
    }
    if (next) return buildSet(tr.state.doc, next)
    if (tr.docChanged) return buildSet(tr.state.doc, tr.state.field(lintDiagSource))
    return _set
  },
})

/** Recur sev-icon gutter for LSP / setDiagnostics diagnostics. */
export function recurLintGutter(): Extension {
  return [
    lintDiagSource,
    lintSevMarkers,
    gutter({
      class: 'cm-lint-sev-gutter',
      markers: (v) => v.state.field(lintSevMarkers),
      initialSpacer: () => collapsedSpacer,
      updateSpacer: (_spacer, update) =>
        update.state.field(lintSevMarkers).size > 0 ? spacer : collapsedSpacer,
    }),
    EditorView.baseTheme({
      '.cm-lint-sev-gutter': {
        minWidth: '0',
      },
      '.cm-lint-sev-gutter .cm-gutterElement': {
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: '0 1px 0 4px',
      },
      '.cm-lint-sev-gutter .cm-gutterElement.cm-gutter-spacer-0, .cm-lint-sev-gutter:not(:has(.cm-lint-sev)) .cm-gutterElement:empty': {
        padding: '0',
        width: '0',
        minWidth: '0',
      },
      '.cm-lint-sev': {
        display: 'flex',
        width: '12px',
        height: '12px',
        color: 'var(--muted)',
      },
      '.cm-lint-sev svg': {
        width: '12px',
        height: '12px',
        display: 'block',
      },
      '.cm-lint-sev-error': {
        color: 'var(--coral)',
      },
      '.cm-lint-sev-warning': {
        color: 'var(--amber)',
      },
      '.cm-lint-sev-info': {
        color: 'var(--cyan)',
      },
      '.cm-lint-sev-hint': {
        color: 'var(--muted)',
      },
    }),
  ]
}
