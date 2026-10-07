import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const hex = ctx.input.replace(/0x/gi, "").replace(/[^0-9a-fA-F]/g, "")
  if (!hex) throw new Error("No hex digits")
  if (hex.length % 2) throw new Error("Odd number of hex digits")
  const bytes = new Uint8Array(hex.length / 2)
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return replaceEdited(ctx, new TextDecoder().decode(bytes))
}
