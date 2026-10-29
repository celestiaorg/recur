/**
 * vscode-textmate tokenizer + per-document line-state cache (main thread).
 *
 * v2 architecture (see TM-REDESIGN.md): this engine is the ONLY tokenizer.
 * The CM6 plugin calls `tokenizeViewport` synchronously for first frames and
 * settled scrolls; `fillSlice` runs in idle time to sweep the rest of the
 * file and correct heuristic guesses. There is no worker.
 *
 * Two token tiers per document:
 *  - accurate: state-chained from line 1, watermark = `cache.filled`
 *  - provisional: guessed-start viewport tokens beyond the watermark
 *    (VS Code's tokenizeHeuristically — never written to the state store,
 *    corrected when the accurate sweep reaches them)
 */
import {
  INITIAL,
  Registry,
  parseRawGrammar,
  type IGrammar,
  type IRawGrammar,
  type IRawTheme,
  type StateStack,
} from 'vscode-textmate'
import { loadWASM, OnigScanner, OnigString } from 'vscode-oniguruma'
import { spliceLineMap, type LineReplace } from './cm-tm-delta'
import { tmDocFingerprint, type TmDocFingerprint } from './cm-tm-doc'
import { TmStackCache, tokenizeLineRange } from './cm-tm-stacks'

export type GrammarSource = { content: string; pathHint: string }
export type GrammarSourceLoader = (scopeName: string) => Promise<GrammarSource | null>

export type TmRawTheme = {
  name?: string
  settings: Array<{
    name?: string
    scope?: string | string[]
    settings: {
      fontStyle?: string
      foreground?: string
      background?: string
    }
  }>
}

export type TmViewportResult = {
  /** accurate = state-chained; guessed = heuristic start, provisional tokens. */
  mode: 'accurate' | 'guessed' | 'none'
  tokenized: number
  /** Every requested line has tokens (accurate or provisional). */
  complete: boolean
}

export type TmFillProgress = {
  filled: number
  total: number
  /** 1-based range whose tokens differ from what was previously shown. */
  changedFrom: number | null
  changedTo: number
  done: boolean
}

type TmDoc = {
  scope: string
  lines: string[]
  cache: TmStackCache
  /** Guessed-start tokens for lines beyond the accurate watermark. */
  provisional: Map<number, ArrayLike<number>>
}

/**
 * Retention for documents nothing currently displays (open-but-inactive
 * tabs). VS Code keeps tokens for every open tab with no count limit and
 * frees them when the tab closes; we mirror that — CodeEditor calls
 * `tmForgetDocs` on real closure — and bound what is left by lines rather
 * than doc count, since 128 configs and 128 big files differ by ~100x in
 * memory.
 *
 * Measured retained heap for tokens + per-line rule stacks (see
 * `scripts/measure-tm-memory.mjs`): 46 B/line for markup-heavy Svelte, ~250
 * B/line for dense TS. So this budget tops out around 125 MB, and only for a
 * session holding half a million lines of *inactive* tabs — normal use (50
 * tabs averaging 1k lines) sits near 12 MB and never evicts. Eviction is
 * cheap anyway: a re-open costs one guessed-start viewport tokenize (~3ms),
 * not a full-prefix fill.
 */
export const MAX_IDLE_DOCS = 128
export const MAX_IDLE_LINES = 500_000
/** Lines longer than this get a single default token (VS Code: 20k). */
const MAX_TOKENIZE_LINE_LENGTH = 20_000
/** Upper bound on the backwards indentation walk for guessed starts. */
const MAX_GUESS_SCAN_LINES = 5_000

let onigLoaded = false

async function ensureOnig(data: ArrayBuffer | ArrayBufferView): Promise<void> {
  if (onigLoaded) return
  await loadWASM(data)
  onigLoaded = true
}

/**
 * Splice without spreading the insert into an argument list. A full-document
 * replace (live refresh of a changed file) hands us every line of the buffer,
 * and `splice(i, n, ...lines)` throws `RangeError: Maximum call stack size
 * exceeded` well inside the supported file size.
 */
const SPLICE_SPREAD_MAX = 8_192

function spliceLines(lines: string[], index: number, deleteCount: number, insert: string[]): string[] {
  if (insert.length <= SPLICE_SPREAD_MAX) {
    lines.splice(index, deleteCount, ...insert)
    return lines
  }
  return lines.slice(0, index).concat(insert, lines.slice(index + deleteCount))
}

