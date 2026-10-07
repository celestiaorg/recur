import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const s = ctx.input
  if (s.startsWith("~~") && s.endsWith("~~") && s.length >= 4) {
    return replaceEdited(ctx, s.slice(2, -2))
  }
  return replaceEdited(ctx, "~~" + s + "~~")
}
