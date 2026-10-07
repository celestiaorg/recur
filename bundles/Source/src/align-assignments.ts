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

/** Align `=` / `:=` / `=>` / `:` assignments in the selected block. */
export default function (ctx: MacroContext): MacroResult {
  const lines = linesOf(ctx.input)
  const trailing = lines.length && lines[lines.length - 1] === "" ? lines.pop()! : null
  const re = /(.*?)([ \t]*)(:=|=>|=|:)(.*)$/
  type Part = { left: string; op: string; right: string } | null
  const parts: Part[] = lines.map((line) => {
    if (!line.trim() || line.trimStart().startsWith("//") || line.trimStart().startsWith("#")) {
      return null
    }
    const m = line.match(re)
    if (!m || m[1].trim() === "") return null
    // Skip comparisons: `==` leaves a leading `=` in right; `!=`/`<=`/`>=` end left with the op char.
    if (m[3] === "=" && (/[=!<>]$/.test(m[1]) || m[4].startsWith("="))) return null
    return { left: m[1].replace(/[ \t]+$/, ""), op: m[3], right: m[4] }
  })
  let maxLeft = 0
  for (const p of parts) {
    if (p) maxLeft = Math.max(maxLeft, p.left.length)
  }
  const out = lines.map((line, i) => {
    const p = parts[i]
    if (!p) return line
    const pad = " ".repeat(Math.max(1, maxLeft - p.left.length + 1))
    const right = p.right.startsWith(" ") || p.right === "" ? p.right : " " + p.right
    return p.left + pad + p.op + right
  })
  if (trailing !== null) out.push(trailing)
  return replaceEdited(ctx, joinLines(out))
}
