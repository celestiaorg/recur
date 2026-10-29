import type { Finding } from '../../bindings/recur/internal/domain/models'

/** Exact candidate first, then a unique slash-anchored suffix — a wrong-file hit is worse than a miss. */
export function matchFindingPath(path: string, candidates: readonly string[]): string {
  if (!path) return ''
  const exact = candidates.find((x) => x === path)
  if (exact) return exact
  const hits = candidates.filter((x) => x.endsWith('/' + path) || path.endsWith('/' + x))
  return hits.length === 1 ? hits[0] : ''
}

export function findingsForPath(
  byPath: ReadonlyMap<string, Finding[]> | undefined,
  path: string,
): Finding[] {
  if (!byPath || !path) return []
  const exact = byPath.get(path)
  if (exact?.length) return exact
  for (const [p, fs] of byPath) {
    if (!fs.length) continue
    if (p === path || path.endsWith('/' + p) || p.endsWith('/' + path)) {
      return fs
    }
  }
  return []
}

/** RIGHT-side (new) line numbers GitHub will accept on this diff. */
export function rightDiffLineSet(
  lines: readonly { old?: string; new?: string; type?: string }[] | undefined,
): Set<number> {
  const out = new Set<number>()
  for (const ln of lines ?? []) {
    if (ln.type === 'del' || ln.type === 'hunk') continue
    const n = parseInt(ln.new || '0', 10) || 0
    if (n) out.add(n)
  }
  return out
}

/** True when every inclusive line in [start, end] is a RIGHT-side diff row. */
export function rightDiffHasRange(
  lines: readonly { old?: string; new?: string; type?: string }[] | undefined,
  start: number,
  end: number,
): boolean {
  if (start <= 0 || end < start) return false
  const right = rightDiffLineSet(lines)
  for (let n = start; n <= end; n++) {
    if (!right.has(n)) return false
  }
  return true
}

/** 1-based new-side line, or 0 when the row has none (deleted / blank). */
function newLineNo(line: { new?: string; type?: string }): number {
  if (line.type === 'del') return 0
  return parseInt(line.new || '0', 10) || 0
}

/** Inclusive span; EndLine 0 or == Line is a single line. Mirrors Finding.CoversLine. */
export function findingCoversLine(f: { line?: number; endLine?: number }, lineNo: number): boolean {
  const start = f.line || 0
  if (start <= 0 || lineNo <= 0) return false
  const end = f.endLine && f.endLine > start ? f.endLine : start
  return lineNo >= start && lineNo <= end
}

/**
 * Findings whose **start** is this diff row's new line (card sits here).
 * Deleted rows have no new number and never match.
 */
export function findingsOnLine(
  findings: readonly Finding[],
  line: { new?: string; type?: string },
): Finding[] {
  const n = newLineNo(line)
  if (!n) return []
  return findings.filter((f) => (f.line || 0) === n)
}

/** Findings whose span includes this row's new line (range highlight). */
export function findingsCoveringLine(
  findings: readonly Finding[],
  line: { new?: string; type?: string },
): Finding[] {
  const n = newLineNo(line)
  if (!n) return []
  return findings.filter((f) => findingCoversLine(f, n))
}

/** Findings whose span **ends** on this row's new line (note-end gutter). */
export function findingsEndingLine(
  findings: readonly Finding[],
  line: { new?: string; type?: string },
): Finding[] {
  const n = newLineNo(line)
  if (!n) return []
  return findings.filter((f) => {
    const start = f.line || 0
    if (start <= 0) return false
    const end = f.endLine && f.endLine > start ? f.endLine : start
    return n === end
  })
}

/** Drop backend-projected `.flag` ExtraHTML so inline finding cards can replace it. */
export function extraHtmlWithoutFindingFlags(html: string | undefined): string {
  if (!html) return ''
  if (!html.includes('class="flag"')) return html
  return html.replace(/<div class="flag"[^>]*>[\s\S]*?<div class="fm">[\s\S]*?<\/div>\s*<\/div>\s*<\/div>/g, '').trim()
}
