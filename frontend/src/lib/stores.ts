// Reactive app state. Data stores are populated from the bound Go services
// (lib/api.ts) and updated live by events.
import { writable, derived, get } from 'svelte/store'
import { Events, System } from '@wailsio/runtime'
import type {
  Project, ProjectGroup, Workspace, PR, InboxItem, Policy, Settings, Resource, PermissionRequest, UserQuestion,
  ScheduledPrompt,
} from '../../bindings/recur/internal/domain/models'
import { fallbackEnabledEditorThemeId, resolveEditorThemeKey, editorThemesForCycle, cycleEditorThemeKey } from './cm-themes'
import { disabledThemeIdSet, setDisabledBundleContribs } from './tm-registry'
import { LANGUAGE_INDENT_DEFAULTS } from './cm-lang'
import { LEGACY_FONT_FACE_KEYS, monoFontStack } from './mono-fonts'
import { wsMainBlocked, type ChatMode } from './modes'
import { neighborWorkspaceId, sidebarWorkspaceIds, SCAN_COMMIT_MS, projectIdFromMainScan } from './workspace-nav'
import { persistShellState, snapshotShell, type ShellView } from './shell-state'
import { prAgentWorkspaceId, prIdForChatWorkspace } from './pr-workspace'
import type { Snapshot as ProviderUsageSnapshot } from '../../bindings/recur/internal/usage/models'
import { parseChatRouteTarget, parseWorkspaceRouteTarget } from './chat-route'
import { SetTerminalDockChrome } from '../../bindings/recur/internal/app/terminalservice'
import { chatOverlayOpen } from './chat-occlusion'
import { keybindingMenu } from './keybinding-menu'
import { type PRListLife, type PRListSort, PR_LIST_SEGS, PR_LIST_SORT_DEFAULT, PR_LIST_SORTS } from './pr-list-filter'

export type ViewName =
  | 'detail' | 'settings' | 'project'
  | 'pr' | 'issues' | 'schedules' | 'prdetail' | 'inbox' | 'briefing' | 'welcome' | 'escalation'

// ---- appearance / theme ----
// Themes are color-only (identity & layout stay constant). Each entry carries the
// preview swatches shown in the Settings picker; 'default' means "no data-theme attr".
export type Theme = {
  key: string; name: string; d: string
  bg: string; pane: string; text: string; a: string; ok: string; warn: string
}
export const themes: Theme[] = [
  { key: 'default', name: 'Charcoal', d: 'default', bg: '#141414', pane: '#1c1c1c', text: '#e6e4e1', a: '#e07a5f', ok: '#4ade80', warn: '#f87171' },
  { key: 'paper', name: 'Paper', d: 'light', bg: '#ebecef', pane: '#f7f8fa', text: '#1a1c20', a: '#d4634a', ok: '#2d9a5a', warn: '#d14b4b' },
  { key: 'slate', name: 'Slate', d: 'cool', bg: '#0A0C10', pane: '#161A21', text: '#DBE2EA', a: '#F2B33D', ok: '#5DBE7E', warn: '#EB7A5C' },
  { key: 'coolglow', name: 'Cool Glow', d: 'navy', bg: '#0a0c22', pane: '#0e1130', text: '#d8dce8', a: '#45c4b8', ok: '#6ecf8a', warn: '#e87890' },
  { key: 'catppuccin-mocha', name: 'Catppuccin Mocha', d: 'mocha', bg: '#0e0e16', pane: '#181825', text: '#e0e6f8', a: '#cba6f7', ok: '#a6e3a1', warn: '#f38ba8' },
  { key: 'catppuccin-latte', name: 'Catppuccin Latte', d: 'latte', bg: '#e6e9ef', pane: '#eff1f5', text: '#4c4f69', a: '#8839ef', ok: '#40a02b', warn: '#d20f39' },
  { key: 'nord', name: 'Nord', d: 'frost', bg: '#181c24', pane: '#222730', text: '#eceff4', a: '#88c0d0', ok: '#a3be8c', warn: '#bf616a' },
]
const THEME_KEY = 'recur.theme'
// Legacy keys map to Charcoal / Cool Glow (Gruvbox / Charcoal named key → default Charcoal;
// Tokyo Night app chrome → Cool Glow). Unknown keys (e.g. removed everforest) fall through to Charcoal.
const THEME_ALIASES: Record<string, string> = {
  gruvbox: 'default', conductor: 'default', charcoal: 'default', tokyonight: 'coolglow', catppuccin: 'catppuccin-mocha',
}
export const theme = writable<string>('default')

/** True for light app-chrome palettes (affects mermaid, file-icons, terminal selection). */
export function isLightTheme(key: string): boolean {
  return key === 'paper' || key === 'catppuccin-latte'
}

// ---- "Sync with system": light/dark picks for app, editor and terminal themes ----
// The webview's prefers-color-scheme can miss the desktop setting (WebKitGTK in a
// container), so the native answer from Wails wins whenever it is available. The
// media query is only a fallback in a plain browser: on Linux without
// xdg-desktop-portal, Wails reports light rather than an error. The last native
// answer is persisted so startup paints the right pick before the bridge replies.
const SYSTEM_DARK_KEY = 'recur.systemDark'
export const systemDark = writable(false)
let nativeDark: boolean | null = null

function setNativeDark(dark: boolean) {
  nativeDark = dark
  updateSystemDark()
  try { localStorage.setItem(SYSTEM_DARK_KEY, dark ? '1' : '0') } catch { /* ignore */ }
}

function updateSystemDark() {
  const media = typeof window.matchMedia === 'function' && window.matchMedia('(prefers-color-scheme: dark)').matches
  systemDark.set(nativeDark ?? media)
}

async function refreshNativeDark() {
  try {
    const dark = await System.IsDarkMode()
    if (typeof dark === 'boolean') setNativeDark(dark)
  } catch { /* no native bridge (browser) */ }
}

function watchSystemDark() {
  try {
    const saved = localStorage.getItem(SYSTEM_DARK_KEY)
    if (saved) nativeDark = saved === '1'
  } catch { /* ignore */ }
  updateSystemDark()
  window.matchMedia?.('(prefers-color-scheme: dark)').addEventListener('change', updateSystemDark)
  void refreshNativeDark()
  Events.On('system.theme', (e: any) => { if (typeof e.data === 'boolean') setNativeDark(e.data) })
  window.addEventListener('focus', () => { void refreshNativeDark() })
}

// A sync holds the mode plus the light and dark pick for one kind of theme (persisted
// as recur.<name>Auto / Light / Dark) and re-applies the matching pick on every change.
function createSystemSync(name: string, defaults: { light: string; dark: string }, apply: (key: string) => void) {
  const auto = writable(false)
  const light = writable(defaults.light)
  const dark = writable(defaults.dark)
  const key = (suffix: string) => `recur.${name}${suffix}`
  const save = (suffix: string, value: string) => {
    try { localStorage.setItem(key(suffix), value) } catch { /* ignore */ }
  }
  const run = () => { if (get(auto)) apply(get(get(systemDark) ? dark : light)) }
  systemDark.subscribe(run)
  return {
    auto, light, dark,
    setAuto(on: boolean) { auto.set(on); save('Auto', on ? '1' : '0'); run() },
    setPick(mode: 'light' | 'dark', value: string) {
      (mode === 'dark' ? dark : light).set(value)
      save(mode === 'dark' ? 'Dark' : 'Light', value)
      run()
    },
    load() {
      try {
        light.set(localStorage.getItem(key('Light')) ?? defaults.light)
        dark.set(localStorage.getItem(key('Dark')) ?? defaults.dark)
        auto.set(localStorage.getItem(key('Auto')) === '1')
      } catch { /* ignore */ }
      run()
    },
  }
}

const appSync = createSystemSync('theme', { light: 'paper', dark: 'default' }, applyTheme)
export const themeAuto = appSync.auto
export const themeLight = appSync.light
export const themeDark = appSync.dark
export const setThemeAuto = appSync.setAuto
export const setSystemTheme = appSync.setPick

function applyTheme(key: string) {
  const resolved = THEME_ALIASES[key] ?? key
  const valid = themes.some((t) => t.key === resolved) ? resolved : 'default'
  theme.set(valid)
  if (valid === 'default') document.documentElement.removeAttribute('data-theme')
  else document.documentElement.setAttribute('data-theme', valid)
  try { localStorage.setItem(THEME_KEY, valid) } catch { /* ignore */ }
}

// setTheme applies a theme to the document root and persists the choice. 'default'
// removes the attribute so the base :root palette applies. An explicit pick turns
// system-follow off.
export function setTheme(key: string) {
  applyTheme(key)
  setThemeAuto(false)
}

// initTheme reads the saved theme and applies it. Called before mount to avoid a
// flash of the default palette on startup.
export function initTheme() {
  let saved = 'default'
  try { saved = localStorage.getItem(THEME_KEY) ?? 'default' } catch { /* ignore */ }
  applyTheme(saved)
  watchSystemDark()
  appSync.load()
}

// ---- code editor theme (independent of app chrome theme) ----
const EDITOR_THEME_KEY = 'recur.editorTheme'
export const editorTheme = writable<string>('recur')

export function applyEditorTheme(key: string) {
  const valid = fallbackEnabledEditorThemeId(disabledThemeIdSet(), resolveEditorThemeKey(key))
  editorTheme.set(valid)
  try { localStorage.setItem(EDITOR_THEME_KEY, valid) } catch { /* ignore */ }
}

const editorSync = createSystemSync('editorTheme', { light: 'paper', dark: 'recur' }, applyEditorTheme)
export const editorThemeAuto = editorSync.auto
export const editorThemeLight = editorSync.light
export const editorThemeDark = editorSync.dark
export const setEditorThemeAuto = editorSync.setAuto
export const setSystemEditorTheme = editorSync.setPick

export function setEditorTheme(key: string) {
  applyEditorTheme(key)
  setEditorThemeAuto(false)
}

