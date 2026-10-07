/**
 * Open-file outline from TextMate token scopes + bundle [[symbol]] rules.
 *
 * Workspace Find Defs stay on Go regex until LSP. This only covers the
 * buffer the editor has already tokenized.
 */
import { BundleService } from '../../bindings/recur/internal/app'
import type { SymbolInfo } from '../../bindings/recur/internal/app'
import type { OutlineSymbol } from '../../bindings/recur/internal/domain/models'
import { fuzzyFilterPrepared, prepareFuzzy, type FuzzyHit } from './fuzzy'
import { tmEnsureFill } from './tm-fill'
import { peekTmEngine, ensureTmEngine } from './tm-registry'
import type { TmEngine } from './tm-engine'
import {
  compileSelector,
  scopePrefixMatch,
  selectorScore,
  type CompiledSelector,
} from './tm-scope-selector'

export type SymbolRule = {
  id: string
  scope: string
  show: boolean
  transform: string
  kind: string
}

export type ScopeToken = { start: number; end: number; scopes: string[] }

type CompiledRule = SymbolRule & { sel: CompiledSelector }

function compileRules(rules: SymbolRule[]): CompiledRule[] {
  return rules.filter((r) => r.scope).map((r) => ({ ...r, sel: compileSelector(r.scope) }))
}

const MAX_SYMBOLS = 200

let ruleCache: { items: CompiledRule[]; at: number } | null = null
const CACHE_MS = 4000
const fillUnsubs = new Map<string, () => void>()

function fromInfo(s: SymbolInfo): SymbolRule {
  return {
    id: s.id,
    scope: s.scope || '',
    show: s.show !== false,
    transform: s.transform || '',
    kind: s.kind || '',
  }
}

async function loadRules(): Promise<CompiledRule[]> {
  if (ruleCache && Date.now() - ruleCache.at < CACHE_MS) return ruleCache.items
  try {
    const list = await BundleService.ListSymbols()
    const items = (list ?? []).map(fromInfo).filter((r) => r.scope)
    const compiled = compileRules(items)
    ruleCache = { items: compiled, at: Date.now() }
    return compiled
  } catch {
    return ruleCache?.items ?? []
  }
}

export function applySymbolTransform(text: string, transform: string): string {
  const src = transform.trim()
  if (!src) return text
  let out = text
  const re = /^\s*s\/((?:\\.|[^/])*)\/((?:\\.|[^/])*)\/([gimsuy]*)/gm
  let m: RegExpExecArray | null
  let any = false
  while ((m = re.exec(src)) !== null) {
    any = true
    try {
      out = out.replace(new RegExp(m[1], m[3] || ''), unescapeRepl(m[2]))
    } catch {
      // Drop invalid JS regex (Oniguruma leftovers).
    }
  }
  return any ? out : text
}

function unescapeRepl(s: string): string {
  return s
    .replace(/\\u([0-9a-fA-F]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)))
    .replace(/\\n/g, '\n')
    .replace(/\\t/g, '\t')
    .replace(/\\\//g, '/')
}

/** Fenced / indented code in Markdown (and similar) — don't list inner langs. */
function inFencedCode(scopes: string[]): boolean {
  for (const s of scopes) {
    if (scopePrefixMatch('markup.fenced_code', s) || scopePrefixMatch('markup.raw.block', s)) return true
  }
  return false
}

export function inferSymbolKind(rule: SymbolRule): string {
  if (rule.kind) return rule.kind
  const s = rule.scope
  if (/heading|section|toc-list/.test(s)) return 'heading'
  if (/\bclass\b/.test(s)) return 'class'
  if (/function/.test(s)) return 'func'
  if (/entity\.name\.type/.test(s)) return 'type'
  if (/\.tag\b/.test(s)) return 'key'
  if (/selector|attribute\.id/.test(s)) return 'id'
  return 'symbol'
}

export function bestSymbolRule(scopes: string[], rules: CompiledRule[]): CompiledRule | null {
  let best: CompiledRule | null = null
  let bestScore = 0
  for (const rule of rules) {
    const score = selectorScore(rule.sel, scopes)
    if (score > bestScore) {
      best = rule
      bestScore = score
    }
  }
  return best
}

export function collectSymbols(
  lines: string[],
  tokensForLine: (n: number) => ScopeToken[] | undefined,
  rules: SymbolRule[],
  max = MAX_SYMBOLS,
): OutlineSymbol[] {
  const compiled = compileRules(rules)
  const out: OutlineSymbol[] = []
  if (compiled.length === 0) return out

  type Pending = { rule: CompiledRule; name: string; line: number; src: string }
  let pending: Pending | null = null

  const flushPending = () => {
    if (!pending) return
    const { rule, name, line, src } = pending
    pending = null
    // Trim the raw token text, not the transform result — Markdown heading
    // indent is leading em-spaces (TextMate symbolTransformation).
    const label = applySymbolTransform(qualifyGoReceiver(src, name.trim()), rule.transform)
    if (!label.trim()) return
    out.push({ name: label, kind: inferSymbolKind(rule), line })
  }

  for (let n = 1; n <= lines.length && out.length < max; n++) {
    const tokens = tokensForLine(n)
    if (!tokens) {
      flushPending()
      continue
    }
    const line = lines[n - 1] ?? ''
    for (const tok of tokens) {
      if (out.length >= max) break
      if (inFencedCode(tok.scopes)) {
        flushPending()
        continue
      }
      const rule = bestSymbolRule(tok.scopes, compiled)
      if (!rule || !rule.show) {
        flushPending()
        continue
      }
      const chunk = line.slice(tok.start, tok.end)
      if (!chunk) continue
      if (pending && pending.rule.id === rule.id && pending.line === n) {
        pending.name += chunk
        continue
      }
      flushPending()
      pending = { rule, name: chunk, line: n, src: line }
    }
    flushPending()
  }
  return out
}

