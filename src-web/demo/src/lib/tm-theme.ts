/**
 * TextMate / VS Code theme loading and CM6 chrome mapping.
 * Syntax colors are applied via vscode-textmate Registry.setTheme + tokenizeLine2
 * (full scope fidelity — no CodeMirror/Lezer token conversion).
 */
import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import type { IRawTheme } from 'vscode-textmate'

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
  settings?: IRawTheme['settings']
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
  rawTheme: IRawTheme
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
 * vscode-textmate hard-codes default token background to #ffffff when the
 * global (scopeless) theme rule omits `background`. Dark themes that only set
 * foreground then paint white boxes behind every token once we honour bg bits.
 */
export function ensureGlobalThemeChrome(
  settings: MutableTmRule[],
  colors?: Record<string, string>,
): void {
  const fg = colors?.['editor.foreground'] ?? '#cccccc'
  const bg = colors?.['editor.background'] ?? '#1e1e1e'
  const global = settings.find((s) => !s.scope)
  if (!global) {
    settings.unshift({ settings: { foreground: fg, background: bg } })
    return
  }
  const gs = global.settings ?? {}
  global.settings = {
    ...gs,
    foreground: gs.foreground || fg,
    background: gs.background || bg,
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

/** Language-root `source` / `text` only — not `source.ts` or `text source`. */
export function isRootLanguageScope(scope: unknown): boolean {
  const sels = themeScopeSelectors(scope)
  return sels.length > 0 && sels.every(isRootLanguageSelector)
}

/**
 * Classic TextMate themes shade the whole document via `scope: source` / `text`.
 * Span highlighters must not paint those as glyph-sized boxes.
 */
export function stripRootScopeTokenBackgrounds(settings: MutableTmRule[]): void {
  for (const entry of settings) {
    if (!isRootLanguageScope(entry.scope) || !entry.settings?.background) continue
    const next = { ...entry.settings }
    delete next.background
    entry.settings = next
  }
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

/** Convert VS Code theme JSON into vscode-textmate IRawTheme.settings. */
export function toRawTheme(json: VsCodeThemeJson, fallbackName = 'theme'): IRawTheme {
  const settings = copyThemeSettings(json)
  ensureGlobalThemeChrome(settings, json.colors)
  stripRootScopeTokenBackgrounds(settings)
  return { name: json.name ?? fallbackName, settings }
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

/** 0–1 opacity from hex; omitted alpha is opaque. Non-hex returns 1. */
export function cssColorAlpha(c: string): number {
  const s = c.trim().toLowerCase()
  if (/^#[0-9a-f]{4}$/.test(s)) return parseInt(s[3], 16) / 15
  if (/^#[0-9a-f]{8}$/.test(s)) return parseInt(s.slice(7, 9), 16) / 255
  return 1
}

/**
 * Skip near-transparent theme washes. On glyph-sized spans they read as patchy
 * boxes (e.g. Made of Code `meta.embedded` `#B1B3BA08` on Svelte).
 */
export const TOKEN_BG_MIN_ALPHA = 32 / 255

/** Whether a token background should be painted (vs document / editor fill). */
export function shouldPaintTokenBackground(
  color: string | undefined,
  documentBgs: string[],
): boolean {
  if (!color || isCssTransparent(color)) return false
  if (cssColorAlpha(color) < TOKEN_BG_MIN_ALPHA) return false
  const key = cssColorKey(color)
  for (const bg of documentBgs) {
    if (bg && cssColorKey(bg) === key) return false
  }
  return true
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

/** Blend `fg` over `bg` using `fg`’s alpha (opaque `fg` returned as 6-digit hex). */
export function compositeCssColor(fg: string, bg: string): string {
  const top = parseHexRgb(fg)
  const bot = parseHexRgb(bg)
  if (!top || !bot) return fg
  const a = cssColorAlpha(fg)
  if (a >= 1) return cssColorKey(fg)
  return hexRgb({
    r: Math.round(a * top.r + (1 - a) * bot.r),
    g: Math.round(a * top.g + (1 - a) * bot.g),
    b: Math.round(a * top.b + (1 - a) * bot.b),
  })
}

function srgbLuma(c: Rgb): number {
  const lin = (n: number) => {
    const x = n / 255
    return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/** WCAG contrast ratio of two CSS colours. `fg` alpha is composited onto `bg`. */
export function contrastRatio(fg: string, bg: string): number | null {
  const a = parseHexRgb(compositeCssColor(fg, bg))
  const b = parseHexRgb(bg)
  if (!a || !b) return null
  const l1 = srgbLuma(a)
  const l2 = srgbLuma(b)
  const hi = Math.max(l1, l2)
  const lo = Math.min(l1, l2)
  return (hi + 0.05) / (lo + 0.05)
}

/**
 * Skip token foregrounds that vanish into the editor (Brilliance Black
 * `punctuation.definition` `#00000080` on `#0D0D0D`). TextMate does the same;
 * Recur falls back to the editor foreground instead.
 */
export const TOKEN_FG_MIN_CONTRAST = 1.6

export function tokenForegroundReadable(fg: string | undefined, bg: string | undefined): boolean {
  if (!fg || !bg) return true
  const c = contrastRatio(fg, bg)
  if (c == null) return true
  return c >= TOKEN_FG_MIN_CONTRAST
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
 * keep the previous hairline — Charcoal/Paper were designed against that.
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
 * TextMate has no soften-algo here — authors set `selection` on the unscoped
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
    rawTheme: toRawTheme(json, fallbackName),
    colors: mergeThemeChrome(json),
    rootBackgrounds,
  }
}

/** Editor chrome from theme `colors` / global settings — not syntax tokens. */
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
      // Pane fill on the scroller — not `.cm-content`. Content sits above
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

export function isTextMateTextScope(scopeName: string): boolean {
  return scopeName === 'text' || scopeName.startsWith('text.')
}

/**
 * Pick the TextMate root-scope fill (`source` vs `text`) for this grammar.
 * Colours live on chrome CSS variables so theme switches stay live.
 */
export function tmRootContentExtension(scopeName: string): Extension {
  const varName = isTextMateTextScope(scopeName) ? '--cm-tm-text-bg' : '--cm-tm-source-bg'
  return EditorView.theme({
    '&': { '--cm-tm-content-bg': `var(${varName})` },
  })
}

/** Bit layout for tokenizeLine2 metadata (vscode-textmate / VS Code). */
export const TM_META = {
  FONT_STYLE_MASK: 0b00000000000000000111100000000000,
  FONT_STYLE_OFFSET: 11,
  FOREGROUND_MASK: 0b00000000111111111000000000000000,
  FOREGROUND_OFFSET: 15,
  BACKGROUND_MASK: 0b11111111000000000000000000000000,
  BACKGROUND_OFFSET: 24,
} as const

export const TM_FONT = {
  Italic: 1,
  Bold: 2,
  Underline: 4,
  Strikethrough: 8,
} as const

export function decodeForeground(meta: number): number {
  return (meta & TM_META.FOREGROUND_MASK) >>> TM_META.FOREGROUND_OFFSET
}

/** Colour-map index for token background; 0 means “no token background”. */
export function decodeBackground(meta: number): number {
  return (meta & TM_META.BACKGROUND_MASK) >>> TM_META.BACKGROUND_OFFSET
}

export function decodeFontStyle(meta: number): number {
  return (meta & TM_META.FONT_STYLE_MASK) >>> TM_META.FONT_STYLE_OFFSET
}

/**
 * Build CSS for colour-map classes used by the TM highlighter.
 * Foreground: `.cm-tm-<i>`; background: `.cm-tm-bg-<i>` (skip index 0 — “none”).
 */
export function colorMapCss(colorMap: string[], prefix = 'cm-tm'): string {
  const lines: string[] = []
  for (let i = 0; i < colorMap.length; i++) {
    const color = colorMap[i]
    if (!color) continue
    lines.push(`.${prefix}-${i} { color: ${color}; }`)
    if (i > 0) {
      lines.push(
        `.${prefix}-bg-${i} { background-color: ${color}; border-radius: 2px; padding: 0.05em 0.12em; margin: 0 -0.12em; }`,
      )
    }
  }
  lines.push(`.${prefix}-italic { font-style: italic; }`)
  lines.push(`.${prefix}-bold { font-weight: bold; }`)
  lines.push(`.${prefix}-underline { text-decoration: underline; }`)
  lines.push(`.${prefix}-strike { text-decoration: line-through; }`)
  lines.push(
    `.${prefix}-underline.${prefix}-strike { text-decoration: underline line-through; }`,
  )
  return lines.join('\n')
}
