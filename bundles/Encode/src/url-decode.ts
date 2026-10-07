import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  try {
    return replaceEdited(ctx, decodeURIComponent(ctx.input.replace(/\+/g, "%20")))
  } catch (err) {
    throw new Error(`URL decode failed: ${err instanceof Error ? err.message : String(err)}`)
  }
}
