// Recur command registry: discovered bundle commands (typescript + shebang).
// Text transforms ship as typescript under bundles/Text — not hardcoded here.
import { BundleService } from '../../bindings/recur/internal/app'
import type { CommandInfo, RunContextDTO, RunResultDTO } from '../../bindings/recur/internal/app'
import { languageIdForPath, type EditorLangId } from './cm-lang'
import { resolveScopeForPath } from './tm-registry'
import { get, writable } from 'svelte/store'
import { editorPrefs, resolveEditorIndent, settings } from './stores'
import { runMacroModule, validateOutputRouting, type MacroContext, type OutputRouting } from './macro-worker'
import { effectiveKeyEquivalent, formatKeyEquivalentDisplay } from './keyequiv'

export type { OutputRouting }

export type CommandPrompt = {
  name: string
  label: string
  type: string
  default?: string
  choices?: string[]
}

export type EditorContext = {
  selectedText: string
  document: string
  currentLine: string
  lineNumber: number
  column: number
  currentWord: string
  scope: string
  tabSize: number
  softTabs: boolean
  path: string
  directory?: string
  projectDirectory?: string
  workDir?: string
  clipboard?: string
  resolvedInputMode?: string
  /** Shebang / Worker wall-clock limit (ms). Omit → backend/Worker default. */
  timeoutMs?: number
}

export type RegisteredCommand = {
  id: string
  label: string
  category: string
  order: number
  scope?: string
  fileGlob?: string
  triggers: string[]
  prompts: CommandPrompt[]
  input: string
  fallbackInput: string
  output: OutputRouting
  interpreter: 'typescript' | 'shebang' | string
  source: 'bundle'
  /** Bundle display name from the manifest (Settings subtitle). */
  bundleName?: string
  /** Authoring string from bundle.toml (may be empty / invalid / reserved). */
  keyEquivalentRaw?: string
  /** Canonical chord when valid and not app-reserved; else empty. */
  keyEquivalent?: string
  /** Glyph form for UI (⇧⌘U). */
  keyEquivalentDisplay?: string
  /** Scratch-tab language/extension for newDocument (e.g. diff, json, html). */
  language?: string
  /** Open newDocument scratches on the Preview chip when supported. */
  openInPreview?: boolean
}

/** TextMate-style grammar scope for a file path. */
export function scopeForPath(path: string): string {
  const fromGrammar = resolveScopeForPath(path)
  if (fromGrammar) return fromGrammar
  // .tsx/.jsx share EditorLangId with .ts/.js; don't collapse to source.ts/js.
  const lower = path.toLowerCase()
  if (lower.endsWith('.tsx')) return 'source.tsx'
  if (lower.endsWith('.jsx')) return 'source.js.jsx'
  return scopeForLang(languageIdForPath(path))
}

export function scopeForLang(id: EditorLangId): string {
  const map: Partial<Record<EditorLangId, string>> = {
    go: 'source.go',
    python: 'source.python',
    javascript: 'source.js',
    typescript: 'source.ts',
    json: 'source.json',
    yaml: 'source.yaml',
    markdown: 'text.html.markdown',
    rust: 'source.rust',
    html: 'text.html.basic',
    css: 'source.css',
    shell: 'source.shell',
    svelte: 'source.svelte',
    vue: 'source.vue',
    java: 'source.java',
    c: 'source.c',
    cpp: 'source.cpp',
    ruby: 'source.ruby',
    php: 'source.php',
    sql: 'source.sql',
    toml: 'source.toml',
  }
  return map[id] ?? 'text.plain'
}

/** One `{a,b,c}` brace group → alternatives (matches Go bundles.expandBraces). */
function expandBraces(pattern: string): string[] {
  const start = pattern.indexOf('{')
  const end = pattern.lastIndexOf('}')
  if (start < 0 || end <= start) return [pattern]
  const prefix = pattern.slice(0, start)
  const suffix = pattern.slice(end + 1)
  const out: string[] = []
  for (const part of pattern.slice(start + 1, end).split(',')) {
    const p = part.trim()
    if (p) out.push(prefix + p + suffix)
  }
  return out.length ? out : [pattern]
}

