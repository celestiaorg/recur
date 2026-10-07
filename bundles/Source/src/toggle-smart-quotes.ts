import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

const FANCY =
  /[\u201C\u201D\u201E\u2018\u2019\u201A\u00AB\u00BB\u2039\u203A]/

function straighten(s: string): string {
  return s
    .replace(/[\u201C\u201D\u201E\u00AB\u00BB]/g, '"')
    .replace(/[\u2018\u2019\u201A\u2039\u203A]/g, "'")
}

/** Pair straight quotes into curly open/close; mid-word ' → apostrophe. */
function curlify(s: string): string {
  let out = ""
  let inDouble = false
  let inSingle = false
  for (let i = 0; i < s.length; i++) {
    const ch = s[i]!
    const prev = i > 0 ? s[i - 1]! : ""
    if (ch === '"') {
      out += inDouble ? "\u201D" : "\u201C"
      inDouble = !inDouble
      continue
    }
    if (ch === "'") {
      if (/[A-Za-z0-9]/.test(prev)) {
        out += "\u2019" // apostrophe / close after word
        continue
      }
      out += inSingle ? "\u2019" : "\u2018"
      inSingle = !inSingle
      continue
    }
    out += ch
  }
  return out
}

/** Toggle between straight and smart (curly) quotes. */
export default function (ctx: MacroContext): MacroResult {
  const s = ctx.input
  const text = FANCY.test(s) ? straighten(s) : curlify(s)
  return replaceEdited(ctx, text)
}
