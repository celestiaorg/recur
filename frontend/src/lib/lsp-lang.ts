import { languageIdForPath, type EditorLangId } from './cm-lang'
import { getGrammarMeta, resolveScopeForPath } from './tm-registry'

/** Map a repo-relative path to an LSP `languageId` (and registry key). */
export function lspLanguageIdForPath(path: string): string {
  const mapped = lspLanguageId(languageIdForPath(path), path)
  if (mapped) return mapped
  const scope = resolveScopeForPath(path)
  if (!scope) return ''
  return getGrammarMeta(scope)?.language?.trim() ?? ''
}

const JSONC_NAMES = new Set([
  '.eslintrc',
  '.eslintrc.json',
  '.jsfmtrc',
  '.jshintrc',
  '.swcrc',
  '.hintrc',
  '.babelrc',
  '.babelrc.json',
  'babel.config.json',
  'typedoc.json',
  'settings.json',
  'launch.json',
  'tasks.json',
  'keybindings.json',
  'extensions.json',
  'devcontainer.json',
])

function jsoncBasename(base: string): boolean {
  if (base.endsWith('.jsonc')) return true
  if (JSONC_NAMES.has(base)) return true
  if (/^[jt]sconfig(\..*)?\.json$/.test(base)) return true
  if (/^[jt]sconfig-.*\.json$/.test(base)) return true
  return false
}

export function lspLanguageId(id: EditorLangId, path = ''): string {
  const lower = path.split('/').pop()?.toLowerCase() ?? ''
  if (lower.endsWith('.tsx')) return 'typescriptreact'
  if (lower.endsWith('.jsx')) return 'javascriptreact'
  if (lower === 'go.mod') return 'go.mod'
  if (lower === 'go.work') return 'go.work'
  if (lower === 'go.sum') return 'go.sum'
  if (lower.endsWith('.cs')) return 'csharp'
  if (lower.endsWith('.hs') || lower.endsWith('.lhs')) return 'haskell'
  if (lower.endsWith('.scss')) return 'scss'
  if (lower.endsWith('.sass')) return 'sass'
  if (lower.endsWith('.less')) return 'less'
  if (jsoncBasename(lower)) return 'jsonc'
  if (lower.endsWith('.json5')) return 'json5'
  if (lower === 'dockerfile' || lower.endsWith('.dockerfile')) return 'dockerfile'
  if (lower === 'makefile' || lower === 'gnumakefile') return 'makefile'
  if (lower.endsWith('.ps1') || lower.endsWith('.psm1')) return 'powershell'
  if (lower.endsWith('.fish')) return 'fish'
  switch (id) {
    case 'typescript':
    case 'javascript':
    case 'go':
    case 'python':
    case 'rust':
    case 'json':
    case 'yaml':
    case 'toml':
    case 'markdown':
    case 'c':
    case 'cpp':
    case 'java':
    case 'html':
    case 'css':
    case 'sql':
    case 'ruby':
    case 'php':
      return id
    case 'shell':
      return 'shellscript'
    case 'svelte':
      return 'svelte'
    case 'vue':
      return 'vue'
    default:
      return id === 'other' ? '' : id
  }
}
