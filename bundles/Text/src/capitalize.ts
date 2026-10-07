import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const s = ctx.input
  const text = s ? s.charAt(0).toUpperCase() + s.slice(1) : s
  return replaceEdited(ctx, text)
}
