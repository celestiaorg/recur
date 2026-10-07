import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const delim = ctx.prompts.delimiter ?? ","
  if (delim === "") return replaceEdited(ctx, ctx.input)
  const ends = ctx.input.endsWith("\n")
  const body = ends ? ctx.input.slice(0, -1) : ctx.input
  // Escape regex specials in delimiter.
  const re = new RegExp(delim.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g")
  let text = body.split(re).join("\n")
  if (ends) text += "\n"
  return replaceEdited(ctx, text)
}
