/** Conversation snippets keep a few lines above the note, like GitHub. */
export const COMMENT_HUNK_CONTEXT = 3

/** One code row from a GitHub-style review-comment diff_hunk. */
export type HunkLine = {
  type: 'add' | 'del' | 'ctx'
  old: string
  new: string
  text: string
  /** The line the review comment is anchored to. */
  target?: boolean
  /** Inclusive range for a GitHub multiline comment. */
  inRange?: boolean
}

/** Turn a unified diff_hunk into numbered add/del/ctx rows (skips the @@ header). */
export function parseDiffHunk(hunk: string): HunkLine[] {
  const raw = hunk.replace(/\r\n/g, '\n').trim()
  if (!raw) return []
  let oldN = 0
  let newN = 0
  const out: HunkLine[] = []
  for (const line of raw.split('\n')) {
    const header = line.match(/^@@\s+-(\d+)(?:,\d+)?\s+\+(\d+)(?:,\d+)?/)
    if (header) {
      oldN = Number.parseInt(header[1], 10)
      newN = Number.parseInt(header[2], 10)
      continue
    }
    const ch = line.charAt(0)
    if (ch === '+') {
      out.push({ type: 'add', old: '', new: String(newN++), text: line.slice(1) })
      continue
    }
    if (ch === '-') {
      out.push({ type: 'del', old: String(oldN++), new: '', text: line.slice(1) })
      continue
    }
    if (ch === '\\') continue
    const text = ch === ' ' ? line.slice(1) : line
    out.push({ type: 'ctx', old: String(oldN++), new: String(newN++), text })
  }
  return out
}

/** Line range for a context header: `12` or `10-15`. */
export function commentHunkLineLabel(line?: number, startLine?: number): string {
  const end = line ?? 0
  if (end <= 0) return ''
  const start = startLine ?? 0
  return start > 0 && start < end ? `${start}-${end}` : String(end)
}

/** Combined `path:line` (or `path:start-end`) label. */
export function commentHunkPathLabel(path: string, line?: number, startLine?: number, outdated = false): string {
  const p = path.trim()
  if (!p) return ''
  const loc = commentHunkLineLabel(line, startLine)
  const base = loc ? `${p}:${loc}` : p
  return outdated ? `${base} (outdated)` : base
}

/** Index of the commented row; GitHub's hunk usually ends there. */
export function commentHunkTargetIndex(
  lines: readonly HunkLine[],
  line?: number,
  side?: string,
): number {
  if (!lines.length) return -1
  const n = line ?? 0
  const left = (side || 'RIGHT').toUpperCase() === 'LEFT'
  if (n > 0) {
    for (let i = lines.length - 1; i >= 0; i--) {
      const hl = lines[i]
      if (left) {
        if (hl.type !== 'add' && Number(hl.old) === n) return i
      } else if (hl.type !== 'del' && Number(hl.new) === n) {
        return i
      }
    }
  }
  return lines.length - 1
}

/** Keep `context` rows above the note (or range); drop the rest of the GitHub hunk. */
export function clipCommentHunk(
  lines: readonly HunkLine[],
  opts?: { line?: number; side?: string; startLine?: number; startSide?: string; context?: number },
): HunkLine[] {
  if (!lines.length) return []
  const end = commentHunkTargetIndex(lines, opts?.line, opts?.side)
  if (end < 0) return []
  const startLine = opts?.startLine ?? 0
  let from = end
  if (startLine > 0 && startLine !== (opts?.line ?? 0)) {
    const idx = commentHunkTargetIndex(lines, startLine, opts?.startSide || opts?.side)
    if (idx >= 0) from = Math.min(idx, end)
  }
  const ctx = opts?.context ?? COMMENT_HUNK_CONTEXT
  const start = Math.max(0, from - ctx)
  return lines.slice(start, end + 1).map((hl, i) => {
    const idx = start + i
    if (idx === end) return { ...hl, target: true, inRange: true }
    if (idx >= from) return { ...hl, inRange: true }
    return hl
  })
}