function globMatch(pattern: string, filePath: string): boolean {
  pattern = pattern.trim()
  if (!pattern) return true
  const alts = expandBraces(pattern)
  if (alts.length > 1) return alts.some((p) => globMatch(p, filePath))
  const base = filePath.split('/').pop() ?? filePath
  const toRe = (p: string) => {
    // Escape regex metas; treat ? as one non-slash char (filepath.Match); * / ** as globs.
    const s = p
      .replace(/[.+^${}()|[\]\\]/g, '\\$&')
      .replace(/\?/g, '[^/]')
      .replace(/\*\*/g, '::DS::')
      .replace(/\*/g, '[^/]*')
      .replace(/::DS::/g, '.*')
    return new RegExp(`^${s}$`)
  }
  if (toRe(pattern).test(base) || toRe(pattern).test(filePath)) return true
  if (pattern.startsWith('**/')) {
    const suf = pattern.slice(3)
    if (toRe(suf).test(base)) return true
    const parts = filePath.split('/')
    for (let i = 0; i < parts.length; i++) {
      if (toRe(suf).test(parts.slice(i).join('/'))) return true
    }
  }
  return false
}

function scopePrefixMatch(want: string, have: string): boolean {
  if (!want) return true
  if (!have) return false
  for (const alt of want.split(',').map((s) => s.trim()).filter(Boolean)) {
    if (have === alt || have.startsWith(alt + '.')) return true
  }
  return false
}

export function commandMatches(cmd: Pick<RegisteredCommand, 'scope' | 'fileGlob'>, scope: string, path: string): boolean {
  if (!cmd.scope && !cmd.fileGlob) return true
  if (cmd.scope && !scopePrefixMatch(cmd.scope, scope)) return false
  if (cmd.fileGlob && !globMatch(cmd.fileGlob, path)) return false
  return true
}

function fromBundleInfo(c: CommandInfo): RegisteredCommand {
  const raw = c.keyEquivalent || ''
  const keyEquivalent = effectiveKeyEquivalent(raw)
  return {
    id: c.id,
    label: c.name,
    category: c.category || 'Commands',
    order: c.order ?? 0,
    scope: c.scope || undefined,
    fileGlob: c.fileGlob || undefined,
    triggers: c.triggers?.length ? [...c.triggers] : ['palette'],
    prompts: (c.prompts ?? []).map((p) => ({
      name: p.name,
      label: p.label || p.name,
      type: p.type || 'text',
      default: p.default,
      choices: p.choices,
    })),
    input: c.input || 'selection',
    fallbackInput: c.fallbackInput || '',
    output: (c.output || 'replaceSelection') as OutputRouting,
    interpreter: c.interpreter || 'shebang',
    source: 'bundle',
    bundleName: c.bundleName?.trim() || undefined,
    keyEquivalentRaw: raw || undefined,
    keyEquivalent: keyEquivalent || undefined,
    keyEquivalentDisplay: keyEquivalent ? formatKeyEquivalentDisplay(keyEquivalent) : undefined,
    language: c.language?.trim() || undefined,
    openInPreview: !!c.openInPreview,
  }
}

/** UUIDs the user disabled in Settings → Bundles (domain.Settings). */
export function disabledCommandIds(): Set<string> {
  const list = get(settings)?.disabledBundleCommands
  if (!list?.length) return new Set()
  return new Set(list.filter(Boolean))
}

function filterEnabled(cmds: RegisteredCommand[]): RegisteredCommand[] {
  const disabled = disabledCommandIds()
  if (disabled.size === 0) return cmds
  return cmds.filter((c) => !disabled.has(c.id))
}

function sortCommands(cmds: RegisteredCommand[]): RegisteredCommand[] {
  return [...cmds].sort((a, b) => {
    if (a.order !== b.order) return a.order - b.order
    if (a.category !== b.category) return a.category.localeCompare(b.category)
    return a.label.localeCompare(b.label)
  })
}

let bundleCache: { key: string; cmds: RegisteredCommand[]; at: number } | null = null
const CACHE_MS = 2000

async function loadBundleCommands(scope: string, path: string): Promise<RegisteredCommand[]> {
  const key = `${scope}\0${path}`
  if (bundleCache && bundleCache.key === key && Date.now() - bundleCache.at < CACHE_MS) {
    return bundleCache.cmds
  }
  try {
    const list = await BundleService.ListCommands(scope, path)
    const cmds = (list ?? []).map(fromBundleInfo).filter((c) => commandMatches(c, scope, path))
    bundleCache = { key, cmds, at: Date.now() }
    return cmds
  } catch {
    return []
  }
}

