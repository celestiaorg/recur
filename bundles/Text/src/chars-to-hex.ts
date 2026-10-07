import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const bytes = new TextEncoder().encode(ctx.input)
  const text = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join(" ")
  return replaceEdited(ctx, text)
}
