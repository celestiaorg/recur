import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { foldable } from '@codemirror/language'
import {
  compileFoldMarkers,
  foldByIndent,
  foldByMarkers,
  textmateFolding,
} from './cm-tm-fold'
import { extractFoldMarkerSources } from './tm-registry'

function state(doc: string) {
  return EditorState.create({ doc })
}

describe('extractFoldMarkerSources', () => {
  it('reads TM folding markers from grammar JSON', () => {
    const src = extractFoldMarkerSources(
      JSON.stringify({
        scopeName: 'source.scala',
        foldingStartMarker: '/\\*\\*|\\{\\s*$',
        foldingStopMarker: '\\*\\*/|^\\s*}',
        patterns: [],
      }),
    )
    expect(src).toEqual({
      startSource: '/\\*\\*|\\{\\s*$',
      stopSource: '\\*\\*/|^\\s*}',
    })
  })

  it('returns null when markers are absent', () => {
    expect(extractFoldMarkerSources('{"scopeName":"source.go","patterns":[]}')).toBeNull()
  })
})

describe('foldByIndent', () => {
  it('folds a deeper-indented block', () => {
    const s = state('func main() {\n\tx := 1\n\ty := 2\n}\n')
    const line = s.doc.line(1)
    expect(foldByIndent(s, line.from)).toEqual({ from: line.to, to: s.doc.line(3).to })
  })

  it('returns null when nothing is deeper', () => {
    const s = state('a\nb\n')
    expect(foldByIndent(s, s.doc.line(1).from)).toBeNull()
  })
})

describe('foldByMarkers', () => {
  const markers = compileFoldMarkers({
    startSource: '/\\*\\*|\\{\\s*$',
    stopSource: '\\*\\*/|^\\s*}',
  })!

  it('folds from { line through content before }', () => {
    const s = state('object X {\n  val a = 1\n  val b = 2\n}\n')
    const line = s.doc.line(1)
    expect(foldByMarkers(s, line.from, markers.start, markers.stop)).toEqual({
      from: line.to,
      to: s.doc.line(3).to,
    })
  })

  it('handles nested braces', () => {
    const s = state('outer {\n  inner {\n    x\n  }\n  y\n}\n')
    const outer = s.doc.line(1)
    expect(foldByMarkers(s, outer.from, markers.start, markers.stop)).toEqual({
      from: outer.to,
      to: s.doc.line(5).to,
    })
    const inner = s.doc.line(2)
    expect(foldByMarkers(s, inner.from, markers.start, markers.stop)).toEqual({
      from: inner.to,
      to: s.doc.line(3).to,
    })
  })
})

describe('textmateFolding', () => {
  it('exposes indent folds via foldable()', () => {
    const doc = 'func main() {\n\tx := 1\n\ty := 2\n}\n'
    const s = EditorState.create({
      doc,
      extensions: [textmateFolding('source.go')],
    })
    const line = s.doc.line(1)
    expect(foldable(s, line.from, line.to)).toEqual({
      from: line.to,
      to: s.doc.line(3).to,
    })
  })
})