/** Step through editor themes: dark A–Z (Charcoal first), then light A–Z (Paper first). */
export function cycleEditorTheme(delta: 1 | -1): { key: string; name: string } {
  const cards = editorThemesForCycle()
  const key = cycleEditorThemeKey(get(editorTheme), delta, cards.map((c) => c.key))
  setEditorTheme(key)
  const name = cards.find((c) => c.key === key)?.name ?? key
  return { key, name }
}

export function initEditorTheme() {
  let saved = 'recur'
  try { saved = localStorage.getItem(EDITOR_THEME_KEY) ?? 'recur' } catch { /* ignore */ }
  applyEditorTheme(saved)
  editorSync.load()
}

// ---- code editor layout prefs (font / wrap / tab / indent) ----
export const EDITOR_FONT_SIZES = [10, 11, 12, 13, 14, 16] as const
export type EditorFontSize = (typeof EDITOR_FONT_SIZES)[number]
export const EDITOR_TAB_SIZES = [2, 4, 8] as const
export type EditorTabSize = (typeof EDITOR_TAB_SIZES)[number]
export type EditorIndent = 'spaces-2' | 'spaces-4' | 'tabs'

export const EDITOR_INDENT_OPTIONS: { key: EditorIndent; label: string; short: string }[] = [
  { key: 'spaces-2', label: '2 spaces', short: '2' },
  { key: 'spaces-4', label: '4 spaces', short: '4' },
  { key: 'tabs', label: 'Tabs', short: 'Tabs' },
]

/** CSS stack for the editor — `fontFace` is a family name (or legacy key). */
export function editorFontStack(fontFace: string): string {
  return monoFontStack(fontFace)
}

export type EditorWhitespace = 'off' | 'trailing' | 'all'
export type EditorLineHeight = 1.4 | 1.55 | 1.7
export type EditorRulerColumn = 0 | 80 | 100 | 120
export type EditorKeymap = 'default' | 'vim' | 'emacs'

export const EDITOR_WHITESPACE_OPTIONS: { key: EditorWhitespace; label: string }[] = [
  { key: 'off', label: 'Off' },
  { key: 'trailing', label: 'Trailing' },
  { key: 'all', label: 'All' },
]

export const EDITOR_LINE_HEIGHTS: EditorLineHeight[] = [1.4, 1.55, 1.7]

export const EDITOR_LINE_HEIGHT_LABELS: Record<EditorLineHeight, string> = {
  1.4: 'Compact',
  1.55: 'Default',
  1.7: 'Relaxed',
}

export const EDITOR_RULER_COLUMNS: EditorRulerColumn[] = [0, 80, 100, 120]

export const EDITOR_KEYMAP_OPTIONS: { key: EditorKeymap; label: string }[] = [
  { key: 'default', label: 'Default' },
  { key: 'vim', label: 'Vim' },
  { key: 'emacs', label: 'Emacs' },
]

export type EditorPrefs = {
  fontSize: EditorFontSize
  /** Font family name (e.g. "JetBrains Mono") or legacy key. */
  fontFace: string
  lineWrapping: boolean
  tabSize: EditorTabSize
  /** Global fallback when a language has neither an override nor a convention. */
  indent: EditorIndent
  /** Per-language indent overrides (missing = language convention, then indent). */
  indentByLang: Partial<Record<string, EditorIndent>>
  /** Off / trailing only / all whitespace marks. */
  whitespace: EditorWhitespace
  /** Highlight NBSP / zero-width / other special chars. */
  specialChars: boolean
  lineNumbers: boolean
  activeLine: boolean
  foldGutter: boolean
  /** SCM add/modify/delete bars in the code-view gutter (vs FileAtBase). */
  diffGutter: boolean
  bracketMatching: boolean
  scrollPastEnd: boolean
  lineHeight: EditorLineHeight
  cursorBlink: boolean
  thickCaret: boolean
  rectangularSelect: boolean
  dropCursor: boolean
  /** Print-margin column; 0 = off. */
  rulerColumn: EditorRulerColumn
  keymap: EditorKeymap
  /** Max open editor tabs; 0 = unlimited. */
  maxOpenFiles: number
}

/** Soft upper bound when a numeric max is set (guards against huge typos). */
export const MAX_OPEN_FILES_CAP = 200

export const DEFAULT_EDITOR_PREFS: EditorPrefs = {
  fontSize: 12,
  fontFace: 'JetBrains Mono',
  lineWrapping: true,
  tabSize: 4,
  indent: 'spaces-4',
  indentByLang: {},
  whitespace: 'off',
  specialChars: true,
  lineNumbers: true,
  activeLine: true,
  foldGutter: true,
  diffGutter: true,
  bracketMatching: true,
  scrollPastEnd: false,
  lineHeight: 1.55,
  cursorBlink: true,
  thickCaret: false,
  rectangularSelect: true,
  dropCursor: true,
  rulerColumn: 0,
  keymap: 'default',
  maxOpenFiles: 32,
}

const EDITOR_PREFS_KEY = 'recur.editorPrefs'
export const editorPrefs = writable<EditorPrefs>({ ...DEFAULT_EDITOR_PREFS, indentByLang: {} })

function isEditorIndent(v: unknown): v is EditorIndent {
  return v === 'spaces-2' || v === 'spaces-4' || v === 'tabs'
}

function normalizeIndentByLang(raw: unknown): Partial<Record<string, EditorIndent>> {
  if (!raw || typeof raw !== 'object') return {}
  const out: Partial<Record<string, EditorIndent>> = {}
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (isEditorIndent(v)) out[k] = v
  }
  return out
}

function isEditorWhitespace(v: unknown): v is EditorWhitespace {
  return v === 'off' || v === 'trailing' || v === 'all'
}

function isEditorLineHeight(v: unknown): v is EditorLineHeight {
  return v === 1.4 || v === 1.55 || v === 1.7
}

function isEditorRulerColumn(v: unknown): v is EditorRulerColumn {
  return v === 0 || v === 80 || v === 100 || v === 120
}

function isEditorKeymap(v: unknown): v is EditorKeymap {
  return v === 'default' || v === 'vim' || v === 'emacs'
}

function boolPref(v: unknown, fallback: boolean): boolean {
  return typeof v === 'boolean' ? v : fallback
}

function normalizeMaxOpenFiles(v: unknown): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) return DEFAULT_EDITOR_PREFS.maxOpenFiles
  const n = Math.floor(v)
  if (n <= 0) return 0
  return Math.min(n, MAX_OPEN_FILES_CAP)
}

function normalizeEditorPrefs(raw: Partial<EditorPrefs> | null | undefined): EditorPrefs {
  const fontSize = EDITOR_FONT_SIZES.includes(raw?.fontSize as EditorFontSize)
    ? (raw!.fontSize as EditorFontSize)
    : DEFAULT_EDITOR_PREFS.fontSize
  const rawFace = (raw?.fontFace ?? '').trim()
  const fontFace = rawFace
    ? (LEGACY_FONT_FACE_KEYS[rawFace] ?? rawFace)
    : DEFAULT_EDITOR_PREFS.fontFace
  const tabSize = EDITOR_TAB_SIZES.includes(raw?.tabSize as EditorTabSize)
    ? (raw!.tabSize as EditorTabSize)
    : DEFAULT_EDITOR_PREFS.tabSize
  const indent = isEditorIndent(raw?.indent) ? raw.indent : DEFAULT_EDITOR_PREFS.indent
  return {
    fontSize,
    fontFace,
    lineWrapping: boolPref(raw?.lineWrapping, DEFAULT_EDITOR_PREFS.lineWrapping),
    tabSize,
    indent,
    indentByLang: normalizeIndentByLang(raw?.indentByLang),
    whitespace: isEditorWhitespace(raw?.whitespace) ? raw.whitespace : DEFAULT_EDITOR_PREFS.whitespace,
    specialChars: boolPref(raw?.specialChars, DEFAULT_EDITOR_PREFS.specialChars),
    lineNumbers: boolPref(raw?.lineNumbers, DEFAULT_EDITOR_PREFS.lineNumbers),
    activeLine: boolPref(raw?.activeLine, DEFAULT_EDITOR_PREFS.activeLine),
    foldGutter: boolPref(raw?.foldGutter, DEFAULT_EDITOR_PREFS.foldGutter),
    diffGutter: boolPref(raw?.diffGutter, DEFAULT_EDITOR_PREFS.diffGutter),
    bracketMatching: boolPref(raw?.bracketMatching, DEFAULT_EDITOR_PREFS.bracketMatching),
    scrollPastEnd: boolPref(raw?.scrollPastEnd, DEFAULT_EDITOR_PREFS.scrollPastEnd),
    lineHeight: isEditorLineHeight(raw?.lineHeight) ? raw.lineHeight : DEFAULT_EDITOR_PREFS.lineHeight,
    cursorBlink: boolPref(raw?.cursorBlink, DEFAULT_EDITOR_PREFS.cursorBlink),
    thickCaret: boolPref(raw?.thickCaret, DEFAULT_EDITOR_PREFS.thickCaret),
    rectangularSelect: boolPref(raw?.rectangularSelect, DEFAULT_EDITOR_PREFS.rectangularSelect),
    dropCursor: boolPref(raw?.dropCursor, DEFAULT_EDITOR_PREFS.dropCursor),
    rulerColumn: isEditorRulerColumn(raw?.rulerColumn) ? raw.rulerColumn : DEFAULT_EDITOR_PREFS.rulerColumn,
    keymap: isEditorKeymap(raw?.keymap) ? raw.keymap : DEFAULT_EDITOR_PREFS.keymap,
    maxOpenFiles: normalizeMaxOpenFiles(raw?.maxOpenFiles),
  }
}

/** Effective indent for a language id (override → language convention → global). */
export function resolveEditorIndent(prefs: EditorPrefs, langId: string): EditorIndent {
  const override = prefs.indentByLang[langId]
  if (override) return override
  return LANGUAGE_INDENT_DEFAULTS[langId as keyof typeof LANGUAGE_INDENT_DEFAULTS] ?? prefs.indent
}

