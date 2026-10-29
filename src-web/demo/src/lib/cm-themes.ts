// Editor colour themes — all from bundle TextMate themes (Recur + Themes packs).
import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import {
  getParsedTheme,
  getThemeMeta,
  listThemeMeta,
  refreshTmColorMapStyle,
  setActiveEditorBackground,
  setActiveRootBackgrounds,
  setActiveTheme,
  isThemeDisabled,
  disabledThemeIdSet,
} from './tm-registry'
import { tmChromeExtension } from './tm-theme'

const DEFAULT_THEME_ID = 'recur'

/** Recur first-party themes pinned at the top of the Settings list. */
export const PINNED_EDITOR_THEME_KEYS = ['recur', 'paper'] as const

export type EditorThemeCard = {
  key: string
  name: string
  kind: 'dark' | 'light'
  bg: string
  fg: string
  a: string
  b: string
  c: string
  source?: 'bundle'
  pinned?: boolean
}

/** Layout chrome shared by every color theme (size/font/height — not colors). */
export function editorChromeExtension(
  fontSize = 12,
  fontFamily = "'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, monospace",
  lineHeight = 1.55,
  thickCaret = false,
): Extension {
  // 2px matches the native caret (diff view); 3px is Settings → Thick caret.
  const caretW = thickCaret ? '3px' : '2px'
  const caretNudge = thickCaret ? '-1.5px' : '-1px'
  // Note: &dark/&light only work in EditorView.baseTheme(); using them in
  // EditorView.theme() throws and blanks the editor.
  return EditorView.theme({
    '&': { height: '100%', maxHeight: '100%', minHeight: '0', fontSize: `${fontSize}px` },
    '.cm-scroller': {
      fontFamily,
      lineHeight: String(lineHeight),
      overflow: 'auto',
      minHeight: '0',
      overscrollBehavior: 'contain',
    },
    '.cm-content': { padding: '8px 0' },
    '.cm-cursor, .cm-dropCursor': { borderLeftWidth: caretW, marginLeft: caretNudge },
    '&.cm-focused': { outline: 'none' },
  })
}

/**
 * App chrome key → bundle editor theme. Settings script editors use this
 * instead of the user's selected editor theme.
 */
export const APP_CHROME_EDITOR_THEME: Record<string, string> = {
  default: 'recur',
  paper: 'paper',
  slate: 'github-dark-default',
  coolglow: 'cool-glow',
  'catppuccin-mocha': 'catppuccin-mocha',
  'catppuccin-latte': 'catppuccin-latte',
  nord: 'nord',
}

/**
 * Editor theme that matches `appThemeKey` (stores `theme`). If the preferred
 * bundle theme is missing or disabled, fall back to Paper / Charcoal by chrome.
 */
export function editorThemeIdForAppChrome(appThemeKey: string): string {
  const preferred = APP_CHROME_EDITOR_THEME[appThemeKey] ?? 'recur'
  if (usableEditorTheme(preferred)) return preferred
  const fallback = appThemeKey === 'paper' || appThemeKey === 'catppuccin-latte' ? 'paper' : 'recur'
  if (fallback !== preferred && usableEditorTheme(fallback)) return fallback
  return preferred
}

function usableEditorTheme(id: string): boolean {
  return !!getParsedTheme(id) && !isThemeDisabled(id)
}

/**
 * Editor chrome (background, gutter, caret, selection) for a bundle theme.
 * Does not publish the global TextMate colour map — safe for surfaces that
 * must not restyle the code editor.
 */
export function editorChromeOnlyExtension(themeId: string): Extension {
  const parsed = getParsedTheme(themeId)
  if (!parsed) return EditorView.darkTheme.of(getThemeMeta(themeId)?.uiTheme !== 'light')
  return tmChromeExtension(parsed)
}

/** Color theme only — pair with editorChromeExtension for layout prefs. */
export function editorThemeExtension(key: string): Extension {
  const id = resolveEnabledThemeId(key)
  const parsed = getParsedTheme(id)
  if (!parsed) {
    const meta = getThemeMeta(id)
    return EditorView.darkTheme.of(meta?.uiTheme !== 'light')
  }
  setActiveTheme(parsed.rawTheme)
  setActiveEditorBackground(parsed.colors['editor.background'] ?? '#1e1e1e')
  setActiveRootBackgrounds(parsed.rootBackgrounds)
  refreshTmColorMapStyle()
  return editorChromeOnlyExtension(id)
}

export function isEditorThemeKey(key: string): boolean {
  if (!key) return false
  if (getThemeMeta(key)) return true
  // Accept shipped preference ids before bootstrap finishes.
  return /^[a-z0-9][a-z0-9._-]*$/i.test(key)
}

export function resolveEditorThemeKey(key: string): string {
  if (!key) return DEFAULT_THEME_ID
  return isEditorThemeKey(key) ? key : DEFAULT_THEME_ID
}

/**
 * First still-enabled editor theme. Prefers pinned Recur themes, then catalog
 * order. Falls back to `current` / Charcoal when every theme is disabled so
 * the editor always has a palette.
 */
