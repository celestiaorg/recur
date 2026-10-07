/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import { termDockOwnsFocus } from './term-focus'

function mount(html: string, cls = 'term-dock') {
  const root = document.createElement('div')
  root.className = cls
  root.innerHTML = html
  document.body.appendChild(root)
  return root
}

afterEach(() => {
  document.body.replaceChildren()
})

describe('termDockOwnsFocus', () => {
  test('is true when focus is inside the dock', () => {
    const root = mount('<textarea></textarea>')
    const ta = root.querySelector('textarea')!
    ta.focus()
    expect(termDockOwnsFocus()).toBe(true)
    expect(termDockOwnsFocus(ta)).toBe(true)
  })

  test('is true when the dock chrome itself is focused', () => {
    const root = mount('')
    root.tabIndex = 0
    root.focus()
    expect(termDockOwnsFocus()).toBe(true)
    expect(termDockOwnsFocus(root)).toBe(true)
  })

  test('is false when a different dock has the keys', () => {
    const a = mount('<textarea></textarea>')
    const b = mount('<textarea></textarea>')
    a.querySelector('textarea')!.focus()
    expect(termDockOwnsFocus(b.querySelector('textarea'))).toBe(false)
    expect(termDockOwnsFocus(a.querySelector('textarea'))).toBe(true)
  })

  test('is false when the editor has the keys', () => {
    const editor = document.createElement('div')
    editor.className = 'cm-editor'
    editor.innerHTML = '<div class="cm-content" contenteditable="true"></div>'
    document.body.appendChild(editor)
    mount('<textarea></textarea>')
    editor.querySelector<HTMLElement>('.cm-content')!.focus()
    expect(termDockOwnsFocus()).toBe(false)
  })

  test('is false when the agent composer has the keys', () => {
    const left = document.createElement('div')
    left.className = 'obs-left'
    left.innerHTML = '<div class="chat-composer" contenteditable="true"></div>'
    document.body.appendChild(left)
    mount('<textarea></textarea>')
    left.querySelector<HTMLElement>('.chat-composer')!.focus()
    expect(termDockOwnsFocus()).toBe(false)
  })

  test('is false when focus is on body after the previous dock unmounted', () => {
    const root = mount('<textarea></textarea>')
    root.querySelector('textarea')!.focus()
    root.querySelector('textarea')!.blur()
    expect(termDockOwnsFocus()).toBe(false)
  })
})
