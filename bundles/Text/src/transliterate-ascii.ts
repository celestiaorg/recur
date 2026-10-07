import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const folded = ctx.input
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .replace(/[—–]/g, "-")
    .replace(/[“”]/g, '"')
    .replace(/[‘’]/g, "'")
  return replaceEdited(ctx, folded.replace(/[^\t\n\r\x20-\x7E]/g, ""))
}
