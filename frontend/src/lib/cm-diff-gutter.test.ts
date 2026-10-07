/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView, highlightActiveLine, highlightActiveLineGutter } from '@codemirror/view'
import {
  diffGutterExtension,
  diffMarksFromDocs,
  setEditorDiffBase,
} from './cm-diff-gutter'

describe('diffMarksFromDocs', () => {
  it('returns nothing when the documents match', () => {
    expect(diffMarksFromDocs('a\nb\n', 'a\nb\n')).toEqual([])
  })

  it('marks inserted lines as add', () => {
    expect(diffMarksFromDocs('a\nc\n', 'a\nIN\nc\n')).toEqual([
      { line: 2, kind: 'add' },
    ])
  })

  it('marks a replaced line as modified, not add+delete', () => {
    expect(diffMarksFromDocs('keep\nalpha = 1\ntail\n', 'keep\nalpha = 2\ntail\n')).toEqual([
      { line: 2, kind: 'mod' },
    ])
  })

  it('puts a delete mark on the line after the gap', () => {
    expect(diffMarksFromDocs('a\nX\nY\nc\n', 'a\nc\n')).toEqual([
      { line: 2, kind: 'del' },
    ])
  })

  it('treats a new file (empty base) as all additions', () => {
    expect(diffMarksFromDocs('', 'one\ntwo\n')).toEqual([
      { line: 1, kind: 'add' },
      { line: 2, kind: 'add' },
      { line: 3, kind: 'add' },
    ])
  })

  it('emits nothing when the line alphabet is exhausted (no false all-modified wall)', () => {
    const a = Array.from({ length: 8000 }, (_, i) => `L${i}_aaaa`).join('\n')
    const b = a.replace('L50_aaaa', 'L50_EDIT')
    expect(diffMarksFromDocs(a, b)).toEqual([])
  })

  it('normalizes CRLF before comparing', () => {
    expect(diffMarksFromDocs('a\r\nb\r\n', 'a\nb\n')).toEqual([])
    expect(diffMarksFromDocs('a\r\nb\r\n', 'a\nX\n')).toEqual([
      { line: 2, kind: 'mod' },
    ])
  })
})

describe('diffGutterExtension', () => {
  it('paints added-line markers for a new file', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'one\ntwo\n',
        extensions: [diffGutterExtension()],
      }),
    })
    setEditorDiffBase(view, '')
    const n = view.dom.querySelectorAll('.cm-diff-hint-add').length
    view.destroy()
    parent.remove()
    expect(n).toBeGreaterThan(0)
  })

  it('paints a delete marker when lines were removed', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'a\nc\n',
        extensions: [diffGutterExtension()],
      }),
    })
    setEditorDiffBase(view, 'a\nX\nY\nc\n')
    const n = view.dom.querySelectorAll('.cm-diff-hint-del').length
    view.destroy()
    parent.remove()
    expect(n).toBe(1)
  })

  it('clears markers when the base is dropped', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'only\n',
        extensions: [diffGutterExtension()],
      }),
    })
    setEditorDiffBase(view, '')
    expect(view.dom.querySelectorAll('.cm-diff-hint').length).toBeGreaterThan(0)
    setEditorDiffBase(view, null)
    const n = view.dom.querySelectorAll('.cm-diff-hint').length
    view.destroy()
    parent.remove()
    expect(n).toBe(0)
  })

  it('fills the SCM gutter cell with the active-line wash', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'one\ntwo\n',
        extensions: [
          highlightActiveLine(),
          highlightActiveLineGutter(),
          EditorView.theme({
            '.cm-gutterElement.cm-activeLineGutter': { backgroundColor: 'rgb(255, 0, 0)' },
          }),
          diffGutterExtension(),
        ],
      }),
    })
    setEditorDiffBase(view, '')
    const cell = view.dom.querySelector('.cm-diff-gutter .cm-gutterElement.cm-activeLineGutter')
    const bg = cell ? getComputedStyle(cell).backgroundColor : ''
    const hint = view.dom.querySelector('.cm-diff-hint-add')
    view.destroy()
    parent.remove()
    expect(cell).toBeTruthy()
    expect(hint).toBeTruthy()
    expect(bg).toMatch(/255,\s*0,\s*0/)
  })

  it('does not dispatch when the base is unchanged', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'only\n',
        extensions: [diffGutterExtension()],
      }),
    })
    setEditorDiffBase(view, '')
    const afterSet = view.state
    setEditorDiffBase(view, '')
    expect(view.state).toBe(afterSet)
    view.destroy()
    parent.remove()
  })
})
