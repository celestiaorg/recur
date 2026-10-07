import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  try {
    const bin = atob(ctx.input.trim())
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    return replaceEdited(ctx, new TextDecoder().decode(bytes))
  } catch (err) {
    throw new Error(`Base64 decode failed: ${err instanceof Error ? err.message : String(err)}`)
  }
}
