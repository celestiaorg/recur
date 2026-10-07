/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { EditorState, Text } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { defaultKeymap, history, historyKeymap, undo } from '@codemirror/commands'
import {
  bufferMountState, diskBaselineMoved, editorText, editorTextEq, isBufferDirty, normalizeEditorNewlines,
} from './editor-dirty'

describe('normalizeEditorNewlines', () => {
  it('leaves LF text unchanged', () => {
    expect(normalizeEditorNewlines('a\nb\n')).toBe('a\nb\n')
  })

  it('converts CRLF and bare CR to LF', () => {
    expect(normalizeEditorNewlines('a\r\nb\r\n')).toBe('a\nb\n')
    expect(normalizeEditorNewlines('a\rb\r')).toBe('a\nb\n')
  })
})

describe('editorTextEq', () => {
  it('treats identical strings as equal', () => {
    expect(editorTextEq('hello\n', 'hello\n')).toBe(true)
  })

  it('treats CRLF and LF of the same document as equal', () => {
    expect(editorTextEq('a\r\nb\r\n', 'a\nb\n')).toBe(true)
    expect(editorTextEq('a\nb\n', 'a\r\nb\r\n')).toBe(true)
  })

  it('rejects genuine content differences', () => {
    expect(editorTextEq('a\nb\n', 'a\nc\n')).toBe(false)
    expect(editorTextEq('a\r\nb\r\n', 'a\r\nc\r\n')).toBe(false)
  })
})

describe('isBufferDirty', () => {
  it('is clean against a missing snapshot', () => {
    expect(isBufferDirty(Text.of(['hi']), null)).toBe(false)
  })

  it('is dirty after an insert and clean after undo back to the snapshot', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'package main\n',
        extensions: [history(), keymap.of([...defaultKeymap, ...historyKeymap])],
      }),
    })
    const clean = view.state.doc
    expect(isBufferDirty(view.state.doc, clean)).toBe(false)

    view.dispatch({ changes: { from: view.state.doc.length, insert: 'X' } })
    expect(isBufferDirty(view.state.doc, clean)).toBe(true)

    view.dispatch({ changes: { from: view.state.doc.length - 1, to: view.state.doc.length } })
    expect(isBufferDirty(view.state.doc, clean)).toBe(false)

    view.dispatch({ changes: { from: view.state.doc.length, insert: 'X' } })
    expect(isBufferDirty(view.state.doc, clean)).toBe(true)

    undo(view)
    expect(isBufferDirty(view.state.doc, clean)).toBe(false)
    expect(view.state.doc.toString()).toBe('package main\n')
    expect(editorTextEq(view.state.doc.toString(), 'package main\n')).toBe(true)

    view.destroy()
    parent.remove()
  })
})

describe('bufferMountState', () => {
  it('treats a first open as clean without an EditorView or a throwaway Text', () => {
    const snap = bufferMountState('package main\n')
    expect(snap).toEqual({ dirty: false })
  })

  it('treats CRLF disk text matching the LF buffer as clean', () => {
    expect(bufferMountState('a\nb\n', 'a\r\nb\r\n').dirty).toBe(false)
  })

  it('keeps the disk snapshot when restoring an unsaved buffer', () => {
    const snap = bufferMountState('func main() { KEEPME }\n', 'func main() {}\n')
    expect(snap.dirty).toBe(true)
    if (!snap.dirty) throw new Error('expected dirty')
    expect(snap.clean.toString()).toBe('func main() {}\n')
  })
})

describe('diskBaselineMoved', () => {
  it('is true when there is no baseline yet', () => {
    expect(diskBaselineMoved(undefined, 'package main\n')).toBe(true)
  })

  it('is false when saved text matches the clean rope, including CRLF', () => {
    const clean = editorText('a\nb\n')
    expect(diskBaselineMoved(clean, 'a\nb\n')).toBe(false)
    expect(diskBaselineMoved(clean, 'a\r\nb\r\n')).toBe(false)
  })

  it('is true when disk moved under a dirty buffer', () => {
    const clean = editorText('func main() {}\n')
    expect(diskBaselineMoved(clean, 'func main() { AGENT }\n')).toBe(true)
  })

  it('is true for same-length content that actually changed', () => {
    const clean = editorText('abc')
    expect(diskBaselineMoved(clean, 'abd')).toBe(true)
  })
})
