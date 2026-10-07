import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { lineReplaceFromChanges, spliceLineMap } from './cm-tm-delta'

describe('lineReplaceFromChanges', () => {
  it('replaces a single edited line', () => {
    const before = EditorState.create({ doc: 'a\nb\nc' })
    const tr = before.update({ changes: { from: 2, to: 3, insert: 'X' } })
    expect(lineReplaceFromChanges(before.doc, tr.state.doc, tr.changes)).toEqual({
      fromLine: 2,
      oldLineCount: 1,
      newLines: ['X'],
    })
  })

  it('splits a line when a newline is inserted', () => {
    const before = EditorState.create({ doc: 'hello' })
    const tr = before.update({ changes: { from: 2, insert: '\n' } })
    expect(lineReplaceFromChanges(before.doc, tr.state.doc, tr.changes)).toEqual({
      fromLine: 1,
      oldLineCount: 1,
      newLines: ['he', 'llo'],
    })
  })

  it('joins lines when a newline is deleted', () => {
    const before = EditorState.create({ doc: 'ab\ncd' })
    const tr = before.update({ changes: { from: 2, to: 3, insert: '' } })
    expect(lineReplaceFromChanges(before.doc, tr.state.doc, tr.changes)).toEqual({
      fromLine: 1,
      oldLineCount: 2,
      newLines: ['abcd'],
    })
  })

  it('returns null when nothing changed', () => {
    const state = EditorState.create({ doc: 'hello' })
    const tr = state.update({})
    expect(lineReplaceFromChanges(state.doc, tr.state.doc, tr.changes)).toBeNull()
  })
})

describe('spliceLineMap', () => {
  it('drops replaced keys and shifts the tail', () => {
    const map = new Map([
      [1, 'a'],
      [2, 'b'],
      [3, 'c'],
      [4, 'd'],
    ])
    expect([...spliceLineMap(map, 2, 1, 2)]).toEqual([
      [1, 'a'],
      [4, 'c'],
      [5, 'd'],
    ])
  })
})

