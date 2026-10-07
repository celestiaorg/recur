/** @vitest-environment happy-dom */
import { describe, expect, it, beforeEach } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { undo, redo } from '@codemirror/commands'
import {
  captureEditorHistory,
  captureEditorHistoryFromState,
  restoreEditorState,
  forgetEditorHistory,
  forgetEditorHistoryTree,
  remapEditorHistory,
  editorHistoryKey,
  resetEditorHistoryForTests,
  setEditorHistoryMaxBytesForTests,
  editorHistoryCacheStatsForTests,
  editorHistoryExtension,
  EDITOR_HISTORY_DOC_CAP,
} from './cm-editor-history'

function mount(doc: string): EditorView {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  return new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [editorHistoryExtension()],
    }),
  })
}

function typeAtEnd(view: EditorView, text: string): void {
  view.dispatch({
    changes: { from: view.state.doc.length, insert: text },
  })
}

describe('cm-editor-history', () => {
  beforeEach(() => {
    resetEditorHistoryForTests()
  })

  it('restores undo and redo onto a fresh state', () => {
    const v1 = mount('hello')
    typeAtEnd(v1, '!')
    expect(v1.state.doc.toString()).toBe('hello!')
    captureEditorHistory('a.ts', v1)
    v1.destroy()
    v1.dom.parentElement?.remove()

    const restored = restoreEditorState('a.ts', 'hello!', [editorHistoryExtension()])
    expect(restored).not.toBeNull()
    const v2 = new EditorView({ state: restored!.state })
    expect(undo(v2)).toBe(true)
    expect(v2.state.doc.toString()).toBe('hello')
    expect(redo(v2)).toBe(true)
    expect(v2.state.doc.toString()).toBe('hello!')
    v2.destroy()
  })

  it('keeps independent stacks per file', () => {
    const a = mount('aaa')
    const b = mount('bbb')
    typeAtEnd(a, '1')
    typeAtEnd(b, '2')
    captureEditorHistory('a.ts', a)
    captureEditorHistory('b.ts', b)
    a.destroy()
    b.destroy()
    a.dom.parentElement?.remove()
    b.dom.parentElement?.remove()

    const ra = restoreEditorState('a.ts', 'aaa1', [editorHistoryExtension()])
    const rb = restoreEditorState('b.ts', 'bbb2', [editorHistoryExtension()])
    const va = new EditorView({ state: ra!.state })
    const vb = new EditorView({ state: rb!.state })
    undo(va)
    expect(va.state.doc.toString()).toBe('aaa')
    expect(vb.state.doc.toString()).toBe('bbb2')
    undo(vb)
    expect(vb.state.doc.toString()).toBe('bbb')
    va.destroy()
    vb.destroy()
  })

  it('drops history when the document no longer matches', () => {
    const v = mount('hello')
    typeAtEnd(v, '!')
    captureEditorHistory('a.ts', v)
    v.destroy()
    v.dom.parentElement?.remove()

    expect(restoreEditorState('a.ts', 'hello from disk', [editorHistoryExtension()])).toBeNull()
    expect(restoreEditorState('a.ts', 'hello!', [editorHistoryExtension()])).toBeNull()
  })

  it('forgetEditorHistoryTree removes a file and its children', () => {
    const v = mount('x')
    typeAtEnd(v, '!')
    captureEditorHistory(editorHistoryKey('ws', 'src/a.ts'), v)
    captureEditorHistory(editorHistoryKey('ws', 'src/nested/b.ts'), v)
    captureEditorHistory(editorHistoryKey('ws', 'other.ts'), v)
    v.destroy()
    v.dom.parentElement?.remove()

    forgetEditorHistoryTree('ws', 'src')
    expect(restoreEditorState(editorHistoryKey('ws', 'src/a.ts'), 'x!', [editorHistoryExtension()])).toBeNull()
    expect(restoreEditorState(editorHistoryKey('ws', 'src/nested/b.ts'), 'x!', [editorHistoryExtension()])).toBeNull()
    expect(restoreEditorState(editorHistoryKey('ws', 'other.ts'), 'x!', [editorHistoryExtension()])).not.toBeNull()
  })

  it('remapEditorHistory follows a rename', () => {
    const v = mount('hello')
    typeAtEnd(v, '!')
    captureEditorHistory(editorHistoryKey('ws', 'old.ts'), v)
    v.destroy()
    v.dom.parentElement?.remove()

    remapEditorHistory('ws', 'old.ts', 'new.ts')
    expect(restoreEditorState(editorHistoryKey('ws', 'old.ts'), 'hello!', [editorHistoryExtension()])).toBeNull()
    const restored = restoreEditorState(editorHistoryKey('ws', 'new.ts'), 'hello!', [editorHistoryExtension()])
    expect(restored).not.toBeNull()
    const v2 = new EditorView({ state: restored!.state })
    undo(v2)
    expect(v2.state.doc.toString()).toBe('hello')
    v2.destroy()
  })

  it('forgetEditorHistory drops a single key', () => {
    const v = mount('x')
    typeAtEnd(v, '!')
    captureEditorHistory('a.ts', v)
    v.destroy()
    v.dom.parentElement?.remove()
    forgetEditorHistory('a.ts')
    expect(restoreEditorState('a.ts', 'x!', [editorHistoryExtension()])).toBeNull()
  })

  it('evicts the oldest file when the byte budget is exceeded', () => {
    const a = mount('aaaaaaaaaa')
    const b = mount('bbbbbbbbbb')
    typeAtEnd(a, '1')
    typeAtEnd(b, '2')
    captureEditorHistory('a.ts', a)
    captureEditorHistory('b.ts', b)
    expect(editorHistoryCacheStatsForTests().files).toBe(2)
    expect(editorHistoryCacheStatsForTests().bytes).toBeGreaterThan(0)

    // a is oldest; a 1-byte cap must drop it and keep the newest snapshot.
    setEditorHistoryMaxBytesForTests(1)
    expect(restoreEditorState('a.ts', 'aaaaaaaaaa1', [editorHistoryExtension()])).toBeNull()
    expect(restoreEditorState('b.ts', 'bbbbbbbbbb2', [editorHistoryExtension()])).not.toBeNull()
    expect(editorHistoryCacheStatsForTests().files).toBe(1)

    a.destroy()
    b.destroy()
    a.dom.parentElement?.remove()
    b.dom.parentElement?.remove()
  })

  it('does not snapshot a view with no undo or redo events', () => {
    const v = mount('hello')
    captureEditorHistory('a.ts', v)
    expect(editorHistoryCacheStatsForTests().files).toBe(0)
    v.destroy()
    v.dom.parentElement?.remove()
  })

  it('serializes a mid-size dirty doc and skips one past DOC_CAP', () => {
    const keep = mount('hello')
    typeAtEnd(keep, '!')
    captureEditorHistory('small.ts', keep)

    let mid = EditorState.create({
      doc: 'm'.repeat(2 * 1024 * 1024),
      extensions: [editorHistoryExtension()],
    })
    mid = mid.update({ changes: { from: mid.doc.length, insert: '!' } }).state
    captureEditorHistoryFromState('mid.ts', mid)

    let huge = EditorState.create({
      doc: 'x'.repeat(EDITOR_HISTORY_DOC_CAP + 1),
      extensions: [editorHistoryExtension()],
    })
    huge = huge.update({ changes: { from: huge.doc.length, insert: '!' } }).state
    captureEditorHistoryFromState('huge.ts', huge)

    expect(editorHistoryCacheStatsForTests().files).toBe(2)
    expect(restoreEditorState('small.ts', 'hello!', [editorHistoryExtension()])).not.toBeNull()
    expect(restoreEditorState('mid.ts', mid.doc.toString(), [editorHistoryExtension()])).not.toBeNull()
    expect(restoreEditorState('huge.ts', huge.doc.toString(), [editorHistoryExtension()])).toBeNull()
    keep.destroy()
    keep.dom.parentElement?.remove()
  })

  it('does not let an empty stack overwrite a richer snapshot of the same doc', () => {
    const rich = mount('hello')
    typeAtEnd(rich, '!')
    captureEditorHistory('a.ts', rich)
    const empty = mount('hello!')
    captureEditorHistory('a.ts', empty)
    const restored = restoreEditorState('a.ts', 'hello!', [editorHistoryExtension()])
    expect(restored).not.toBeNull()
    const v = new EditorView({ state: restored!.state })
    expect(undo(v)).toBe(true)
    expect(v.state.doc.toString()).toBe('hello')
    v.destroy()
    rich.destroy()
    empty.destroy()
    rich.dom.parentElement?.remove()
    empty.dom.parentElement?.remove()
  })

  it('is not capped by file count under the default 20MB budget', () => {
    const v = mount('small')
    typeAtEnd(v, '!')
    for (let i = 0; i < 60; i++) captureEditorHistory(`f${i}.ts`, v)
    expect(editorHistoryCacheStatsForTests().files).toBe(60)
    expect(editorHistoryCacheStatsForTests().bytes).toBeLessThan(20 * 1024 * 1024)
    v.destroy()
    v.dom.parentElement?.remove()
  })
})
