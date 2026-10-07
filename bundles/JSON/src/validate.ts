import { Output, parseJSONC, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  try {
    parseJSONC(ctx.input)
    return { text: "Valid JSON", output: Output.showAsToast }
  } catch (err) {
    throw new Error(`Invalid JSON: ${err instanceof Error ? err.message : String(err)}`)
  }
}