export function editorIndentLabel(indent: EditorIndent): string {
  return EDITOR_INDENT_OPTIONS.find((o) => o.key === indent)?.label ?? indent
}

export function setEditorPrefs(next: Partial<EditorPrefs>) {
  const cur = get(editorPrefs)
  const merged = normalizeEditorPrefs({
    ...cur,
    ...next,
    indentByLang: next.indentByLang !== undefined ? next.indentByLang : cur.indentByLang,
  })
  editorPrefs.set(merged)
  try { localStorage.setItem(EDITOR_PREFS_KEY, JSON.stringify(merged)) } catch { /* ignore */ }
}

/** Set or clear a per-language indent override (`null` = inherit default). */
export function setEditorIndentForLang(langId: string, indent: EditorIndent | null) {
  const cur = { ...get(editorPrefs).indentByLang }
  if (indent == null) delete cur[langId]
  else cur[langId] = indent
  setEditorPrefs({ indentByLang: cur })
}

export function initEditorPrefs() {
  let parsed: Partial<EditorPrefs> | null = null
  try {
    const raw = localStorage.getItem(EDITOR_PREFS_KEY)
    if (raw) parsed = JSON.parse(raw) as Partial<EditorPrefs>
  } catch { /* ignore */ }
  editorPrefs.set(normalizeEditorPrefs(parsed))
}

// ---- terminal font prefs (independent of the editor) ----
//
// The terminal gets its own font settings rather than riding the editor's: they
// are read in different ways (dense output you scan vs code you study), and a
// line height that suits CodeMirror wastes rows in a short dock.
//
// fontFace '' and fontSize 0 mean "match the editor", which is the default — so
// the two stay in sync until you deliberately separate them.
export const TERMINAL_MATCH_EDITOR = ''

/** Terminals read better tighter than code; 1.55 would cost ~4 rows in the dock. */
export const TERMINAL_LINE_HEIGHTS = [1, 1.15, 1.35] as const
export type TerminalLineHeight = (typeof TERMINAL_LINE_HEIGHTS)[number]

export const TERMINAL_LINE_HEIGHT_LABELS: Record<TerminalLineHeight, string> = {
  1: 'Compact',
  1.15: 'Default',
  1.35: 'Relaxed',
}

export type TerminalPrefs = {
  /** Family name, or '' to follow the editor's font. */
  fontFace: string
  /** Point size, or 0 to follow the editor's size. */
  fontSize: EditorFontSize | 0
  lineHeight: TerminalLineHeight
  /** Bundle [[terminalTheme]] id, or '' to follow the app chrome. */
  theme: string
}

export const DEFAULT_TERMINAL_PREFS: TerminalPrefs = {
  fontFace: TERMINAL_MATCH_EDITOR,
  fontSize: 0,
  lineHeight: 1.15,
  theme: TERMINAL_MATCH_EDITOR,
}

const TERMINAL_PREFS_KEY = 'recur.terminalPrefs'

function normalizeTerminalPrefs(raw: Partial<TerminalPrefs> | null): TerminalPrefs {
  const lineHeight = TERMINAL_LINE_HEIGHTS.includes(raw?.lineHeight as TerminalLineHeight)
    ? (raw!.lineHeight as TerminalLineHeight)
    : DEFAULT_TERMINAL_PREFS.lineHeight
  const fontSize = raw?.fontSize === 0 || EDITOR_FONT_SIZES.includes(raw?.fontSize as EditorFontSize)
    ? (raw!.fontSize as EditorFontSize | 0)
    : DEFAULT_TERMINAL_PREFS.fontSize
  return {
    fontFace: typeof raw?.fontFace === 'string' ? raw.fontFace : DEFAULT_TERMINAL_PREFS.fontFace,
    fontSize,
    lineHeight,
    theme: typeof raw?.theme === 'string' ? raw.theme : DEFAULT_TERMINAL_PREFS.theme,
  }
}

export const terminalPrefs = writable<TerminalPrefs>({ ...DEFAULT_TERMINAL_PREFS })

function patchTerminalPrefs(next: Partial<TerminalPrefs>) {
  const merged = normalizeTerminalPrefs({ ...get(terminalPrefs), ...next })
  terminalPrefs.set(merged)
  try { localStorage.setItem(TERMINAL_PREFS_KEY, JSON.stringify(merged)) } catch { /* ignore */ }
}

// '' (TERMINAL_MATCH_EDITOR) follows the app chrome, which already follows the system.
const terminalSync = createSystemSync(
  'terminalTheme', { light: TERMINAL_MATCH_EDITOR, dark: TERMINAL_MATCH_EDITOR },
  (id) => patchTerminalPrefs({ theme: id }),
)
export const terminalThemeAuto = terminalSync.auto
export const terminalThemeLight = terminalSync.light
export const terminalThemeDark = terminalSync.dark
export const setTerminalThemeAuto = terminalSync.setAuto
export const setSystemTerminalTheme = terminalSync.setPick

export function setTerminalPrefs(next: Partial<TerminalPrefs>) {
  patchTerminalPrefs(next)
  if ('theme' in next) setTerminalThemeAuto(false)
}

export function initTerminalPrefs() {
  let parsed: Partial<TerminalPrefs> | null = null
  try {
    const raw = localStorage.getItem(TERMINAL_PREFS_KEY)
    if (raw) parsed = JSON.parse(raw) as Partial<TerminalPrefs>
  } catch { /* ignore */ }
  terminalPrefs.set(normalizeTerminalPrefs(parsed))
  terminalSync.load()
}

/** Resolves the terminal's effective font, applying the "match editor" fallbacks. */
export function resolveTerminalFont(t: TerminalPrefs, e: EditorPrefs): {
  fontFace: string
  fontSize: number
  lineHeight: number
} {
  return {
    fontFace: t.fontFace || e.fontFace,
    fontSize: t.fontSize || e.fontSize,
    lineHeight: t.lineHeight,
  }
}

// ---- Code observatory panel sides (which dock each tool opens on) ----
export type CodePanelKey = 'maestro' | 'plan' | 'findings' | 'files' | 'scm' | 'outline' | 'problems' | 'search' | 'refs' | 'artifacts'
export type PanelSide = 'left' | 'right'

export const CODE_PANEL_KEYS: CodePanelKey[] = [
  'maestro', 'plan', 'findings', 'files', 'scm', 'outline', 'problems', 'search', 'refs', 'artifacts',
]

export function isCodePanelKey(v: unknown): v is CodePanelKey {
  return typeof v === 'string' && (CODE_PANEL_KEYS as string[]).includes(v)
}

export const CODE_PANEL_LABELS: Record<CodePanelKey, string> = {
  maestro: 'Maestro',
  plan: 'Plan',
  findings: 'Findings',
  files: 'Files',
  scm: 'Source Control',
  outline: 'Outline',
  problems: 'Problems',
  search: 'Search',
  refs: 'Refs',
  artifacts: 'Artifacts',
}

export const CODE_PANEL_SIDE_DEFAULTS: Record<CodePanelKey, PanelSide> = {
  maestro: 'left',
  plan: 'left',
  findings: 'left',
  files: 'right',
  scm: 'right',
  outline: 'right',
  problems: 'right',
  search: 'right',
  refs: 'right',
  artifacts: 'right',
}

const CODE_PANEL_SIDES_KEY = 'recur.codePanelSides'

function isPanelSide(v: unknown): v is PanelSide {
  return v === 'left' || v === 'right'
}

export const codePanelSides = writable<Record<CodePanelKey, PanelSide>>({ ...CODE_PANEL_SIDE_DEFAULTS })

export function setCodePanelSide(key: CodePanelKey, side: PanelSide) {
  codePanelSides.update((cur) => {
    const next = { ...cur, [key]: side }
    try { localStorage.setItem(CODE_PANEL_SIDES_KEY, JSON.stringify(next)) } catch { /* ignore */ }
    return next
  })
  reclampObservatoryDocks()
  persistPanelWidths()
}

export function panelsOnSide(
  sides: Record<CodePanelKey, PanelSide>,
  side: PanelSide,
): CodePanelKey[] {
  return CODE_PANEL_KEYS.filter((k) => sides[k] === side)
}

export function initCodePanelSides() {
  let saved: Record<string, unknown> = {}
  try {
    const raw = localStorage.getItem(CODE_PANEL_SIDES_KEY)
    if (raw) saved = JSON.parse(raw) as Record<string, unknown>
  } catch { /* ignore */ }
  const next = { ...CODE_PANEL_SIDE_DEFAULTS }
  for (const key of CODE_PANEL_KEYS) {
    if (isPanelSide(saved[key])) next[key] = saved[key]
  }
  // overview → plan (Activity/Overview panels were removed).
  if (!isPanelSide(saved.plan) && isPanelSide(saved.overview)) next.plan = saved.overview
  codePanelSides.set(next)
  reclampObservatoryDocks()
}

// ---- Files explorer prefs (files.exclude globs; localStorage) ----
export type ExplorerPrefs = {
  /** VS Code–style globs; matching paths are hidden from the file tree. */
  exclude: string[]
}

/** Defaults: VCS dirs + common OS junk (same starter set as VS Code). */
export const DEFAULT_EXPLORER_EXCLUDE: string[] = [
  '**/.git',
  '**/.svn',
  '**/.hg',
  '**/.DS_Store',
  '**/Thumbs.db',
]

export const DEFAULT_EXPLORER_PREFS: ExplorerPrefs = {
  exclude: [...DEFAULT_EXPLORER_EXCLUDE],
}

const EXPLORER_PREFS_KEY = 'recur.explorerPrefs'

export const explorerPrefs = writable<ExplorerPrefs>({
  exclude: [...DEFAULT_EXPLORER_EXCLUDE],
})

function normalizeExplorerPrefs(raw: Partial<ExplorerPrefs> | null | undefined): ExplorerPrefs {
  // Keep blank rows so "Add pattern" stays editable; matcher ignores empty globs.
  const exclude = Array.isArray(raw?.exclude)
    ? raw!.exclude.map((p) => String(p))
    : [...DEFAULT_EXPLORER_EXCLUDE]
  return { exclude }
}

