import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

/** Sample typescript command: replace with your transform. */
export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.toUpperCase())
}
