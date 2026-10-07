/** Pair a unified diff into side-by-side rows (GitHub-style: dels zip with following adds). */

export type SplitDiffLine = {
  old: string
  new: string
  type: string
  codeHtml: string
  extraHtml?: string
}

export type SplitCell = { i: number; line: SplitDiffLine }

export type SplitRow =
  | { kind: 'hunk'; i: number; line: SplitDiffLine }
  | { kind: 'pair'; i: number; left: SplitCell | null; right: SplitCell | null }

export function splitDiffRows(lines: SplitDiffLine[]): SplitRow[] {
  const out: SplitRow[] = []
  let i = 0
  while (i < lines.length) {
    const line = lines[i]
    if (line.type === 'hunk') {
      out.push({ kind: 'hunk', i, line })
      i++
      continue
    }
    if (line.type !== 'del' && line.type !== 'add') {
      out.push({ kind: 'pair', i, left: { i, line }, right: { i, line } })
      i++
      continue
    }
    const dels: SplitCell[] = []
    const adds: SplitCell[] = []
    while (i < lines.length && lines[i].type === 'del') {
      dels.push({ i, line: lines[i] })
      i++
    }
    while (i < lines.length && lines[i].type === 'add') {
      adds.push({ i, line: lines[i] })
      i++
    }
    const n = Math.max(dels.length, adds.length)
    for (let k = 0; k < n; k++) {
      const left = dels[k] ?? null
      const right = adds[k] ?? null
      out.push({ kind: 'pair', i: (left ?? right)!.i, left, right })
    }
  }
  return out
}

/** Same identity as DiffFile's split `data-line`: prefer the right/new number. */
export function splitRowDataLine(row: SplitRow): number {
  if (row.kind !== 'pair') return 0
  const line = row.right?.line ?? row.left?.line
  if (!line) return 0
  return parseInt(line.new || line.old || '0', 10) || 0
}

/** Posted thread on this file: jump to its unified row, not the first matching number. */
export function slotOfUnifiedHighlight(
  lines: readonly { old: string; new: string; type?: string }[],
  slots: readonly number[],
  n: number,
  threads: readonly { line?: number; side?: string }[] = [],
): number {
  if (!n) return -1
  const thread = threads.find((t) => (t.line ?? 0) === n)
  if (thread) {
    const left = (thread.side || 'RIGHT').toUpperCase() === 'LEFT'
    for (let s = 0; s < slots.length; s++) {
      const l = lines[slots[s]]
      if (!l || l.type === 'hunk') continue
      const raw = left ? l.old : l.new
      if ((parseInt(raw || '0', 10) || 0) === n) return s
    }
  }
  for (let s = 0; s < slots.length; s++) {
    const l = lines[slots[s]]
    if (l && l.type !== 'hunk' && (parseInt(l.new || l.old || '0', 10) || 0) === n) return s
  }
  return -1
}

/** Posted thread on this file: jump to its split cell, not the row's data-line. */
export function slotOfSplitHighlight(
  slots: readonly SplitRow[],
  n: number,
  threads: readonly { line?: number; side?: string }[] = [],
): number {
  if (!n) return -1
  const thread = threads.find((t) => (t.line ?? 0) === n)
  if (thread) {
    const left = (thread.side || 'RIGHT').toUpperCase() === 'LEFT'
    for (let s = 0; s < slots.length; s++) {
      const row = slots[s]
      if (row.kind !== 'pair') continue
      const cell = left ? row.left : row.right
      const raw = cell ? (left ? cell.line.old : cell.line.new) : ''
      if ((parseInt(raw || '0', 10) || 0) === n) return s
    }
  }
  for (let s = 0; s < slots.length; s++) {
    if (splitRowDataLine(slots[s]) === n) return s
  }
  return -1
}
