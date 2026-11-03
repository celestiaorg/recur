/**
 * TextMate / VS Code theme loading and CM6 chrome mapping (background,
 * gutter, caret, selection).
 */
import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'

/** VS Code colour-theme JSON (tokenColors) or classic TextMate settings[]. */
export type VsCodeThemeJson = {
  name?: string
  type?: string
  colors?: Record<string, string>
  tokenColors?: Array<{
    name?: string
    scope?: string | string[]
    settings: {
      foreground?: string
      background?: string
      fontStyle?: string
      caret?: string
      selection?: string
      lineHighlight?: string
      invisibles?: string
    }
  }>
  settings?: MutableTmRule[]
  /** Classic TextMate plist `gutterSettings` (optional; Recur also maps VS Code gutter keys). */
  gutterSettings?: {
    background?: string
    foreground?: string
    divider?: string
    selectionBackground?: string
    selectionForeground?: string
  }
  /**
   * TextMate plist `colorSpaceName`. Classic ports are converted Generic RGB →
   * sRGB at port time (`scripts/convert-tmtheme.mjs`) and stamped `sRGB`.
   * Shiki / Recur first-party themes are authored in sRGB and omit this.
   */
  colorSpaceName?: string
}

/** Root-language backgrounds from classic TextMate `source` / `text` rules. */
export type RootScopeBackgrounds = {
  source?: string
  text?: string
}

export type ParsedTmTheme = {
  name: string
  uiTheme: 'dark' | 'light'
  colors: Record<string, string>
  rootBackgrounds: RootScopeBackgrounds
}

type MutableTmRule = {
  name?: string
  scope?: string | string[]
  settings: {
    foreground?: string
    background?: string
    fontStyle?: string
    caret?: string
    selection?: string
    lineHighlight?: string
    invisibles?: string
  }
}

/**
 * Comma-separated or array scopes. A space-separated selector (`text source`)
 * is one compound selector, not two roots.
 */
export function themeScopeSelectors(scope: unknown): string[] {
  if (scope == null || scope === '') return []
  const parts = Array.isArray(scope) ? scope : [scope]
  const out: string[] = []
  for (const p of parts) {
    if (typeof p !== 'string') continue
    for (const sel of p.split(',')) {
      const s = sel.trim()
      if (s) out.push(s)
    }
  }
  return out
}

function isRootLanguageSelector(sel: string): boolean {
  return sel === 'source' || sel === 'text'
}

/** Language-root `source` / `text` only: not `source.ts` or `text source`. */
export function isRootLanguageScope(scope: unknown): boolean {
  const sels = themeScopeSelectors(scope)
  return sels.length > 0 && sels.every(isRootLanguageSelector)
}

function copyThemeSettings(json: VsCodeThemeJson): MutableTmRule[] {
  if (Array.isArray(json.settings) && json.settings.length) {
    return json.settings.map((entry) => ({ ...entry, settings: { ...entry.settings } }))
  }
  const tokenColors = json.tokenColors ?? []
  return tokenColors.map((entry) => ({
    name: entry.name,
    scope: entry.scope,
    settings: { ...(entry.settings ?? {}) },
  }))
}