export function setExplorerPrefs(next: Partial<ExplorerPrefs>) {
  const cur = get(explorerPrefs)
  const merged = normalizeExplorerPrefs({ ...cur, ...next })
  explorerPrefs.set(merged)
  try { localStorage.setItem(EXPLORER_PREFS_KEY, JSON.stringify(merged)) } catch { /* ignore */ }
}

export function setExplorerExclude(patterns: string[]) {
  setExplorerPrefs({ exclude: patterns })
}

export function resetExplorerExclude() {
  setExplorerExclude([...DEFAULT_EXPLORER_EXCLUDE])
}

export function initExplorerPrefs() {
  let parsed: Partial<ExplorerPrefs> | null = null
  try {
    const raw = localStorage.getItem(EXPLORER_PREFS_KEY)
    if (raw) parsed = JSON.parse(raw) as Partial<ExplorerPrefs>
  } catch { /* ignore */ }
  explorerPrefs.set(normalizeExplorerPrefs(parsed))
}

// ---- panel sizes (resizable docks; persisted like themes) ----
//
// Every key here is a panel's size along the axis it resizes on. That is a width
// for the side docks and a HEIGHT for terminalDock, which is the one horizontal
// splitter in the app — the store itself is axis-agnostic (a number and a CSS
// var per key); only ResizeHandle needs to know which way a given panel drags.
export type PanelKey =
  | 'rail' | 'loopInspector' | 'obsTree' | 'obsSide' | 'prFileList' | 'maestroDock'
  | 'prChatDock' | 'terminalDock' | 'inboxChat'

export const PANEL_DEFAULTS: Record<PanelKey, number> = {
  rail: 272,
  loopInspector: 312,
  obsTree: 220,
  obsSide: 260,
  prFileList: 250,
  maestroDock: 600,
  // Narrower than the workspace Agent dock so PR conversation/diff stay usable.
  prChatDock: 360,
  // Height. ~14 rows at the default font — enough for a build's tail without
  // taking the editor's half of the screen.
  terminalDock: 260,
  // Ask Recur dock on the Inbox — same footprint as the PR chat dock.
  inboxChat: 360,
}

const PANEL_BOUNDS: Record<PanelKey, { min: number; max: number }> = {
  rail: { min: 180, max: 600 },
  loopInspector: { min: 220, max: 720 },
  // The Observatory docks get a generous ceiling; their real cap is applied at
  // drag time relative to the editor row (ResizeHandle), keeping the code column ~⅓.
  obsTree: { min: 140, max: 2000 },
  obsSide: { min: 160, max: 2000 },
  prFileList: { min: 160, max: 900 },
  maestroDock: { min: 260, max: 2000 },
  prChatDock: { min: 260, max: 2000 },
  // Height. The floor is a few usable rows; the drag-time cap in ResizeHandle is
  // what actually stops it swallowing the editor.
  terminalDock: { min: 80, max: 2000 },
  inboxChat: { min: 280, max: 2000 },
}

// .obs-tb-left/right: 8px pad, 28px hide toggle, 2px gap, then the icon group
// (2px margin, 4px pad, 1px rule) of 28px pane toggles with 1px gaps.
const OBS_TB_TOGGLE = 28
const OBS_TB_PAD = 16
const OBS_TB_GAP = 2
const OBS_TB_ICONS_CHROME = 7
const OBS_TB_SIDE_RULE = 1

export function obsToolbarMinWidth(panelCount: number): number {
  const n = Math.max(0, panelCount)
  const icons = n === 0 ? 0 : OBS_TB_ICONS_CHROME + n * OBS_TB_TOGGLE + Math.max(0, n - 1)
  return OBS_TB_PAD + OBS_TB_TOGGLE + OBS_TB_GAP + icons + OBS_TB_SIDE_RULE
}

function observatoryDockMin(key: PanelKey): number {
  const base = PANEL_BOUNDS[key].min
  if (key !== 'maestroDock' && key !== 'obsSide') return base
  const side = key === 'maestroDock' ? 'left' : 'right'
  return Math.max(base, obsToolbarMinWidth(panelsOnSide(get(codePanelSides), side).length))
}

const PANEL_CSS_VARS: Record<PanelKey, string> = {
  rail: '--rail-width',
  loopInspector: '--insp-width',
  obsTree: '--obs-tree-width',
  obsSide: '--obs-side-width',
  prFileList: '--pr-filelist-width',
  maestroDock: '--maestro-dock-width',
  prChatDock: '--pr-chat-dock-width',
  terminalDock: '--terminal-dock-height',
  inboxChat: '--inbox-chat-width',
}

const PANEL_STORAGE_KEY = 'recur.panelWidths'

function clampPanel(key: PanelKey, w: number): number {
  const min = observatoryDockMin(key)
  const { max } = PANEL_BOUNDS[key]
  // max here is a generous safety ceiling only. The Observatory docks are capped
  // more precisely at drag time so the code column keeps ~a third of the editor
  // row (see ResizeHandle) — that layout-aware cap, not this static one, is what
  // normally binds.
  return Math.round(Math.min(max, Math.max(min, w)))
}

function reclampObservatoryDocks() {
  const cur = get(panelWidths)
  setPanelWidthLive('maestroDock', cur.maestroDock)
  setPanelWidthLive('obsSide', cur.obsSide)
}

function applyPanelCSS(widths: Record<PanelKey, number>) {
  const root = document.documentElement
  for (const key of Object.keys(PANEL_CSS_VARS) as PanelKey[]) {
    root.style.setProperty(PANEL_CSS_VARS[key], `${widths[key]}px`)
  }
}

export const panelWidths = writable<Record<PanelKey, number>>({ ...PANEL_DEFAULTS })

/**
 * Move a panel edge without touching storage. Drag handlers use this per frame;
 * serialising the whole width map and writing localStorage on every pointermove
 * was synchronous work on the drag path.
 */
export function setPanelWidthLive(key: PanelKey, width: number) {
  panelWidths.update((cur) => {
    const next = { ...cur, [key]: clampPanel(key, width) }
    applyPanelCSS(next)
    return next
  })
}

/** Persist the current widths — call once a drag settles. */
export function persistPanelWidths() {
  try {
    localStorage.setItem(PANEL_STORAGE_KEY, JSON.stringify(get(panelWidths)))
  } catch { /* ignore */ }
}

export function setPanelWidth(key: PanelKey, width: number) {
  setPanelWidthLive(key, width)
  persistPanelWidths()
}

// Prior factory defaults for maestroDock — if the user never customized the left
// pane (still sitting on one of these), adopt the new default on boot.
const MAESTRO_DOCK_LEGACY_DEFAULTS = new Set([400, 480])

// initPanelWidths restores saved widths and applies CSS vars before mount.
export function initPanelWidths() {
  let saved: Partial<Record<PanelKey, number>> = {}
  try {
    const raw = localStorage.getItem(PANEL_STORAGE_KEY)
    if (raw) saved = JSON.parse(raw) as Partial<Record<PanelKey, number>>
  } catch { /* ignore */ }
  const next = { ...PANEL_DEFAULTS }
  for (const key of Object.keys(PANEL_DEFAULTS) as PanelKey[]) {
    const v = saved[key]
    if (typeof v !== 'number' || !Number.isFinite(v)) continue
    // Skip stale factory defaults for the left dock so a wider default lands
    // without forcing users who deliberately resized to lose their choice.
    if (key === 'maestroDock' && MAESTRO_DOCK_LEGACY_DEFAULTS.has(v)) continue
    next[key] = clampPanel(key, v)
  }
  panelWidths.set(next)
  applyPanelCSS(next)
}

// ---- Inbox Ask Recur dock visibility (titlebar toggle; persists) ----
const INBOX_CHAT_OPEN_KEY = 'recur.askRecurOpen'
export const inboxChatOpen = writable((() => {
  try { return localStorage.getItem(INBOX_CHAT_OPEN_KEY) !== '0' } catch { return true }
})())

export function setInboxChatOpen(v: boolean) {
  inboxChatOpen.set(v)
  try { localStorage.setItem(INBOX_CHAT_OPEN_KEY, v ? '1' : '0') } catch { /* ignore */ }
}

// ---- left rail visibility (titlebar toggle; persists like panel widths) ----
const RAIL_COLLAPSED_KEY = 'recur.railCollapsed'
export const railCollapsed = writable(false)

export function setRailCollapsed(v: boolean) {
  railCollapsed.set(v)
  try { localStorage.setItem(RAIL_COLLAPSED_KEY, v ? '1' : '0') } catch { /* ignore */ }
}

export function toggleRail() {
  setRailCollapsed(!get(railCollapsed))
}

export function initRailCollapsed() {
  try {
    railCollapsed.set(localStorage.getItem(RAIL_COLLAPSED_KEY) === '1')
  } catch { /* ignore */ }
}

// ---- terminal dock visibility (Observatory bottom drawer; ⌃`) ----
//
// Persisted rather than component-local because the dock costs a real PTY per tab:
// someone who never opens it should never pay for one, and someone who lives in it
// shouldn't have to reopen it every launch. Closing the dock only unmounts the
// panes — the shells keep running, and reopening replays them (internal/term).
// Both are keyed by workspace: a terminal belongs to the workspace you opened it
// in, so its drawer should too. Carrying one global flag meant opening a shell in
// one workspace silently opened (and maximized) the dock in every other one.
//
// Chrome (open / maximized / editor-hidden) lives on terminals.json.gz in the
// workspace data dir — same file as the tab snapshot, not webview localStorage.
export const terminalDockOpenBy = writable<Record<string, boolean>>({})

/** Live Observatory tabs for @terminal mentions (id + OSC title + focused). */
export type WorkspaceTerminalTab = {
  id: string
  title: string
  exited?: boolean
  active?: boolean
}
export const workspaceTerminals = writable<Record<string, WorkspaceTerminalTab[]>>({})

