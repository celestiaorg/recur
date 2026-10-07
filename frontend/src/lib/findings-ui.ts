/** Shared Findings list-row helpers (panel + chat transcript). */

import { matchFindingPath, rightDiffHasRange } from './finding-paths'
import type { IconName } from './icons'

/** Severity → Tabler icon used in dense finding rows. */
export function findingSevIcon(severity: string | undefined): IconName {
  switch ((severity || '').toLowerCase()) {
    case 'critical': return 'sev-critical'
    case 'high': return 'sev-high'
    case 'medium': return 'sev-medium'
    case 'info': return 'sev-info'
    default: return 'sev-low'
  }
}

/** First sentence / line for the dense list title. */
export function findingTitle(msg: string): string {
  const t = (msg || '').replace(/\s+/g, ' ').trim()
  if (!t) return 'Finding'
  const cut = t.search(/[.!?](?:\s|$)/)
  const head = cut > 0 && cut <= 110 ? t.slice(0, cut + 1) : t
  return head.length > 120 ? `${head.slice(0, 117)}…` : head
}

export type FindingFlag = { count: number; severity: string }

const EMPTY_FINDING_FLAG: FindingFlag = { count: 0, severity: 'info' }

export function severityRank(s: string): number {
  return s === 'critical' ? 4 : s === 'high' ? 3 : s === 'medium' ? 2 : s === 'low' ? 1 : 0
}

/**
 * Explorer badge for a tree path. Always returns an object: Svelte compiles
 * `{#if flag}{flag.severity}` to `get(flag).severity`, which throws when the
 * flag flips to null mid-update (workspace switch clears findings while the
 * tree is rewritten).
 */
export function findingFlagForPath(
  path: string,
  isDir: boolean,
  byPath: ReadonlyMap<string, FindingFlag>,
): FindingFlag {
  if (!isDir) return byPath.get(path) ?? EMPTY_FINDING_FLAG
  const prefix = path + '/'
  let count = 0
  let severity = 'info'
  for (const [p, f] of byPath) {
    if (!p.startsWith(prefix)) continue
    count += f.count
    if (severityRank(f.severity) > severityRank(severity)) severity = f.severity
  }
  return count ? { count, severity } : EMPTY_FINDING_FLAG
}

/** True when the full message adds detail beyond the title line. */
export function findingBodyNeeded(msg: string): boolean {
  const title = findingTitle(msg)
  if (!msg?.trim() || title === 'Finding') return !!msg?.trim()
  const flat = msg.replace(/\s+/g, ' ').trim()
  return flat.length > title.length + 2 || /\n/.test(msg)
}

export type ListedFindingNote = {
  id?: string
  author?: string
  body: string
  createdAt?: string
}

/** One finding as list_findings / update_finding JSON (and the chat accordion). */
export type ListedFinding = {
  id: string
  file: string
  line?: number
  endLine?: number
  severity: string
  message: string
  summary?: string
  status: string
  statusReason?: string
  notes?: ListedFindingNote[]
}

/** Minimal store row used to hydrate a truncated list_findings payload. */
export type ListedFindingRef = {
  id: string
  file?: string
  line?: number
  endLine?: number
  severity?: string
  message?: string
  summary?: string
  status?: string
  notes?: ListedFindingNote[]
}

export type ListedFindings = { status: string; count: number; findings: ListedFinding[] }

const SEV_TALLY_ORDER = ['critical', 'high', 'medium', 'low', 'info'] as const

export type FindingSeverityTally = { severity: string; count: number }

export type ReviewAnchorFile = {
  path: string
  lines?: readonly { old?: string; new?: string; type?: string }[]
}

type ReviewAnchor = { file: string; line: number; startLine: number }

type ReviewAnchorLoc = {
  file?: string
  line?: number
  startLine?: number
  side?: string
}

function reviewStartLine(start?: number, line?: number): number {
  const s = start ?? 0
  return s > 0 && s !== (line ?? 0) ? s : 0
}

function locMatchesAnchor(loc: ReviewAnchorLoc, a: ReviewAnchor): boolean {
  return loc.file === a.file
    && (loc.line ?? 0) === a.line
    && reviewStartLine(loc.startLine, loc.line) === a.startLine
    && (loc.side || 'RIGHT').toUpperCase() !== 'LEFT'
}

/** GitHub pending-comment line/startLine for a finding. Null unless the path
 *  resolves to a unique PR file and the RIGHT-side range is in that file's diff. */
