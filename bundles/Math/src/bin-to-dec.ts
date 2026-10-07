import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const t = ctx.input.trim().replace(/_/g, "")
  if (!t) throw new Error("Nothing to convert")
  let n: bigint
  if (/^0b[01]+$/i.test(t)) n = BigInt(t)
  else if (/^[01]+$/.test(t)) n = BigInt("0b" + t)
  else throw new Error("Not a binary integer")
  return replaceEdited(ctx, n.toString(10))
}
