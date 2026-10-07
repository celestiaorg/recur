/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { recurGotoLine } from './cm-goto-line'
import { recurSearchExtension } from './cm-search-panel'

function mount(doc = 'a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm') {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      extensions: [recurSearchExtension()],
    }),
  })
  return { view, parent }
}

describe('recur go-to-line', () => {
  it('opens a Recur dialog instead of stock cm-textfield chrome', () => {
    const { view, parent } = mount()
    recurGotoLine(view)
    const dialog = parent.querySelector('.cm-recur-goto')
    expect(dialog).toBeTruthy()
    expect(dialog!.querySelector('.cm-recur-goto-form')).toBeTruthy()
    expect(dialog!.querySelector('input[name=line]')).toBeTruthy()
    expect(dialog!.querySelector('.cm-button')).toBeNull()
    expect(dialog!.querySelector('.cm-textfield')).toBeNull()
    view.destroy()
    parent.remove()
  })

  it('jumps to the typed line on submit', async () => {
    const { view, parent } = mount()
    recurGotoLine(view)
    const input = parent.querySelector('input[name=line]') as HTMLInputElement
    const form = parent.querySelector('.cm-recur-goto-form') as HTMLFormElement
    input.value = '12'
    form.requestSubmit()
    // showDialog resolves on microtask after submit
    await Promise.resolve()
    await new Promise((r) => setTimeout(r, 20))
    const line = view.state.doc.lineAt(view.state.selection.main.head)
    expect(line.number).toBe(12)
    view.destroy()
    parent.remove()
  })
})