export function findingReviewAnchor(
  f: { file?: string; line?: number; endLine?: number },
  files: readonly ReviewAnchorFile[],
): ReviewAnchor | null {
  const raw = (f.file || '').trim()
  const start = f.line ?? 0
  if (!raw || start <= 0) return null
  const file = matchFindingPath(raw, files.map((x) => x.path))
  if (!file) return null
  const end = (f.endLine ?? 0) > start ? f.endLine! : start
  const fd = files.find((x) => x.path === file)
  if (!rightDiffHasRange(fd?.lines, start, end)) return null
  return { file, line: end, startLine: end > start ? start : 0 }
}

/** AddPRComment trims the body; GitHub echoes the trimmed text. */
export function findingCommentBody(message?: string): string {
  return (message ?? '').trim()
}

function sameCommentBody(a?: string, b?: string): boolean {
  return findingCommentBody(a) === findingCommentBody(b)
}

/** True when a pending review comment already queues this finding's GitHub anchor. */
export function findingHasPendingComment(
  f: { file?: string; line?: number; endLine?: number; message?: string },
  pending: readonly (ReviewAnchorLoc & { body?: string })[],
  files: readonly ReviewAnchorFile[],
): boolean {
  const a = findingReviewAnchor(f, files)
  if (!a) return false
  return pending.some((pc) => locMatchesAnchor(pc, a) && sameCommentBody(pc.body, f.message))
}

/** True when a submitted line comment already occupies this finding's GitHub anchor. */
export function findingHasSubmittedComment(
  f: { file?: string; line?: number; endLine?: number; message?: string },
  comments: readonly { path?: string; line?: number; startLine?: number; side?: string; bodyMd?: string }[],
  files: readonly ReviewAnchorFile[],
): boolean {
  const a = findingReviewAnchor(f, files)
  if (!a) return false
  return comments.some((c) => locMatchesAnchor({ file: c.path, line: c.line, startLine: c.startLine, side: c.side }, a)
    && sameCommentBody(c.bodyMd, f.message))
}

/** True when this session queued the finding and a submitted comment now occupies that anchor. */
export function findingReadyToSettle(
  f: { id?: string; file?: string; line?: number; endLine?: number; message?: string },
  pending: readonly (ReviewAnchorLoc & { body?: string })[],
  comments: readonly { path?: string; line?: number; startLine?: number; side?: string; bodyMd?: string }[],
  files: readonly ReviewAnchorFile[],
  postedIds: ReadonlySet<string>,
): boolean {
  if (!f.id || !postedIds.has(f.id)) return false
  return !findingHasPendingComment(f, pending, files) && findingHasSubmittedComment(f, comments, files)
}

/** Finding IDs persisted on pending drafts (survives reload). */
export function findingIdsOnPending(pending: readonly { findingId?: string }[]): string[] {
  const ids: string[] = []
  const seen = new Set<string>()
  for (const pc of pending) {
    const id = (pc.findingId ?? '').trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    ids.push(id)
  }
  return ids
}

/** Drop posted markers for drafts the operator deleted (not for a review submit). */
export function dropFindingIdsRemovedFromPending(
  posted: ReadonlySet<string>,
  prev: readonly { findingId?: string }[],
  next: readonly { findingId?: string }[],
): Set<string> {
  const kept = new Set(findingIdsOnPending(next))
  let out: Set<string> | undefined
  for (const id of findingIdsOnPending(prev)) {
    if (kept.has(id) || !posted.has(id)) continue
    if (!out) out = new Set(posted)
    out.delete(id)
  }
  return out ?? (posted instanceof Set ? posted : new Set(posted))
}

/** `:12` or `:12–40` (en dash). Empty when line is missing. */
export function findingLocSuffix(line?: number, endLine?: number): string {
  const start = line && line > 0 ? line : 0
  if (!start) return ''
  const end = endLine && endLine > start ? endLine : 0
  return end ? `:${start}–${end}` : `:${start}`
}

/** `file.go:12–40` for dense rows. Basename only — full path lives in the title. */
export function findingLocLabel(file: string, line?: number, endLine?: number): string {
  const slash = file.lastIndexOf('/')
  const name = slash >= 0 ? file.slice(slash + 1) : file
  const loc = findingLocSuffix(line, endLine)
  if (name) return `${name}${loc}`
  return loc.replace(/^:/, '')
}

export function findingSeverityTally(findings: { severity?: string }[]): FindingSeverityTally[] {
  const counts = new Map<string, number>()
  for (const f of findings) {
    const s = (f.severity || 'info').toLowerCase()
    counts.set(s, (counts.get(s) || 0) + 1)
  }
  const out: FindingSeverityTally[] = []
  for (const s of SEV_TALLY_ORDER) {
    const n = counts.get(s)
    if (n) out.push({ severity: s, count: n })
  }
  for (const [s, n] of counts) {
    if (!(SEV_TALLY_ORDER as readonly string[]).includes(s)) out.push({ severity: s, count: n })
  }
  return out
}

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? v as Record<string, unknown> : null
}