/** Invalidate cached bundle discovery (e.g. after install). */
export function invalidateCommandCache() {
  bundleCache = null
}

/** Bumps after Reload Bundles so views holding derived bundle state rebuild. */
export const bundlesEpoch = writable(0)

export async function listCommands(scope: string, path: string): Promise<RegisteredCommand[]> {
  const cmds = await loadBundleCommands(scope, path)
  return sortCommands(filterEnabled(cmds))
}

/** All discovered commands (no scope/path filter). Used by the shortcuts cheatsheet. */
export async function listAllCommands(): Promise<RegisteredCommand[]> {
  return filterEnabled(await listAllCommandsUnfiltered())
}

/**
 * Every discovered command including those disabled in Settings.
 * Settings → Bundles uses this so toggles remain visible.
 */
export async function listAllCommandsUnfiltered(): Promise<RegisteredCommand[]> {
  try {
    const list = await BundleService.ListCommands('', '')
    return sortCommands((list ?? []).map(fromBundleInfo))
  } catch {
    return []
  }
}

/** Palette commands that have an effective (non-reserved) keyEquivalent. */
export async function listBoundCommands(): Promise<RegisteredCommand[]> {
  const all = await listAllCommands()
  return all.filter((c) => c.triggers.includes('palette') && !!c.keyEquivalentDisplay)
}

export async function categories(scope: string, path: string): Promise<string[]> {
  const cmds = await listCommands(scope, path)
  const meta = new Map<string, number>()
  for (const c of cmds) {
    const prev = meta.get(c.category)
    if (prev === undefined || c.order < prev) meta.set(c.category, c.order)
  }
  return [...meta.entries()]
    .sort((a, b) => (a[1] !== b[1] ? a[1] - b[1] : a[0].localeCompare(b[0])))
    .map(([name]) => name)
}

export async function commandsForTrigger(trigger: string, scope: string, path: string): Promise<RegisteredCommand[]> {
  const cmds = await listCommands(scope, path)
  return cmds.filter((c) => c.triggers.includes(trigger))
}

/** Palette-triggered commands bound to a canonical chord for this file. */
export async function commandsForKeyEquivalent(
  canonical: string,
  scope: string,
  path: string,
): Promise<RegisteredCommand[]> {
  if (!canonical) return []
  const cmds = await listCommands(scope, path)
  return cmds.filter(
    (c) => c.triggers.includes('palette') && c.keyEquivalent === canonical,
  )
}

export function resolveInputText(cmd: RegisteredCommand, ctx: EditorContext): { text: string; mode: string } {
  const pick = (mode: string) => {
    switch (mode) {
      case 'none':
        return ''
      case 'selection':
        return ctx.selectedText
      case 'document':
        return ctx.document
      case 'line':
        return ctx.currentLine
      case 'word':
        return ctx.currentWord
      default:
        return ctx.selectedText
    }
  }
  let mode = cmd.input || 'selection'
  let v = pick(mode)
  if (!v && cmd.fallbackInput) {
    mode = cmd.fallbackInput
    v = pick(mode)
  }
  return { text: v, mode }
}

export type RunCommandResult = {
  stdout: string
  stderr: string
  exitCode: number
  output: OutputRouting
  inputMode?: string
  /** Effective language for newDocument (macro override or command default). */
  language?: string
  /** Effective openInPreview (macro override or command default). */
  openInPreview?: boolean
}

/** Sanitize a language/extension token for scratch:// paths. */
export function scratchLanguage(raw?: string): string {
  const s = (raw ?? '').trim().toLowerCase().replace(/^\./, '')
  if (!s || !/^[a-z0-9][a-z0-9_-]{0,15}$/.test(s)) return 'txt'
  return s
}

