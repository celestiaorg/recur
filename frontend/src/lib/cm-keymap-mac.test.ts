/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest'

/**
 * CodeMirror snapshots `navigator.platform` at `@codemirror/view` load
 * (`browser.mac`). happy-dom reports "X11; Darwin arm64", so the sibling
 * cm-keymap.test.ts never registers the mac Ctrl-Arrow page-select bindings
 * that share a slot with ⌃⇧↑/↓. Stub MacIntel *then* import.
 */
describe('recurDefaultKeymap on macOS', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  it('adds a caret on Ctrl-Shift-ArrowDown instead of page-selecting', async () => {
    vi.resetModules()
    vi.stubGlobal('navigator', { platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' })
    const { EditorState } = await import('@codemirror/state')
    const { EditorView, keymap } = await import('@codemirror/view')
    const { recurDefaultKeymap } = await import('./cm-keymap')

    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'aa\nbb\ncc',
        extensions: [
          EditorState.allowMultipleSelections.of(true),
          keymap.of(recurDefaultKeymap),
        ],
      }),
    })
    view.contentDOM.dispatchEvent(new KeyboardEvent('keydown', {
      bubbles: true,
      cancelable: true,
      key: 'ArrowDown',
      code: 'ArrowDown',
      ctrlKey: true,
      shiftKey: true,
    }))
    expect(view.state.selection.ranges).toHaveLength(2)
    view.destroy()
    parent.remove()
  })
})
