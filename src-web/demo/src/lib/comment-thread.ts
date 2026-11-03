/** Top-level conversation rows: skip the description and nested review kids. */
export function isTimelineRoot(c: { kind?: string; parentId?: string; inReplyToId?: string } | null | undefined): boolean {
  if (!c || c.kind === 'description' || c.kind === 'hidden') return false
  if ((c.inReplyToId ?? '').trim()) return false
  return !(c.parentId ?? '').trim()
}

/** Root line comments submitted with a review, oldest first. */
export function reviewChildren<T extends { id?: string; parentId?: string; inReplyToId?: string; at?: string }>(
  items: T[],
  parentId: string,
): T[] {
  if (!parentId) return []
  return items
    .filter((c) => c.parentId === parentId && !(c.inReplyToId ?? '').trim())
    .slice()
    .sort((a, b) => {
      const at = (a.at ?? '').localeCompare(b.at ?? '')
      if (at) return at
      return (a.id ?? '').localeCompare(b.id ?? '')
    })
}

/** Replies on a line-note thread, oldest first. */
export function threadReplies<T extends { id?: string; inReplyToId?: string; at?: string }>(
  items: T[],
  rootId: string,
): T[] {
  if (!rootId) return []
  const ids = new Set<string>([rootId])
  const rest = rootId.replace(/^gh-inline-/, '')
  if (rest && rest !== rootId) ids.add(rest)
  return items
    .filter((c) => {
      const to = (c.inReplyToId ?? '').trim()
      return ids.has(to) || ids.has(to.replace(/^gh-inline-/, ''))
    })
    .slice()
    .sort((a, b) => {
      const at = (a.at ?? '').localeCompare(b.at ?? '')
      if (at) return at
      return (a.id ?? '').localeCompare(b.id ?? '')
    })
}

export function isLineNote(c: { id?: string; path?: string; inReplyToId?: string; diffHunk?: string } | null | undefined): boolean {
  if (!c) return false
  return !!(c.path || c.inReplyToId || c.diffHunk || (c.id ?? '').startsWith('gh-inline-'))
}

export function sameCommentPath(a?: string, b?: string): boolean {
  const x = (a ?? '').trim()
  const y = (b ?? '').trim()
  if (!x || !y) return false
  return x === y || x.endsWith('/' + y) || y.endsWith('/' + x)
}

/** Root of a posted line-note thread (not a reply). */
export function isLineNoteRoot(c: { path?: string; line?: number; inReplyToId?: string; id?: string; diffHunk?: string; outdated?: boolean } | null | undefined): boolean {
  if (!c || c.outdated || !isLineNote(c) || (c.inReplyToId ?? '').trim()) return false
  return !!(c.path && (c.line ?? 0) > 0)
}

type DiffRow = { old?: string; new?: string; type?: string }
type LineNoteShape = {
  id?: string
  path?: string
  line?: number
  side?: string
  startLine?: number
  startSide?: string
  inReplyToId?: string
  diffHunk?: string
  resolved?: boolean
  outdated?: boolean
}

export function lineNoteRange(c: { line?: number; startLine?: number }): { start: number; end: number } {
  const end = c.line ?? 0
  const start = c.startLine ?? 0
  if (end <= 0) return { start: 0, end: 0 }
  if (start > 0 && start < end) return { start, end }
  return { start: end, end }
}

function resolveNotePath(path: string, filePaths: readonly string[]): string {
  if (!path) return ''
  if (!filePaths.length) return path
  const exact = filePaths.find((p) => p === path)
  if (exact) return exact
  return filePaths.find((p) => sameCommentPath(p, path)) || path
}

/** Unresolved line-note roots grouped by review-file path. */
export function unresolvedLineNotesByPath<T extends LineNoteShape>(
  items: readonly T[],
  filePaths: readonly string[] = [],
): Map<string, T[]> {
  const m = new Map<string, T[]>()
  for (const c of items) {
    if (!isLineNoteRoot(c) || c.resolved) continue
    const raw = (c.path ?? '').trim()
    if (!raw) continue
    const p = resolveNotePath(raw, filePaths)
    m.set(p, [...(m.get(p) ?? []), c])
  }
  for (const list of m.values()) {
    list.sort((a, b) => (a.line ?? 0) - (b.line ?? 0) || (a.id ?? '').localeCompare(b.id ?? ''))
  }
  return m
}

export function commentsForPath<T>(
  byPath: ReadonlyMap<string, T[]> | undefined,
  path: string,
): T[] {
  if (!byPath || !path) return []
  const exact = byPath.get(path)
  if (exact?.length) return exact
  for (const [p, cs] of byPath) {
    if (!cs.length) continue
    if (sameCommentPath(p, path)) return cs
  }
  return []
}

export function commentCountForPath(
  byPath: ReadonlyMap<string, unknown[]> | undefined,
  path: string,
  isDir = false,
): number {
  if (!byPath || !path) return 0
  if (!isDir) return commentsForPath(byPath, path).length
  const prefix = path + '/'
  let n = 0
  for (const [p, cs] of byPath) {
    if (p === path || p.startsWith(prefix)) n += cs.length
  }
  return n
}

function rowLine(raw?: string): number {
  return parseInt(raw || '0', 10) || 0
}

/** Posted line-note roots anchored to this diff row (and optional split side). */
export function lineNotesOnRow<T extends LineNoteShape>(
  items: T[],
  path: string,
  line: DiffRow,
  side?: 'left' | 'right',
): T[] {
  const oldN = rowLine(line.old)
  const newN = rowLine(line.new)
  return items
    .filter((c) => {
      if (!isLineNoteRoot(c) || !sameCommentPath(c.path, path)) return false
      const want = (c.side || 'RIGHT').toUpperCase()
      const n = c.line ?? 0
      if (side === 'left') return want === 'LEFT' && oldN === n
      if (side === 'right') return want === 'RIGHT' && newN === n
      if (line.type === 'del') return want === 'LEFT' && oldN === n
      if (line.type === 'add') return want === 'RIGHT' && newN === n
      return want === 'LEFT' ? oldN === n : newN === n
    })
    .slice()
    .sort((a, b) => (a.id ?? '').localeCompare(b.id ?? ''))
}

function noteHitsRow(c: LineNoteShape, line: DiffRow, side?: 'left' | 'right'): boolean {
  if (!isLineNoteRoot(c)) return false
  const { start, end } = lineNoteRange(c)
  if (end <= 0) return false
  const want = (c.side || 'RIGHT').toUpperCase()
  const oldN = rowLine(line.old)
  const newN = rowLine(line.new)
  if (side === 'left' || line.type === 'del') return want === 'LEFT' && oldN >= start && oldN <= end
  if (side === 'right' || line.type === 'add') return want === 'RIGHT' && newN >= start && newN <= end
  return want === 'LEFT' ? oldN >= start && oldN <= end : newN >= start && newN <= end
}

/** Line-note roots whose GitHub range includes this diff row. */
export function lineNotesCoveringRow<T extends LineNoteShape>(
  items: T[],
  path: string,
  line: DiffRow,
  side?: 'left' | 'right',
): T[] {
  return items
    .filter((c) => sameCommentPath(c.path, path) && noteHitsRow(c, line, side))
    .slice()
    .sort((a, b) => (a.id ?? '').localeCompare(b.id ?? ''))
}

export function commentSource(c: { bodyMd?: string; bodyHtml?: string } | null | undefined): string {
  const md = (c?.bodyMd ?? '').trim()
  if (md) return md
  return (c?.bodyHtml ?? '').replace(/<[^>]+>/g, '').trim()
}
