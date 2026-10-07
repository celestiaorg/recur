/**
 * CodeMirror languageData from bundle [[languageData]] (comment tokens + indent).
 *
 * Recur does not load @codemirror/lang-*, so toggleComment / indentOnInput need
 * these facets. Patterns are JavaScript regex sources from the manifest.
 *
 * Enter between an opening tag and its closer (`<div>|</div>`) or empty
 * `{}`/`()`/`[]` is TextMate's Return-inside-empty-item — not
 * increaseIndentPattern, which only sees the previous *line*.
 */
import { indentService, indentString, getIndentUnit, type IndentContext } from '@codemirror/language'
import { EditorSelection, EditorState, Prec, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { getGrammarMeta } from './tm-registry'

export type LanguageDataFields = {
  lineComment?: string
  blockCommentStart?: string
  blockCommentEnd?: string
  increaseIndentPattern?: string
  decreaseIndentPattern?: string
  indentNextLinePattern?: string
  zeroIndentPattern?: string
  disableIndentCorrections?: boolean
  /** Extra opener characters merged onto CodeMirror defaults. */
  closeBrackets?: string[]
}

export type CommentTokens = {
  line?: string
  block?: { open: string; close: string }
}

export type CompiledIndent = {
  increase: RegExp | null
  decrease: RegExp | null
  indentNext: RegExp | null
  zero: RegExp | null
  disableIndent: boolean
}

export function commentTokensOf(d: LanguageDataFields): CommentTokens | undefined {
  const line = d.lineComment?.trim()
  const open = d.blockCommentStart?.trim()
  const close = d.blockCommentEnd?.trim()
  const tokens: CommentTokens = {}
  if (line) tokens.line = line
  if (open && close) tokens.block = { open, close }
  if (!tokens.line && !tokens.block) return undefined
  return tokens
}

export function compileRegex(src: string | undefined): RegExp | null {
  const s = src?.trim()
  if (!s) return null
  try {
    return new RegExp(s)
  } catch {
    return null
  }
}

export function compileIndent(d: LanguageDataFields): CompiledIndent | null {
  const increase = compileRegex(d.increaseIndentPattern)
  const decrease = compileRegex(d.decreaseIndentPattern)
  const indentNext = compileRegex(d.indentNextLinePattern)
  const zero = compileRegex(d.zeroIndentPattern)
  if (!increase && !decrease && !indentNext && !zero && !d.disableIndentCorrections) {
    return null
  }
  return {
    increase,
    decrease,
    indentNext,
    zero,
    disableIndent: !!d.disableIndentCorrections,
  }
}

export function matchesPattern(re: RegExp | null, text: string): boolean {
  if (!re) return false
  re.lastIndex = 0
  return re.test(text)
}

/** Leading whitespace width in columns. */
export function leadingIndent(text: string, tabSize: number): number {
  let n = 0
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]
    if (ch === ' ') n++
    else if (ch === '\t') n += tabSize
    else break
  }
  return n
}

/**
 * Indent columns for `lineText` given the previous non-empty line.
 * Pure helper so tests do not need a full editor.
 */
export function computeIndent(opts: {
  prevText: string | null
  lineText: string
  unit: number
  tabSize: number
  patterns: CompiledIndent
}): number {
  const { prevText, lineText, unit, tabSize, patterns } = opts
  const empty = !/\S/.test(lineText)

  if (matchesPattern(patterns.zero, lineText)) return 0
  if (matchesPattern(patterns.decrease, lineText)) {
    let base = prevText != null ? leadingIndent(prevText, tabSize) : 0
    if (
      prevText != null &&
      (matchesPattern(patterns.increase, prevText) || matchesPattern(patterns.indentNext, prevText))
    ) {
      base += unit
    }
    return Math.max(0, base - unit)
  }
  if (!empty && patterns.disableIndent) {
    return leadingIndent(lineText, tabSize)
  }
  if (prevText == null) return 0
  let base = leadingIndent(prevText, tabSize)
  if (matchesPattern(patterns.increase, prevText) || matchesPattern(patterns.indentNext, prevText)) {
    base += unit
  }
  return base
}

function previousNonEmptyText(ctx: IndentContext, lineFrom: number): string | null {
  if (lineFrom <= 0) return null
  let pos = lineFrom - 1
  while (pos >= 0) {
    const line = ctx.lineAt(pos)
    if (/\S/.test(line.text)) return line.text
    if (line.from === 0) return null
    pos = line.from - 1
  }
  return null
}

export function indentUsing(ctx: IndentContext, pos: number, patterns: CompiledIndent): number {
  const line = ctx.lineAt(pos)
  return computeIndent({
    prevText: previousNonEmptyText(ctx, line.from),
    lineText: line.text,
    unit: ctx.unit,
    tabSize: ctx.state.tabSize,
    patterns,
  })
}

/** HTML/XML void elements — same set as bundles/HTML insert-tag. */
const VOID_TAG =
  /^(?:area|base|basefont|br|col|embed|frame|hr|img|input|isindex|link|meta|param|source|track|wbr)$/i

const OPEN_TAG_END = /<([A-Za-z][-A-Za-z0-9._]*)\b[^>]*?>$/

/** True when the caret sits in an empty open/close tag pair (`<div>|</div>`). */
export function betweenTagPair(before: string, after: string): { name: string } | null {
  const left = before.replace(/\s+$/, '')
  const right = after.replace(/^\s+/, '')
  const open = OPEN_TAG_END.exec(left)
  if (!open) return null
  if (/\/\s*>$/.test(left)) return null
  const name = open[1]
  if (VOID_TAG.test(name)) return null
  const close = new RegExp(`^</${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*>`, 'i')
  if (!close.test(right)) return null
  return { name }
}

