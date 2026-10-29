/**
 * TextMate grammar / theme catalog on the UI thread.
 *
 * This module owns the singleton main-thread TmEngine (lazily started so
 * vscode-textmate + Oniguruma stay out of the main chunk) plus path
 * resolution, fold markers, theme chrome, and the colour-map CSS.
 */
import { writable } from 'svelte/store'
import type { IRawTheme } from 'vscode-textmate'
import type { GrammarInfo, ThemeInfo } from '../../bindings/recur/internal/app/models'
import type { GrammarSourceLoader, TmEngine } from './tm-engine'
import {
  SYNTAX_PLAIN,
  _resetSyntaxModeForTests,
  fileTypeAssociationForPath,
  setSyntaxForPath,
  syntaxFileTypeOverride,
  syntaxOverrideForPath,
} from './syntax-mode'
import { colorMapCss, isTextMateTextScope, parseThemeJson, type ParsedTmTheme } from './tm-theme'

export type GrammarMeta = GrammarInfo

type GrammarLoader = () => Promise<string>

let colorMap: string[] = []
let colorMapEpoch = 0
const colorMapSubs = new Set<(epoch: number) => void>()

/** Start/stop regex sources from a TextMate grammar JSON. */
export type TmFoldMarkers = { startSource: string; stopSource: string }

const grammarLoaders = new Map<string, GrammarLoader>()
const grammarMeta = new Map<string, GrammarMeta>()
/** Top-level TM folding markers (null = loaded, none present). */
const foldMarkerSources = new Map<string, TmFoldMarkers | null>()
const themeCache = new Map<string, ParsedTmTheme>()
const themeMeta = new Map<string, ThemeInfo>()

/** Grammar / theme ids the user turned off in Settings → Bundles. */
let disabledGrammarIds = new Set<string>()
let disabledThemeIds = new Set<string>()

function sameIdSet(a: Set<string>, ids: Iterable<string>): boolean {
  const b = [...ids]
  if (a.size !== b.length) return false
  for (const id of b) {
    if (!a.has(id)) return false
  }
  return true
}

/** Sync disabled bundle grammar/theme ids from Settings (no store import). */
export function setDisabledBundleContribs(opts: {
  grammarIds?: Iterable<string>
  themeIds?: Iterable<string>
}): void {
  if (opts.grammarIds) {
    const next = new Set([...opts.grammarIds].filter(Boolean))
    if (!sameIdSet(disabledGrammarIds, next)) {
      disabledGrammarIds = next
      grammarCatalogEpoch.update((n) => n + 1)
    }
  }
  if (opts.themeIds) {
    const next = new Set([...opts.themeIds].filter(Boolean))
    if (!sameIdSet(disabledThemeIds, next)) disabledThemeIds = next
  }
}

export function isGrammarDisabled(id: string): boolean {
  return !!id && disabledGrammarIds.has(id)
}

export function isThemeDisabled(id: string): boolean {
  return !!id && disabledThemeIds.has(id)
}

export function disabledThemeIdSet(): Set<string> {
  return disabledThemeIds
}

/** Extract `foldingStartMarker` / `foldingStopMarker` from grammar file text. */
export function extractFoldMarkerSources(content: string): TmFoldMarkers | null {
  const trimmed = content.trimStart()
  if (!trimmed.startsWith('{')) return null
  try {
    const raw = JSON.parse(content) as {
      foldingStartMarker?: unknown
      foldingStopMarker?: unknown
    }
    if (
      typeof raw.foldingStartMarker === 'string' &&
      typeof raw.foldingStopMarker === 'string' &&
      raw.foldingStartMarker.length > 0 &&
      raw.foldingStopMarker.length > 0
    ) {
      return { startSource: raw.foldingStartMarker, stopSource: raw.foldingStopMarker }
    }
  } catch {
    /* plist / invalid JSON */
  }
  return null
}

function noteFoldMarkers(scopeName: string, content: string): void {
  if (foldMarkerSources.has(scopeName)) return
  foldMarkerSources.set(scopeName, extractFoldMarkerSources(content))
}

/** Markers recorded when the grammar was last loaded (null if none). */
export function getFoldMarkerSources(scopeName: string): TmFoldMarkers | null {
  return foldMarkerSources.get(scopeName) ?? null
}

/** Bumps when grammars register so UI (status bar language label) can recompute. */
export const grammarCatalogEpoch = writable(0)

// --- Main-thread tokenizer engine (lazy) ------------------------------------

