import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const s = ctx.input
  let text: string
  if (s.includes("'") && !s.includes('"')) {
    text = s.replace(/'/g, '"')
  } else if (s.includes('"') && !s.includes("'")) {
    text = s.replace(/"/g, "'")
  } else {
    // Mixed or neither: flip whichever is more common.
    const singles = (s.match(/'/g) || []).length
    const doubles = (s.match(/"/g) || []).length
    text = singles >= doubles ? s.replace(/'/g, '"') : s.replace(/"/g, "'")
  }
  return replaceEdited(ctx, text)
}
