import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const ends = ctx.input.endsWith("\n")
  const body = ends ? ctx.input.slice(0, -1) : ctx.input
  let i = 0
  const out = body.split("\n").map((line) => {
    if (!line.trim()) return line
    i++
    const stripped = line.replace(/^(\s*)(?:\d+\.[ \t]+|[-*+][ \t]+)/, "$1")
    const indent = stripped.match(/^\s*/)?.[0] ?? ""
    return indent + i + ". " + stripped.trimStart()
  }).join("\n")
  return replaceEdited(ctx, ends ? out + "\n" : out)
}
