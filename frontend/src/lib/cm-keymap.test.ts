/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { addCursorAbove, addCursorBelow } from '@codemirror/commands'
import { EditorState } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { recurDefaultKeymap } from './cm-keymap'

function mount(doc = 'aa\nbb\ncc') {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [
        EditorState.allowMultipleSelections.of(true),
        keymap.of(recurDefaultKeymap),
      ],
    }),
  })
  return { view, parent }
}

function fire(view: EditorView, init: KeyboardEventInit) {
  view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
    bubbles: true,
    cancelable: true,
    ...init,
  }))
}

describe('recurDefaultKeymap', () => {
  it('does not bind Mod-Alt-Arrow (workspace cycle owns that)', () => {
    expect(recurDefaultKeymap.some((b) => b.key === 'Mod-Alt-ArrowUp')).toBe(false)
    expect(recurDefaultKeymap.some((b) => b.key === 'Mod-Alt-ArrowDown')).toBe(false)
  })

  it('binds add-cursor to Ctrl-Shift-Arrow', () => {
    expect(recurDefaultKeymap.find((b) => b.key === 'Ctrl-Shift-ArrowUp')?.run).toBe(addCursorAbove)
    expect(recurDefaultKeymap.find((b) => b.key === 'Ctrl-Shift-ArrowDown')?.run).toBe(addCursorBelow)
  })

  it('adds a caret below on Ctrl-Shift-ArrowDown', () => {
    const { view, parent } = mount()
    fire(view, { key: 'ArrowDown', code: 'ArrowDown', ctrlKey: true, shiftKey: true })
    expect(view.state.selection.ranges).toHaveLength(2)
    view.destroy()
    parent.remove()
  })

  it('leaves a single caret on Mod-Alt-ArrowDown', () => {
    const { view, parent } = mount()
    fire(view, { key: 'ArrowDown', code: 'ArrowDown', metaKey: true, altKey: true })
    expect(view.state.selection.ranges).toHaveLength(1)
    view.destroy()
    parent.remove()
  })
})
