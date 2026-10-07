import { describe, expect, it } from 'vitest'
import { EditorSelection, EditorState } from '@codemirror/state'
import { editorFeatureExtensions } from './cm-editor-prefs'
import { DEFAULT_EDITOR_PREFS } from './stores'

function twoCursors() {
  return EditorSelection.create([
    EditorSelection.cursor(0),
    EditorSelection.cursor(3),
  ])
}

describe('editorFeatureExtensions multiple selections', () => {
  it('keeps extra carets that CM otherwise collapses', () => {
    const collapsed = EditorState.create({
      doc: 'ab\ncd',
      selection: twoCursors(),
    })
    expect(collapsed.selection.ranges).toHaveLength(1)

    const enabled = EditorState.create({
      doc: 'ab\ncd',
      selection: twoCursors(),
      extensions: [editorFeatureExtensions(DEFAULT_EDITOR_PREFS)],
    })
    expect(enabled.selection.ranges).toHaveLength(2)
  })

  it('still allows multiple carets when rectangular select is off (Mod-d)', () => {
    const state = EditorState.create({
      doc: 'ab\ncd',
      selection: twoCursors(),
      extensions: [editorFeatureExtensions({ ...DEFAULT_EDITOR_PREFS, rectangularSelect: false })],
    })
    expect(state.selection.ranges).toHaveLength(2)
  })
})