function parseJSONObject(text: string): Record<string, unknown> | null {
  const t = text.trim()
  if (!t) return null
  const tryParse = (s: string): Record<string, unknown> | null => {
    try {
      return asRecord(JSON.parse(s))
    } catch {
      return null
    }
  }
  const direct = tryParse(t)
  if (direct) return direct
  const i = t.indexOf('{')
  if (i > 0) return tryParse(t.slice(i))
  return null
}

function parseListedNotes(raw: unknown): ListedFindingNote[] | undefined {
  if (!Array.isArray(raw) || !raw.length) return undefined
  const notes = raw
    .map((n) => {
      const o = asRecord(n)
      return {
        id: typeof o?.id === 'string' ? o.id : '',
        author: typeof o?.author === 'string' ? o.author : undefined,
        body: typeof o?.body === 'string' ? o.body : '',
        createdAt: typeof o?.createdAt === 'string' ? o.createdAt : undefined,
      }
    })
    .filter((n) => n.body.trim())
  return notes.length ? notes : undefined
}

export function parseListedFinding(f: unknown): ListedFinding {
  const o = asRecord(f) ?? {}
  return {
    id: typeof o.id === 'string' ? o.id : String(o.id ?? ''),
    file: typeof o.file === 'string' ? o.file : '',
    line: typeof o.line === 'number' ? o.line : 0,
    endLine: typeof o.endLine === 'number' ? o.endLine : 0,
    severity: typeof o.severity === 'string' ? o.severity : '',
    message: typeof o.message === 'string' ? o.message : '',
    summary: typeof o.summary === 'string' ? o.summary : '',
    status: typeof o.status === 'string' ? o.status : '',
    statusReason: typeof o.statusReason === 'string' ? o.statusReason : '',
    notes: parseListedNotes(o.notes),
  }
}

/**
 * Parse the list_findings tool result. Accepts a leading prose prefix so a
 * wrapped MCP payload still yields rows instead of a minified JSON dump.
 */
export function parseListFindingsResult(text: string | undefined | null): ListedFindings | null {
  if (!text) return null
  const raw = parseJSONObject(text)
  if (!raw || !Array.isArray(raw.findings)) return null
  return {
    status: typeof raw.status === 'string' && raw.status ? raw.status : 'open',
    count: typeof raw.count === 'number' ? raw.count : raw.findings.length,
    findings: raw.findings.map(parseListedFinding),
  }
}

/** Prefer the tool payload; fill only empty or ellipsis-truncated fields. */
function preferPayloadText(payload: string | undefined, store: string | undefined): string {
  const p = (payload || '').trim()
  const s = (store || '').trim()
  if (!p) return s
  if (s && p.endsWith('…') && s.length > p.length) return s
  return p
}

function preferPayloadLine(payload?: number, store?: number): number {
  if (payload && payload > 0) return payload
  return store && store > 0 ? store : 0
}

/** Fill truncated list_findings rows from the live workspace store (same ids). */
export function hydrateListedFindings(
  listed: ListedFindings,
  store: readonly ListedFindingRef[],
): ListedFindings {
  if (!store.length) return listed
  const byId = new Map(store.map((f) => [f.id, f]))
  return {
    ...listed,
    findings: listed.findings.map((f) => {
      const s = f.id ? byId.get(f.id) : undefined
      if (!s) return f
      const notes = (f.notes && f.notes.length ? f.notes : s.notes) ?? undefined
      return {
        ...f,
        file: f.file || s.file || '',
        line: preferPayloadLine(f.line, s.line),
        endLine: preferPayloadLine(f.endLine, s.endLine),
        severity: f.severity || s.severity || '',
        message: preferPayloadText(f.message, s.message),
        summary: preferPayloadText(f.summary, s.summary),
        status: f.status || s.status || 'open',
        notes,
      }
    }),
  }
}

/**
 * Structured list for the chat accordion. Prefer the tool JSON. Hydration
 * fills truncated/empty fields only — a historical card must not rewrite
 * itself from today's store. Unparseable results return null so the card
 * pretty-prints instead of inventing today's list.
 */
export function resolveListFindings(
  result: string | undefined,
  resultError: boolean | undefined,
  store: readonly ListedFindingRef[],
): ListedFindings | null {
  if (resultError) return null
  const parsed = parseListFindingsResult(result)
  if (parsed) return hydrateListedFindings(parsed, store)
  return null
}