function sameLines(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

function sameTokenArrays(a: ArrayLike<number> | undefined, b: ArrayLike<number>): boolean {
  if (!a || a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false
  }
  return true
}

function firstUnfreshLine(cache: TmStackCache, first: number, last: number): number {
  for (let n = first; n <= last; n++) {
    if (n > cache.filled || !cache.tokensAt(n)) return n
  }
  return last + 1
}

/** 1-based column of the first non-whitespace char; 0 when blank. */
function firstNonWhitespaceColumn(text: string): number {
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i)
    if (c !== 32 /* space */ && c !== 9 /* tab */) return i + 1
  }
  return 0
}

export class TmEngine {
  private registry: Registry | null = null
  private theme: IRawTheme | null = null
  private sources = new Map<string, GrammarSource>()
  private injectTo = new Map<string, Set<string>>()
  private pathHints = new Map<string, string>()
  private grammarCache = new Map<string, IGrammar>()
  private docs = new Map<string, TmDoc>()
  private sourceLoader: GrammarSourceLoader | null = null
  private liveRefs = new Map<string, number>()
  private idleDocs: string[] = []

  setSourceLoader(loader: GrammarSourceLoader | null): void {
    this.sourceLoader = loader
  }

  async initOnig(data: ArrayBuffer | ArrayBufferView): Promise<void> {
    await ensureOnig(data)
  }

  noteGrammar(scopeName: string, pathHint: string, injectTo?: string[]): void {
    this.sources.delete(scopeName)
    this.pathHints.set(scopeName, pathHint)
    for (const host of injectTo ?? []) {
      let set = this.injectTo.get(host)
      if (!set) {
        set = new Set()
        this.injectTo.set(host, set)
      }
      set.add(scopeName)
    }
    if (this.registry) this.dropRegistry()
  }

  registerGrammar(scopeName: string, content: string, pathHint: string, injectTo?: string[]): void {
    this.noteGrammar(scopeName, pathHint, injectTo)
    this.sources.set(scopeName, { content, pathHint })
  }

  /**
   * Apply a theme and drop every document.
   *
   * Token metadata embeds colour ids and rule stacks cache scopes resolved
   * under the old theme, so nothing survives a theme change — VS Code does
   * the same (todo_resetTokenization discards all tokens and states, then
   * re-tokenizes visible ranges). Clearing the documents outright, rather
   * than just emptying their caches, gives every view the same clean path a
   * close-and-reopen takes: the next paint re-opens the document and
   * tokenizes it fresh. Theme changes are rare and a viewport re-tokenize is
   * ~3ms, so there is no reason to be clever here.
   */
  setTheme(raw: TmRawTheme | IRawTheme): string[] {
    this.theme = raw as IRawTheme
    const reg = this.ensureRegistry()
    reg.setTheme(this.theme)
    this.docs.clear()
    this.idleDocs = []
    this.liveRefs.clear()
    return reg.getColorMap()
  }

  getColorMap(): string[] {
    return this.ensureRegistry().getColorMap()
  }

  hasGrammar(scopeName: string): boolean {
    return this.grammarCache.has(scopeName)
  }

  async ensureGrammar(scopeName: string): Promise<boolean> {
    const hit = this.grammarCache.get(scopeName)
    if (hit) return true
    const g = await this.ensureRegistry().loadGrammar(scopeName)
    if (g) this.grammarCache.set(scopeName, g)
    return !!g
  }

  /**
   * Open or reuse a document, and take a live ref on it. Same id + scope +
   * text keeps the line-state cache (tab switch / ViewPlugin remount). A text
   * mismatch resets it.
   */
  openDoc(docId: string, scopeName: string, text: string): { reused: boolean } {
    this.retainDoc(docId)
    const lines = text.split('\n')
    const prev = this.docs.get(docId)
    if (prev && prev.scope === scopeName && sameLines(prev.lines, lines)) {
      return { reused: true }
    }
    this.docs.set(docId, {
      scope: scopeName,
      lines,
      cache: new TmStackCache(),
      provisional: new Map(),
    })
    return { reused: false }
  }

  /**
   * Reuse the resident doc when the fingerprint matches — skips serializing
   * the CM document on remount. Returns false when the caller must openDoc
   * with the full text.
   */
  reopenDoc(docId: string, scopeName: string, fp: TmDocFingerprint): boolean {
    if (!this.matchDoc(docId, scopeName, fp)) return false
    this.retainDoc(docId)
    return true
  }

