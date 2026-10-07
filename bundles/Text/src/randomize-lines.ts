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
  // Fisher–Yates
  for (let i = lines.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1))
    ;[lines[i], lines[j]] = [lines[j], lines[i]]
  }
  if (trailing !== null) lines.push(trailing)
  return replaceEdited(ctx, joinLines(lines))
}
