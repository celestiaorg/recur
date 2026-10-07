/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { unifiedMergeView } from '@codemirror/merge'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { lineOrientedDiff } from './cm-line-diff'
import { editorFeatureExtensions } from './cm-editor-prefs'
import { DEFAULT_EDITOR_PREFS, type EditorPrefs } from './stores'

describe('unified merge + Code-view chrome', () => {
  function mount(prefs: EditorPrefs) {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'keep\nNEW\ntail\n',
        extensions: [
          editorFeatureExtensions(prefs),
          unifiedMergeView({
            original: 'keep\ntail\n',
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
        ],
      }),
    })
    return { view, parent }
  }

  it('paints B-side line numbers, fold gutter, and active-line chrome', () => {
    const { view, parent } = mount(DEFAULT_EDITOR_PREFS)
    const nums = [...parent.querySelectorAll('.cm-lineNumbers .cm-gutterElement')]
      .map((el) => el.textContent?.trim())
      .filter((t) => t && t !== '0')
    const fold = parent.querySelector('.cm-foldGutter')
    const active = parent.querySelector('.cm-activeLine')
    view.destroy()
    parent.remove()
    expect(nums).toEqual(expect.arrayContaining(['1', '2', '3']))
    expect(fold).toBeTruthy()
    expect(active).toBeTruthy()
  })

  it('marks an active hunk line with both active-line and changed-line classes', () => {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'keep\nNEW\ntail\n',
        extensions: [
          editorFeatureExtensions(DEFAULT_EDITOR_PREFS),
          unifiedMergeView({
            original: 'keep\ntail\n',
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
        ],
      }),
    })
    view.dispatch({ selection: { anchor: view.state.doc.line(2).from } })
    const line = [...parent.querySelectorAll('.cm-line')].find((el) => el.textContent === 'NEW')
    view.destroy()
    parent.remove()
    expect(line).toBeTruthy()
    expect(line!.classList.contains('cm-activeLine')).toBe(true)
    expect(line!.classList.contains('cm-changedLine')).toBe(true)
  })

  it('omits those gutters when the prefs are off', () => {
    const { view, parent } = mount({
      ...DEFAULT_EDITOR_PREFS,
      lineNumbers: false,
      foldGutter: false,
      activeLine: false,
    })
    const ln = parent.querySelectorAll('.cm-lineNumbers .cm-gutterElement').length
    const fold = parent.querySelector('.cm-foldGutter')
    const active = parent.querySelector('.cm-activeLine')
    view.destroy()
    parent.remove()
    expect(ln).toBe(0)
    expect(fold).toBeNull()
    expect(active).toBeNull()
  })
})
