/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import { composerOwnsDialogTab, cycleDialogTab, dialogFocusables } from './dialog-focus'

function mount(html: string): HTMLElement {
  const root = document.createElement('div')
  root.innerHTML = html
  document.body.appendChild(root)
  return root
}

afterEach(() => {
  document.body.replaceChildren()
})

describe('dialogFocusables', () => {
  test('returns enabled buttons, inputs, and textareas in DOM order', () => {
    const root = mount(`
      <button type="button">Repo</button>
      <button type="button" disabled>Nope</button>
      <textarea></textarea>
      <button type="button">Create</button>
    `)
    expect(dialogFocusables(root).map((el) => el.textContent || el.tagName)).toEqual([
      'Repo',
      'TEXTAREA',
      'Create',
    ])
  })

  test('skips slash and mention menus', () => {
    const root = mount(`
      <button type="button">Trigger</button>
      <div class="chat-mention">
        <button type="button">Skill</button>
      </div>
      <button type="button">Create</button>
    `)
    expect(dialogFocusables(root).map((el) => el.textContent)).toEqual(['Trigger', 'Create'])
  })

  test('skips tabindex=-1 and nested select menus', () => {
    const root = mount(`
      <button type="button">Trigger</button>
      <div class="sel-menu">
        <input type="text" />
        <button type="button">Option</button>
      </div>
      <button type="button" tabindex="-1">Close</button>
      <button type="button">Next</button>
    `)
    expect(dialogFocusables(root).map((el) => el.textContent)).toEqual(['Trigger', 'Next'])
  })
})

describe('cycleDialogTab', () => {
  test('Tab wraps from the last control back to the first', () => {
    const root = mount(`
      <button type="button" id="a">A</button>
      <button type="button" id="b">B</button>
    `)
    root.querySelector<HTMLElement>('#b')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('a')
  })

  test('Shift+Tab wraps from the first control to the last', () => {
    const root = mount(`
      <button type="button" id="a">A</button>
      <button type="button" id="b">B</button>
    `)
    root.querySelector<HTMLElement>('#a')!.focus()
    cycleDialogTab(root, true)
    expect(document.activeElement?.id).toBe('b')
  })

  test('Tab from a nested menu control moves from its trigger', () => {
    const root = mount(`
      <div class="sel">
        <button type="button" class="sel-btn" id="trig">Repo</button>
        <div class="sel-menu"><input id="search" type="text" /></div>
      </div>
      <button type="button" class="sel-btn" id="next">Next</button>
    `)
    root.querySelector<HTMLElement>('#search')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('next')
    expect(document.activeElement?.classList.contains('kb-focus')).toBe(true)
  })

  test('Tab from a portaled icon menu resumes at its trigger', () => {
    const root = mount(`
      <div class="ico-pick sel" data-sel-id="ico-a">
        <button type="button" class="sel-btn" id="trig">Icon</button>
      </div>
      <button type="button" class="sel-btn" id="next">Next</button>
    `)
    const menu = document.createElement('div')
    menu.className = 'ico-menu'
    menu.setAttribute('data-sel-id', 'ico-a')
    menu.innerHTML = '<input id="search" type="text" />'
    document.body.appendChild(menu)
    menu.querySelector<HTMLElement>('#search')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('next')
  })

  test('Tab from a portaled menu resumes at its trigger', () => {
    const root = mount(`
      <div class="sel" data-sel-id="sel-a">
        <button type="button" class="sel-btn" id="trig">Repo</button>
      </div>
      <button type="button" class="sel-btn" id="next">Next</button>
    `)
    const menu = document.createElement('div')
    menu.className = 'sel-menu'
    menu.setAttribute('data-sel-id', 'sel-a')
    menu.innerHTML = '<input id="search" type="text" />'
    document.body.appendChild(menu)
    menu.querySelector<HTMLElement>('#search')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('next')
  })

  test('does not put a tab ring on the prompt textarea', () => {
    const root = mount(`
      <button type="button" class="sel-btn" id="a">A</button>
      <textarea class="nt-in" id="prompt"></textarea>
    `)
    root.querySelector<HTMLElement>('#a')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('prompt')
    expect(document.activeElement?.classList.contains('kb-focus')).toBe(false)
  })

  test('does not put a tab ring on the contenteditable prompt', () => {
    const root = mount(`
      <button type="button" class="sel-btn" id="a">A</button>
      <div class="nt-in chat-composer" id="prompt" tabindex="0" contenteditable="true"></div>
    `)
    root.querySelector<HTMLElement>('#a')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('prompt')
    expect(document.activeElement?.classList.contains('kb-focus')).toBe(false)
  })

  test('puts a tab ring on the Simplify chip', () => {
    const root = mount(`
      <button type="button" class="sel-btn" id="a">A</button>
      <button type="button" class="wsv-simplify" id="s">Simplify</button>
    `)
    root.querySelector<HTMLElement>('#a')!.focus()
    cycleDialogTab(root, false)
    expect(document.activeElement?.id).toBe('s')
    expect(document.activeElement?.classList.contains('kb-focus')).toBe(true)
  })
})

describe('composerOwnsDialogTab', () => {
  test('is true while the slash menu is open', () => {
    const root = mount(`
      <div class="nt-in chat-composer" id="prompt" tabindex="0" contenteditable="true"></div>
      <div class="chat-mention"><button type="button">/review</button></div>
    `)
    expect(composerOwnsDialogTab(root, root.querySelector('#prompt'))).toBe(true)
  })

  test('is true on a prompt argument input even without a menu', () => {
    const root = mount(`
      <input class="chat-composer-prompt-arg-input" id="arg" />
    `)
    expect(composerOwnsDialogTab(root, root.querySelector('#arg'))).toBe(true)
  })

  test('is false for the empty composer', () => {
    const root = mount(`
      <div class="nt-in chat-composer" id="prompt" tabindex="0" contenteditable="true"></div>
      <button type="button" class="nt-go">Create</button>
    `)
    expect(composerOwnsDialogTab(root, root.querySelector('#prompt'))).toBe(false)
  })
})