  /** Does the resident doc match this fingerprint? */
  matchDoc(docId: string, scopeName: string, fp: TmDocFingerprint): boolean {
    const doc = this.docs.get(docId)
    if (!doc || doc.scope !== scopeName || doc.lines.length !== fp.lines) return false
    let length = doc.lines.length - 1
    for (const line of doc.lines) length += line.length
    if (length !== fp.length) return false
    const mine = tmDocFingerprint((n) => doc.lines[n - 1], doc.lines.length, length)
    return mine.sample === fp.sample
  }

  private retainDoc(docId: string): void {
    this.liveRefs.set(docId, (this.liveRefs.get(docId) ?? 0) + 1)
    this.idleDocs = this.idleDocs.filter((id) => id !== docId)
  }

  /** Drop a live ref; keep caches until the idle budget overflows. */
  releaseDoc(docId: string): void {
    const n = (this.liveRefs.get(docId) ?? 1) - 1
    if (n > 0) {
      this.liveRefs.set(docId, n)
      return
    }
    this.liveRefs.delete(docId)
    this.idleDocs = this.idleDocs.filter((id) => id !== docId)
    this.idleDocs.push(docId)
    this.trimIdle()
  }

  /** Evict oldest idle documents until both budgets are satisfied. */
  private trimIdle(): void {
    let lines = 0
    for (const id of this.idleDocs) lines += this.docs.get(id)?.lines.length ?? 0
    while (
      this.idleDocs.length > 0 &&
      (this.idleDocs.length > MAX_IDLE_DOCS || lines > MAX_IDLE_LINES)
    ) {
      const evict = this.idleDocs.shift()!
      lines -= this.docs.get(evict)?.lines.length ?? 0
      if (!this.liveRefs.has(evict)) this.docs.delete(evict)
    }
  }

  /**
   * Free a document unless a view still holds it (tab closed / file
   * discarded). Mirrors VS Code disposing the TextModel on tab close.
   */
  closeIfIdle(docId: string): boolean {
    if (this.liveRefs.has(docId)) return false
    this.closeDoc(docId)
    return true
  }

  hasDoc(docId: string): boolean {
    return this.docs.has(docId)
  }

  editDoc(docId: string, replace: LineReplace): void {
    const doc = this.docs.get(docId)
    if (!doc) return
    const from = Math.max(1, replace.fromLine)
    doc.lines = spliceLines(doc.lines, from - 1, replace.oldLineCount, replace.newLines)
    doc.cache.splice(from, replace.oldLineCount, replace.newLines.length)
    if (doc.provisional.size > 0) {
      doc.provisional = spliceLineMap(doc.provisional, from, replace.oldLineCount, replace.newLines.length)
    }
  }

  closeDoc(docId: string): void {
    this.docs.delete(docId)
    this.liveRefs.delete(docId)
    this.idleDocs = this.idleDocs.filter((id) => id !== docId)
  }

  lineCount(docId: string): number {
    return this.docs.get(docId)?.lines.length ?? 0
  }

  docScope(docId: string): string | undefined {
    return this.docs.get(docId)?.scope
  }

  lineAt(docId: string, n: number): string | undefined {
    const doc = this.docs.get(docId)
    if (!doc || n < 1 || n > doc.lines.length) return undefined
    return doc.lines[n - 1]
  }

  /**
   * Advance the accurate sweep until EOF or budgetMs. Returns whether the
   * document is fully filled.
   */
  fillSync(docId: string, budgetMs = 80): boolean {
    const doc = this.docs.get(docId)
    if (!doc) return false
    const deadline = performance.now() + budgetMs
    while (performance.now() < deadline) {
      const remain = deadline - performance.now()
      if (remain <= 0) break
      const p = this.fillSlice(docId, Math.min(8, remain))
      if (p.done) return true
      if (p.total === 0) return false
    }
    return doc.cache.filled >= doc.lines.length
  }

  /**
   * Scope-stack tokens for 1-based line n (tokenizeLine, not tokenizeLine2).
   * Needs the accurate stack from line n-1.
   */
  scopesForLine(docId: string, n: number): Array<{ start: number; end: number; scopes: string[] }> | undefined {
    const doc = this.docs.get(docId)
    if (!doc || n < 1 || n > doc.lines.length) return undefined
    const grammar = this.grammarCache.get(doc.scope)
    if (!grammar) return undefined
    let stack: StateStack
    if (n === 1) {
      stack = INITIAL
    } else {
      const prev = doc.cache.stackAt(n - 1)
      if (!prev) return undefined
      stack = prev
    }
    const r = grammar.tokenizeLine(this.lineTextForTokenize(doc, n), stack)
    return r.tokens.map((t) => ({ start: t.startIndex, end: t.endIndex, scopes: t.scopes }))
  }