export function fallbackEnabledEditorThemeId(disabled: Set<string>, current?: string): string {
  if (current && !disabled.has(current) && isEditorThemeKey(current)) return current
  const enabled = listThemeMeta().filter((t) => t.id && !disabled.has(t.id))
  for (const id of PINNED_EDITOR_THEME_KEYS) {
    if (enabled.some((t) => t.id === id)) return id
  }
  if (enabled[0]) return enabled[0].id
  return current || DEFAULT_THEME_ID
}

function resolveEnabledThemeId(key: string): string {
  const id = getThemeMeta(key) ? key : DEFAULT_THEME_ID
  if (!isThemeDisabled(id)) return id
  return fallbackEnabledEditorThemeId(disabledThemeIdSet(), id)
}

/** dark | light from the active / catalogued theme (independent of app chrome). */
export function editorThemeKind(key: string): 'dark' | 'light' {
  const id = resolveEditorThemeKey(key)
  const meta = getThemeMeta(id)
  if (meta?.uiTheme === 'light') return 'light'
  if (meta?.uiTheme === 'dark') return 'dark'
  const parsed = getParsedTheme(id)
  return parsed?.uiTheme === 'light' ? 'light' : 'dark'
}

/** Bundle TextMate themes for the Settings picker / menus. */
export function allEditorThemesForPicker(): EditorThemeCard[] {
  const pinnedRank = new Map<string, number>(
    PINNED_EDITOR_THEME_KEYS.map((k, i) => [k, i]),
  )
  const themes = [...listThemeMeta()]
    .filter((t) => !isThemeDisabled(t.id))
    .sort((a, b) => {
    const pa = pinnedRank.get(a.id)
    const pb = pinnedRank.get(b.id)
    if (pa !== undefined || pb !== undefined) {
      if (pa === undefined) return 1
      if (pb === undefined) return -1
      return pa - pb
    }
    return a.name.localeCompare(b.name)
  })
  return themes.map((t) => {
    const parsed = getParsedTheme(t.id)
    const colors = parsed?.colors ?? {}
    const p = t.preview
    const bg = p?.bg || colors['editor.background'] || '#1e1e1e'
    const fg = p?.fg || colors['editor.foreground'] || '#cccccc'
    const a = p?.name || colors['editorCursor.foreground'] || fg
    const b = p?.fn || a
    const c = p?.string || a
    const key = t.id
    return {
      key,
      name: t.name,
      kind: (t.uiTheme === 'light' ? 'light' : 'dark') as 'dark' | 'light',
      bg,
      fg,
      a,
      b,
      c,
      source: 'bundle' as const,
      pinned: pinnedRank.has(key),
    }
  })
}

/** Compact Settings list: pinned Recur themes first, then alphabetical matches. */
export function filterEditorThemesForList(
  cards: EditorThemeCard[],
  query: string,
  kind: 'all' | 'dark' | 'light',
): EditorThemeCard[] {
  const q = query.trim().toLowerCase()
  const match = (t: EditorThemeCard) => {
    if (kind !== 'all' && t.kind !== kind) return false
    if (!q) return true
    return t.name.toLowerCase().includes(q) || t.key.toLowerCase().includes(q)
  }
  const pinned = PINNED_EDITOR_THEME_KEYS
    .map((k) => cards.find((t) => t.key === k))
    .filter((t): t is EditorThemeCard => !!t && match(t))
  const rest = cards
    .filter((t) => !t.pinned && match(t))
    .sort((a, b) => a.name.localeCompare(b.name))
  return [...pinned, ...rest]
}

export type EditorThemeCycleItem = { key: string; name: string; kind: 'dark' | 'light' }

/** Dark A–Z (Charcoal first), then light A–Z (Paper first). Same as the Appearance menu. */
export function compareEditorThemesForCycle(a: EditorThemeCycleItem, b: EditorThemeCycleItem): number {
  if (a.kind !== b.kind) return a.kind === 'dark' ? -1 : 1
  const pinned = PINNED_EDITOR_THEME_KEYS as readonly string[]
  const pa = pinned.indexOf(a.key)
  const pb = pinned.indexOf(b.key)
  if (pa !== -1 || pb !== -1) {
    if (pa === -1) return 1
    if (pb === -1) return -1
    return pa - pb
  }
  return a.name.localeCompare(b.name)
}

export function editorThemesForCycle(): EditorThemeCard[] {
  return [...allEditorThemesForPicker()].sort(compareEditorThemesForCycle)
}

/**
 * Next/previous editor theme in cycle order. Wraps. Unknown current keys start
 * at the first theme (Charcoal when the catalog is loaded).
 */
export function cycleEditorThemeKey(current: string, delta: number, keys: readonly string[]): string {
  if (keys.length === 0) return resolveEditorThemeKey(current)
  const cur = resolveEditorThemeKey(current)
  let i = keys.indexOf(cur)
  if (i < 0) i = 0
  const n = keys.length
  return keys[((i + (delta | 0)) % n + n) % n]
}
