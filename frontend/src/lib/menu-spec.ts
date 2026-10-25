// Renders the Go menu tree (internal/app/menuspec.go) for the in-app menu. The
// native menubar is built from the same tree, so the two cannot drift.
import type { MenuNode } from '../../bindings/recur/internal/app'
import type { MenuState } from './menu'

export type ThemeEntry = { key: string; name: string; kind?: string }

/** Catalogs behind the dynamic Appearance submenus plus their current selection. */
export type ThemeCatalog = {
  app: ThemeEntry[]
  editor: ThemeEntry[]
  terminal: ThemeEntry[]
  appSelected: string
  editorSelected: string
  terminalSelected: string
}

export type MenuEntry = {
  kind: 'item' | 'separator' | 'submenu'
  label: string
  accel: string
  disabled: boolean
  /** Radio dot (theme pickers). */
  checked: boolean
  action: string
  role: string
  children: MenuEntry[]
}

const entry = (over: Partial<MenuEntry>): MenuEntry => ({
  kind: 'item', label: '', accel: '', disabled: false, checked: false, action: '', role: '', children: [], ...over,
})

/** "CmdOrCtrl+OptionOrAlt+O" as shown on Linux: "Ctrl+Alt+O". */
export function accelLabel(chord: string): string {
  return chord.replace(/CmdOrCtrl/g, 'Ctrl').replace(/OptionOrAlt/g, 'Alt')
}

function radios(list: ThemeEntry[], selected: string, prefix: string): MenuEntry[] {
  if (list.length === 0) return [entry({ label: 'Loading…', disabled: true })]
  return list.map((t) => entry({ label: t.name, checked: t.key === selected, action: `${prefix}:${t.key}` }))
}

function dynamicChildren(kind: string, c: ThemeCatalog): MenuEntry[] {
  switch (kind) {
    case 'app-theme':
      return radios(c.app, c.appSelected, 'app-theme')
    case 'editor-theme-dark':
      return radios(c.editor.filter((t) => t.kind !== 'light'), c.editorSelected, 'editor-theme')
    case 'editor-theme-light':
      return radios(c.editor.filter((t) => t.kind === 'light'), c.editorSelected, 'editor-theme')
    default:
      return radios(c.terminal, c.terminalSelected, 'terminal-theme')
  }
}

/** Resolve spec nodes against the live enable map and theme catalogs. */
export function resolveMenu(nodes: MenuNode[], state: MenuState, catalog: ThemeCatalog): MenuEntry[] {
  return nodes.map((n) => {
    if (n.kind === 'separator') return entry({ kind: 'separator' })
    if (n.kind === 'submenu') {
      const children = n.dynamic ? dynamicChildren(n.dynamic, catalog) : resolveMenu(n.children ?? [], state, catalog)
      return entry({ kind: 'submenu', label: n.label ?? '', children })
    }
    return entry({
      label: n.label ?? '',
      accel: accelLabel(n.accel ?? ''),
      disabled: !!n.enabledKey && state[n.enabledKey as keyof MenuState] !== true,
      action: n.action ?? '',
      role: n.role ?? '',
    })
  })
}
