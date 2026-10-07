import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, wrapItalic(ctx.input))
}

function wrapItalic(s: string): string {
  if (s.startsWith("**") || s.startsWith("__")) {
    return "_" + s + "_"
  }
  if (s.startsWith("_") && s.endsWith("_") && s.length >= 2) {
    return s.slice(1, -1)
  }
  if (s.startsWith("*") && s.endsWith("*") && s.length >= 2) {
    return s.slice(1, -1)
  }
  return "_" + s + "_"
}
