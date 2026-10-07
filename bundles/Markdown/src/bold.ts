import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, wrapFence(ctx.input, ["**", "__"]))
}

function wrapFence(s: string, fences: string[]): string {
  for (const f of fences) {
    if (s.startsWith(f) && s.endsWith(f) && s.length >= 2 * f.length) {
      return s.slice(f.length, s.length - f.length)
    }
  }
  return fences[0]! + s + fences[0]
}
