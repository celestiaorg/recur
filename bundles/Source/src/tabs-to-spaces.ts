import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const n = Math.max(1, ctx.tabSize || 4)
  const pad = " ".repeat(n)
  const text = ctx.input.replace(/^\t+/gm, (m) => pad.repeat(m.length))
  return replaceEdited(ctx, text)
}