let engine: TmEngine | null = null
let engineStart: Promise<TmEngine | null> | null = null

/** Isolated tokenizer for settings script editors (Charcoal/Paper chrome). */
let chromePairedEngine: TmEngine | null = null
let chromePairedStart: Promise<TmEngine | null> | null = null
let chromePairedThemeId: string | null = null

const sourceLoader: GrammarSourceLoader = async (scopeName) => {
  const meta = grammarMeta.get(scopeName)
  if (meta && isGrammarDisabled(meta.id)) return null
  const loader = grammarLoaders.get(scopeName)
  if (!loader) return null
  const content = await loader()
  noteFoldMarkers(scopeName, content)
  const pathHint = grammarMeta.get(scopeName)?.path ?? `${scopeName}.tmLanguage.json`
  return { content, pathHint }
}

async function loadOnigWasm(): Promise<ArrayBuffer> {
  const wasmUrl = (await import('vscode-oniguruma/release/onig.wasm?url')).default
  const resp = await fetch(wasmUrl)
  if (!resp.ok) throw new Error(`failed to fetch onig.wasm (${resp.status})`)
  return resp.arrayBuffer()
}

function attachGrammars(e: TmEngine): void {
  e.setSourceLoader(sourceLoader)
  for (const meta of grammarMeta.values()) {
    e.noteGrammar(meta.scopeName, meta.path || `${meta.scopeName}.tmLanguage.json`, meta.injectTo)
  }
}

async function startEngine(): Promise<TmEngine | null> {
  try {
    const [{ TmEngine: Engine }, wasm] = await Promise.all([
      import('./tm-engine'),
      loadOnigWasm(),
    ])
    const e = new Engine()
    await e.initOnig(wasm)
    attachGrammars(e)
    engine = e
    if (activeRawTheme) applyColorMap(e.setTheme(activeRawTheme), true)
    return e
  } catch (err) {
    console.error('TextMate engine failed to start', err)
    engineStart = null
    return null
  }
}

/** The engine, if it finished starting — sync callers (plugin paint path). */
export function peekTmEngine(): TmEngine | null {
  return engine
}

export function ensureTmEngine(): Promise<TmEngine | null> {
  if (!engineStart) engineStart = startEngine()
  return engineStart
}

/**
 * A second tokenizer themed to the app-chrome-paired editor theme. Settings
 * script editors use this so they can paint a complete Recur theme without
 * calling setActiveTheme (which would restyle the code editor and flush its
 * token cache).
 */
export async function ensureChromePairedTmEngine(themeId: string): Promise<TmEngine | null> {
  const parsed = getParsedTheme(themeId)
  if (!parsed) return null
  // Oniguruma lives on the main engine; starting it first makes mini init a no-op.
  const main = await ensureTmEngine()
  if (!main) return null

  if (!chromePairedEngine) {
    if (!chromePairedStart) {
      chromePairedStart = (async () => {
        const { TmEngine: Engine } = await import('./tm-engine')
        const e = new Engine()
        await e.initOnig(new ArrayBuffer(0))
        attachGrammars(e)
        return e
      })().catch((err) => {
        chromePairedStart = null
        console.error('Chrome-paired TextMate engine failed to start', err)
        return null
      })
    }
    chromePairedEngine = await chromePairedStart
  }
  if (!chromePairedEngine) return null
  if (chromePairedThemeId !== themeId) {
    chromePairedEngine.setTheme(parsed.rawTheme)
    chromePairedThemeId = themeId
  }
  return chromePairedEngine
}

/** Theme id last applied to the chrome-paired engine, or null if none. */
export function getChromePairedThemeId(): string | null {
  return chromePairedThemeId
}

/**
 * Release token caches for closed files. Documents still displayed by a view
 * are kept (the engine ignores those); everything else is freed now instead
 * of waiting for the idle budget to evict it.
 *
 * `force` drops the document even while a view holds it — for callers that
 * know the buffer itself is being discarded (reload from disk). Those must
 * not rely on the openDoc fingerprint to notice the new text: it samples only
 * part of the file, so a same-length edit on an unsampled line looks
 * identical. A live plugin re-opens from its current text on the next paint.
 */
export function tmForgetDocs(docIds: Iterable<string>, force = false): void {
  if (!engine) return
  for (const id of docIds) {
    if (force) engine.closeDoc(id)
    else engine.closeIfIdle(id)
  }
}

