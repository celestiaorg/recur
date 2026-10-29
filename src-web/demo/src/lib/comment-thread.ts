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

/** Optimistic line-note replies the UI inserts before ReplyPRComment returns. */
export const PENDING_REPLY_PREFIX = 'c-pending-'

export function isPendingComment(c: { id?: string } | null | undefined): boolean {
  return (c?.id ?? '').startsWith(PENDING_REPLY_PREFIX)
}

export function sameLogin(a?: string, b?: string): boolean {
  return !!a && !!b && a.toLowerCase() === b.toLowerCase()
}

export function commentSource(c: { bodyMd?: string; bodyHtml?: string } | null | undefined): string {
  const md = (c?.bodyMd ?? '').trim()
  if (md) return md
  return (c?.bodyHtml ?? '').replace(/<[^>]+>/g, '').trim()
}

export function canEditPRComment(
  pr: { source?: string; viewerLogin?: string } | null | undefined,
  c: { id?: string; kind?: string; author?: string; authorLogin?: string } | null | undefined,
): boolean {
  if (!c || c.kind === 'action' || isPendingComment(c)) return false
  if (pr?.source !== 'gh') return c.author === 'You'
  return sameLogin(pr?.viewerLogin, c.authorLogin)
}

export function isReviewCard(c: { id?: string } | null | undefined): boolean {
  const id = c?.id ?? ''
  return id.startsWith('gh-review-') && !id.startsWith('gh-review-action-')
}

export function canDeletePRComment(
  pr: { source?: string; viewerLogin?: string; viewerCanWrite?: boolean } | null | undefined,
  c: { id?: string; kind?: string; author?: string; authorLogin?: string; path?: string; inReplyToId?: string; diffHunk?: string } | null | undefined,
): boolean {
  if (!c || isPendingComment(c) || c.kind === 'action' || c.kind === 'description' || c.kind === 'hidden') return false
  if (isReviewCard(c)) return false
  if (canEditPRComment(pr, c)) return true
  return pr?.source === 'gh' && !!pr?.viewerCanWrite
}

/** Authenticated viewers can react on conversation items (not commits / generic actions). */
export function canReactPRComment(
  pr: { source?: string; viewerLogin?: string } | null | undefined,
  c: { id?: string; kind?: string; commitSha?: string; bodyMd?: string; bodyHtml?: string } | null | undefined,
): boolean {
  if (!c || isPendingComment(c) || c.kind === 'hidden') return false
  if ((c.commitSha ?? '').trim()) return false
  // GitHub's UI has no reaction picker on timeline events (force-push, merge,
  // empty approve / request-changes). Reviews with a written body are cards.
  if (c.kind === 'action') return false
  if (isReviewCard(c) && !commentSource(c)) return false
  if (pr?.source !== 'gh') return true
  return !!(pr?.viewerLogin ?? '').trim()
}

export function isDroppedComment(c: { id?: string; inReplyToId?: string }, id: string): boolean {
  return c.id === id || c.inReplyToId === id
}

type ReplyShape = { id?: string; inReplyToId?: string; threadId?: string; bodyMd?: string; bodyHtml?: string }

function replyAnchor(c: ReplyShape): string {
  const raw = (c.inReplyToId ?? '').trim() || (c.threadId ?? '').trim()
  return raw.replace(/^gh-inline-/, '')
}

function replyBody(c: ReplyShape): string {
  const md = (c.bodyMd ?? '').trim()
  if (md) return md
  return (c.bodyHtml ?? '').replace(/<[^>]+>/g, '').trim()
}

/** Same thread + same body. Used to pair an optimistic reply with the GH row. */
export function sameThreadReply(a: ReplyShape, b: ReplyShape): boolean {
  const body = replyBody(a)
  if (!body || body !== replyBody(b)) return false
  const key = replyAnchor(a)
  return !!key && key === replyAnchor(b)
}

