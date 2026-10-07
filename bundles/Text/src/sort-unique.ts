import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

function linesOf(text: string): string[] {
  if (text === "") return []
  const ends = text.endsWith("\n")
  const parts = text.replace(/\n$/, "").split("\n")
  return ends ? [...parts, ""] : parts
}

function joinLines(lines: string[]): string {
  if (lines.length === 0) return ""
  if (lines[lines.length - 1] === "" && lines.length > 1) {
    return lines.slice(0, -1).join("\n") + "\n"
  }
  return lines.join("\n")
}

export default function (ctx: MacroContext): MacroResult {
  const lines = linesOf(ctx.input)
  const trailing = lines.length && lines[lines.length - 1] === "" ? lines.pop()! : null
  const sorted = [...new Set(lines)].sort((a, b) => a.localeCompare(b))
  if (trailing !== null) sorted.push(trailing)
  return replaceEdited(ctx, joinLines(sorted))
}