/** Register a grammar loader for a scope (overwrites prior). */
export function addGrammar(meta: GrammarMeta, loader: GrammarLoader): void {
  grammarLoaders.set(meta.scopeName, loader)
  grammarMeta.set(meta.scopeName, meta)
  foldMarkerSources.delete(meta.scopeName)
  const pathHint = meta.path || `${meta.scopeName}.tmLanguage.json`
  engine?.noteGrammar(meta.scopeName, pathHint, meta.injectTo)
  chromePairedEngine?.noteGrammar(meta.scopeName, pathHint, meta.injectTo)
  grammarCatalogEpoch.update((n) => n + 1)
}

export function listGrammarMeta(): GrammarMeta[] {
  return Array.from(grammarMeta.values())
}

export function getGrammarMeta(scopeName: string): GrammarMeta | undefined {
  return grammarMeta.get(scopeName)
}

/** Load + compile a grammar in the engine. True when the scope is ready. */
export async function loadGrammar(scopeName: string): Promise<boolean> {
  const e = await ensureTmEngine()
  return e ? e.ensureGrammar(scopeName) : false
}

/** Sync: grammar already compiled and ready for same-turn tokenization. */
export function hasGrammar(scopeName: string): boolean {
  return engine?.hasGrammar(scopeName) ?? false
}

let activeRawTheme: IRawTheme | null = null

export function setActiveTheme(raw: IRawTheme): void {
  // Re-applying the active theme must be a no-op. editorThemeExtension() runs
  // on every editor mount/reconfigure (twice per tab switch), and a redundant
  // setTheme wiped every document's token cache and unpainted open editors —
  // that was the tab-switch flash. Parsed themes are cached per id, so
  // reference identity is a stable "same theme" check.
  if (raw === activeRawTheme) return
  activeRawTheme = raw
  if (engine) {
    // Synchronous on purpose: cache flush, colour-map CSS, and the epoch bump
    // land in the same turn as the caller's transaction, so visible viewports
    // re-tokenize before the browser paints (VS Code's todo_resetTokenization
    // ends the same way). No async hop, no flash window.
    applyColorMap(engine.setTheme(raw), true)
  } else {
    void ensureTmEngine() // startEngine applies the pending theme
  }
}

export function getColorMap(): string[] {
  return colorMap
}

export function getColorMapEpoch(): number {
  return colorMapEpoch
}

export function onColorMapChange(fn: (epoch: number) => void): () => void {
  colorMapSubs.add(fn)
  return () => {
    colorMapSubs.delete(fn)
  }
}