export function terminalTabsEqual(a: WorkspaceTerminalTab[], b: WorkspaceTerminalTab[]): boolean {
  if (a === b) return true
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!, y = b[i]!
    if (x.id !== y.id || x.title !== y.title || !!x.exited !== !!y.exited || !!x.active !== !!y.active) {
      return false
    }
  }
  return true
}

/** Request the dock focus a session (chat bubble chip click). */
export const terminalFocus = writable<{ workspaceId: string; id: string } | null>(null)

/** Workspace artifacts for @artifact mentions (id + title + kind). */
export type WorkspaceArtifact = {
  id: string
  title: string
  kind: string
  path: string
}
export const workspaceArtifacts = writable<Record<string, WorkspaceArtifact[]>>({})

export function artifactsEqual(a: WorkspaceArtifact[], b: WorkspaceArtifact[]): boolean {
  if (a === b) return true
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!, y = b[i]!
    if (x.id !== y.id || x.title !== y.title || x.kind !== y.kind || x.path !== y.path) {
      return false
    }
  }
  return true
}

/**
 * Maximized = the dock takes over the whole Observatory. Persisted per workspace
 * like open/closed, so a terminal-first layout comes back after quit. Hiding the
 * dock still drops it (setTerminalDockOpen) so a mid-session ⌃` does not ambush
 * you with a full-screen terminal.
 */
export const terminalDockMaximizedBy = writable<Record<string, boolean>>({})

/**
 * Editor hidden = the center CodeMirror surface (crumbs, panes, status) is tucked
 * away so the terminal can own the middle panel. Side docks and the toolbar stay —
 * unlike maximize, which covers the whole Observatory. Persisted per workspace so
 * a terminal-first loop remembers the preference.
 */
export const editorHiddenBy = writable<Record<string, boolean>>({})
export const terminalBesideBy = writable<Record<string, boolean>>({})
export const terminalBesidePctBy = writable<Record<string, number>>({})

function setFlag(store: typeof terminalDockOpenBy, id: string, v: boolean) {
  store.update((cur) => {
    if (!!cur[id] === v) return cur
    const next = { ...cur }
    if (v) next[id] = true
    else delete next[id]
    return next
  })
}

function clampBesidePct(n: number): number {
  if (!Number.isFinite(n) || n <= 0) return 0
  return Math.round(Math.min(80, Math.max(20, n)))
}

function persistDockChrome(id: string) {
  if (!id) return
  terminalDockChromeTouched.add(id)
  const open = !!get(terminalDockOpenBy)[id]
  const maximized = open && !!get(terminalDockMaximizedBy)[id]
  const editorHidden = open && !!get(editorHiddenBy)[id]
  const beside = !!get(terminalBesideBy)[id]
  const besidePct = clampBesidePct(get(terminalBesidePctBy)[id] ?? 0)
  void SetTerminalDockChrome(id, { open, maximized, editorHidden, beside, besidePct }).catch(() => {})
}

/** Whether the dock is showing for a given workspace. */
export function isTerminalDockOpen(map: Record<string, boolean>, id: string): boolean {
  return !!id && !!map[id]
}

export function isEditorHidden(map: Record<string, boolean>, id: string): boolean {
  return !!id && !!map[id]
}

export function setTerminalDockOpen(id: string, v: boolean) {
  if (!id) return
  setFlag(terminalDockOpenBy, id, v)
  // Hiding drops maximize, so re-opening never surprises you with a full-screen
  // terminal you set up minutes ago. Also restore the editor — an empty middle
  // panel (both hidden) is never useful.
  if (!v) {
    setFlag(terminalDockMaximizedBy, id, false)
    setFlag(editorHiddenBy, id, false)
  }
  persistDockChrome(id)
}

export function toggleTerminalDock(id: string) {
  if (!id) return
  setTerminalDockOpen(id, !get(terminalDockOpenBy)[id])
}

export function setTerminalDockMaximized(id: string, v: boolean) {
  if (!id) return
  // Maximizing implies showing — otherwise the shortcut arms a state with no
  // visible effect from a closed dock.
  if (v) setTerminalDockOpen(id, true)
  setFlag(terminalDockMaximizedBy, id, v)
  persistDockChrome(id)
}

export function toggleTerminalDockMaximized(id: string) {
  if (!id) return
  setTerminalDockMaximized(id, !get(terminalDockMaximizedBy)[id])
}

export function setEditorHidden(id: string, v: boolean) {
  if (!id) return
  // Hiding the editor with no terminal would leave an empty middle panel — open
  // the dock so the terminal can take the space (the user's stated goal).
  if (v) setTerminalDockOpen(id, true)
  setFlag(editorHiddenBy, id, v)
  persistDockChrome(id)
}

export function toggleEditorHidden(id: string) {
  if (!id) return
  setEditorHidden(id, !get(editorHiddenBy)[id])
}

export function isTerminalBeside(map: Record<string, boolean>, id: string): boolean {
  return !!id && !!map[id]
}

export function terminalBesidePct(map: Record<string, number>, id: string): number {
  const n = id ? map[id] : 0
  return n >= 20 && n <= 80 ? n : 50
}

export function setTerminalBeside(id: string, v: boolean) {
  if (!id) return
  setFlag(terminalBesideBy, id, v)
  if (v) setTerminalDockOpen(id, true)
  else persistDockChrome(id)
}

export function setTerminalBesidePct(id: string, n: number, persist = true) {
  if (!id) return
  const pct = clampBesidePct(n)
  terminalBesidePctBy.update((cur) => {
    if (cur[id] === pct) return cur
    return { ...cur, [id]: pct }
  })
  if (persist) persistDockChrome(id)
}

export type TerminalDockChromeRow = {
  workspaceId?: string
  open?: boolean
  maximized?: boolean
  editorHidden?: boolean
  beside?: boolean
  besidePct?: number
}

/** Workspaces whose dock chrome the user already toggled this session. */
export const terminalDockChromeTouched = new Set<string>()

/** Apply Go-side dock chrome (ListTerminalDockChrome) before first Observatory paint. */
export function applyTerminalDockChrome(rows: TerminalDockChromeRow[] | null | undefined) {
  const open: Record<string, boolean> = {}
  const max: Record<string, boolean> = {}
  const hidden: Record<string, boolean> = {}
  const beside: Record<string, boolean> = {}
  const pct: Record<string, number> = {}
  for (const r of rows ?? []) {
    const id = (r.workspaceId ?? '').trim()
    if (!id) continue
    if (r.open) {
      open[id] = true
      if (r.maximized) max[id] = true
      if (r.editorHidden) hidden[id] = true
    }
    if (r.beside) beside[id] = true
    const n = clampBesidePct(Number(r.besidePct))
    if (n) pct[id] = n
  }
  // Merge: a ⌃` (or click) during loadInitial already wrote the store and
  // SetTerminalDockChrome — do not wholesale .set() and revert it.
  terminalDockOpenBy.update((cur) => mergeDockChrome(open, cur))
  terminalDockMaximizedBy.update((cur) => mergeDockChrome(max, cur))
  editorHiddenBy.update((cur) => mergeDockChrome(hidden, cur))
  terminalBesideBy.update((cur) => mergeDockChrome(beside, cur))
  terminalBesidePctBy.update((cur) => mergeDockPct(pct, cur))
}

function mergeDockChrome(fromDisk: Record<string, boolean>, cur: Record<string, boolean>): Record<string, boolean> {
  const next = { ...cur }
  for (const id of Object.keys(fromDisk)) {
    if (terminalDockChromeTouched.has(id)) continue
    if (fromDisk[id]) next[id] = true
  }
  return next
}

function mergeDockPct(fromDisk: Record<string, number>, cur: Record<string, number>): Record<string, number> {
  const next = { ...cur }
  for (const id of Object.keys(fromDisk)) {
    if (terminalDockChromeTouched.has(id)) continue
    next[id] = fromDisk[id]!
  }
  return next
}

// ---- navigation ----
export const view = writable<ViewName>('briefing')
export const selectedWorkspaceId = writable<string>('')

// Schedules across all projects, published by the Schedules view for the
// titlebar counts. scheduleEditor opens the always-mounted New/Edit schedule
// modal from anywhere (sidebar "+", lists); the editor clears it once consumed.
// schedulesVersion bumps after a save so lists refresh.
export const schedules = writable<ScheduledPrompt[]>([])
export type ScheduleEditorRequest = { projectId?: string; schedule?: ScheduledPrompt }
export const scheduleEditor = writable<ScheduleEditorRequest | null>(null)
export const schedulesVersion = writable(0)

/** Sidebar listens: keyboard-cycle lands peek-expand collapsed ancestors; commit = dwell/click. */
export type WorkspaceScanEvent = {
  seq: number
  kind: 'land' | 'commit'
  workspaceId: string
}
export const workspaceScanEvent = writable<WorkspaceScanEvent>({ seq: 0, kind: 'commit', workspaceId: '' })
/** True between a cycle land and its dwell/leave commit — skip seen-marking and git fetch. */
export const workspaceScanPending = writable(false)
let workspaceScanSeq = 0
let workspaceScanTimer: ReturnType<typeof setTimeout> | undefined
/** Last cycle cursor, including `main:` sentinels that are not yet a workspace id. */
let workspaceScanCursor = ''

function emitWorkspaceScan(kind: WorkspaceScanEvent['kind'], workspaceId: string) {
  // Sentinel commit still waits on OpenMainWorkspace; keep pending so we do not
  // mark the previous workspace seen or fetch git for a row we have not opened.
  const hold = kind === 'land' || (kind === 'commit' && !!projectIdFromMainScan(workspaceId))
  workspaceScanPending.set(hold)
  workspaceScanEvent.set({ seq: ++workspaceScanSeq, kind, workspaceId })
}

function armWorkspaceScanCommit(workspaceId: string) {
  clearTimeout(workspaceScanTimer)
  workspaceScanTimer = setTimeout(() => emitWorkspaceScan('commit', workspaceId), SCAN_COMMIT_MS)
}

function commitWorkspaceScan(workspaceId = '') {
  clearTimeout(workspaceScanTimer)
  emitWorkspaceScan('commit', workspaceId)
}

