import { parseJSONC, replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  try {
    const value = parseJSONC(ctx.input)
    return replaceEdited(ctx, JSON.stringify(value))
  } catch (err) {
    throw new Error(`JSON minify failed: ${err instanceof Error ? err.message : String(err)}`)
  }
}