function sameColorMap(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

/**
 * Publish a colour map.
 *
 * `reset` must be true when the engine dropped its documents for this change.
 * Two themes can resolve to an identical colour map (variants differing only
 * in workbench colours), and without it the short-circuit below would skip the
 * epoch bump — leaving every plugin believing nothing happened while its
 * engine document no longer exists, so the editors would go unhighlighted
 * until reopened.
 */
export function applyColorMap(next: string[], reset = false): void {
  // Identical map and nothing was discarded: token colour indices stay valid,
  // so bumping the epoch would repaint all open editors for nothing.
  if (!reset && sameColorMap(colorMap, next)) {
    refreshTmColorMapStyle()
    return
  }
  colorMap = next
  colorMapEpoch++
  refreshTmColorMapStyle()
  for (const fn of colorMapSubs) fn(colorMapEpoch)
}

const styleElId = 'recur-tm-colormap'

/** Refresh colour-map CSS after the worker applies a theme. */
export function refreshTmColorMapStyle(): void {
  if (typeof document === 'undefined') return
  const css = colorMapCss(getColorMap())
  let el = document.getElementById(styleElId) as HTMLStyleElement | null
  if (!el) {
    el = document.createElement('style')
    el.id = styleElId
    document.head.appendChild(el)
  }
  if (el.textContent !== css) el.textContent = css
}

/** Editor chrome background for the active TM theme (skip painting matching token bgs). */
let activeEditorBackground = '#1e1e1e'
let activeRootFills: { source?: string; text?: string } = {}

export function setActiveEditorBackground(color: string): void {
  if (color?.trim()) activeEditorBackground = color.trim()
}

export function getActiveEditorBackground(): string {
  return activeEditorBackground
}

export function setActiveRootBackgrounds(colors: { source?: string; text?: string } | undefined): void {
  activeRootFills = {
    source: colors?.source?.trim() || undefined,
    text: colors?.text?.trim() || undefined,
  }
}

export function getActiveRootBackgrounds(): string[] {
  const next: string[] = []
  if (activeRootFills.source) next.push(activeRootFills.source)
  if (activeRootFills.text) next.push(activeRootFills.text)
  return next
}

/** Pane fill the tokens actually sit on (`source` vs `text` root, else editor). */
export function getActivePaneBackground(scopeName: string): string {
  if (isTextMateTextScope(scopeName) && activeRootFills.text) return activeRootFills.text
  if (!isTextMateTextScope(scopeName) && activeRootFills.source) return activeRootFills.source
  return activeEditorBackground
}

export function registerThemeMeta(info: ThemeInfo, parsed: ParsedTmTheme): void {
  themeMeta.set(info.id, info)
  themeCache.set(info.id, parsed)
}

export function getThemeMeta(id: string): ThemeInfo | undefined {
  return themeMeta.get(id)
}

export function getParsedTheme(id: string): ParsedTmTheme | undefined {
  return themeCache.get(id)
}

export function listThemeMeta(): ThemeInfo[] {
  return Array.from(themeMeta.values())
}

export async function loadAndRegisterTheme(
  info: ThemeInfo,
  read: () => Promise<string>,
): Promise<ParsedTmTheme> {
  const cached = themeCache.get(info.id)
  if (cached) return cached
  const text = await read()
  const parsed = parseThemeJson(text, info.name)
  if (info.uiTheme === 'light' || info.uiTheme === 'dark') {
    parsed.uiTheme = info.uiTheme
  }
  registerThemeMeta(info, parsed)
  return parsed
}

/**
 * Grammar claimed by fileTypes / firstLineMatch — ignores user language-mode
 * overrides. Used to offer Auto Detect and to skip redundant path overrides.
 */
export function detectScopeForPath(path: string, firstLine?: string): string | null {
  const base = path.split(/[/\\]/).pop() ?? path
  const lower = base.toLowerCase()
  const ext = lower.includes('.') ? lower.slice(lower.lastIndexOf('.') + 1) : ''

  // .env / .env.* → dotenv when registered (ext alone is "local"/"example"/…).
  if (lower === '.env' || lower.startsWith('.env.')) {
    for (const g of grammarMeta.values()) {
      if (isGrammarDisabled(g.id)) continue
      if (
        g.language === 'dotenv' ||
        g.scopeName === 'source.dotenv' ||
        (g.fileTypes ?? []).some((ft) => ft.toLowerCase() === 'env')
      ) {
        return g.scopeName
      }
    }
  }

  // Prefer exact basename > extension > suffix; break ties by longer needle.
  let best: { scope: string; score: number } | null = null
  for (const g of grammarMeta.values()) {
    if (isGrammarDisabled(g.id)) continue
    for (const ft of g.fileTypes ?? []) {
      const needle = ft.toLowerCase().replace(/^\./, '')
      let score = 0
      if (needle === lower) score = 1000 + needle.length
      else if (ext && needle === ext) score = 100 + needle.length
      else if (needle.includes('.') && lower.endsWith(`.${needle}`)) score = 200 + needle.length
      else if (lower.endsWith(`.${needle}`)) score = 50 + needle.length
      else continue
      if (!best || score > best.score) best = { scope: g.scopeName, score }
    }
  }
  if (best) return best.scope

  if (firstLine) {
    for (const g of grammarMeta.values()) {
      if (isGrammarDisabled(g.id)) continue
      if (!g.firstLineMatch) continue
      try {
        if (new RegExp(g.firstLineMatch).test(firstLine)) return g.scopeName
      } catch {
        // ignore bad firstLineMatch
      }
    }
  }
  return null
}

/**
 * Resolve a TextMate scope for a file path: user override (this file / file
 * type) first, then registered grammar fileTypes / firstLineMatch. Returns
 * null when nothing claims the path (Plain Text).
 */
export function resolveScopeForPath(path: string, firstLine?: string): string | null {
  const override = syntaxOverrideForPath(path)
  if (override !== undefined) return override || null
  return detectScopeForPath(path, firstLine)
}

/** Awkward / acronym display names for the status bar and language picker. */
const LANGUAGE_DISPLAY_ALIASES: Record<string, string> = {
  cpp: 'C++',
  csharp: 'C#',
  fsharp: 'F#',
  json: 'JSON',
  jsonc: 'JSONC',
  json5: 'JSON5',
  jsonl: 'JSONL',
  yaml: 'YAML',
  html: 'HTML',
  css: 'CSS',
  sql: 'SQL',
  xml: 'XML',
  svg: 'SVG',
  php: 'PHP',
  toml: 'TOML',
  jsx: 'JSX',
  tsx: 'TSX',
  'objective-c': 'Objective-C',
  'objective-cpp': 'Objective-C++',
  shellscript: 'Shell',
  wasm: 'WebAssembly',
}

export function languageDisplayName(id: string): string {
  const key = id.trim()
  if (!key) return ''
  const alias = LANGUAGE_DISPLAY_ALIASES[key.toLowerCase()]
  if (alias) return alias
  return key.split(/[-_]/).map((part) => (
    part ? part[0].toUpperCase() + part.slice(1) : part
  )).join(' ')
}

/** Human-facing label for a TextMate scope (`[[grammar]].language`, else scope). */
export function languageLabelForScope(scope: string): string {
  const meta = getGrammarMeta(scope)
  const lang = meta?.language?.trim()
  if (lang) return languageDisplayName(lang)
  const m = /^(?:source|text)\.([a-z0-9_+-]+)/i.exec(scope)
  if (m) return languageDisplayName(m[1].replace(/_/g, '-'))
  return scope
}

/**
 * Human-facing language label for the editor status bar.
 * Uses the TextMate grammar that claims the path (`[[grammar]].language`,
 * else a short form of the scope name). Empty when no grammar matches.
 */
export function languageLabelForPath(path: string): string {
  const scope = resolveScopeForPath(path)
  return scope ? languageLabelForScope(scope) : ''
}

export type SyntaxLanguage = { scopeName: string; label: string; language: string }

/** Grammars the language-mode picker can apply as a root highlighter. */
export function listSyntaxLanguages(): SyntaxLanguage[] {
  const out: SyntaxLanguage[] = []
  const seen = new Set<string>()
  for (const g of listGrammarMeta()) {
    if (seen.has(g.scopeName)) continue
    if (g.injectTo?.length) continue
    seen.add(g.scopeName)
    out.push({
      scopeName: g.scopeName,
      label: languageLabelForScope(g.scopeName),
      language: g.language?.trim() ?? '',
    })
  }
  out.sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }))
  return out
}