export const selectedPRId = writable<string>('')
/** PR dashboard lifecycle (titlebar). Open is live PRs that are not drafts. */
export type PRListSeg = PRListLife
export const prListSeg = writable<PRListSeg>('open')
/** Status facets (review requested, checks, …). Empty = no extra cut. */
export const prListFacets = writable<string[]>([])
/** Within-group list order. Default is recently updated. */
export const prListSort = writable<PRListSort>(PR_LIST_SORT_DEFAULT)
/** Empty = every git project. */
export const prListProjectIds = writable<string[]>([])
export const prListSyncing = writable(false)
export { PR_LIST_SEGS, PR_LIST_SORTS }

/** PRs visible under the dashboard's project filter (empty selection = all).
 *  Legacy rows with an empty projectId resolve through their workspace. */
export function scopedPRs(list: PR[], projectIds: string[], workspaceProject: Record<string, string> = {}): PR[] {
  if (!projectIds.length) return list
  return list.filter((p) => {
    let pid = (p.projectId || '').trim()
    if (!pid && p.workspaceId) pid = (workspaceProject[p.workspaceId] || '').trim()
    return projectIds.includes(pid)
  })
}

/** Conversation / commits / files / checks tab on the PR detail view. */
export type PRDetailTab = 'conv' | 'commits' | 'files' | 'checks'
export const prDetailTab = writable<PRDetailTab>('conv')
/** Conversation-page comment draft. Review text lives in prReviewDraft. */
export const prComposerDraft = writable('')
/** Approve / Request-changes modal (titlebar split, any PR tab). */
export type PRReviewKind = 'approve' | 'request'
export const prReviewModal = writable<PRReviewKind | null>(null)
export const prReviewDraft = writable('')
/** Files-changed jump requested from the review modal's pending list. */
export const prDiffJump = writable<{ file: string; line: number } | null>(null)

export function openPRReview(kind: PRReviewKind) {
  prReviewModal.set(kind)
}

export function closePRReview() {
  prReviewModal.set(null)
}
// Which project the Project screen edits; '' means "create a NEW project".
export const projectFormId = writable<string>('')
// Prefill for NewTask's repo picker (sidebar "+" / project tree). Cleared after apply.
export const newTaskRepoId = writable<string>('')
// New-loop form is a modal overlay (not a navigable view).
export const newTaskOpen = writable<boolean>(false)
/** Quick-create project modal (sidebar + / Welcome). Not a full-page view. */
export const newProjectOpen = writable<boolean>(false)
/** After create, reopen New workspace with the new project selected. */
let newProjectResumeNewTask = false
export const paletteOpen = writable<boolean>(false)
// Recur Bundle / editor-command palette (⌘⇧P) — separate from ⌘K go-to search.
export const commandsOpen = writable<boolean>(false)
// Keyboard-shortcuts cheatsheet overlay (⌘⇧/).
export const shortcutsOpen = writable<boolean>(false)
// Workspace "Open in" picker (⌘O) — Finder, Cursor, Terminal, copy path, …
export const openInOpen = writable<boolean>(false)
// Named-run picker (⌘R). Stop-run dropdown when several commands are going.
export const runMenuOpen = writable<boolean>(false)
export const stopMenuOpen = writable<boolean>(false)
// Editor status-bar language-mode picker (footer click / ⌘K).
export const syntaxModeOpen = writable<boolean>(false)

/** Toggle the shortcuts list; opening it dismisses other overlays. */
export function toggleShortcuts() {
  const next = !get(shortcutsOpen)
  if (next) {
    paletteOpen.set(false)
    commandsOpen.set(false)
    openInOpen.set(false)
    syntaxModeOpen.set(false)
    newTaskOpen.set(false)
    newProjectOpen.set(false)
    runMenuOpen.set(false)
    stopMenuOpen.set(false)
  }
  shortcutsOpen.set(next)
}

/** Toggle the Run Command palette (⌘⇧P); mutually exclusive with ⌘K / shortcuts. */
export function toggleCommandsPalette() {
  const next = !get(commandsOpen)
  if (next) {
    paletteOpen.set(false)
    shortcutsOpen.set(false)
    openInOpen.set(false)
    syntaxModeOpen.set(false)
    newTaskOpen.set(false)
    newProjectOpen.set(false)
    runMenuOpen.set(false)
    stopMenuOpen.set(false)
  }
  commandsOpen.set(next)
}

/** Open the Run Command palette (status-bar Bundles). */
export function openCommandsPalette() {
  paletteOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  newTaskOpen.set(false)
  newProjectOpen.set(false)
  runMenuOpen.set(false)
  stopMenuOpen.set(false)
  commandsOpen.set(true)
}

/** Toggle the go-to palette (⌘K); mutually exclusive with Run Command / shortcuts. */
export function toggleGoPalette() {
  const next = !get(paletteOpen)
  if (next) {
    commandsOpen.set(false)
    shortcutsOpen.set(false)
    openInOpen.set(false)
    syntaxModeOpen.set(false)
    newTaskOpen.set(false)
    newProjectOpen.set(false)
    runMenuOpen.set(false)
    stopMenuOpen.set(false)
  }
  paletteOpen.set(next)
}

/** Toggle Open in… (⌘O). No-op open when no workspace is on screen. */
export function toggleOpenIn() {
  const next = !get(openInOpen)
  if (next) {
    const v = get(view)
    if (v !== 'detail') return
    const id = get(selectedWorkspaceId)
    if (!id) return
    if (wsMainBlocked(get(workspaces).find((w) => w.id === id))) return
    paletteOpen.set(false)
    commandsOpen.set(false)
    shortcutsOpen.set(false)
    syntaxModeOpen.set(false)
    newTaskOpen.set(false)
    newProjectOpen.set(false)
    runMenuOpen.set(false)
    stopMenuOpen.set(false)
  }
  openInOpen.set(next)
}

/** Open the language-mode picker for the focused editor file. */
export function openSyntaxModePicker() {
  paletteOpen.set(false)
  commandsOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  newTaskOpen.set(false)
  newProjectOpen.set(false)
  runMenuOpen.set(false)
  stopMenuOpen.set(false)
  syntaxModeOpen.set(true)
}
// Deep-link into the code observatory (escalation / findings → file:line,
// or Findings panel + optional note composer via findingId).
export type ObsJump = {
  path?: string
  line?: number
  /** Inclusive end line — when > line, editor selects the full range. */
  endLine?: number
  /** 1-based inclusive columns for a partial-line @file chip. */
  startCol?: number
  endCol?: number
  workspaceId?: string
  projectId?: string
  /** Open this code panel (e.g. findings) even without a findingId. */
  panel?: 'findings' | 'plan' | 'review'
  /** With panel 'plan': which pane to show (plan card step → todos). */
  planTab?: 'details' | 'todos' | 'attachments'
  findingId?: string
  focusNote?: boolean
  preferDiff?: boolean
}
export const observatoryJump = writable<ObsJump | null>(null)
/** One-shot: expand a merged child's transcript inside its wave card (FanOutCard consumes). */
export const childTranscriptRequest = writable<string | null>(null)
/** One-shot: the Plan panel pane a jump asked for; WorkspaceView applies and clears it. */
export const planPanelTab = writable<'details' | 'todos' | 'attachments' | null>(null)

// ---- data (seeded from Go on boot, updated by events) ----
export const projectGroups = writable<ProjectGroup[]>([])
export const projects = writable<Project[]>([])
export const workspaces = writable<Workspace[]>([])
export const prs = writable<PR[]>([])
/** PRs visible under the dashboard's project filter (empty selection = all). */
export const prListScoped = derived([prs, prListProjectIds, workspaces], ([$prs, $ids, $ws]) => {
  const byWs: Record<string, string> = {}
  for (const w of $ws) {
    if (w.id && w.projectId) byWs[w.id] = w.projectId
  }
  return scopedPRs($prs, $ids, byWs)
})
export const inbox = writable<InboxItem[]>([])

/** Unread notifications per workspace — the read/unread signal on Briefing cards. */
export const inboxByWorkspace = derived(inbox, ($inbox) => {
  const m: Record<string, InboxItem[]> = {}
  for (const i of $inbox) {
    if (i.unread && i.workspaceId) (m[i.workspaceId] ??= []).push(i)
  }
  return m
})

/** Overview section to scroll into view (needs / working / pickup). */
export const briefingFocus = writable<string>('')
export type BriefingSnapshotState = {
  generatedAt: number
  counts: { needs: number; working: number; pickup: number }
  workspaces: import('../../bindings/recur/internal/domain/models').WorkspaceBrief[]
}
export const emptyBriefing = (): BriefingSnapshotState => ({
  generatedAt: 0,
  counts: { needs: 0, working: 0, pickup: 0 },
  workspaces: [],
})
export const briefing = writable<BriefingSnapshotState>(emptyBriefing())

// Unseen finished chat tabs + rollups for sidebar/project badges (D-attention).
// Dock badge is applied natively from Go; this store drives in-app chrome.
export type AttentionState = {
  unseen: number
  inboxUnread: number
  dock: number
  appUnseen: number
  byWorkspace: Record<string, number>
  byProject: Record<string, number>
}
export const attention = writable<AttentionState>({
  unseen: 0,
  inboxUnread: 0,
  dock: 0,
  appUnseen: 0,
  byWorkspace: {},
  byProject: {},
})
export const policy = writable<Policy | null>(null)
export const settings = writable<Settings | null>(null)
settings.subscribe((s) => {
  const grammarIds = s?.disabledBundleGrammars ?? []
  const themeIds = s?.disabledBundleThemes ?? []
  setDisabledBundleContribs({ grammarIds, themeIds })
  const current = get(editorTheme)
  if (themeIds.includes(current)) applyEditorTheme(current)
})
export const resource = writable<Resource | null>(null)
// Optional RECUR_INSTANCE_LABEL (dogfood titlebar chip). Empty in packaged Recur.
export const instanceLabel = writable('')
// Subscription quotas for the sidebar-footer usage chip (Claude / Codex / Cursor).
export const providerUsage = writable<ProviderUsageSnapshot | null>(null)
// Workspace IDs with at least one agent in a live status (working/directing/planning/replanning).
// Sidebar and fan-out rows also pulse for an in-flight loop (workspaceIsRunning).
// Footer running count follows that; "N needs you" wins while any workspace is waiting on the human.
export const workspacesRunning = writable<ReadonlySet<string>>(new Set())
// Individual agent IDs (`workspaceId-code`) currently live. Chat dock busy state
// follows this (same source as the sidebar) so a lost Send promise / remount
// cannot clear the footer dots while the engine is still mid-turn.
export const agentsRunning = writable<ReadonlySet<string>>(new Set())

