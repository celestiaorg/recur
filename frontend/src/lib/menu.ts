// Frontend dispatch for native menubar actions (menu.action events from Go).
// App-level chords land here; Observatory subscribes for workspace-scoped ones.
// Enabled state is pushed back to Go via menu.state so contextual items grey out.
import { Events } from '@wailsio/runtime'
import { get } from 'svelte/store'
import { themes, theme, editorTheme, terminalPrefs } from './stores'
import { allEditorThemesForPicker, compareEditorThemesForCycle } from './cm-themes'
import { terminalThemeCatalog } from './terminal'

export type MenuAction =
  | 'new-loop'
  | 'new-blank-workspace'
  | 'new-tab'
  | 'save'
  | 'open-path'
  | 'open-in'
  | 'copy-path'
  | 'close-tab'
  | 'toggle-sidebar'
  | 'toggle-left-panel'
  | 'toggle-right-panel'
  | 'toggle-editor'
  | 'toggle-terminal'
  | 'maximize-terminal'
  | 'focus-agent'
  | 'focus-editor'
  | 'focus-terminal'
  | 'prev-tab'
  | 'next-tab'
  | 'prev-workspace'
  | 'next-workspace'
  | 'palette'
  | 'run-command'
  | 'shortcuts'
  | 'quick-open'
  | 'go-symbol'
  | 'settings'
  | 'reload-bundles'
  | 'run-dev'
  | 'stop-run'
  | `app-theme:${string}`
  | `editor-theme:${string}`
  | `terminal-theme:${string}`
  | 'prev-editor-theme'
  | 'next-editor-theme'

/** Partial enable map — omit a key to leave that item unchanged. */
export type MenuState = {
  newTab?: boolean
  save?: boolean
  openIn?: boolean
  copyPath?: boolean
  closeTab?: boolean
  toggleLeftPanel?: boolean
  toggleRightPanel?: boolean
  toggleEditor?: boolean
  toggleTerminal?: boolean
  maximizeTerminal?: boolean
  focusAgent?: boolean
  focusEditor?: boolean
  focusTerminal?: boolean
  prevTab?: boolean
  nextTab?: boolean
  runDev?: boolean
  stopRun?: boolean
}

/** All observatory-scoped items off — used when leaving a workspace view. */
export const MENU_STATE_IDLE: MenuState = {
  newTab: false,
  save: false,
  closeTab: false,
  toggleLeftPanel: false,
  toggleRightPanel: false,
  toggleEditor: false,
  toggleTerminal: false,
  maximizeTerminal: false,
  focusAgent: false,
  focusEditor: false,
  focusTerminal: false,
  prevTab: false,
  nextTab: false,
  runDev: false,
  stopRun: false,
}

type Handler = (action: MenuAction) => void

const handlers = new Set<Handler>()

/** Subscribe to menubar / accelerator actions. Returns an unsubscribe. */
export function onMenuAction(handler: Handler): () => void {
  handlers.add(handler)
  return () => { handlers.delete(handler) }
}

function dispatch(action: string) {
  for (const h of handlers) h(action as MenuAction)
}

/** Wire once from App boot — fans Go menu.action events out to subscribers. */
export function wireMenuEvents() {
  Events.On('menu.action', (e: any) => {
    const action = typeof e?.data === 'string' ? e.data : e?.data?.action
    if (typeof action === 'string' && action) dispatch(action)
  })
}

/** Push enable/disable state to the native menubar (no-op outside Wails). */
export function setMenuState(state: MenuState) {
  void Events.Emit('menu.state', state).catch(() => {})
}

const MENU_TERMINAL_MATCH = 'match-app'

function terminalMenuKey(id: string): string {
  return id || MENU_TERMINAL_MATCH
}

export function terminalIdFromMenu(key: string): string {
  return key === MENU_TERMINAL_MATCH ? '' : key
}

/** Rebuild View → Appearance theme radio menus from the current catalogs. */
export function pushMenuThemes() {
  const editor = allEditorThemesForPicker()
    .map((t) => ({ key: t.key, name: t.name, kind: t.kind }))
    .sort(compareEditorThemesForCycle)
  const terminal = [
    { key: MENU_TERMINAL_MATCH, name: 'Match App' },
    ...get(terminalThemeCatalog).map((t) => ({ key: t.id, name: t.name })),
  ]
  void Events.Emit('menu.themes', {
    app: themes.map((t) => ({ key: t.key, name: t.name })),
    editor,
    terminal,
    appSelected: get(theme),
    editorSelected: get(editorTheme),
    terminalSelected: terminalMenuKey(get(terminalPrefs).theme),
  }).catch(() => {})
}

/** Update Appearance radio checks without rebuilding the catalog. */
export function pushMenuThemeState() {
  void Events.Emit('menu.themeState', {
    appSelected: get(theme),
    editorSelected: get(editorTheme),
    terminalSelected: terminalMenuKey(get(terminalPrefs).theme),
  }).catch(() => {})
}