export async function runCommand(
  id: string,
  ctx: EditorContext,
  inputs?: Record<string, string>,
): Promise<RunCommandResult> {
  const all = await listCommands(ctx.scope, ctx.path)
  const cmd = all.find((c) => c.id === id)
  if (!cmd) throw new Error(`Unknown command ${id}`)

  const resolved = resolveInputText(cmd, ctx)
  const input = resolved.text
  const prefs = get(editorPrefs)
  const indent = resolveEditorIndent(prefs, languageIdForPath(ctx.path))
  const softTabs = indent.startsWith('spaces')
  const tabSize = indent === 'spaces-2' ? 2 : indent === 'spaces-4' ? 4 : prefs.tabSize || 4

  if (cmd.interpreter === 'typescript') {
    const code = await BundleService.CompiledModule(id)
    const macroCtx: MacroContext = {
      input,
      selectedText: ctx.selectedText,
      document: ctx.document,
      currentLine: ctx.currentLine,
      lineNumber: ctx.lineNumber,
      column: ctx.column,
      currentWord: ctx.currentWord,
      filepath: ctx.path,
      filename: ctx.path.split('/').pop() ?? ctx.path,
      scope: ctx.scope,
      clipboard: ctx.clipboard ?? '',
      tabSize: ctx.tabSize || tabSize,
      softTabs: ctx.softTabs ?? softTabs,
      prompts: inputs ?? {},
    }
    try {
      const result = await runMacroModule(code, macroCtx, ctx.timeoutMs)
      const output = (result.output || cmd.output) as string
      const bad = validateOutputRouting(output)
      if (bad) {
        return {
          stdout: result.text,
          stderr: bad,
          exitCode: 1,
          output: cmd.output,
          inputMode: resolved.mode,
          language: result.language || cmd.language,
          openInPreview: result.openInPreview ?? cmd.openInPreview,
        }
      }
      return {
        stdout: result.text,
        stderr: '',
        exitCode: 0,
        output: output as OutputRouting,
        inputMode: resolved.mode,
        language: result.language || cmd.language,
        openInPreview: result.openInPreview ?? cmd.openInPreview,
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      return {
        stdout: '',
        stderr: message,
        exitCode: 1,
        output: cmd.output,
        inputMode: resolved.mode,
        language: cmd.language,
        openInPreview: cmd.openInPreview,
      }
    }
  }

  const rc: RunContextDTO = {
    selectedText: ctx.selectedText,
    currentLine: ctx.currentLine,
    lineNumber: ctx.lineNumber,
    columnNumber: ctx.column,
    currentWord: ctx.currentWord,
    filePath: ctx.path,
    fileName: ctx.path.split('/').pop() ?? ctx.path,
    directory: ctx.directory ?? '',
    projectDirectory: ctx.projectDirectory ?? '',
    scope: ctx.scope,
    tabSize: ctx.tabSize || tabSize,
    softTabs: ctx.softTabs ?? softTabs,
    clipboard: ctx.clipboard ?? '',
    input,
    promptValues: inputs ?? {},
    workDir: ctx.workDir ?? ctx.projectDirectory ?? '',
    timeoutMs: ctx.timeoutMs ?? 0,
  }
  const res: RunResultDTO = await BundleService.RunCommand(id, rc)
  const output = (res.output || cmd.output) as string
  const bad = validateOutputRouting(output)
  if (bad && (res.exitCode ?? 0) === 0) {
    return {
      stdout: res.stdout ?? '',
      stderr: bad,
      exitCode: 1,
      output: cmd.output,
      inputMode: resolved.mode,
      language: cmd.language,
      openInPreview: cmd.openInPreview,
    }
  }
  return {
    stdout: res.stdout ?? '',
    stderr: res.stderr ?? '',
    exitCode: res.exitCode ?? 0,
    output: (bad ? cmd.output : output) as OutputRouting,
    inputMode: resolved.mode,
    language: cmd.language,
    openInPreview: cmd.openInPreview,
  }
}

/** Active Observatory registers how to run a command against the focused editor. */
type EditorCommandHandler = (id: string) => Promise<void>
let editorCommandHandler: EditorCommandHandler | null = null

export function setEditorCommandHandler(fn: EditorCommandHandler | null) {
  editorCommandHandler = fn
}

export async function runEditorCommand(id: string): Promise<void> {
  if (!editorCommandHandler) {
    throw new Error('No editor is focused — open a file in the Observatory first')
  }
  await editorCommandHandler(id)
}

type EditorFocusProvider = () => { scope: string; path: string }
let editorFocusProvider: EditorFocusProvider | null = null

export function setEditorFocusProvider(fn: EditorFocusProvider | null) {
  editorFocusProvider = fn
}

export function currentEditorFocus(): { scope: string; path: string } {
  return editorFocusProvider?.() ?? { scope: '', path: '' }
}