// Live tool-permission prompts, oldest first, raised by agents while
// Settings.bypassPermissions is off. Not persisted anywhere: an agent is
// blocked on an open request for as long as one sits here, so the queue is
// rebuilt from the backend on boot rather than from disk.
export const permissionRequests = writable<PermissionRequest[]>([])

/** Full-screen dialogs (new workspace, palette, shortcuts, …). In-page menus should close. */
export const appOverlayOpen = derived(
  [newTaskOpen, newProjectOpen, paletteOpen, commandsOpen, openInOpen, shortcutsOpen, syntaxModeOpen, runMenuOpen, stopMenuOpen, keybindingMenu, chatOverlayOpen, permissionRequests, prReviewModal],
  ([task, project, palette, commands, openIn, shortcuts, syntax, run, stop, keys, chat, perms, review]) =>
    task || project || palette || commands || openIn || shortcuts || syntax || run || stop || !!keys || chat || perms.length > 0 || !!review,
)

// Live ask_user prompts (interactive agents). Do not escalate the workspace.
export const userQuestions = writable<UserQuestion[]>([])

// Chat tab the human is currently looking at (workspace detail + AgentChat).
// Chat-surface permission prompts for this pair render inline in the transcript;
// other permission prompts keep the floating card. ask_user is different: focused
// tabs show an inline card; off-tab is a short auto-dismiss toast (inbox remains).
export const focusedChat = writable<{ workspaceId: string; code: string } | null>(null)

// One-shot: WorkspaceView selects this agent tab after openWorkspaceChat / inbox
// "question:" targets land on a workspace (ask_user toast / Answer).
export const pendingChatFocus = writable<{ workspaceId: string; code: string } | null>(null)

/** PR-detail slot that hosts the linked workspace's agent dock (or null). */
export const prAgentDockHost = writable<HTMLElement | null>(null)

/** Whether the portaled agent dock is shown on PR detail (session-only, like Observatory's left gutter). */
export const prAgentDockOpen = writable(true)

/** Which PR-detail chrome has keyboard focus — amber tab underline, matching Observatory. */
export const prDetailFocus = writable<'agent' | 'pr'>('pr')

/** Agent transcript is the on-screen chat surface (workspace detail or PR dock). */
export const agentChatOnScreen = derived(
  [view, prs, workspaces, selectedPRId, prAgentDockOpen],
  ([$view, $prs, $workspaces, prId, $dockOpen]) => {
    if ($view === 'detail') return true
    if ($view === 'prdetail') {
      const pr = $prs.find((p) => p.id === prId)
      return $dockOpen && !!prAgentWorkspaceId(pr, $workspaces)
    }
    return false
  },
)

// The breadcrumb shown in the title bar; derived on navigation.
export const crumb = writable<string>('<b class="on">Inbox</b>')

// Persist the last view only after boot restore, so the in-memory Briefing
// default cannot clobber a saved workspace on the way up. Subscribe to the
// selection ids as well as `view`: saving a new project updates projectFormId
// without another go(), and that id must still land in shell.json.
let shellPersistEnabled = false
export function enableShellPersist() {
  if (shellPersistEnabled) return
  shellPersistEnabled = true
  const flush = () => persistOpenShell()
  view.subscribe(flush)
  selectedWorkspaceId.subscribe(flush)
  selectedPRId.subscribe(flush)
  projectFormId.subscribe(flush)
}

function persistOpenShell() {
  if (!shellPersistEnabled) return
  const v = get(view)
  persistShellState(snapshotShell(
    v as ShellView,
    get(selectedWorkspaceId),
    get(selectedPRId),
    get(projectFormId),
  ))
}

export type GoOpts = { focus?: string }

// go navigates to a top-level view and updates the breadcrumb (mirrors mockup go()).
export function go(v: ViewName, opts?: GoOpts) {
  // The old full-page escalation view is gone — questions live in Loop chat.
  if (v === 'escalation') v = 'detail'
  // Legacy destination: the notification stream folded into Briefing.
  if (v === 'inbox') v = 'briefing'
  if (v === 'briefing' && opts?.focus !== undefined) briefingFocus.set(opts.focus)
  if (v !== 'detail') {
    workspaceScanCursor = ''
    commitWorkspaceScan('')
  }
  view.set(v)
  paletteOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  syntaxModeOpen.set(false)
  runMenuOpen.set(false)
  stopMenuOpen.set(false)
  crumb.set(crumbFor(v))
}

export type SettingsSection =
  | 'agent' | 'models' | 'loops' | 'prs' | 'integrations' | 'general'
  | 'appearance' | 'editor' | 'skills' | 'mcp' | 'data'
  | 'bundle-commands' | 'bundle-prompts' | 'bundle-grammars' | 'bundle-lsp' | 'bundle-themes' | 'bundle-snippets'

/** Active Settings pane. Survives leaving the view (Settings stays mounted). */
export const settingsSection = writable<SettingsSection>('general')

/** Open Settings, optionally landing on a specific pane. */
export function openSettings(section?: SettingsSection | 'autonomy' | 'guardrails') {
  if (section === 'autonomy' || section === 'guardrails') section = 'loops'
  if (section) settingsSection.set(section)
  go('settings')
}

// crumbEsc escapes HTML — the crumb is rendered with {@html} and a workspace name
// is user-provided.
function crumbEsc(s: string): string {
  return (s ?? '').replace(/[&<>"]/g, (c) => (
    c === '&' ? '&amp;' : c === '<' ? '&lt;' : c === '>' ? '&gt;' : '&quot;'
  ))
}

function crumbFor(v: ViewName): string {
  switch (v) {
    case 'detail':
    case 'escalation': {
      const l = get(workspaces).find((x) => x.id === get(selectedWorkspaceId))
      const name = crumbEsc(l?.short || l?.branch || l?.id || '')
      const branch = l?.branch ? ` <span class="crumb-branch">${crumbEsc(l.branch)}</span>` : ''
      return `<b>${crumbEsc(l?.projectId ?? '')}</b> / <span class="on">${name}</span>${branch}`
    }
    case 'schedules': return '<b class="on">Schedules</b>'
    case 'issues': return '<b class="on">Issues</b>'
    case 'pr': return '<b class="on">Pull requests</b>'
    case 'prdetail': {
      const id = get(selectedPRId)
      const n = get(prs).find((p) => p.id === id)?.number
      return `<b>Pull requests</b> / <span class="on">#${n || id}</span>`
    }
    case 'welcome': return '<b class="on">Get started</b>'
    case 'inbox':
    case 'briefing': return '<b class="on">Inbox</b>'
    case 'settings': return '<b class="on">Settings</b>'
    default: return `<b class="on">${v}</b>`
  }
}

// openWorkspace always lands on workspace detail. An open loop escalation is
// the inline Loop-chat card.
export function openWorkspace(id: string, opts?: { scan?: boolean }) {
  if (!opts?.scan) {
    workspaceScanCursor = id
    commitWorkspaceScan(id)
  }
  const pending = get(pendingChatFocus)
  if (pending && pending.workspaceId !== id) {
    pendingChatFocus.set(null)
  }
  selectedWorkspaceId.set(id)
  const l = get(workspaces).find((x) => x.id === id)
  if (l?.state === 'escalated' && !get(pendingChatFocus)) {
    pendingChatFocus.set({ workspaceId: id, code: 'MA' })
  }
  go('detail')
}

function workspaceCycleBlocked(): boolean {
  return get(newTaskOpen) || get(newProjectOpen) || get(paletteOpen) || get(commandsOpen) || get(openInOpen)
    || get(shortcutsOpen) || get(syntaxModeOpen) || get(runMenuOpen) || get(stopMenuOpen) || !!get(keybindingMenu) || get(chatOverlayOpen)
    || !!get(prReviewModal)
}

/** Cycle to the next (+1) or previous (−1) sidebar workspace, wrapping. */
export function cycleWorkspace(delta: 1 | -1) {
  if (workspaceCycleBlocked()) return
  const showHidden = !!get(settings)?.showHiddenProjects
  const ids = sidebarWorkspaceIds(get(workspaces), get(projects).filter((p) => showHidden || !p.hidden), get(projectGroups))
  const current = workspaceScanCursor || get(selectedWorkspaceId)
  const next = neighborWorkspaceId(ids, current, delta)
  if (!next) return
  workspaceScanCursor = next
  if (!projectIdFromMainScan(next)) openWorkspace(next, { scan: true })
  emitWorkspaceScan('land', next)
  armWorkspaceScanCommit(next)
}

/** Open a workspace and select a specific chat tab (ask_user toast / inbox). */
export function openWorkspaceChat(workspaceId: string, agentCode?: string) {
  const code = (agentCode ?? '').trim()
  if (code) pendingChatFocus.set({ workspaceId, code })
  const hostPrId = prIdForChatWorkspace(workspaceId, get(workspaces))
  if (hostPrId) {
    prAgentDockOpen.set(true)
    // Same PR is already selected — don't reset composer/review drafts or the tab.
    if (get(selectedPRId) === hostPrId) {
      go('prdetail')
      return
    }
    openPR(hostPrId)
    return
  }
  openWorkspace(workspaceId)
}