  /** Best tokens for a line: accurate, else provisional, else stale cache. */
  tokensForLine(docId: string, n: number): ArrayLike<number> | undefined {
    const doc = this.docs.get(docId)
    if (!doc) return undefined
    if (n <= doc.cache.filled) return doc.cache.tokensAt(n)
    return doc.provisional.get(n) ?? doc.cache.tokensAt(n)
  }

  /**
   * Every line in `from..to` has tokens that match its CURRENT text — the
   * accurate chain covers it, or it carries a guessed-start provisional entry.
   *
   * Stale arrays past the watermark do NOT count. `TmStackCache.splice`
   * deliberately keeps them so a re-tokenize can `equals` the previous end
   * state and stop early; treating them as usable made an edit repaint from
   * pre-edit tokens and wait for the idle sweep to correct itself.
   */
  hasFreshTokens(docId: string, fromLine: number, toLine: number): boolean {
    const doc = this.docs.get(docId)
    if (!doc) return false
    const last = Math.min(toLine, doc.lines.length)
    for (let n = Math.max(1, fromLine); n <= last; n++) {
      if (n <= doc.cache.filled && doc.cache.tokensAt(n)) continue
      if (doc.provisional.has(n)) continue
      return false
    }
    return true
  }

  /** The accurate sweep can reach `fromLine` with at most `slack` extra lines. */
  canResumeAccurately(docId: string, fromLine: number, slack = 0): boolean {
    const doc = this.docs.get(docId)
    if (!doc) return false
    return doc.cache.filled >= fromLine - 1 - slack
  }

  /**
   * Synchronous viewport tokenization (VS Code's tokenizeHeuristically).
   * Accurate when the state chain reaches the viewport; otherwise synthesizes
   * a start state from the enclosing less-indented lines and stores the
   * results as provisional. Budgeted — returns partial work when over. The
   * default is generous because the first tokenize of a fresh grammar pays
   * lazy rule compilation (VS Code's stabilized viewport pass is unbudgeted);
   * warm viewports finish in ~2–3ms.
   */
  tokenizeViewport(docId: string, fromLine: number, toLine: number, budgetMs = 25): TmViewportResult {
    const doc = this.docs.get(docId)
    if (!doc) return { mode: 'none', tokenized: 0, complete: false }
    const grammar = this.grammarCache.get(doc.scope)
    if (!grammar) return { mode: 'none', tokenized: 0, complete: false }
    const last = Math.min(toLine, doc.lines.length)
    const first = Math.max(1, fromLine)
    if (first > last) return { mode: 'accurate', tokenized: 0, complete: true }
    const deadline = performance.now() + budgetMs

    if (doc.cache.filled >= first - 1) {
      const start = firstUnfreshLine(doc.cache, first, last)
      let tokenized = 0
      if (start <= last) {
        const r = tokenizeLineRange({
          lineText: (n) => this.lineTextForTokenize(doc, n),
          cache: doc.cache,
          tokenize: (text, stack) => grammar.tokenizeLine2(text, stack),
          fromLine: start,
          toLine: last,
          stopWhenStable: true,
          deadlineMs: deadline,
        })
        tokenized = r.tokenized
      }
      for (let n = first; n <= Math.min(last, doc.cache.filled); n++) doc.provisional.delete(n)
      return { mode: 'accurate', tokenized, complete: doc.cache.hasTokens(first, last) }
    }

    // Cold jump: guessed start state. Never advances the accurate watermark.
    let stack = this.guessStartStack(doc, grammar, first)
    let tokenized = 0
    let n = first
    for (; n <= last; n++) {
      if (n > first && performance.now() > deadline) break
      const r = grammar.tokenizeLine2(this.lineTextForTokenize(doc, n), stack)
      stack = r.ruleStack
      doc.provisional.set(n, r.tokens)
      tokenized++
    }
    return { mode: 'guessed', tokenized, complete: n > last }
  }

  /**
   * VS Code's findLikelyRelevantLines: walk backwards collecting only lines
   * whose first-non-whitespace column strictly decreases (the enclosing
   * scopes), stop at column 1 or a line with a known end state, then tokenize
   * those few lines to synthesize the state entering `lineNumber`.
   */
  private guessStartStack(doc: TmDoc, grammar: IGrammar, lineNumber: number): StateStack {
    const relevant: string[] = []
    let column = firstNonWhitespaceColumn(doc.lines[lineNumber - 1] ?? '')
    let initial: StateStack | null = null
    const scanFloor = Math.max(1, lineNumber - MAX_GUESS_SCAN_LINES)
    for (let i = lineNumber - 1; column > 1 && i >= scanFloor; i--) {
      const c = firstNonWhitespaceColumn(doc.lines[i - 1] ?? '')
      if (c === 0) continue
      if (c < column) {
        relevant.push(doc.lines[i - 1])
        column = c
        const known = i <= doc.cache.filled ? doc.cache.stackAt(i) : undefined
        if (known) {
          initial = known
          break
        }
      }
    }
    relevant.reverse()
    let stack: StateStack = initial ?? INITIAL
    for (const text of relevant) {
      stack = grammar.tokenizeLine2(text, stack).ruleStack
    }
    return stack
  }

