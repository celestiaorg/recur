import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.toLowerCase())
}
