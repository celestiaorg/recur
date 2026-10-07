import { parseJSONC, replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  try {
    const value = parseJSONC(ctx.input)
    const indent = ctx.softTabs === false ? "\t" : " ".repeat(Math.max(2, ctx.tabSize || 2))
    let text = JSON.stringify(value, null, indent)
    if (ctx.input.endsWith("\n")) text += "\n"
    return replaceEdited(ctx, text)
  } catch (err) {
    throw new Error(`JSON format failed: ${err instanceof Error ? err.message : String(err)}`)
  }
}
