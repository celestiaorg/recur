/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { setDiagnostics } from '@codemirror/lint'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView, highlightActiveLineGutter } from '@codemirror/view'
import { editorFeatureExtensions } from './cm-editor-prefs'
import { diffGutterExtension } from './cm-diff-gutter'
import { findingsGutterExtension, setEditorFindings } from './cm-findings-gutter'
import { recurLintGutter } from './cm-lint-gutter'
import { DEFAULT_EDITOR_PREFS } from './stores'

function mount(extensions: Extension) {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: 'one\ntwo\n',
      extensions,
    }),
  })
  return { view, parent }
}

function gutter(view: EditorView, cls: string) {
  return view.dom.querySelector(cls)
}

function assertCollapsed(view: EditorView, cls: string) {
  const el = gutter(view, cls)
  expect(el).toBeTruthy()
  expect(el!.getBoundingClientRect().width).toBeLessThan(2)
  expect(el!.querySelector('.cm-gutter-spacer-0')).toBeTruthy()
}

function assertExpanded(view: EditorView, cls: string) {
  const el = gutter(view, cls)
  expect(el).toBeTruthy()
  const w = el!.getBoundingClientRect().width
  if (w > 0) expect(w).toBeGreaterThanOrEqual(10)
  expect(el!.querySelector('.cm-gutter-spacer-0')).toBeNull()
}

describe('findings gutter collapse', () => {
  it('starts collapsed, grows with marks, and collapses when cleared', () => {
    const { view, parent } = mount([
      findingsGutterExtension({ current: undefined }),
      highlightActiveLineGutter(),
    ])
    assertCollapsed(view, '.cm-findings-gutter')
    setEditorFindings(view, [{ id: 'f1', line: 1, severity: 'high', title: 'Bug' }])
    assertExpanded(view, '.cm-findings-gutter')
    expect(view.dom.querySelector('.cm-finding-mark')).toBeTruthy()
    setEditorFindings(view, [])
    assertCollapsed(view, '.cm-findings-gutter')
    view.destroy()
    parent.remove()
  })
})

describe('lint gutter collapse', () => {
  it('starts collapsed, grows with diagnostics, and collapses when cleared', () => {
    const { view, parent } = mount([recurLintGutter(), highlightActiveLineGutter()])
    assertCollapsed(view, '.cm-lint-sev-gutter')
    view.dispatch(setDiagnostics(view.state, [{ from: 0, to: 1, severity: 'error', message: 'nope' }]))
    assertExpanded(view, '.cm-lint-sev-gutter')
    expect(view.dom.querySelector('.cm-lint-sev')).toBeTruthy()
    view.dispatch(setDiagnostics(view.state, []))
    assertCollapsed(view, '.cm-lint-sev-gutter')
    view.destroy()
    parent.remove()
  })
})

describe('gutter order', () => {
  it('places findings and lint left of line numbers, fold and diff against the code', () => {
    const { view, parent } = mount([
      findingsGutterExtension({ current: undefined }),
      recurLintGutter(),
      editorFeatureExtensions(DEFAULT_EDITOR_PREFS),
      diffGutterExtension(),
    ])
    const classes = [...view.dom.querySelectorAll('.cm-gutters-before > .cm-gutter')]
      .map((el) => el.className)
    const names = ['cm-findings-gutter', 'cm-lint-sev-gutter', 'cm-lineNumbers', 'cm-foldGutter', 'cm-diff-gutter']
    const idx = names.map((n) => classes.findIndex((c) => c.includes(n)))
    expect(idx.every((i) => i >= 0)).toBe(true)
    expect(idx).toEqual([...idx].sort((a, b) => a - b))
    assertCollapsed(view, '.cm-findings-gutter')
    assertCollapsed(view, '.cm-lint-sev-gutter')
    const bar = view.dom.querySelector('.cm-lineNumbers .cm-gutterElement.cm-activeLineGutter')
    expect(bar).toBeTruthy()
    expect(getComputedStyle(bar!).boxShadow).toMatch(/inset/)
    view.destroy()
    parent.remove()
  })

  it('moves the active-line inset to the lint column when diagnostics exist', () => {
    const { view, parent } = mount([
      recurLintGutter(),
      editorFeatureExtensions(DEFAULT_EDITOR_PREFS),
    ])
    view.dispatch(setDiagnostics(view.state, [{ from: 0, to: 1, severity: 'warning', message: 'nope' }]))
    const lintBar = view.dom.querySelector('.cm-lint-sev-gutter .cm-gutterElement.cm-activeLineGutter')
    const numBar = view.dom.querySelector('.cm-lineNumbers .cm-gutterElement.cm-activeLineGutter')
    expect(lintBar).toBeTruthy()
    expect(getComputedStyle(lintBar!).boxShadow).toMatch(/inset/)
    expect(getComputedStyle(numBar!).boxShadow).not.toMatch(/inset/)
    view.destroy()
    parent.remove()
  })
})
