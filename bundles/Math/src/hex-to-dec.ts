import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const t = ctx.input.trim().replace(/_/g, "").replace(/^#/, "")
  if (!t) throw new Error("Nothing to convert")
  let n: bigint
  if (/^0x[0-9a-fA-F]+$/i.test(t)) n = BigInt(t)
  else if (/^[0-9a-fA-F]+$/.test(t)) n = BigInt("0x" + t)
  else throw new Error("Not a hexadecimal integer")
  return replaceEdited(ctx, n.toString(10))
}
