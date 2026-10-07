import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, wrapCode(ctx.input))
}

function wrapCode(s: string): string {
  if (s.startsWith("`") && s.endsWith("`") && s.length >= 2) {
    const inner = s.slice(1, -1)
    if (!inner.startsWith("`")) return inner
  }
  const ticks = s.includes("`") ? "``" : "`"
  return ticks + s + ticks
}
