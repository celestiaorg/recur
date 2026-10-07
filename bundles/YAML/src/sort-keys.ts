import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'

export default function (ctx: MacroContext): MacroResult {
  const ends = ctx.input.endsWith("\n")
  const body = ends ? ctx.input.slice(0, -1) : ctx.input
  const sorted = sortLines(body.split("\n")).join("\n")
  return replaceEdited(ctx, ends ? sorted + "\n" : sorted)
}

function indentOf(line: string): number {
  let n = 0
  while (n < line.length && line[n] === " ") n++
  return n
}

function isBlankOrComment(line: string): boolean {
  const t = line.trim()
  return t === "" || t.startsWith("#")
}

function isListItem(line: string): boolean {
  const t = line.slice(indentOf(line))
  return t.startsWith("- ") || t === "-" || t.startsWith("-\t")
}

function mapKey(line: string): string | null {
  if (isListItem(line) || isBlankOrComment(line)) return null
  const t = line.slice(indentOf(line))
  const quoted = t.match(/^(['"])((?:\\.|(?!\1).)*)\1\s*:/)
  if (quoted) return quoted[2]!
  const m = t.match(/^([^:#\s][^:]*?)\s*:(\s|$)/)
  return m ? m[1]!.trim() : null
}

function sortLines(lines: string[]): string[] {
  if (!lines.length) return lines
  let indent = Infinity
  for (const line of lines) {
    if (isBlankOrComment(line) || !line.trim()) continue
    indent = Math.min(indent, indentOf(line))
  }
  if (!Number.isFinite(indent)) return lines
  return sortRange(lines, 0, lines.length, indent)
}

type Entry = { key: string | null; lines: string[] }

function sortRange(lines: string[], start: number, end: number, indent: number): string[] {
  const entries: Entry[] = []
  let i = start
  while (i < end) {
    const lead: string[] = []
    while (i < end && isBlankOrComment(lines[i]!)) {
      lead.push(lines[i]!)
      i++
    }
    if (i >= end) {
      if (lead.length) entries.push({ key: null, lines: lead })
      break
    }
    const head = lines[i]!
    const headIndent = indentOf(head)
    if (headIndent < indent) {
      entries.push({ key: null, lines: lead.concat(lines.slice(i, end)) })
      break
    }
    i++
    const childStart = i
    while (i < end) {
      const line = lines[i]!
      if (isBlankOrComment(line)) {
        i++
        continue
      }
      if (indentOf(line) <= headIndent) break
      i++
    }
    let childEnd = i
    while (childEnd > childStart && isBlankOrComment(lines[childEnd - 1]!)) childEnd--
    const child = childStart < childEnd
      ? sortRange(lines, childStart, childEnd, headIndent + 1)
      : []
    const trailing = lines.slice(childEnd, i)
    entries.push({ key: mapKey(head), lines: lead.concat([head], child, trailing) })
  }
  const allKeyed = entries.length > 1 && entries.every((e) => e.key != null)
  if (!allKeyed) return entries.flatMap((e) => e.lines)
  entries.sort((a, b) => (a.key! < b.key! ? -1 : a.key! > b.key! ? 1 : 0))
  return entries.flatMap((e) => e.lines)
}