function collectFromEngine(engine: TmEngine, docId: string, rules: CompiledRule[]): OutlineSymbol[] {
  const total = engine.lineCount(docId)
  if (total <= 0) return []
  const filled = Math.min(total, engine.filledThrough(docId))
  const lines: string[] = []
  for (let n = 1; n <= filled; n++) {
    lines.push(engine.lineAt(docId, n) ?? '')
  }
  return collectSymbols(lines, (n) => engine.scopesForLine(docId, n), rules)
}

/**
 * Outline the open TextMate document. Returns null when this file has no
 * grammar / engine doc / rules (caller should use CodeService.Outline).
 * An empty array means TM ran and found nothing.
 */
export async function outlineOpenFile(
  docId: string,
  onUpdate?: (syms: OutlineSymbol[]) => void,
  retryMs = 0,
): Promise<OutlineSymbol[] | null> {
  fillUnsubs.get(docId)?.()
  fillUnsubs.delete(docId)
  if (!docId) return null
  const rules = await loadRules()
  if (rules.length === 0) return null
  const engine = peekTmEngine() ?? (await ensureTmEngine())
  if (!engine?.hasDoc(docId)) {
    if (retryMs > 0 && onUpdate) {
      globalThis.setTimeout(() => {
        void outlineOpenFile(docId, onUpdate).then((syms) => {
          if (syms) onUpdate(syms)
        })
      }, retryMs)
    }
    return null
  }
  const scope = engine.docScope(docId)
  if (!scope) return null
  await engine.ensureGrammar(scope)
  engine.fillSync(docId, 80)
  const syms = collectFromEngine(engine, docId, rules)
  if (engine.filledThrough(docId) < engine.lineCount(docId) && onUpdate) {
    fillUnsubs.set(
      docId,
      tmEnsureFill(engine, docId, (_ch, done) => {
        if (!done) return
        fillUnsubs.get(docId)?.()
        fillUnsubs.delete(docId)
        onUpdate(collectFromEngine(engine, docId, rules))
      }),
    )
  }
  return syms
}

/** Last outline symbol at or before `line` (TextMate status-bar “current symbol”). */
export function symbolAtLine(syms: OutlineSymbol[], line: number): OutlineSymbol | null {
  let best: OutlineSymbol | null = null
  for (const s of syms) {
    if (s.line > line) continue
    if (!best || s.line >= best.line) best = s
  }
  return best
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * Prefix a Go method with its receiver type (`Engine.Foo`). Same-line only —
 * that's the TextMate-shaped part we can see without LSP. Other langs nest
 * methods inside a class/impl on earlier lines.
 */
export function qualifyGoReceiver(line: string, name: string): string {
  if (!name || name.includes('.')) return name
  const trimmed = line.trimStart()
  if (!trimmed.startsWith('func')) return name
  const m = trimmed.match(new RegExp(`^func\\s+\\(([^)]*)\\)\\s+${escapeRegExp(name)}\\b`))
  if (!m) return name
  let recv = m[1].trim().replace(/^[_A-Za-z][\w]*\s+/, '').replace(/^\*+/, '')
  recv = recv.replace(/\[[^\]]*\]$/, '').trim()
  const type = recv.match(/^[A-Za-z_][\w.]*/)?.[0]
  if (!type || type === name) return name
  return `${type}.${name}`
}

/** Bare identifier for Find Defs / refs (strip Type. prefix and heading indent). */
export function symbolLookupName(name: string, kind = ''): string {
  const t = name.trim()
  if (kind && kind !== 'func') return t
  const i = t.lastIndexOf('.')
  return i >= 0 ? t.slice(i + 1) : t
}

/**
 * Go to Symbol ranking. Empty query keeps document (line) order so indented
 * Markdown headings stay a tree, not an alphabetical scramble. Typed queries
 * fuzzy-match the trimmed name and tie-break by line.
 */
export function filterSymbols<T extends OutlineSymbol>(
  symbols: T[],
  query: string,
  limit = 40,
): FuzzyHit<T>[] {
  if (!query.trim()) {
    return symbols.slice(0, limit).map((s) => ({ item: s, score: 0, label: s.name }))
  }
  const ranked = fuzzyFilterPrepared(
    prepareFuzzy(symbols, (s) => `${s.name.trim()} ${s.kind}`),
    query,
    limit,
  )
  ranked.sort((a, b) => b.score - a.score || a.item.line - b.item.line)
  return ranked
}
