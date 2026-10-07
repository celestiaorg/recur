import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, wrapUnwrap(ctx.input, "{", "}"))
}

function wrapUnwrap(s: string, open: string, close: string): string {
  if (s.startsWith(open) && s.endsWith(close) && s.length >= open.length + close.length) {
    return s.slice(open.length, s.length - close.length)
  }
  return open + s + close
}