/** Normalize #RGB / #RRGGBB / #RRGGBBAA for equality (alpha ignored). */
export function cssColorKey(c: string): string {
  const s = c.trim().toLowerCase()
  if (/^#[0-9a-f]{3}$/.test(s)) {
    const r = s[1], g = s[2], b = s[3]
    return `#${r}${r}${g}${g}${b}${b}`
  }
  if (/^#[0-9a-f]{8}$/.test(s)) return s.slice(0, 7)
  if (/^#[0-9a-f]{6}$/.test(s)) return s
  return s
}

/** True when colour is missing, `transparent`, or fully alpha-0 hex. */
export function isCssTransparent(c: string | undefined): boolean {
  if (!c?.trim()) return true
  const s = c.trim().toLowerCase()
  if (s === 'transparent') return true
  if (/^#[0-9a-f]{4}$/.test(s)) return s[3] === '0'
  if (/^#[0-9a-f]{8}$/.test(s)) return s.slice(7) === '00'
  return false
}

/** Last-wins `source` / `text` backgrounds from theme settings (before strip). */
export function extractRootScopeBackgrounds(settings: MutableTmRule[]): RootScopeBackgrounds {
  const out: RootScopeBackgrounds = {}
  for (const entry of settings) {
    if (!isRootLanguageScope(entry.scope)) continue
    const bg = entry.settings?.background
    if (!bg || isCssTransparent(bg)) continue
    for (const sel of themeScopeSelectors(entry.scope)) {
      if (sel === 'source') out.source = bg
      if (sel === 'text') out.text = bg
    }
  }
  return out
}

type Rgb = { r: number; g: number; b: number }

function parseHexRgb(c: string): Rgb | null {
  let s = cssColorKey(c)
  if (!/^#[0-9a-f]{6}$/.test(s)) return null
  return {
    r: parseInt(s.slice(1, 3), 16),
    g: parseInt(s.slice(3, 5), 16),
    b: parseInt(s.slice(5, 7), 16),
  }
}

function hexRgb({ r, g, b }: Rgb): string {
  const h = (n: number) => Math.max(0, Math.min(255, n)).toString(16).padStart(2, '0')
  return `#${h(r)}${h(g)}${h(b)}`
}

/** Rec. 601 luma; TextMate `color_is_dark` in theme.cc. */
export function isCssColorDark(c: string): boolean | null {
  const rgb = parseHexRgb(c)
  if (!rgb) return null
  return 0.30 * (rgb.r / 255) + 0.59 * (rgb.g / 255) + 0.11 * (rgb.b / 255) < 0.5
}

/**
 * TextMate `OakColorCreateCopySoften`: mix a dark colour toward white, or a
 * light colour toward black, by keeping `factor` of the original (0–1).
 */
export function softenCssColor(c: string, factor: number): string | null {
  const rgb = parseHexRgb(c)
  if (!rgb) return null
  const t = Math.max(0, Math.min(1, factor))
  const dark = isCssColorDark(c) === true
  const ch = (n: number) => {
    const x = n / 255
    const y = dark ? 1 - t * (1 - x) : x * t
    return Math.round(y * 255)
  }
  return hexRgb({ r: ch(rgb.r), g: ch(rgb.g), b: ch(rgb.b) })
}

/**
 * Gutter fill: honour opaque `editorGutter.background`; otherwise TextMate’s
 * default (`OakColorCreateCopySoften` of the global editor background, 0.87).
 */
export function resolveGutterBackground(
  colors: Record<string, string>,
  editorBg: string,
): string {
  const explicit = colors['editorGutter.background']
  if (explicit && !isCssTransparent(explicit)) return explicit
  return softenCssColor(editorBg, 0.87) ?? editorBg
}

/**
 * Gutter edge: `gutterSettings.divider` / `editorGroup.border` when set.
 * Classic TextMate themes (no VS Code gutter fill) get a softened-foreground
 * divider. Recur/VS Code themes that already set `editorGutter.background`
 * keep the previous hairline: Charcoal/Paper were designed against that.
 */
export function resolveGutterBorder(
  colors: Record<string, string>,
  editorFg: string,
  uiTheme: 'dark' | 'light',
): string {
  const explicit = colors['gutter.divider'] || colors['editorGroup.border']
  if (explicit && !isCssTransparent(explicit)) return explicit
  const gutterFill = colors['editorGutter.background']
  if (!gutterFill || isCssTransparent(gutterFill)) {
    return softenCssColor(editorFg, 0.4) ?? (uiTheme === 'dark' ? '#282828' : '#cfd3db')
  }
  return uiTheme === 'dark' ? '#282828' : '#cfd3db'
}

/** SCM gutter hint: theme `editorGutter.added/deletedBackground`, else Recur git tokens. */
export function resolveGutterHintColor(
  colors: Record<string, string>,
  key: 'editorGutter.addedBackground' | 'editorGutter.deletedBackground',
  fallback: string,
): string {
  const explicit = colors[key]
  if (explicit && !isCssTransparent(explicit)) return explicit
  return fallback
}

/** Line numbers: theme key, else TextMate default (soften editor foreground by 0.5). */
export function resolveLineNumberForeground(
  colors: Record<string, string>,
  editorFg: string,
): string {
  const explicit = colors['editorLineNumber.foreground']
  if (explicit && !isCssTransparent(explicit)) return explicit
  return softenCssColor(editorFg, 0.5) ?? '#6b7280'
}

function mergeGutterSettings(json: VsCodeThemeJson): Record<string, string> {
  const colors = { ...(json.colors ?? {}) }
  const gs = json.gutterSettings
  if (gs) {
    if (gs.background && !colors['editorGutter.background']) {
      colors['editorGutter.background'] = gs.background
    }
    if (gs.foreground && !colors['editorLineNumber.foreground']) {
      colors['editorLineNumber.foreground'] = gs.foreground
    }
    if (gs.divider && !colors['gutter.divider']) {
      colors['gutter.divider'] = gs.divider
    }
  }
  return colors
}

/**
 * Classic TextMate global (unscoped) settings: `selection`, `lineHighlight`,
 * `caret`, `invisibles`. Lift into VS Code `colors` when those keys are absent.
 */
function liftGlobalChromeSettings(
  json: VsCodeThemeJson,
  colors: Record<string, string>,
): Record<string, string> {
  const global = copyThemeSettings(json).find((s) => !s.scope)?.settings
  if (!global) return colors
  const lift = (from: string | undefined, key: string) => {
    if (from && !colors[key]) colors[key] = from
  }
  lift(global.background, 'editor.background')
  lift(global.foreground, 'editor.foreground')
  lift(global.caret, 'editorCursor.foreground')
  lift(global.selection, 'editor.selectionBackground')
  lift(global.lineHighlight, 'editor.lineHighlightBackground')
  lift(global.invisibles, 'editorWhitespace.foreground')
  return colors
}

function mergeThemeChrome(json: VsCodeThemeJson): Record<string, string> {
  return liftGlobalChromeSettings(json, mergeGutterSettings(json))
}

/** Append an alpha byte to a 6-digit hex colour (`#rrggbb` + `aa`). */
export function withCssAlpha(c: string, alphaByte: number): string | null {
  const key = cssColorKey(c)
  if (!/^#[0-9a-f]{6}$/.test(key)) return null
  const a = Math.max(0, Math.min(255, Math.round(alphaByte))).toString(16).padStart(2, '0')
  return `${key}${a}`
}

/**
 * Selection wash: theme `editor.selectionBackground` / classic `selection`.
 * TextMate has no soften-algo here: authors set `selection` on the unscoped
 * rule. If omitted, tint the foreground at ~30% so the wash still tracks the
 * theme (Recur used to hardcode a purple).
 */
export function resolveSelectionBackground(
  colors: Record<string, string>,
  editorFg: string,
  uiTheme: 'dark' | 'light',
): string {
  const explicit = colors['editor.selectionBackground']
  if (explicit && !isCssTransparent(explicit)) return explicit
  return withCssAlpha(editorFg, 0x4d) ?? (uiTheme === 'dark' ? 'rgba(255,255,255,0.25)' : 'rgba(0,0,0,0.18)')
}

/**
 * Current-line fill: theme `editor.lineHighlightBackground` / `lineHighlight`.
 * If omitted, TextMate-style soften of the editor background (same 0.95 factor
 * as theme.cc gutter selectionBackground).
 */
export function resolveLineHighlightBackground(
  colors: Record<string, string>,
  editorBg: string,
  uiTheme: 'dark' | 'light',
): string {
  const explicit = colors['editor.lineHighlightBackground']
  if (explicit && !isCssTransparent(explicit)) return explicit
  return softenCssColor(editorBg, 0.95)
    ?? (uiTheme === 'dark' ? 'rgba(255,255,255,0.06)' : 'rgba(0,0,0,0.06)')
}

export function parseThemeJson(text: string, fallbackName = 'theme'): ParsedTmTheme {
  const json = JSON.parse(text) as VsCodeThemeJson
  const uiTheme: 'dark' | 'light' =
    json.type === 'light' ? 'light' : 'dark'
  const settings = copyThemeSettings(json)
  const rootBackgrounds = extractRootScopeBackgrounds(settings)
  return {
    name: json.name ?? fallbackName,
    uiTheme,
    colors: mergeThemeChrome(json),
    rootBackgrounds,
  }
}

/** Editor chrome from theme `colors` / global settings: not syntax tokens. */
export function tmChromeExtension(parsed: ParsedTmTheme): Extension {
  const c = parsed.colors
  const bg = c['editor.background'] ?? '#1e1e1e'
  const fg = c['editor.foreground'] ?? '#cccccc'
  const caret = c['editorCursor.foreground'] ?? fg
  const selection = resolveSelectionBackground(c, fg, parsed.uiTheme)
  const lineHL = resolveLineHighlightBackground(c, bg, parsed.uiTheme)
  const dark = parsed.uiTheme === 'dark'
  const gutterBg = resolveGutterBackground(c, bg)
  const gutterBorder = resolveGutterBorder(c, fg, parsed.uiTheme)
  const lineNum = resolveLineNumberForeground(c, fg)
  const sourceBg = parsed.rootBackgrounds?.source || bg
  const textBg = parsed.rootBackgrounds?.text || bg
  return [
    EditorView.darkTheme.of(dark),
    EditorView.theme({
      '&': {
        backgroundColor: bg,
        color: fg,
        '--cm-active-line': lineHL,
        '--cm-tm-source-bg': sourceBg,
        '--cm-tm-text-bg': textBg,
        '--cm-gutter-added': resolveGutterHintColor(c, 'editorGutter.addedBackground', 'var(--git-a)'),
        '--cm-gutter-deleted': resolveGutterHintColor(c, 'editorGutter.deletedBackground', 'var(--git-d)'),
      },
      // Pane fill on the scroller: not `.cm-content`. Content sits above
      // CodeMirror’s selection layer; an opaque content background hides it.
      '.cm-scroller': {
        backgroundColor: 'var(--cm-tm-content-bg, var(--cm-tm-source-bg))',
      },
      '.cm-content': {
        caretColor: caret,
      },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: caret },
      // CM baseTheme uses `.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground`
      // (lavender #d7d4f0 / charcoal #233). A shorter selector never wins.
      '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, & > .cm-scroller > .cm-selectionLayer .cm-selectionBackground': {
        background: selection,
      },
      '.cm-line ::selection, .cm-content ::selection': {
        backgroundColor: 'transparent',
      },
      '.cm-gutters': {
        backgroundColor: gutterBg,
        color: lineNum,
        border: 'none',
        borderRight: `1px solid ${gutterBorder}`,
      },
      '.cm-activeLine, .cm-gutterElement.cm-activeLineGutter': {
        backgroundColor: 'var(--cm-active-line)',
      },
      '.cm-panels': { backgroundColor: 'var(--pane)', color: 'var(--text)', zIndex: '2' },
    }, { dark }),
  ]
}
