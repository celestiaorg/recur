import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

const named: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: "\u00a0",
}

export default function (ctx: MacroContext): MacroResult {
  const text = ctx.input.replace(/&(#x?[0-9a-fA-F]+|\w+);/g, (full, body: string) => {
    if (body[0] === "#") {
      const hex = body[1] === "x" || body[1] === "X"
      const n = parseInt(body.slice(hex ? 2 : 1), hex ? 16 : 10)
      if (!Number.isFinite(n)) return full
      return String.fromCodePoint(n)
    }
    return named[body] ?? full
  })
  return replaceEdited(ctx, text)
}
