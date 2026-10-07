import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

/**
 * Replace em dashes (—) and en dashes (–) with a colon, normalizing spaces
 * around the dash to ": " (handy for AI clause breaks).
 */
export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, ctx.input.replace(/\s*[\u2014\u2013]\s*/g, ": "))
}
