import { Output, type MacroContext, type MacroResult } from '@recur/sdk'

/** HTML void elements that take no close tag (TM Insert Tag Pair). */
const VOID_NO_ARG = /^(?:br|hr)$/i
const VOID = /^(?:area|base|basefont|br|col|embed|frame|hr|img|input|isindex|link|meta|param|source|track|wbr)$/i
const TAG = /^[\w\-:_]+$/i

/** `$1` / `${…}` / `{` cannot be escaped in a CodeMirror snippet template. */
function snippetUnsafe(s: string): boolean {
  return /[#$]\{|\$\d|[{}]/.test(s)
}

/** Left of caret is the tag, right of caret stays after the pair (TM). */
export function splitAtCaret(ctx: Pick<MacroContext, 'selectedText' | 'currentLine' | 'column'>, raw: string): {
  tag: string
  suffix: string
} {
  if (ctx.selectedText) return { tag: raw, suffix: '' }
  const line = ctx.currentLine || ''
  const col = ctx.column
  if (!raw || !line || col < 1) return { tag: raw, suffix: '' }
  const caret = col - 1
  let from = -1
  for (let i = 0; i < line.length; ) {
    const idx = line.indexOf(raw, i)
    if (idx < 0) break
    if (caret >= idx && caret <= idx + raw.length) {
      from = idx
      break
    }
    i = idx + 1
  }
  if (from < 0) return { tag: raw, suffix: '' }
  const offset = caret - from
  if (offset >= raw.length) return { tag: raw, suffix: '' }
  return { tag: raw.slice(0, offset), suffix: raw.slice(offset) }
}

export function insertOpenCloseTag(tag: string): string {
  if (VOID_NO_ARG.test(tag)) return `<${tag}>`
  if (VOID.test(tag)) return `<${tag} $1>`
  if (TAG.test(tag)) {
    const close = tag.trim().split(/\s+/)[0] || tag
    return `<${tag}>$1</${close}>`
  }
  return `${tag}<\${1:p}>$2</$1>`
}

/** Plain-text fallback when tag/suffix would be parsed as snippet fields. */
function insertOpenCloseTagLiteral(tag: string, suffix: string): string {
  if (VOID_NO_ARG.test(tag)) return `<${tag}>${suffix}`
  if (VOID.test(tag)) return `<${tag}>${suffix}`
  if (TAG.test(tag)) {
    const close = tag.trim().split(/\s+/)[0] || tag
    return `<${tag}></${close}>${suffix}`
  }
  return `${tag}<p></p>${suffix}`
}

export default function (ctx: MacroContext): MacroResult {
  const { tag, suffix } = splitAtCaret(ctx, ctx.input || '')
  if (snippetUnsafe(tag) || snippetUnsafe(suffix)) {
    return { text: insertOpenCloseTagLiteral(tag, suffix), output: Output.insertText }
  }
  return {
    text: insertOpenCloseTag(tag) + suffix,
    output: Output.insertSnippet,
  }
}