const BRACKET_CLOSE: Record<string, string> = { '(': ')', '[': ']', '{': '}' }

/** True when the caret sits in empty `{}` / `()` / `[]` (TM Source Return Inside Empty Item). */
export function betweenBracketPair(before: string, after: string): boolean {
  const left = before.replace(/\s+$/, '')
  const right = after.replace(/^\s+/, '')
  if (!left) return false
  const close = BRACKET_CLOSE[left[left.length - 1]!]
  return !!close && right.startsWith(close)
}

/**
 * TextMate "Return Inside Empty …": `<div>|</div>` or `{|}` → a newline, an
 * indented inner line, and the closer on its own line. increaseIndentPattern
 * cannot see the closer on the same line.
 */
export function insertNewlineBetweenPairs(view: EditorView): boolean {
  const { state } = view
  const sel = state.selection.main
  if (!sel.empty) return false
  const pos = sel.head
  const line = state.doc.lineAt(pos)
  const before = line.text.slice(0, pos - line.from)
  const after = line.text.slice(pos - line.from)
  if (!betweenTagPair(before, after) && !betweenBracketPair(before, after)) return false

  const left = before.replace(/\s+$/, '')
  const rightTrim = after.search(/\S/)
  const from = line.from + left.length
  const to = rightTrim < 0 ? pos : pos + rightTrim
  const base = leadingIndent(line.text, state.tabSize)
  const inner = indentString(state, base + getIndentUnit(state))
  const outer = indentString(state, base)
  const insert = `\n${inner}\n${outer}`
  view.dispatch({
    changes: { from, to, insert },
    selection: EditorSelection.cursor(from + 1 + inner.length),
    userEvent: 'input',
  })
  return true
}

export const insertNewlineBetweenTags = insertNewlineBetweenPairs

/** CodeMirror defaults plus backtick (Source TM; CM's built-in list omits it). */
const BASE_BRACKETS = ['(', '[', '{', "'", '"', '`']

/** Self-pairing chars the skip-inside-quote handler applies to — not `(`/`[`/`{`/`<`. */
const SELF_PAIR_QUOTES = new Set(["'", '"', '`', '_', '*'])

export type CloseBracketsConfig = {
  brackets: string[]
  before: string
}

export function closeBracketsOf(d: LanguageDataFields): CloseBracketsConfig {
  const brackets = [...BASE_BRACKETS]
  const seen = new Set(brackets)
  for (const raw of d.closeBrackets ?? []) {
    const ch = raw.trim()
    if (!ch || [...ch].length !== 1 || seen.has(ch)) continue
    seen.add(ch)
    brackets.push(ch)
  }
  return { brackets, before: ')]}:;>' }
}

/**
 * True when an odd number of unescaped `quote` chars sit before `pos` on its line.
 * Used to skip auto-pairing `"` / `'` / `` ` `` / `_` / `*` inside an open pair.
 */
export function insidePairedQuote(doc: string, pos: number, quote: string): boolean {
  if (quote.length !== 1) return false
  let lineStart = pos
  while (lineStart > 0 && doc[lineStart - 1] !== '\n') lineStart--
  let open = false
  const escapeQuotes = quote === '"' || quote === "'" || quote === '`'
  for (let i = lineStart; i < pos; i++) {
    const ch = doc[i]
    if (ch === '\\' && escapeQuotes) {
      i++
      continue
    }
    if (ch === quote) open = !open
  }
  return open
}

function skipPairedQuoteHandler(): Extension {
  return Prec.high(
    EditorView.inputHandler.of((view, from, to, insert) => {
      if (insert.length !== 1 || from !== to) return false
      const sel = view.state.selection.main
      if (!sel.empty || from !== sel.from) return false
      const conf = view.state.languageDataAt('closeBrackets', from)[0] as CloseBracketsConfig | undefined
      const brackets = conf?.brackets ?? BASE_BRACKETS
      if (!SELF_PAIR_QUOTES.has(insert) || !brackets.includes(insert)) return false
      const line = view.state.doc.lineAt(from)
      if (!insidePairedQuote(line.text, from - line.from, insert)) return false
      view.dispatch({
        changes: { from, to, insert },
        selection: EditorSelection.cursor(from + 1),
        userEvent: 'input.type',
      })
      return true
    }),
  )
}

/** Facets for a language-data record (tests can pass fields directly). */
export function languageDataExtension(fields: LanguageDataFields): Extension {
  const tokens = commentTokensOf(fields)
  const indent = compileIndent(fields)
  const extras = (fields.closeBrackets ?? []).some((c) => c.trim())
  if (!tokens && !indent && !extras) return []
  const facets: Extension[] = []
  const data: Record<string, unknown> = {
    closeBrackets: closeBracketsOf(fields),
  }
  if (tokens) data.commentTokens = tokens
  if (indent?.decrease) data.indentOnInput = indent.decrease
  facets.push(EditorState.languageData.of(() => [data]))
  if (indent) {
    facets.push(indentService.of((ctx, pos) => indentUsing(ctx, pos, indent)))
  }
  facets.push(skipPairedQuoteHandler())
  return facets
}

/** Language data for the TextMate scope currently highlighting the buffer. */
export function textmateLanguageData(scopeName: string): Extension {
  const meta = getGrammarMeta(scopeName)
  if (!meta) return []
  return languageDataExtension(meta)
}
