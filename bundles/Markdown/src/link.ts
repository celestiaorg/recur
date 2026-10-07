import { Output, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const inner = escapeSnippet(ctx.selectedText || ctx.input || "text")
  return {
    text: `[${inner}](\${1:url})`,
    output: Output.insertSnippet,
  }
}

function escapeSnippet(s: string): string {
  return s.replace(/\\/g, "\\\\").replace(/\$/g, "\\$")
}
