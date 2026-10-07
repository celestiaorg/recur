import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  return replaceEdited(ctx, bumpHeadings(ctx.input, 1))
}

function bumpHeadings(text: string, delta: 1 | -1): string {
  const ends = text.endsWith("\n")
  const body = ends ? text.slice(0, -1) : text
  const out = body.split("\n").map((line) => bumpLine(line, delta)).join("\n")
  return ends ? out + "\n" : out
}

function bumpLine(line: string, delta: 1 | -1): string {
  const m = line.match(/^(#{1,6})([ \t]*)(.*)$/)
  if (m) {
    let n = m[1]!.length + delta
    const rest = m[3]!
    if (n <= 0) return rest
    if (n > 6) n = 6
    const space = rest ? (m[2] || " ") : ""
    return "#".repeat(n) + space + rest
  }
  if (delta > 0 && line.trim()) return "# " + line
  return line
}
