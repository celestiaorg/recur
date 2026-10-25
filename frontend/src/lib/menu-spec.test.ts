import { describe, expect, test } from 'vitest'
import type { MenuNode } from '../../bindings/recur/internal/app'
import { accelLabel, resolveMenu, type ThemeCatalog } from './menu-spec'

const catalog: ThemeCatalog = {
  app: [{ key: 'default', name: 'Charcoal' }, { key: 'paper', name: 'Paper' }],
  editor: [
    { key: 'a', name: 'A Dark', kind: 'dark' },
    { key: 'b', name: 'B Light', kind: 'light' },
  ],
  terminal: [],
  appSelected: 'paper',
  editorSelected: 'a',
  terminalSelected: '',
}

const spec = [
  { kind: 'submenu', label: 'File', children: [
    { kind: 'item', label: 'Save', action: 'save', accel: 'CmdOrCtrl+S', enabledKey: 'save' },
    { kind: 'separator' },
    { kind: 'item', label: 'Quit', action: 'quit' },
  ] },
  { kind: 'submenu', label: 'Appearance', children: [
    { kind: 'submenu', label: 'App Theme', dynamic: 'app-theme' },
    { kind: 'submenu', label: 'Dark', dynamic: 'editor-theme-dark' },
    { kind: 'submenu', label: 'Light', dynamic: 'editor-theme-light' },
    { kind: 'submenu', label: 'Terminal', dynamic: 'terminal-theme' },
  ] },
] as MenuNode[]

describe('accelLabel', () => {
  test('maps Wails modifiers to Linux names', () => {
    expect(accelLabel('CmdOrCtrl+OptionOrAlt+O')).toBe('Ctrl+Alt+O')
    expect(accelLabel('')).toBe('')
  })
})

describe('resolveMenu', () => {
  test('items start disabled and follow the pushed state', () => {
    const off = resolveMenu(spec, {}, catalog)[0].children
    expect(off[0]).toMatchObject({ label: 'Save', disabled: true, accel: 'Ctrl+S', action: 'save' })
    expect(off[1].kind).toBe('separator')
    expect(off[2]).toMatchObject({ label: 'Quit', disabled: false, action: 'quit' })
    expect(resolveMenu(spec, { save: true }, catalog)[0].children[0].disabled).toBe(false)
    expect(resolveMenu(spec, { save: false }, catalog)[0].children[0].disabled).toBe(true)
  })

  test('dynamic submenus come from the catalogs with the selection checked', () => {
    const [app, dark, light, term] = resolveMenu(spec, {}, catalog)[1].children
    expect(app.children.map((c) => [c.label, c.checked, c.action])).toEqual([
      ['Charcoal', false, 'app-theme:default'],
      ['Paper', true, 'app-theme:paper'],
    ])
    expect(dark.children.map((c) => c.action)).toEqual(['editor-theme:a'])
    expect(dark.children[0].checked).toBe(true)
    expect(light.children.map((c) => c.action)).toEqual(['editor-theme:b'])
    expect(term.children).toMatchObject([{ label: 'Loading…', disabled: true }])
  })
})