// workspaceSeeds carries an opening instruction + chat mode + model from the
// New-workspace modal to a freshly created workspace, keyed by workspace id so
// a later create cannot erase another workspace's pending or failed opening.
// WorkspaceView consumes an entry once — it applies the chosen mode/model to
// the first chat tab, then sends any prompt as the first message — or
// startParkedOpening does, if the view never claims it.
export type WorkspaceSeed = {
  id: string
  text: string
  /** UI-only chat bubble (e.g. Linear expand). Wire stays in `text`. */
  display?: string
  mode?: string
  model?: string
  flags?: Record<string, string>
  /** Composer Simplify for the first chat (New-workspace modal chip). */
  simplify?: boolean
  /** Images pasted in the New-workspace modal; sent with the opening chat message. */
  attachments?: { id: string; name: string; mime: string; dataUrl: string }[]
}

export const workspaceSeeds = writable<Record<string, WorkspaceSeed>>({})

export function setWorkspaceSeed(seed: WorkspaceSeed) {
  workspaceSeeds.update((m) => ({ ...m, [seed.id]: seed }))
}

/** Claim this workspace's seed for send. Null if another owner already took it. */
export function takeWorkspaceSeed(id: string): WorkspaceSeed | null {
  const cur = get(workspaceSeeds)
  const seed = cur[id]
  if (!seed) return null
  const next = { ...cur }
  delete next[id]
  workspaceSeeds.set(next)
  return seed
}

// focusedProjectId returns the project the user is currently working in: the
// Project screen when open, otherwise the project of the selected workspace.
function focusedProjectId(): string {
  if (get(view) === 'project') {
    const id = get(projectFormId)
    if (id) return id
  }
  const ws = get(workspaces).find((w) => w.id === get(selectedWorkspaceId))
  return ws?.projectId ?? ''
}

/** Git project ⌘N / ⇧⌘N should target. Focused project if it is git; otherwise
 *  the first git project only when nothing is focused. A focused non-git project
 *  returns undefined so the caller can open the modal instead of creating in
 *  some other repo. */
export function gitProjectForNewWorkspace(): Project | undefined {
  const all = get(projects).filter((p) => p.isGit)
  if (!all.length) return undefined
  const id = focusedProjectId()
  if (!id) return all[0]
  return all.find((p) => p.id === id)
}

// openNewTask opens the New workspace modal, optionally pre-selecting a project
// and pre-filling the goal and/or mode (e.g. a Linear issue prompt from ⌘K, or a
// mode picked on Getting Started). When repoId is
// omitted, default to the focused project so ⌘N lands in the project you're in.
/** linearKey: the issue's Linear project name (or team key when it has none).
 *  The modal offers to save it on the chosen project when that differs from
 *  linkedProjectId, the project the issue routes to today. */
export type NewTaskDraft = { text: string; display?: string; mode?: ChatMode; linearKey?: string; linkedProjectId?: string }
export const newTaskDraft = writable<NewTaskDraft | null>(null)
/** Bumped when an issue's assignment may have changed; the Issues view reloads. */
export const issuesStale = writable(0)

export function openNewTask(repoId = '', draft?: NewTaskDraft) {
  newTaskRepoId.set(repoId || focusedProjectId())
  newTaskDraft.set(draft ?? null)
  paletteOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  syntaxModeOpen.set(false)
  runMenuOpen.set(false)
  stopMenuOpen.set(false)
  newProjectOpen.set(false)
  newTaskOpen.set(true)
}

export function closeNewTask() {
  newTaskOpen.set(false)
  newTaskDraft.set(null)
}

/** Open the quick-create project modal. */
export function openNewProject(opts?: { resumeNewTask?: boolean }) {
  newProjectResumeNewTask = !!opts?.resumeNewTask
  paletteOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  syntaxModeOpen.set(false)
  runMenuOpen.set(false)
  stopMenuOpen.set(false)
  newTaskOpen.set(false)
  newProjectOpen.set(true)
}

export function closeNewProject() {
  newProjectOpen.set(false)
  newProjectResumeNewTask = false
}

export function takeNewProjectResumeNewTask(): boolean {
  const resume = newProjectResumeNewTask
  newProjectResumeNewTask = false
  return resume
}

export function openPR(id: string) {
  selectedPRId.set(id)
  prDetailTab.set('conv')
  prComposerDraft.set('')
  prReviewDraft.set('')
  closePRReview()
  go('prdetail')
}

// openProject opens project settings. Empty id opens the quick-create modal
// (Welcome / sidebar "+ Add project") instead of the full settings page.
export function openProject(id = '') {
  if (!id) {
    openNewProject()
    return
  }
  projectFormId.set(id)
  go('project')
}

export { parseChatRouteTarget, parseWorkspaceRouteTarget } from './chat-route'

// routeTarget handles inbox/palette targets like "loop:LOOP-2218" / "pr:482" /
// "view:pr" / "chat:workspaceId:PA2" / "workspace:id:plan".
export function routeTarget(target: string) {
  const [kind, ...rest] = target.split(':')
  const ref = rest.join(':')
  if (kind === 'workspace' || kind === 'loop') {
    const parsed = parseWorkspaceRouteTarget(target)
    if (!parsed) return
    openWorkspace(parsed.workspaceId)
    if (parsed.panel) observatoryJump.set({ workspaceId: parsed.workspaceId, panel: parsed.panel })
  }
  else if (kind === 'pr') openPR(ref)
  else if (kind === 'view') go(ref as ViewName)
  else if (kind === 'chat') {
    const parsed = parseChatRouteTarget(target)
    if (parsed) openWorkspaceChat(parsed.workspaceId, parsed.code)
    else go('briefing')
  }
  else if (kind === 'question') {
    // Open the workspace (+ chat tab) that owns this ask_user prompt.
    const q = get(userQuestions).find((x) => x.id === ref)
    if (q?.workspaceId) openWorkspaceChat(q.workspaceId, q.agentCode)
    else go('briefing')
  }
}

// navKey maps a view to the sidebar nav item that should appear active.
// Workspace detail/escalation highlight the tree row (selectedWorkspaceId), not a top nav item.
export function navKey(v: ViewName): string {
  if (v === 'prdetail') return 'pr'
  if (v === 'detail' || v === 'escalation') return ''
  if (v === 'inbox') return 'briefing'
  return v
}

// ---- toasts ----
export type ToastKind = 'success' | 'error' | 'info'
export type ToastAsk = { project: string; workspace: string; question: string }
export type Toast = {
  id: number
  kind: ToastKind
  text: string
  action?: { label: string; fn: () => void }
  /** Structured ask_user notice — richer layout in Toasts.svelte. */
  ask?: ToastAsk
  /** Auto-dismiss duration (drives the ask toast TTL bar). 0 = sticky. */
  ttlMs?: number
  /** If set, a later toast with the same key replaces this one in place. */
  replace?: string
  /** Called when the toast is dismissed except after its action runs. */
  onDismiss?: () => void
}
export const toasts = writable<Toast[]>([])
let toastSeq = 0
const toastTimers = new Map<number, ReturnType<typeof setTimeout>>()
const TOAST_TTL = 3500
const ERROR_TOAST_TTL = 8000
const ASK_TOAST_TTL = 30_000

function armToastTimer(id: number, ttl: number) {
  const prev = toastTimers.get(id)
  if (prev) clearTimeout(prev)
  if (ttl <= 0) {
    toastTimers.delete(id)
    return
  }
  toastTimers.set(id, setTimeout(() => dismissToast(id), ttl))
}

function isToastAction(v: unknown): v is NonNullable<Toast['action']> {
  return !!v && typeof v === 'object' && typeof (v as NonNullable<Toast['action']>).fn === 'function'
}

export function pushToast(
  kind: ToastKind,
  text: string,
  extra?: Toast['action'] | { action?: Toast['action']; ttlMs?: number; replace?: string; onDismiss?: () => void },
  ttlMs?: number,
) {
  let action: Toast['action'] | undefined
  let replace: string | undefined
  let onDismiss: Toast['onDismiss']
  let ttlArg = ttlMs
  if (isToastAction(extra)) {
    action = extra
  } else if (extra && typeof extra === 'object') {
    action = extra.action
    replace = extra.replace
    onDismiss = extra.onDismiss
    ttlArg = extra.ttlMs ?? ttlArg
  }
  const ttl = ttlArg ?? (kind === 'error' ? ERROR_TOAST_TTL : TOAST_TTL)
  if (replace) {
    const existing = get(toasts).find((t) => t.replace === replace)
    if (existing) {
      toasts.update((arr) =>
        arr.map((t) => (t.id === existing.id ? { ...t, kind, text, action, ttlMs: ttl, replace, onDismiss } : t)),
      )
      armToastTimer(existing.id, ttl)
      return
    }
  }
  const id = ++toastSeq
  toasts.update((arr) => [...arr, { id, kind, text, action, ttlMs: ttl, replace, onDismiss }])
  armToastTimer(id, ttl)
}

/** Theme-cycle toast — replaces the previous Editor theme: … notice if still visible. */
export function toastEditorTheme(name: string) {
  pushToast('success', `Editor theme: ${name}`, { replace: 'editor-theme' })
}
/** Compact off-tab ask_user notice (auto-dismiss; click opens the asking chat). */
export function pushAskToast(
  ask: ToastAsk,
  action: NonNullable<Toast['action']>,
  ttlMs = ASK_TOAST_TTL,
) {
  const id = ++toastSeq
  const where = [ask.project, ask.workspace].filter(Boolean).join(' · ')
  const text = ask.question
    ? `Agent has a question · ${where} — ${ask.question}`
    : `Agent has a question · ${where}`
  toasts.update((arr) => [...arr, { id, kind: 'info', text, action, ask, ttlMs }])
  armToastTimer(id, ttlMs)
}
export function dismissToast(id: number, opts?: { invokeOnDismiss?: boolean }) {
  const timer = toastTimers.get(id)
  if (timer) {
    clearTimeout(timer)
    toastTimers.delete(id)
  }
  const gone = get(toasts).find((t) => t.id === id)
  toasts.update((arr) => arr.filter((t) => t.id !== id))
  if (opts?.invokeOnDismiss !== false) gone?.onDismiss?.()
}

// ---- booted (Phase 4.4 loading states) ----
export const booted = writable(false)
