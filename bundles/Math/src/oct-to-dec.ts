import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const t = ctx.input.trim().replace(/_/g, "")
  if (!t) throw new Error("Nothing to convert")
  let n: bigint
  if (/^0o[0-7]+$/i.test(t)) n = BigInt(t)
  else if (/^[0-7]+$/.test(t)) n = BigInt("0o" + t)
  else throw new Error("Not an octal integer")
  return replaceEdited(ctx, n.toString(10))
}
