import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

const map: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
}

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.replace(/[&<>"']/g, (c) => map[c] || c))
}