/**
 * Apply a language-mode pick to `path`. `null` is Auto Detect (grammar
 * fileTypes, not a `*.ext` association); `''` is Plain Text. Picking the
 * scope that already applies without a per-path override just clears that
 * override.
 */
export function applyLanguageMode(path: string, scope: string | null) {
  const assoc = fileTypeAssociationForPath(path)
  const assocScope = assoc ? syntaxFileTypeOverride(assoc.key) : undefined
  const detected = detectScopeForPath(path)
  // What resolveScopeForPath would return with no per-path override.
  const withoutPath = assocScope !== undefined ? (assocScope || null) : detected

  if (scope == null) {
    // Auto Detect = grammar fileTypes, not a *.ext association.
    if (assocScope !== undefined && (detected || null) !== withoutPath) {
      setSyntaxForPath(path, detected ?? SYNTAX_PLAIN)
    } else {
      setSyntaxForPath(path, null)
    }
    return
  }
  if ((scope === '' && withoutPath == null) || (scope !== '' && scope === withoutPath)) {
    setSyntaxForPath(path, null)
    return
  }
  setSyntaxForPath(path, scope)
}

/** Drop bundle grammars and themes so a reload registers only what is on disk now. */
export function forgetBundleContribs(): void {
  grammarLoaders.clear()
  grammarMeta.clear()
  foldMarkerSources.clear()
  themeCache.clear()
  themeMeta.clear()
  chromePairedThemeId = null
}

/** Reset registry state (tests). */
export function _resetTmRegistryForTests(): void {
  colorMap = []
  colorMapEpoch = 0
  activeRawTheme = null
  colorMapSubs.clear()
  grammarLoaders.clear()
  grammarMeta.clear()
  foldMarkerSources.clear()
  themeCache.clear()
  themeMeta.clear()
  disabledGrammarIds = new Set()
  disabledThemeIds = new Set()
  grammarCatalogEpoch.set(0)
  activeEditorBackground = '#1e1e1e'
  activeRootFills = {}
  engine = null
  engineStart = null
  chromePairedEngine = null
  chromePairedStart = null
  chromePairedThemeId = null
  _resetSyntaxModeForTests()
}

/** Inject a ready engine (tests — skips wasm fetch). */
export function _setTmEngineForTests(e: TmEngine | null): void {
  engine = e
  engineStart = e ? Promise.resolve(e) : null
}
