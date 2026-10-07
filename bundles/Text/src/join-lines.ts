import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const sep = ctx.prompts.separator ?? " "
  const ends = ctx.input.endsWith("\n")
  const body = ends ? ctx.input.slice(0, -1) : ctx.input
  let text = body.split("\n").join(sep)
  if (ends) text += "\n"
  return replaceEdited(ctx, text)
}
