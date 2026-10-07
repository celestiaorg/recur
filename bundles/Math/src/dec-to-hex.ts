import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, "0x" + parseDecish(ctx.input).toString(16))
}

function parseDecish(s: string): bigint {
  const t = s.trim().replace(/_/g, "")
  if (!t) throw new Error("Nothing to convert")
  if (/^0x[0-9a-fA-F]+$/i.test(t) || /^0b[01]+$/i.test(t) || /^0o[0-7]+$/i.test(t)) {
    return BigInt(t)
  }
  if (/^[+-]?\d+$/.test(t)) return BigInt(t)
  throw new Error("Not an integer")
}
