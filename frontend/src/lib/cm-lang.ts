// Map file paths → CodeMirror language support via bundle TextMate grammars.
// Indent prefs still use stable EditorLangId values (independent of tokenizer).
import type { Extension } from '@codemirror/state'
import { loadGrammar, resolveScopeForPath } from './tm-registry'

/** Stable language ids for editor prefs (indent overrides, etc.). */
export type EditorLangId =
  | 'go' | 'python' | 'javascript' | 'typescript' | 'json' | 'yaml'
  | 'markdown' | 'rust' | 'html' | 'css' | 'shell' | 'svelte' | 'vue'
  | 'java' | 'c' | 'cpp' | 'ruby' | 'php' | 'sql' | 'toml' | 'other'

/** Indent style keys shared with editor prefs (keep in sync with stores.EditorIndent). */
export type LangIndent = 'spaces-2' | 'spaces-4' | 'tabs'

/**
 * Conventional indent when the user has no per-language override.
 * Resolution: user override → these defaults → global editor indent.
 */
export const LANGUAGE_INDENT_DEFAULTS: Partial<Record<EditorLangId, LangIndent>> = {
  go: 'tabs',
  python: 'spaces-4',
  typescript: 'spaces-2',
  javascript: 'spaces-2',
  json: 'spaces-2',
  yaml: 'spaces-2',
  markdown: 'spaces-2',
  rust: 'spaces-4',
  html: 'spaces-2',
  css: 'spaces-2',
  shell: 'spaces-2',
  svelte: 'spaces-2',
  vue: 'spaces-2',
  java: 'spaces-4',
  c: 'spaces-4',
  cpp: 'spaces-4',
  ruby: 'spaces-2',
  php: 'spaces-4',
  sql: 'spaces-2',
  toml: 'spaces-2',
}

/** Languages exposed in Settings for per-language indent. */
export const EDITOR_INDENT_LANGS: { id: EditorLangId; label: string; hint?: string }[] = [
  { id: 'go', label: 'Go', hint: 'gofmt' },
  { id: 'python', label: 'Python', hint: 'PEP 8' },
  { id: 'typescript', label: 'TypeScript' },
  { id: 'javascript', label: 'JavaScript' },
  { id: 'json', label: 'JSON' },
  { id: 'yaml', label: 'YAML' },
  { id: 'markdown', label: 'Markdown' },
  { id: 'rust', label: 'Rust', hint: 'rustfmt' },
  { id: 'html', label: 'HTML' },
  { id: 'css', label: 'CSS' },
  { id: 'shell', label: 'Shell' },
  { id: 'svelte', label: 'Svelte' },
  { id: 'vue', label: 'Vue' },
]

function pathParts(path: string): { lower: string; ext: string } {
  const base = path.split('/').pop() ?? path
  const lower = base.toLowerCase()
  const ext = lower.includes('.') ? lower.slice(lower.lastIndexOf('.')) : ''
  return { lower, ext }
}

