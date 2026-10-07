import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

/** Replace em dashes (—) and en dashes (–) with ASCII hyphens. */
export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.replace(/[\u2014\u2013]/g, "-"))
}