/** Keep in-flight optimistic replies across a GetPR snapshot, unless GH echoed them. */
export function keepPendingReplies<T extends ReplyShape>(prev: T[], next: T[]): T[] {
  const prevIds = new Set(prev.map((c) => c.id).filter(Boolean) as string[])
  const arrived = next.filter((n) => !isPendingComment(n) && !!n.id && !prevIds.has(n.id))
  const extras: T[] = []
  for (const c of prev) {
    if (!isPendingComment(c)) continue
    if (next.some((n) => n.id === c.id)) continue
    const matchAt = arrived.findIndex((n) => sameThreadReply(n, c))
    if (matchAt >= 0) {
      arrived.splice(matchAt, 1)
      continue
    }
    extras.push(c)
  }
  return extras.length ? [...next, ...extras] : next
}

/** Swap one optimistic reply for the persisted row (or drop the pending on failure). */
export function settlePostedReply<T extends ReplyShape>(comments: T[], pendingId: string, posted: T | null): T[] {
  const without = comments.filter((c) => c.id !== pendingId)
  if (!posted?.id) return without
  if (without.some((c) => c.id === posted.id)) {
    return without.map((c) => (c.id === posted.id ? { ...c, ...posted } : c))
  }
  return [...without, posted]
}

type ThreadShape = { id?: string; threadId?: string; inReplyToId?: string; resolved?: boolean }

export function lineThreadKey(c: { threadId?: string; id?: string } | null | undefined): string {
  return (c?.threadId || c?.id || '').trim()
}

/** PR author or write access can resolve; GitHub also needs a GraphQL thread id. */
export function canResolveLineNote(
  pr: { source?: string; viewerLogin?: string; authorLogin?: string; viewerCanWrite?: boolean } | null | undefined,
  c: { id?: string; path?: string; inReplyToId?: string; diffHunk?: string; threadId?: string } | null | undefined,
): boolean {
  if (!c || !isLineNote(c)) return false
  const isGH = pr?.source === 'gh'
  const login = (pr?.viewerLogin ?? '').trim().toLowerCase()
  const author = (pr?.authorLogin ?? '').trim().toLowerCase()
  if (isGH && !(!!pr?.viewerCanWrite || (!!login && login === author))) return false
  if (!isGH) return true
  return !!(c.threadId ?? '').trim()
}

export function commentMatchesThread(c: ThreadShape, threadID: string): boolean {
  const key = (threadID ?? '').trim()
  if (!key) return false
  if ((c.threadId ?? '').trim() === key || (c.id ?? '').trim() === key) return true
  const to = (c.inReplyToId ?? '').trim()
  return to === key
}

/** Paint a line-note thread resolved/open without waiting for GetPR. */
export function applyThreadResolved<T extends ThreadShape>(comments: T[], threadID: string, resolved: boolean): T[] {
  if (!threadID) return comments
  return comments.map((c) => {
    if (!commentMatchesThread(c, threadID)) return c
    return { ...c, resolved, threadId: c.threadId || threadID }
  })
}

/** Re-apply in-flight resolve toggles on top of a GetPR snapshot. */
export function applyThreadResolvedHolds<T extends ThreadShape>(comments: T[], holds: Record<string, boolean>): T[] {
  let out = comments
  for (const [id, resolved] of Object.entries(holds)) {
    if (!id) continue
    out = applyThreadResolved(out, id, resolved)
  }
  return out
}

/** Unsent reply/edit while a windowed line-note row is unmounted. */
export type ThreadEditorDraft = {
  replyOpen: boolean
  reply: string
  editingId: string
  edit: string
}

const threadEditorDrafts = new Map<string, ThreadEditorDraft>()

export function threadEditorDraft(id: string): ThreadEditorDraft | undefined {
  return threadEditorDrafts.get(id)
}

export function setThreadEditorDraft(id: string, draft: ThreadEditorDraft): void {
  if (!id) return
  if (!draft.replyOpen && !draft.reply && !draft.editingId && !draft.edit) {
    threadEditorDrafts.delete(id)
    return
  }
  threadEditorDrafts.set(id, { ...draft })
}