/** Map a file path to a prefs language id (fallback: other). */
export function languageIdForPath(path: string): EditorLangId {
  const { lower, ext } = pathParts(path)
  switch (lower) {
    case 'dockerfile':
      return 'shell'
    case 'makefile':
    case 'gnumakefile':
      return 'shell'
    case 'go.mod':
    case 'go.sum':
      return 'go'
    case 'cargo.lock':
    case 'poetry.lock':
    case 'uv.lock':
      return 'toml'
    case 'nginx.conf':
      return 'other'
  }
  switch (ext) {
    case '.ts':
    case '.mts':
    case '.cts':
    case '.tsx':
      return 'typescript'
    case '.js':
    case '.mjs':
    case '.cjs':
    case '.jsx':
      return 'javascript'
    case '.vue':
      return 'vue'
    case '.svelte':
      return 'svelte'
    case '.html':
    case '.htm':
      return 'html'
    case '.css':
    case '.scss':
    case '.sass':
    case '.less':
      return 'css'
    case '.go':
      return 'go'
    case '.py':
    case '.pyi':
    case '.pyw':
      return 'python'
    case '.rs':
      return 'rust'
    case '.java':
      return 'java'
    case '.c':
    case '.h':
      return 'c'
    case '.cc':
    case '.cpp':
    case '.cxx':
    case '.hpp':
    case '.hxx':
    case '.hh':
      return 'cpp'
    case '.rb':
    case '.rake':
    case '.gemspec':
      return 'ruby'
    case '.php':
      return 'php'
    case '.json':
    case '.jsonc':
    case '.json5':
      return 'json'
    case '.yaml':
    case '.yml':
      return 'yaml'
    case '.toml':
      return 'toml'
    case '.md':
    case '.mdx':
    case '.markdown':
      return 'markdown'
    case '.sql':
      return 'sql'
    case '.sh':
    case '.bash':
    case '.zsh':
    case '.ksh':
    case '.fish':
    case '.ps1':
    case '.psm1':
    case '.dockerfile':
      return 'shell'
    default:
      return 'other'
  }
}

function wantsDiffLineChrome(path: string): boolean {
  const { lower, ext } = pathParts(path)
  return ext === '.diff' || ext === '.patch' || lower.endsWith('.diff') || lower.endsWith('.patch')
}

const tmResolved = new Map<string, Extension>()
const tmInflight = new Map<string, Promise<Extension>>()

/** Rebuild language extensions on next use so reloaded fold markers and languageData apply. */
export function forgetLanguageExtensions(): void {
  tmResolved.clear()
  tmInflight.clear()
}

/**
 * The grammar for `path` if it is already in memory. Callers mount with this so
 * a revisited language highlights on the first paint with no flash.
 */
export function languageForPathSync(path: string): Extension {
  const scope = resolveScopeForPath(path)
  if (scope && tmResolved.has(scope)) return tmResolved.get(scope)!
  return []
}

type TmLoad = { kind: 'miss' } | { kind: 'resident' } | { kind: 'ready'; ext: Extension }

async function loadTextMateForPath(path: string): Promise<TmLoad> {
  // Catalog is empty until bootstrap — resolving first made MiniScriptEditor
  // (and any first editor) miss dotenv/shell and never load a grammar.
  const { ensureTextMateBootstrap } = await import('./tm-bootstrap')
  await ensureTextMateBootstrap()
  const scope = resolveScopeForPath(path)
  if (!scope) return { kind: 'miss' }
  if (tmResolved.has(scope)) return { kind: 'resident' }
  const { textmateLanguage } = await import('./cm-textmate')
  if (tmResolved.has(scope)) return { kind: 'resident' }
  let pending = tmInflight.get(scope)
  if (!pending) {
    pending = loadGrammar(scope)
      .then(async (ok) => {
        if (!ok) return [] as Extension
        let ext: Extension = textmateLanguage(scope)
        if (wantsDiffLineChrome(path) || scope === 'source.diff' || scope === 'text.diff') {
          const { diffLineHighlight } = await import('./cm-diff-lines')
          ext = [ext, diffLineHighlight]
        }
        tmResolved.set(scope, ext)
        tmInflight.delete(scope)
        return ext
      })
      .catch((err) => {
        tmInflight.delete(scope)
        console.error(`failed to load TextMate grammar ${scope}`, err)
        return [] as Extension
      })
    tmInflight.set(scope, pending)
  }
  return { kind: 'ready', ext: await pending }
}

/**
 * Load the TextMate grammar for `path`. Resolves to `null` when it is already
 * in memory (so the caller can skip a pointless reconfigure).
 */
export function loadLanguageForPath(path: string): Promise<Extension | null> | null {
  return (async () => {
    const tm = await loadTextMateForPath(path)
    if (tm.kind === 'resident') return null
    if (tm.kind === 'ready') return tm.ext
    return [] as Extension
  })()
}
