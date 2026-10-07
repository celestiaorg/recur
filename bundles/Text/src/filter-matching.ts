import { Output, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const pattern = ctx.prompts.pattern ?? ""
  if (!pattern) throw new Error("pattern is required")
  let re: RegExp
  try {
    re = new RegExp(pattern)
  } catch (err) {
    throw new Error(`invalid regex: ${err instanceof Error ? err.message : String(err)}`)
  }
  const ends = ctx.input.endsWith("\n")
  const lines = ctx.input.replace(/\n$/, "").split("\n")
  const kept = lines.filter((l) => re.test(l))
  let text = kept.join("\n")
  if (ends && kept.length) text += "\n"
  return { text, output: Output.newDocument }
}
