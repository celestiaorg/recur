import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const n = Math.max(1, ctx.tabSize || 4)
  const re = new RegExp(`^(?: {${n}})+`, "gm")
  const text = ctx.input.replace(re, (m) => "\t".repeat(m.length / n))
  return replaceEdited(ctx, text)
}
