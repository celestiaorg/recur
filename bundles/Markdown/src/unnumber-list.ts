import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const ends = ctx.input.endsWith("\n")
  const body = ends ? ctx.input.slice(0, -1) : ctx.input
  const out = body.split("\n").map((line) =>
    line.replace(/^(\s*)\d+\.[ \t]+/, "$1")
  ).join("\n")
  return replaceEdited(ctx, ends ? out + "\n" : out)
}