  private lineTextForTokenize(doc: TmDoc, n: number): string {
    const text = doc.lines[n - 1] ?? ''
    // Minified/generated monster lines: tokenize a stub so the grammar state
    // stays sane without an O(line) regex storm (VS Code nulls these too).
    return text.length > MAX_TOKENIZE_LINE_LENGTH ? '' : text
  }

  /**
   * Advance the accurate sweep for ~budgetMs. Corrects provisional guesses it
   * passes and reports the changed line range so callers repaint only when
   * something visible actually changed.
   */
  fillSlice(docId: string, budgetMs = 1.5): TmFillProgress {
    const doc = this.docs.get(docId)
    const empty: TmFillProgress = { filled: 0, total: 0, changedFrom: null, changedTo: 0, done: true }
    if (!doc) return empty
    const total = doc.lines.length
    const grammar = this.grammarCache.get(doc.scope)
    if (!grammar) return { filled: doc.cache.filled, total, changedFrom: null, changedTo: 0, done: false }

    const deadline = performance.now() + budgetMs
    let changedFrom: number | null = null
    let changedTo = 0
    while (doc.cache.filled < total) {
      const n = doc.cache.filled + 1
      const resume = doc.cache.resumeAt(n)
      let stack: StateStack | null = resume.stack
      let line = resume.startLine
      for (; line <= total; line++) {
        const staleStack = doc.cache.stackAt(line)
        const shown = doc.provisional.get(line) ?? doc.cache.tokensAt(line)
        const r = grammar.tokenizeLine2(this.lineTextForTokenize(doc, line), stack)
        stack = r.ruleStack
        doc.cache.remember(line, stack, r.tokens)
        doc.provisional.delete(line)
        if (!sameTokenArrays(shown, r.tokens)) {
          if (changedFrom === null) changedFrom = line
          changedTo = line
        }
        if (staleStack && stack.equals(staleStack)) {
          // Downstream states re-converged; cached lines are valid again.
          doc.cache.markStable(line)
          break
        }
        if (performance.now() > deadline) {
          return {
            filled: doc.cache.filled,
            total,
            changedFrom,
            changedTo,
            done: false,
          }
        }
      }
      if (performance.now() > deadline) break
    }
    return {
      filled: doc.cache.filled,
      total,
      changedFrom,
      changedTo,
      done: doc.cache.filled >= total,
    }
  }

  /** Test helper — expose cache fill depth. */
  filledThrough(docId: string): number {
    return this.docs.get(docId)?.cache.filled ?? 0
  }

  /** Test helper — provisional line count. */
  provisionalCount(docId: string): number {
    return this.docs.get(docId)?.provisional.size ?? 0
  }

  /** Test helper — INITIAL stack (same object vscode-textmate uses). */
  initialStack(): StateStack {
    return INITIAL
  }

  private dropRegistry(): void {
    this.registry = null
    this.grammarCache.clear()
  }

  private ensureRegistry(): Registry {
    if (!this.registry) {
      this.registry = new Registry({
        onigLib: Promise.resolve({
          createOnigScanner: (patterns: string[]) => new OnigScanner(patterns),
          createOnigString: (s: string) => new OnigString(s),
        }),
        theme: this.theme ?? undefined,
        loadGrammar: async (scopeName: string) => {
          let src = this.sources.get(scopeName)
          if (!src && this.sourceLoader) {
            const got = await this.sourceLoader(scopeName)
            if (got) {
              src = got
              this.sources.set(scopeName, got)
            }
          }
          if (!src) return null
          return parseRawGrammar(src.content, src.pathHint) as IRawGrammar
        },
        getInjections: (scopeName: string) => {
          const set = this.injectTo.get(scopeName)
          return set ? Array.from(set) : undefined
        },
      })
      if (this.theme) this.registry.setTheme(this.theme)
    }
    return this.registry
  }
}

/** Reset the process-wide Oniguruma flag (tests). */
export function _resetTmEngineOnigForTests(): void {
  onigLoaded = false
}
