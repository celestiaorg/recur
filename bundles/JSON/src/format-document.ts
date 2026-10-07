import { Output, parseJSONC, type MacroContext, type MacroResult } from '@recur/sdk'

/** willSave-safe: on parse error, return the original document unchanged. */
export default function (ctx: MacroContext): MacroResult {
  try {
    const value = parseJSONC(ctx.input)
    const indent = ctx.softTabs === false ? "\t" : " ".repeat(Math.max(2, ctx.tabSize || 2))
    let text = JSON.stringify(value, null, indent)
    if (ctx.input.endsWith("\n")) text += "\n"
    return { text, output: Output.replaceDocument }
  } catch {
    return { text: ctx.input, output: Output.replaceDocument }
  }
}
