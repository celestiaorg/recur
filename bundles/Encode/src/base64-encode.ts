import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  // btoa needs Latin1; use UTF-8 via TextEncoder for general text.
  const bytes = new TextEncoder().encode(ctx.input)
  let bin = ""
  for (const b of bytes) bin += String.fromCharCode(b)
  return replaceEdited(ctx, btoa(bin))
}
