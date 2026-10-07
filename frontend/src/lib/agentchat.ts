// Shared agent-chat transcript logic, used by both chat surfaces (the workspace
// coding agent and the loop Maestro) so they render the session store identically —
// text, reasoning, and tool calls, live and on reload. Pure list transforms: the
// component owns the per-workspace map + streaming flags and calls these.
import type { AgentMsg, ChatAttachmentDraft } from '../components/AgentChat.svelte'
import type { ChatItem } from '../../bindings/recur/internal/domain/models'
import type { WorkspaceStep } from './api'
import { isSubagentTool, isSubagentLaunchAck } from './subagent'

/** Marker embedded in wire/transcript text at paste sites (matches Go). */
export const ATTACHMENT_MARKER_RE = /⟦attachment:([^⟧]+)⟧/g

/**
 * Path body of an @-mention (no loc suffix). Absolute segments omit `:` so an
 * optional `:line` / `:start-end` / column span can follow. Keep in sync with
 * Go fileAtRe.
 */
const FILE_AT_PATH =
  '\\/(?:[^\\s@:]+(?:\\/[^\\s@:]+)*)' +
  '|(?:[\\w.-]+\\/)+[\\w./+-]+' +
  '|[\\w.-]+\\/' +
  '|[\\w.-]+\\.[A-Za-z][\\w.-]*' +
  '|\\.[A-Za-z0-9_][\\w.-]*'

/** Subset used by repairFileMentionBoundaries — no bare `name.ext` (emails). */
const FILE_AT_PATH_SLASH_OR_DOT =
  '\\/(?:[^\\s@:]+(?:\\/[^\\s@:]+)*)' +
  '|(?:[\\w.-]+\\/)+[\\w./+-]+' +
  '|[\\w.-]+\\/' +
  '|\\.[A-Za-z0-9_][\\w.-]*'

/**
 * Optional loc after a path: `[:line[:col][-line[:col]]]`.
 *   :12              whole line
 *   :12-40           lines 12–40
 *   :12:5-12:18      same-line columns (both ends always carry line:col)
 *   :12:5-13:20      cross-line columns
 * Keep in sync with Go fileAtRe.
 */
const FILE_AT_LOC = ':\\d+(?::\\d+)?(?:-\\d+(?::\\d+)?)?'

/**
 * @-mention of a workspace file in chat wire text. Matches:
 *   - absolute paths (`/Users/…/app.go`) — as sent to the agent
 *   - paths with a slash (`src/main.go`)
 *   - name.ext (`app.css`)
 *   - leading-dot files (`.gitignore`, `.env`) — no second "extension" dot
 *   - directories with a trailing slash (`src/`, `src/lib/`) so root dirs and
 *     names like `terminal/` stay distinct from `@terminal`
 *   - optional loc (`@path:12`, `@path:12-40`, `@path:12:5-12:18`, `@path:12:5-13:20`)
 * Requires start-of-text or whitespace before `@` so emails, decorators, and
 * scoped packages are not treated as paths.
 * Keep in sync with Go fileAtRe + collectFileMentions (internal/engine/file_mentions.go).
 */
export const FILE_AT_RE = new RegExp(
  `(?:^|(?<=\\s))@((?:${FILE_AT_PATH})(?:${FILE_AT_LOC})?)`,
  'g',
)

/**
 * @terminal or @terminal:<id-or-title> in chat wire text. Keep in sync with
 * Go terminalAtRe (internal/engine/terminal_mentions.go).
 */
export const TERMINAL_AT_RE = /(?:^|(?<=\s))@terminal(?::([^\s@]+))?(?=\s|$)/g

/** Parsed @terminal chip. Empty id means the focused / only session. */
export type TerminalMentionRef = { id: string }

/** Token inserted when the user picks a terminal from the @ menu. */
export function terminalMentionToken(id?: string): string {
  const q = (id || '').trim()
  return q ? `@terminal:${q}` : '@terminal'
}

/** Chip label: tab title when known, else a short id / "terminal". */
export function terminalMentionLabel(chip: string, title?: string): string {
  const t = (title || '').trim()
  if (t) return t
  const id = parseTerminalMentionChip(chip)
  if (!id) return 'terminal'
  return id.length > 8 ? id.slice(0, 8) : id
}

/** Qualifier after @terminal: (session id or title); empty for bare @terminal. */
export function parseTerminalMentionChip(chip: string): string {
  const raw = chip.trim().replace(/^@/, '')
  if (raw === 'terminal') return ''
  if (raw.startsWith('terminal:')) return raw.slice('terminal:'.length)
  return ''
}

/** Extract unique @terminal chips from wire text. */
export function extractTerminalMentions(text: string): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  const re = new RegExp(TERMINAL_AT_RE.source, 'g')
  for (const m of text.matchAll(re)) {
    const token = m[1] ? `terminal:${m[1]}` : 'terminal'
    if (seen.has(token)) continue
    seen.add(token)
    out.push(token)
  }
  return out
}

/**
 * @artifact or @artifact:<id-or-title> in chat wire text. Keep in sync with
 * Go artifactAtRe (internal/engine/artifact_mentions.go).
 */
export const ARTIFACT_AT_RE = /(?:^|(?<=\s))@artifact(?::([^\s@]+))?(?=\s|$)/g

/** Parsed @artifact chip. Empty id means the newest artifact. */
export type ArtifactMentionRef = { id: string }

/** Token inserted when the user picks an artifact from the @ menu. */
export function artifactMentionToken(id?: string): string {
  const q = (id || '').trim()
  return q ? `@artifact:${q}` : '@artifact'
}

/** Chip label: artifact title when known, else a short id / "artifact". */
export function artifactMentionLabel(chip: string, title?: string): string {
  const t = (title || '').trim()
  if (t) return t
  const id = parseArtifactMentionChip(chip)
  if (!id) return 'artifact'
  return id.length > 8 ? id.slice(0, 8) : id
}

/** Qualifier after @artifact: (id or title); empty for bare @artifact. */
export function parseArtifactMentionChip(chip: string): string {
  const raw = chip.trim().replace(/^@/, '')
  if (raw === 'artifact') return ''
  if (raw.startsWith('artifact:')) return raw.slice('artifact:'.length)
  return ''
}

/** Extract unique @artifact chips from wire text. */
export function extractArtifactMentions(text: string): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  const re = new RegExp(ARTIFACT_AT_RE.source, 'g')
  for (const m of text.matchAll(re)) {
    const token = m[1] ? `artifact:${m[1]}` : 'artifact'
    if (seen.has(token)) continue
    seen.add(token)
    out.push(token)
  }
  return out
}

/** @name tokens that are @artifact / @terminal keywords, not panel titles. */
export function reservedAtName(token: string): boolean {
  const t = token.trim().toLowerCase()
  return t === 'artifact' || t.startsWith('artifact:') || t === 'terminal' || t.startsWith('terminal:')
}

/** True when `@token` is a file mention (path, name.ext, dotfile). Full match, like Go. */
const FILE_AT_TOKEN_RE = new RegExp(FILE_AT_RE.source)

export function atNameIsFileMention(token: string): boolean {
  const s = '@' + token
  return FILE_AT_TOKEN_RE.exec(s)?.[0] === s
}

/**
 * Unique Artifacts-panel title → id. Empty when reserved, file-like, missing,
 * or duplicated. Titles with whitespace cannot be `@Name` tokens.
 */
export function uniqueArtifactIdForTitle(
  title: string,
  artifacts: readonly { id: string; title: string }[],
): string {
  const q = title.trim().toLowerCase()
  if (!q || reservedAtName(q) || atNameIsFileMention(q) || /\s/.test(q)) return ''
  const hits = artifacts.filter((a) => a.title.trim().toLowerCase() === q)
  return hits.length === 1 ? hits[0]!.id : ''
}

/** Whitespace-bounded `@token` in wire text. Keep in sync with Go atNameRe. */
export const AT_NAME_RE = /(?:^|(?<=\s))@([^\s@]+)(?=\s|$)/g

export type AtNameHit = { index: number; len: number; token: string }

/** Loose `@Title` tokens that are not @artifact / @terminal / @file. */
export function collectLooseAtNames(text: string): AtNameHit[] {
  const out: AtNameHit[] = []
  const re = new RegExp(AT_NAME_RE.source, 'g')
  for (const m of text.matchAll(re)) {
    if (m.index == null || m[1] == null) continue
    const token = m[1]
    if (reservedAtName(token) || atNameIsFileMention(token)) continue
    out.push({ index: m.index, len: m[0].length, token })
  }
  return out
}

export function tokenSpanOverlaps(
  index: number,
  len: number,
  spans: readonly { index: number; len: number }[],
): boolean {
  return spans.some((t) => index < t.index + t.len && index + len > t.index)
}

/** Trailing loc: `[:line[:col][-line[:col]]]`. */
const FILE_MENTION_LOC_RE = /:(\d+)(?::(\d+))?(?:-(\d+)(?::(\d+))?)?$/

/** Parsed @file chip (path + optional 1-based line / column span). */
export type FileMentionRef = {
  /** Workspace path without loc suffix. */
  path: string
  startLine?: number
  endLine?: number
  /** 1-based inclusive. Set with endCol when the chip is a partial-line span. */
  startCol?: number
  endCol?: number
}

function positiveInt(raw: string | undefined): number {
  if (raw == null) return 0
  const n = Number(raw)
  return Number.isFinite(n) && n > 0 ? n : 0
}

/**
 * Format a chip: `path`, `path:12`, `path:12-40`, `path:12:5-12:18`,
 * or `path:12:5-13:20`. Column spans always write both `line:col` ends.
 */
export function formatFileMentionChip(
  path: string,
  startLine?: number,
  endLine?: number,
  startCol?: number,
  endCol?: number,
): string {
  const p = path.trim()
  if (!p) return ''
  if (startLine == null || startLine <= 0) return p
  const colStart = startCol != null && startCol > 0 ? startCol : 0
  const colEnd = endCol != null && endCol > 0 ? endCol : 0
  if (colStart && colEnd) {
    const endLn = endLine != null && endLine >= startLine ? endLine : startLine
    const end = endLn === startLine && colEnd < colStart ? colStart : colEnd
    return `${p}:${startLine}:${colStart}-${endLn}:${end}`
  }
  if (endLine != null && endLine > startLine) return `${p}:${startLine}-${endLine}`
  return `${p}:${startLine}`
}

/** Format from a parsed mention (round-trip helper). */
export function formatFileMentionRef(ref: FileMentionRef): string {
  return formatFileMentionChip(ref.path, ref.startLine, ref.endLine, ref.startCol, ref.endCol)
}

/** Trailing slash marks a directory mention (`src/` vs file `src`). */
export function dirFileMentionPath(path: string): string {
  const p = path.trim().replace(/\/+$/, '')
  return p ? `${p}/` : ''
}

export function isDirFileMention(chip: string): boolean {
  return chip.trim().replace(/^@/, '').endsWith('/')
}

/** Split `path[:loc]` into path + lines/columns. Invalid loc → whole-file path. */
export function parseFileMentionChip(chip: string): FileMentionRef {
  const raw = chip.trim().replace(/\/+$/, '')
  if (!raw) return { path: '' }
  const m = raw.match(FILE_MENTION_LOC_RE)
  if (!m || m.index == null) return { path: raw }
  const path = raw.slice(0, m.index)
  if (!path) return { path: raw }
  const startLine = positiveInt(m[1])
  if (!startLine) return { path }
  const startCol = positiveInt(m[2])
  const endLine = positiveInt(m[3])
  const endCol = positiveInt(m[4])
  if (startCol && endCol) {
    const endLn = endLine || startLine
    if (endLn < startLine) return { path, startLine }
    if (endLn === startLine && endCol < startCol) return { path, startLine }
    return endLn > startLine
      ? { path, startLine, endLine: endLn, startCol, endCol }
      : { path, startLine, startCol, endCol }
  }
  if (endLine > startLine) return { path, startLine, endLine }
  return { path, startLine }
}

/** Chip label: basename plus `(12)` / `(12-40)` / `(12:5-12:18)`. */
export function fileMentionLabel(chip: string): string {
  const { path, startLine, endLine, startCol, endCol } = parseFileMentionChip(chip)
  const base = path.includes('/') ? path.slice(path.lastIndexOf('/') + 1) : path
  if (!startLine) return base || chip
  if (startCol && endCol) {
    const endLn = endLine && endLine >= startLine ? endLine : startLine
    return `${base} (${startLine}:${startCol}-${endLn}:${endCol})`
  }
  if (endLine && endLine > startLine) return `${base} (${startLine}-${endLine})`
  return `${base} (${startLine})`
}

/**
 * `/skill-name` in wire/transcript text. Lowercase Agent Skills names only —
 * avoids chewing absolute paths like `/Users/…`. Keep in sync with Go skillNameRE.
 */
export const SKILL_SLASH_RE = /(?:^|(?<=\s))\/([a-z][a-z0-9-]*)(?=\s|$|[^\w/-])/g

export function attachmentMarker(id: string): string {
  return `⟦attachment:${id}⟧`
}

/** Token inserted when the user picks a skill from the `/` menu. */
export function skillMentionToken(name: string): string {
  return `/${name.trim()}`
}

/** Plain text with attachment markers stripped (composer display helper). */
export function stripAttachmentMarkers(text: string): string {
  return text.replace(ATTACHMENT_MARKER_RE, '').replace(/\n{3,}/g, '\n\n')
}

/** Strip @path file mentions from text (composer display / empty checks). */
export function stripFileMentions(text: string): string {
  return text.replace(FILE_AT_RE, '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n')
}

/** Strip @terminal chips from text. */
export function stripTerminalMentions(text: string): string {
  return text.replace(new RegExp(TERMINAL_AT_RE.source, 'g'), '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n')
}

/** Strip @artifact chips from text. */
export function stripArtifactMentions(text: string): string {
  return text.replace(new RegExp(ARTIFACT_AT_RE.source, 'g'), '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n')
}

/** Strip `/skill-name` tokens from text. */
export function stripSkillMentions(text: string): string {
  return text.replace(new RegExp(SKILL_SLASH_RE.source, 'g'), ' ')
    .replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').replace(/  +/g, ' ')
}

/** Plain composer text with attachment markers and @file /skill /@terminal /@artifact mentions removed. */
export function stripComposerMarkers(text: string): string {
  return stripSkillMentions(
    stripArtifactMentions(stripTerminalMentions(stripFileMentions(stripAttachmentMarkers(text)))),
  )
}

/**
 * Build the wire/transcript string. Composer text may already contain inline
 * @path file mentions and ⟦attachment:id⟧ image markers at caret positions —
 * keep those in place. Any draft not yet represented gets appended (legacy /
 * stop-restore paths that still hold chips only in the attachments array).
 */
export function composeMarkedText(
  text: string,
  drafts: ChatAttachmentDraft[],
): string {
  let out = text
  for (const d of drafts) {
    const marker = attachmentMarker(d.id)
    if (out.includes(marker)) continue
    out += (out && !out.endsWith('\n') ? '\n' : '') + marker
  }
  return out
}

/** Attachment ids present as ⟦attachment:id⟧ markers in text. */
export function extractAttachmentIds(text: string): string[] {
  const out: string[] = []
  const re = new RegExp(ATTACHMENT_MARKER_RE.source, 'g')
  for (const m of text.matchAll(re)) {
    if (m[1]) out.push(m[1])
  }
  return out
}

/** True when two user-bubble bodies are the same send. Caption text must
 *  match; ⟦attachment:id⟧ may be present on only one side. Different image
 *  ids stay distinct. */
export function sameUserChatBody(a: string | null | undefined, b: string | null | undefined): boolean {
  const x = a ?? ''
  const y = b ?? ''
  if (x === y) return true
  if (stripAttachmentMarkers(x).trim() !== stripAttachmentMarkers(y).trim()) return false
  const idsX = extractAttachmentIds(x)
  const idsY = extractAttachmentIds(y)
  if (idsX.length === 0 || idsY.length === 0) return true
  return idsX.length === idsY.length && idsX.every((id, i) => id === idsY[i])
}

/** Token inserted at the caret when the user picks a file from the @ menu. */
export function fileMentionToken(chip: string): string {
  return `@${chip.trim()}`
}

/**
 * Remove one `@path` (or `@path:lines` / column span) mention from composer text (first match),
 * eating a single adjacent space so “how does @A compare” → “how does compare”
 * cleans up cleanly when the chip is dismissed.
 */
export function removeFileMentionToken(text: string, chip: string): string {
  const token = fileMentionToken(chip)
  if (!token || token === '@') return text
  const idx = text.indexOf(token)
  if (idx < 0) return text
  let start = idx
  let end = idx + token.length
  if (text[end] === ' ') end += 1
  else if (start > 0 && text[start - 1] === ' ') start -= 1
  return text.slice(0, start) + text.slice(end)
}

/** Active @ or / query at the caret. */
export type MentionQuery = { start: number; end: number; query: string }

/**
 * Detect an in-progress @mention: `@` at start-of-text or after whitespace,
 * with the caret inside the following non-space query.
 * Picks insert a trailing space, so the caret sits past the token and this
 * returns null — no special “committed path” check (that closed the menu early
 * for prefixes like `@.git` of `.gitignore`).
 */
export function mentionQueryAt(text: string, caret: number): MentionQuery | null {
  if (caret < 0 || caret > text.length) return null
  let i = caret - 1
  while (i >= 0 && !/\s/.test(text[i]!)) {
    if (text[i] === '@') {
      if (i > 0 && !/\s/.test(text[i - 1]!)) return null
      const query = text.slice(i + 1, caret)
      if (query.includes('@')) return null
      return { start: i, end: caret, query }
    }
    i--
  }
  return null
}

/**
 * Detect an in-progress `/skill` query: `/` at start-of-text or after whitespace,
 * caret inside the following skill-name characters (lowercase, digits, hyphen).
 */
export function skillQueryAt(text: string, caret: number): MentionQuery | null {
  if (caret < 0 || caret > text.length) return null
  let i = caret - 1
  while (i >= 0 && /[a-z0-9-]/.test(text[i]!)) i--
  if (i < 0 || text[i] !== '/') return null
  if (i > 0 && !/\s/.test(text[i - 1]!)) return null
  const query = text.slice(i + 1, caret)
  if (!/^[a-z0-9-]*$/.test(query)) return null
  return { start: i, end: caret, query }
}

/**
 * Placement for a ⌘L / insertFileMention chip: strip an in-progress @file or
 * /skill query at the caret, and ensure a whitespace boundary before the chip
 * so FILE_AT_RE and Go collectFileMentions accept it.
 *
 * Pass `stripQuery: false` when the caret is flush after a committed chip
 * (isCaretAfterFileChip / isCaretAfterSkillChip) — those serialize as `@path`
 * / `/skill`, which mentionQueryAt would otherwise treat as an in-progress
 * query and delete on insert.
 */
export function prepareFileMentionInsert(
  text: string,
  caret: number,
  opts?: { stripQuery?: boolean },
): { before: string; after: string } {
  const at = Math.max(0, Math.min(caret, text.length))
  const strip = opts?.stripQuery !== false
  const q = strip ? (mentionQueryAt(text, at) ?? skillQueryAt(text, at)) : null
  let before = text.slice(0, q ? q.start : at)
  const after = text.slice(q ? q.end : at)
  if (before.length > 0 && !/\s$/.test(before)) before += ' '
  return { before, after }
}

/**
 * Insert a space before @file tokens that lost their whitespace boundary so
 * FILE_AT_RE / hydrateComposer(chipFiles) can see them again.
 * Only repairs paths with `/` or leading-dot names — leaves emails like
 * `john@acme.io` alone.
 */
export function repairFileMentionBoundaries(text: string): string {
  if (!text.includes('@')) return text
  // Absolute, slashy relative, or dotfile + optional :lines — no left-boundary.
  const re = new RegExp(
    `@((?:${FILE_AT_PATH_SLASH_OR_DOT})(?:${FILE_AT_LOC})?)`,
    'g',
  )
  let out = ''
  let last = 0
  for (const m of text.matchAll(re)) {
    const idx = m.index ?? 0
    out += text.slice(last, idx)
    if (idx > 0 && !/\s/.test(text[idx - 1]!)) out += ' '
    out += m[0]
    last = idx + m[0].length
  }
  out += text.slice(last)
  return out
}

/** Extract @path tokens from wire text (for bubble chips / stop restore). */
export function extractFileMentions(text: string): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  const re = new RegExp(FILE_AT_RE.source, 'g')
  for (const m of text.matchAll(re)) {
    const p = m[1]
    if (!p || seen.has(p)) continue
    seen.add(p)
    out.push(p)
  }
  return out
}

/** Operator note shown on a turn-end digest finding. */
export type TurnDigestNote = {
  id?: string
  author?: string
  body: string
  createdAt?: string
}

/** One still-open finding for the chat turn-end digest. */
export type TurnDigestFinding = {
  id: string
  file: string
  line: number
  endLine: number
  severity: string
  /** Clear finding message — accordion title source (same as Findings panel). */
  message: string
  /** End-user body: optional summary markdown, else message. */
  text: string
  notes: TurnDigestNote[]
}

/** Minimal finding fields the turn digest needs from the workspace store. */
export type DigestFindingRef = {
  id: string
  file?: string
  line?: number
  endLine?: number
  severity?: string
  message?: string
  summary?: string
  status?: string
  notes?: TurnDigestNote[]
}

type DigestToolMsg = {
  tool?: string
  input?: string
  result?: string
  resultError?: boolean
  turnEnd?: boolean
  me?: boolean
}

function isReportFindingTool(tool: string | undefined): boolean {
  return !!tool && (tool.endsWith('__report_finding') || tool === 'report_finding')
}

function isUpdateFindingTool(tool: string | undefined): boolean {
  return !!tool && (tool.endsWith('__update_finding') || tool === 'update_finding')
}

function parseToolInput(input: string | undefined): Record<string, unknown> | null {
  if (!input) return null
  try {
    const raw = JSON.parse(input)
    return raw && typeof raw === 'object' && !Array.isArray(raw) ? raw as Record<string, unknown> : null
  } catch {
    return null
  }
}

function normFindingText(s: string | undefined): string {
  return (s || '').replace(/\s+/g, ' ').trim()
}

function digestNotes(notes: DigestFindingRef['notes']): TurnDigestNote[] {
  if (!notes?.length) return []
  return notes
    .map((n) => ({
      id: String(n.id || ''),
      author: n.author,
      body: typeof n.body === 'string' ? n.body : '',
      createdAt: n.createdAt,
    }))
    .filter((n) => n.body.trim())
}

/** Resolve a report_finding tool row to a finding id + fields (JSON result or store match). */
function resolveReportedFinding(
  m: DigestToolMsg,
  findings: DigestFindingRef[],
): DigestFindingRef | null {
  if (m.result && !m.resultError) {
    try {
      const raw = JSON.parse(m.result)
      if (raw && typeof raw === 'object' && !Array.isArray(raw) && raw.id) {
        const id = String(raw.id)
        const fromStore = findings.find((f) => f.id === id)
        return {
          id,
          file: fromStore?.file || (typeof raw.file === 'string' ? raw.file : ''),
          line: (fromStore?.line && fromStore.line > 0 ? fromStore.line : 0)
            || (typeof raw.line === 'number' ? raw.line : 0),
          endLine: (fromStore?.endLine && fromStore.endLine > 0 ? fromStore.endLine : 0)
            || (typeof raw.endLine === 'number' ? raw.endLine : 0),
          severity: fromStore?.severity || (typeof raw.severity === 'string' ? raw.severity : ''),
          message: fromStore?.message || (typeof raw.message === 'string' ? raw.message : ''),
          summary: fromStore?.summary || (typeof raw.summary === 'string' ? raw.summary : ''),
          status: fromStore?.status || (typeof raw.status === 'string' ? raw.status : 'open'),
          notes: digestNotes(fromStore?.notes),
        }
      }
    } catch {
      // Pre-JSON tool results were prose ("Recorded a … finding in …") — fall through.
    }
  }
  // Legacy / pending: match tool input against the workspace findings list.
  const p = parseToolInput(m.input)
  if (!p) return null
  const file = typeof p.file === 'string' ? p.file : ''
  const message = typeof p.message === 'string' ? p.message : ''
  const line = typeof p.line === 'number' ? p.line : 0
  const endLine = typeof p.end_line === 'number' ? p.end_line : 0
  const severity = typeof p.severity === 'string' ? p.severity : ''
  const summary = typeof p.summary === 'string' ? p.summary : ''
  if (!file && !message) return null
  const msg = normFindingText(message)
  let match = findings.find(
    (f) => (f.file || '') === file && (f.line || 0) === line && normFindingText(f.message) === msg,
  )
  if (!match && file) {
    match = findings.find(
      (f) => (f.file || '') === file && (f.line || 0) === line && (f.severity || '') === severity,
    )
  }
  if (match) {
    return {
      id: match.id,
      file: match.file || file,
      line: (match.line && match.line > 0 ? match.line : 0) || line,
      endLine: (match.endLine && match.endLine > 0 ? match.endLine : 0) || endLine,
      severity: match.severity || severity,
      message: match.message || message,
      summary: match.summary || summary,
      status: match.status || 'open',
      notes: digestNotes(match.notes),
    }
  }
  // No store row yet (or already purged) — still surface the filing from input.
  return {
    id: '',
    file,
    line,
    endLine,
    severity,
    message,
    summary,
    status: 'open',
    notes: [],
  }
}

/**
 * Still-open findings filed in the turn that ends at message index endIdx.
 * Supports modern JSON report_finding results and legacy prose results (matched
 * via tool input → workspace findings).
 */
export function stillOpenFindingsForTurn(
  messages: DigestToolMsg[],
  endIdx: number,
  findings: DigestFindingRef[] = [],
): TurnDigestFinding[] {
  let start = 0
  for (let i = endIdx - 1; i >= 0; i--) {
    const m = messages[i]
    if (m?.turnEnd || m?.me) {
      start = i + 1
      break
    }
  }
  const filed: DigestFindingRef[] = []
  const closed = new Set<string>()
  for (let i = start; i < endIdx; i++) {
    const m = messages[i]
    if (!m?.tool || m.resultError) continue
    if (isReportFindingTool(m.tool)) {
      const f = resolveReportedFinding(m, findings)
      if (f) filed.push(f)
      continue
    }
    if (isUpdateFindingTool(m.tool)) {
      const p = parseToolInput(m.input)
      const id = typeof p?.finding_id === 'string' ? p.finding_id : ''
      const status = typeof p?.status === 'string' ? p.status : ''
      if (id && (status === 'resolved' || status === 'invalid')) closed.add(id)
    }
  }
  const out: TurnDigestFinding[] = []
  const seen = new Set<string>()
  for (const f of filed) {
    const key = f.id || `${f.file}|${f.line}|${normFindingText(f.message)}`
    if (seen.has(key)) continue
    seen.add(key)
    if (f.id && closed.has(f.id)) continue
    const status = f.status || 'open'
    if (status !== 'open') continue
    // Prefer live store fields when we have an id (may have changed later).
    let notes = digestNotes(f.notes)
    let message = normFindingText(f.message)
    let summary = normFindingText(f.summary)
    if (f.id) {
      const live = findings.find((x) => x.id === f.id)
      if (live && (live.status || 'open') !== 'open') continue
      if (live?.notes?.length) notes = digestNotes(live.notes)
      const lm = normFindingText(live?.message)
      if (lm) message = lm
      const ls = normFindingText(live?.summary)
      if (ls) summary = ls
    }
    out.push({
      id: f.id,
      file: f.file || '',
      line: f.line || 0,
      endLine: f.endLine || 0,
      severity: f.severity || '',
      message,
      text: summary || message,
      notes,
    })
  }
  return out
}

/**
 * ask_user resume wire (engine formatAskUserResumePrompt). Prefer persisted
 * display_text; this fallback shortens full wire when display_text is empty.
 */
const ASK_USER_RESUME_RE =
  /^The user answered your earlier ask_user question\.\s*\n[\s\S]*?\nAnswer:\s*(.+?)(?:\nDetails:\s*(.+?))?(?:\n\nContinue with this answer[\s\S]*)?$/

/**
 * Plain-text form of an ask_user / escalation option for the engine `choice`.
 * Options may contain HTML (`<code>…</code>`) that MarkdownHtml renders; the
 * resume prompt and durable feed want prose. Only real tags are stripped so
 * comparisons like `count < 5` stay intact (a `/<[^>]+>/` wipe ate those).
 */
export function optionChoiceText(s: string): string {
  return s.replace(/<\/?[a-zA-Z][a-zA-Z0-9]*\b[^>]*>/g, '').replace(/\s+/g, ' ').trim()
}

/**
 * Short chat-bubble label for an ask_user answer. Keep in sync with Go
 * formatAskUserDisplay (internal/engine/ask_user.go).
 */
export function formatAskUserDisplayText(choice = '', freeText = ''): string {
  const c = choice.trim()
  const f = freeText.trim()
  if (c && f) return `${c} — ${f}`
  if (c) return c
  if (f) return f
  return '(empty)'
}

/**
 * Optimistic/composer paths and rows without display_text. Persisted bubbles
 * usually already carry the collapsed display string from flatten.
 * Keep Coder dispatch prefixes in sync with Go coderDispatchDisplay
 * (internal/engine/roles.go).
 */
const CODER_FIX_DISPLAY = 'Fix open reviewer findings'
const CODER_RESUME_DISPLAY = 'Resume coding'

export function displayUserChatText(text: string | null | undefined): string {
  const t = text ?? ''
  const ask = t.match(ASK_USER_RESUME_RE)
  if (ask) {
    return formatAskUserDisplayText(ask[1] ?? '', ask[2] ?? '')
  }
  if (t.startsWith('You are the Coder agent. A reviewer found issues')) return CODER_FIX_DISPLAY
  if (t.startsWith('You stopped while reviewer findings')) return CODER_FIX_DISPLAY
  if (t.startsWith('You were interrupted mid-task')) return CODER_RESUME_DISPLAY
  if (t.startsWith('You are the Coder agent resuming an interrupted task')) return CODER_RESUME_DISPLAY
  return t
}

/** Chat bubble for a queued or flushed send. Prefer display_text; else marked
 *  wire so ⟦attachment:id⟧ stays aligned with session.user and flatten. */
export function pendingSendBubbleBody(displayText: string | null | undefined, marked: string): string {
  return displayUserChatText((displayText ?? '').trim() || marked)
}

// userTurnCount is how many human chat bubbles a transcript has (not queued
// follow-ups). Used to decide whether live rows already include history.
function userTurnCount(list: AgentMsg[]): number {
  let n = 0
  for (const m of list) {
    if (m.me && !m.queued) n++
  }
  return n
}

// rowEqual reports whether a persisted row and a live row are the same turn
// item for alignment. Optimistic live user bubbles never match (a resend of
// the same text is a new turn). Streaming deltas are live-only. Tools match
// on name+input so a live result can overlay a still-pending store snapshot.
function rowEqual(persisted: AgentMsg, live: AgentMsg): boolean {
  if (live.queued || persisted.queued) return false
  if (live.optimistic && live.me) return false
  if (live.streaming || persisted.streaming) return false
  if (persisted.me && live.me) return (persisted.body ?? '') === (live.body ?? '')
  if (persisted.tool && live.tool) {
    return persisted.tool === live.tool && (persisted.toolInput ?? '') === (live.toolInput ?? '')
  }
  if (persisted.thinking && live.thinking) return (persisted.body ?? '') === (live.body ?? '')
  if (persisted.compact && live.compact) return (persisted.body ?? '') === (live.body ?? '') && !!persisted.compacting === !!live.compacting
  if (persisted.note || live.note) return !!persisted.note && !!live.note && (persisted.body ?? '') === (live.body ?? '')
  if (persisted.fallback && live.fallback) return (persisted.body ?? '') === (live.body ?? '') && (persisted.description ?? '') === (live.description ?? '')
  if (persisted.turnEnd || live.turnEnd) return !!persisted.turnEnd && !!live.turnEnd
  if (!persisted.me && !persisted.tool && !persisted.thinking && !persisted.compact && !persisted.note && !persisted.fallback && !live.me && !live.tool && !live.thinking && !live.compact && !live.note && !live.fallback) {
    return (persisted.body ?? '') === (live.body ?? '')
  }
  return false
}

function isPrefix(suffix: AgentMsg[], live: AgentMsg[]): boolean {
  if (suffix.length > live.length) return false
  for (let i = 0; i < suffix.length; i++) {
    if (!rowEqual(suffix[i], live[i])) return false
  }
  return true
}

// liveAlignStart is the persisted index where `live` overlays: the earliest
// start whose suffix is a prefix of live. Empty suffix (append) always matches,
// so a disjoint stream-only tail is concatenated rather than replacing history.
function liveAlignStart(persisted: AgentMsg[], live: AgentMsg[]): number {
  for (let start = 0; start <= persisted.length; start++) {
    if (isPrefix(persisted.slice(start), live)) return start
  }
  return persisted.length
}

function commonPrefixLen(persisted: AgentMsg[], live: AgentMsg[]): number {
  const n = Math.min(persisted.length, live.length)
  let i = 0
  while (i < n && rowEqual(persisted[i], live[i])) i++
  return i
}

function overlayAligned(persisted: AgentMsg[], extra: AgentMsg[], from: number): AgentMsg[] {
  const out = persisted.slice()
  let p = from
  let i = 0
  while (i < extra.length && p < out.length) {
    const e = extra[i]
    if (rowEqual(out[p], e) || (e.me && e.optimistic && out[p].me && sameUserChatBody(e.body, out[p].body))) {
      out[p] = pickRicher(out[p], e)
      i++
      p++
      continue
    }
    let found = -1
    for (let q = p + 1; q < out.length; q++) {
      if (rowEqual(out[q], e) || (e.me && e.optimistic && out[q].me && sameUserChatBody(e.body, out[q].body))) {
        found = q
        break
      }
    }
    if (found < 0) break
    p = found
  }
  return [...out, ...extra.slice(i)]
}

function bodyPrefix(a?: string, b?: string): boolean {
  const x = a ?? ''
  const y = b ?? ''
  if (!x || !y) return x === y
  return x === y || x.startsWith(y) || y.startsWith(x)
}

// Like rowEqual, but a live streaming row can overlay the store's settled
// copy of the same item (warm remount: heap transcript vs WorkspaceChatItems).
function rowEqualLoose(persisted: AgentMsg, live: AgentMsg): boolean {
  if (live.queued || persisted.queued) return false
  if (persisted.me && live.me) return sameUserChatBody(persisted.body, live.body)
  if (persisted.tool && live.tool) {
    return persisted.tool === live.tool && (persisted.toolInput ?? '') === (live.toolInput ?? '')
  }
  if (persisted.thinking && live.thinking) return bodyPrefix(persisted.body, live.body)
  if (persisted.compact && live.compact) {
    return (persisted.body ?? '') === (live.body ?? '') && !!persisted.compacting === !!live.compacting
  }
  if (persisted.fallback && live.fallback) {
    return (persisted.body ?? '') === (live.body ?? '') && (persisted.description ?? '') === (live.description ?? '')
  }
  if (persisted.turnEnd || live.turnEnd) return !!persisted.turnEnd && !!live.turnEnd
  if (
    !persisted.me && !persisted.tool && !persisted.thinking && !persisted.compact && !persisted.fallback && !persisted.turnEnd &&
    !live.me && !live.tool && !live.thinking && !live.compact && !live.fallback && !live.turnEnd
  ) {
    return bodyPrefix(persisted.body, live.body)
  }
  return false
}

function pickRicher(persisted: AgentMsg, live: AgentMsg): AgentMsg {
  if (live.streaming) {
    return {
      ...live,
      ...(live.toolId || persisted.toolId ? { toolId: live.toolId || persisted.toolId } : {}),
      ...(live.ts || persisted.ts ? { ts: live.ts || persisted.ts } : {}),
    }
  }
  if (persisted.me && live.me) return persisted
  if (persisted.tool && live.tool) {
    const result = live.result || persisted.result
    return {
      ...persisted,
      ...live,
      toolId: live.toolId || persisted.toolId,
      ts: persisted.ts || live.ts,
      result,
      resultPending: result ? false : !!(live.resultPending || persisted.resultPending),
    }
  }
  const liveBody = live.body ?? ''
  const persBody = persisted.body ?? ''
  if (liveBody.length > persBody.length) {
    return { ...live, ...(persisted.ts && !live.ts ? { ts: persisted.ts } : {}) }
  }
  return persisted
}

// Overlay a full in-memory transcript onto a store snapshot. Live may be
// behind (missing later tools) or parallel (streaming where the store settled).
function stitchFullTranscripts(persisted: AgentMsg[], live: AgentMsg[]): AgentMsg[] {
  const out: AgentMsg[] = []
  let i = 0
  let j = 0
  while (i < persisted.length && j < live.length) {
    if (rowEqualLoose(persisted[i], live[j])) {
      out.push(pickRicher(persisted[i], live[j]))
      i++
      j++
      continue
    }
    if (i + 1 < persisted.length && rowEqualLoose(persisted[i + 1], live[j])) {
      out.push(persisted[i])
      i++
      continue
    }
    if (j + 1 < live.length && rowEqualLoose(persisted[i], live[j + 1])) {
      out.push(live[j])
      j++
      continue
    }
    out.push(persisted[i])
    i++
  }
  while (i < persisted.length) out.push(persisted[i++])
  while (j < live.length) out.push(live[j++])
  return out
}

function trailingUser(list: AgentMsg[]): AgentMsg | undefined {
  for (let i = list.length - 1; i >= 0; i--) {
    if (list[i].me && !list[i].queued) return list[i]
  }
  return undefined
}

function indexAfterTrailingUser(list: AgentMsg[]): number {
  let last = -1
  for (let i = 0; i < list.length; i++) {
    if (list[i].me && !list[i].queued) last = i
  }
  return last + 1
}

function userHasImageSrc(m: AgentMsg): boolean {
  return (m.attachments ?? []).some((a) => !!(a.dataUrl || a.url))
}

function richerUserRow(a: AgentMsg, b: AgentMsg): AgentMsg {
  const body =
    extractAttachmentIds(b.body ?? '').length > extractAttachmentIds(a.body ?? '').length ? (b.body ?? '') : (a.body ?? '')
  const attachments = userHasImageSrc(b) && !userHasImageSrc(a) ? b.attachments : a.attachments
  if (body === (a.body ?? '') && attachments === a.attachments && !a.optimistic && !a.queued) return a
  return { ...a, body, attachments, optimistic: false, queued: false }
}

function userSendIds(m: AgentMsg): string[] {
  const marked = extractAttachmentIds(m.body ?? '')
  if (marked.length) return marked
  return (m.attachments ?? []).map((a) => a.id).filter((id): id is string => !!id)
}

function isSameSendReplay(prev: AgentMsg, next: AgentMsg): boolean {
  if (!sameUserChatBody(prev.body, next.body)) return false
  if (!prev.optimistic || next.optimistic) return false
  return true
}

function persistHasUserSend(mapped: AgentMsg[], liveRow: AgentMsg, live: AgentMsg[]): boolean {
  const last = trailingUser(mapped)
  if (!last) return false
  if (!sameUserChatBody(last.body, liveRow.body)) return false
  const liveIds = userSendIds(liveRow)
  if (liveIds.length > 0 && userSendIds(last).length === 0) return false
  if (liveIds.length === 0 && userSendIds(last).length > 0) {
    const idx = live.indexOf(liveRow)
    if (idx < 0) return false
    const body = last.body ?? ''
    const settled = (m: AgentMsg) => m.me && !m.optimistic && !m.queued && (m.body ?? '') === body
    for (let i = idx - 1; i >= 0; i--) {
      if (live[i].turnEnd || isAssistantText(live[i])) break
      if (settled(live[i])) return false
    }
    for (let i = idx + 1; i < live.length; i++) {
      if (live[i].turnEnd || isAssistantText(live[i])) break
      if (settled(live[i])) return true
    }
    return lastUserTurnStillOpen(mapped)
  }
  return true
}

function lastUserTurnStillOpen(list: AgentMsg[]): boolean {
  const after = indexAfterTrailingUser(list)
  if (after === 0) return false
  for (let i = after; i < list.length; i++) {
    if (list[i].turnEnd || isAssistantText(list[i])) return false
  }
  return true
}

function isAssistantText(m: AgentMsg): boolean {
  if (m.me || m.queued || m.turnEnd || m.tool || m.thinking || m.compact || m.note || m.fallback || m.log || m.stopped || m.error) {
    return false
  }
  return !!(m.body ?? '').trim()
}

/** Drop extra user rows that replay the same send later in the same turn
 *  (queued flush + session.user + store resync). A resend after assistant
 *  text or a turn footer stays. */
export function collapseDuplicateUsersInTurn(list: AgentMsg[]): AgentMsg[] {
  const out: AgentMsg[] = []
  for (const m of list) {
    if (!m.me || m.queued) {
      out.push(m)
      continue
    }
    let sawAssistant = false
    let hit = -1
    for (let i = out.length - 1; i >= 0; i--) {
      const r = out[i]
      if (r.turnEnd) break
      if (r.me && !r.queued) {
        if (!sawAssistant && isSameSendReplay(r, m)) hit = i
        break
      }
      if (isAssistantText(r)) sawAssistant = true
    }
    if (hit >= 0) {
      out[hit] = richerUserRow(out[hit], m)
      continue
    }
    out.push(m)
  }
  return out
}

// mergePersistedWithLive grafts a durable transcript onto a live turn.
//
// Live session.delta events can populate an empty in-memory bucket with only
// the current stream. Align that tail onto the store snapshot so pending tools
// are not duplicated and settled live rows that post-date the snapshot are kept.
// When live already has the conversation, keep it — it is ahead of the store.
export function mergePersistedWithLive(persisted: AgentMsg[], live: AgentMsg[]): AgentMsg[] {
  return collapseDuplicateUsersInTurn(mergeLiveOntoPersist(persisted, live))
}

function mergeLiveOntoPersist(persisted: AgentMsg[], live: AgentMsg[]): AgentMsg[] {
  if (!live.length) return persisted
  if (!persisted.length) return live
  const liveCore = live.filter((m) => !m.queued)
  if (!liveCore.length) return persisted
  if (userTurnCount(liveCore) >= userTurnCount(persisted) && liveCore.length >= persisted.length) {
    return liveCore
  }
  // Same display prompt on a later dispatch (Reviewer display is reused) must
  // not stitch from persist[0] — that overlays the live tail onto turn 1.
  if (!liveCore[0].optimistic && rowEqualLoose(persisted[0], liveCore[0]) && userTurnCount(liveCore) >= userTurnCount(persisted)) {
    return stitchFullTranscripts(persisted, liveCore)
  }
  const start = liveAlignStart(persisted, liveCore)
  if (start < persisted.length) return [...persisted.slice(0, start), ...liveCore]
  // Live shares a history prefix but is not a suffix overlay (optimistic user
  // breaks rowEqual at the current turn). Keep the store snapshot and append
  // only live rows after that prefix — never the whole conversation.
  const prefix = commonPrefixLen(persisted, liveCore)
  if (prefix > 0) {
    return overlayAligned(persisted, liveCore.slice(prefix), prefix)
  }
  // Leading live users that replay persist's last send are the same turn
  // (queued flush + session.user). A lone optimistic user is a resend.
  const lastUser = trailingUser(persisted)
  let skip = 0
  if (lastUser) {
    while (
      skip < liveCore.length &&
      liveCore[skip].me &&
      !liveCore[skip].queued &&
      sameUserChatBody(liveCore[skip].body, lastUser.body)
    ) {
      skip++
    }
  }
  if (skip === 0) return [...persisted, ...liveCore]
  const extra = liveCore.slice(skip)
  const from = indexAfterTrailingUser(persisted)
  if (!extra.length) {
    if (lastUserTurnStillOpen(persisted)) return persisted
    if (skip === 1 && liveCore[0].optimistic) return [...persisted, ...liveCore]
    return persisted
  }
  if (persisted.slice(from).some((m) => m.turnEnd) && skip === 1 && liveCore[0].optimistic) {
    return [...persisted, liveCore[0], ...extra]
  }
  const overlaid = overlayAligned(persisted, extra, from)
  const matched = extra.length - (overlaid.length - persisted.length)
  if (lastUserTurnStillOpen(persisted) || matched > 0) return overlaid
  const opt = liveCore.slice(0, skip).filter((m) => m.optimistic)
  if (!opt.length) return overlaid
  return [...persisted, ...opt, ...overlaid.slice(persisted.length)]
}

/** Keep client-only optimistic user bubbles the store snapshot has not recorded yet.
 *  Idle transcript overwrites drop live stream rows on purpose (phantom spinners);
 *  they must not also wipe a send that has not landed in WorkspaceChatItems. */
export function retainOptimisticUsers(mapped: AgentMsg[], live: AgentMsg[]): AgentMsg[] {
  const extras = live.filter((m) =>
    !!m.optimistic && !!m.me && !persistHasUserSend(mapped, m, live),
  )
  return collapseDuplicateUsersInTurn(extras.length ? [...mapped, ...extras] : mapped)
}

// mapChatItems flattens a persisted session transcript into chat rows.
export function mapChatItems(items: ChatItem[] | null | undefined): AgentMsg[] {
  const mapped: AgentMsg[] = []
  for (const it of items ?? []) {
    const row = mapChatItem(it)
    if (row) mapped.push(row)
  }
  return markTrailingWidenStream(collapseAgentErrorDupes(mapped))
}

/** Store snapshots omit `streaming`. Mark the trailing assistant row so a
 *  remount's next cumulative event can widen it instead of duplicating. */
function markTrailingWidenStream(list: AgentMsg[]): AgentMsg[] {
  if (!list.length) return list
  const last = list[list.length - 1]
  if (last.me || last.tool || last.turnEnd || last.compact || last.note || last.fallback || last.stopped || last.error) return list
  if (last.streaming || (!last.body && !last.thinking)) return list
  const out = list.slice()
  out[out.length - 1] = { ...last, widenStream: true }
  return out
}

function mapChatItem(it: ChatItem): AgentMsg | null {
  if (it.kind === 'tool') {
    const children = (it.children ?? []).map(mapChatItem).filter((m): m is AgentMsg => !!m)
    return {
      tool: it.tool, toolInput: it.toolInput, toolId: it.toolId, description: it.description,
      input: it.input, result: it.result, resultError: it.resultError,
      resultTruncated: it.resultTruncated, resultPending: it.resultPending,
      ts: it.ts || 0,
      ...(children.length ? { children } : {}),
    }
  }
  if (it.kind === 'thinking') {
    return { thinking: true, body: it.text, tokens: it.tokens || 0, ts: it.ts || 0 }
  }
  if (it.kind === 'compact') {
    return {
      compact: true,
      body: it.text,
      tokens: it.tokens || 0,
      ts: it.ts || 0,
    }
  }
  if (it.kind === 'fallback') {
    return {
      fallback: true,
      body: it.text,
      description: it.description,
      ts: it.ts || 0,
    }
  }
  if (it.kind === 'turn') {
    return {
      turnEnd: true,
      durationMs: it.durationMs || 0,
      messageId: it.messageId || '',
      checkpoint: !!it.checkpoint,
      ts: it.ts || 0,
    }
  }
  if (it.role === 'user' && (isRetryDisplay(it.text) || isModeSwitchContinueDisplay(it.text))) return null
  if (it.role === 'user') {
    const label = subagentNoticeLabel(it.text ?? '')
    if (label !== null) return label ? { note: true, noteLabel: 'Subagent', body: label, ts: it.ts || 0 } : null
  }
  if (isChatStoppedReply(it.text) && it.role !== 'user') {
    return { body: CHAT_STOPPED_REPLY, stopped: true, ts: it.ts || 0 }
  }
  if (isAgentErrorReply(it.text || '') && it.role !== 'user') {
    return { body: it.text, error: true, ts: it.ts || 0 }
  }
  return {
    // User text is already collapsed (display_text || text) in Go flatten;
    // displayUserChatText covers ask_user resume and Coder dispatch wire that
    // landed without display_text (older sessions, e2e mocks).
    body: it.role === 'user' ? displayUserChatText(it.text) : it.text,
    me: it.role === 'user',
    ts: it.ts || 0,
    attachments: (it.attachments ?? []).map((a) => ({
      id: a.id,
      name: a.name || a.id,
      mime: a.mime || '',
      url: a.url || '',
    })),
  }
}

/** Wire payload for Send*Message attachments (base64, no data: prefix). */
export function toSendAttachments(drafts: ChatAttachmentDraft[]): {
  id: string
  name: string
  mime: string
  data: string
}[] {
  return drafts.map((d) => ({
    id: d.id,
    name: d.name,
    mime: d.mime,
    data: d.dataUrl.includes(',') ? d.dataUrl.slice(d.dataUrl.indexOf(',') + 1) : d.dataUrl,
  }))
}

export function newAttachmentId(): string {
  return `img_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`
}

export function readFileAsDataURL(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader()
    r.onload = () => resolve(String(r.result || ''))
    r.onerror = () => reject(r.error)
    r.readAsDataURL(file)
  })
}

/** Image files from a clipboard paste (empty if none). */
export function imageFilesFromPaste(e: ClipboardEvent): File[] {
  const items = e.clipboardData?.items
  if (!items?.length) return []
  const images: File[] = []
  for (const item of items) {
    if (item.kind === 'file' && item.type.startsWith('image/')) {
      const f = item.getAsFile()
      if (f) images.push(f)
    }
  }
  return images
}

/** Build composer drafts from pasted/dropped image files. */
export async function draftsFromImageFiles(files: File[]): Promise<ChatAttachmentDraft[]> {
  const added: ChatAttachmentDraft[] = []
  for (const file of files) {
    if (!file.type.startsWith('image/')) continue
    const id = newAttachmentId()
    const dataUrl = await readFileAsDataURL(file)
    added.push({
      id,
      name: file.name || `${id}.png`,
      mime: file.type || 'image/png',
      dataUrl,
    })
  }
  return added
}

/** Asset-server route that streams an absolute-path chat image (Wails Finder drops). */
export const CHAT_IMAGE_PATH = '/chat-image'

export function chatImageURL(absPath: string): string {
  return `${CHAT_IMAGE_PATH}?${new URLSearchParams({ path: absPath })}`
}

/** Load Finder-dropped image paths into the same drafts paste uses. */
export async function draftsFromImagePaths(paths: readonly string[]): Promise<ChatAttachmentDraft[]> {
  const files: File[] = []
  for (const path of paths) {
    try {
      const r = await fetch(chatImageURL(path))
      if (!r.ok) continue
      const blob = await r.blob()
      const type = (blob.type || '').split(';')[0].trim() || 'image/png'
      if (!type.startsWith('image/')) continue
      const name = path.replace(/\\/g, '/').split('/').pop() || 'image.png'
      files.push(new File([blob], name, { type }))
    } catch {
      /* skip this file */
    }
  }
  return draftsFromImageFiles(files)
}

// appendStream extends the trailing streaming row of the given kind by APPENDING
// a chunk (legacy / non-snapshot path). Prefer applyStream for live deltas —
// those arrive as cumulative snapshots with a monotonic seq.
export function appendStream(list: AgentMsg[], kind: 'text' | 'thinking', chunk: string): AgentMsg[] {
  const out = list.slice()
  const last = out[out.length - 1]
  const sameKind = !!last && !!last.streaming && !last.tool && !last.compact && !last.note && !last.fallback && !!last.thinking === (kind === 'thinking')
  if (sameKind) {
    out[out.length - 1] = { ...last, body: (last.body ?? '') + chunk }
  } else {
    closeOpenStreamsInPlace(out, kind)
    if (out[out.length - 1]?.streaming) out[out.length - 1] = { ...out[out.length - 1], streaming: false }
    out.push(kind === 'thinking' ? { thinking: true, body: chunk, streaming: true } : { body: chunk, streaming: true })
  }
  return out
}

// closeOpenStreamsInPlace clears live text/thinking flags in the open turn.
// A mid-turn user (steer) can sit after a still-streaming assistant row;
// opening another live-text row would duplicate AgentChat's `m:live-text` key.
function closeOpenStreamsInPlace(out: AgentMsg[], kind?: 'text' | 'thinking'): void {
  for (let i = out.length - 1; i >= 0; i--) {
    if (out[i].turnEnd) break
    const m = out[i]
    if (!m.streaming || m.me || m.tool || m.compact || m.note || m.fallback) continue
    if (kind && !!m.thinking !== (kind === 'thinking')) continue
    out[i] = { ...m, streaming: false }
  }
}

// applyStream stamps a CUMULATIVE snapshot onto the trailing streaming row of
// the given kind (or opens one). Body is replaced, not appended, so each
// event is a full snapshot. done:true closes the row (block finished) so the
// next tool/reply starts fresh.
export function applyStream(
  list: AgentMsg[],
  kind: 'text' | 'thinking',
  text: string,
  opts?: { done?: boolean; parentId?: string; uuid?: string },
): AgentMsg[] {
  if (opts?.parentId) {
    const path = findToolPath(list, opts.parentId)
    if (!path) return list
    return updateAtPath(list, path, (parent) => ({
      ...parent,
      children: applyStream(parent.children ?? [], kind, text, { done: opts.done, uuid: opts.uuid }),
    }))
  }
  if (!text && !opts?.done) return list
  const out = list.slice()
  const streaming = !opts?.done
  const uuid = (opts?.uuid || '').trim()
  const isKind = (m: AgentMsg) =>
    !m.tool && !m.turnEnd && !m.me && !m.compact && !m.note && !m.fallback && !!m.thinking === (kind === 'thinking')

  const last = out[out.length - 1]
  if (last?.streaming && isKind(last)) {
    if (uuid && last.providerUUID && uuid !== last.providerUUID) {
      out[out.length - 1] = { ...last, streaming: false }
    } else {
      out[out.length - 1] = {
        ...last,
        body: text || last.body,
        streaming,
        providerUUID: uuid || last.providerUUID,
        ...(kind === 'thinking' ? { thinking: true as const, tokens: last.tokens } : {}),
      }
      return out
    }
  }
  // Warm remount: WorkspaceChatItems maps the same block without streaming.
  // Only widen a row marked by mapChatItems, and only when this snapshot is
  // a continuation — a second consecutive block must open a new row.
  if (last && isKind(last) && !opts?.done && last.widenStream) {
    const prev = last.body ?? ''
    if (!prev || text.startsWith(prev)) {
      out[out.length - 1] = {
        ...last,
        body: text || prev,
        streaming,
        widenStream: false,
        providerUUID: uuid || last.providerUUID,
        ...(kind === 'thinking' ? { thinking: true as const, tokens: last.tokens } : {}),
      }
      return out
    }
    out[out.length - 1] = { ...last, widenStream: false }
  }
  // Final settle when the live row was already closed (or never opened).
  // Stay in the current tail: walking past a tool would rewrite the first
  // thought in a specialist session with every later thinking snapshot.
  if (opts?.done) {
    for (let i = out.length - 1; i >= 0; i--) {
      if (out[i].me || out[i].turnEnd) break
      if (isKind(out[i])) {
        out[i] = { ...out[i], body: text || out[i].body, streaming: false }
        return out
      }
      if (out[i].tool || out[i].compact || out[i].note || out[i].fallback || out[i].thinking || out[i].body) break
    }
  }
  closeOpenStreamsInPlace(out, kind)
  if (out[out.length - 1]?.streaming) out[out.length - 1] = { ...out[out.length - 1], streaming: false }
  out.push(
    kind === 'thinking'
      ? { thinking: true, body: text, streaming, providerUUID: uuid || undefined }
      : { body: text, streaming, providerUUID: uuid || undefined },
  )
  return out
}

// settleThinking stamps the provider's complete thinking block onto the live row
// and closes it. Thin wrapper over applyStream({ done: true }).
export function settleThinking(list: AgentMsg[], text: string): AgentMsg[] {
  return applyStream(list, 'thinking', text, { done: true })
}

// settleText stamps the provider's complete assistant text and closes the row.
export function settleText(list: AgentMsg[], text: string): AgentMsg[] {
  return applyStream(list, 'text', text, { done: true })
}

// noteThinkingTokens records redacted-thinking progress: bump the trailing
// streaming thinking row's token estimate, or open a fresh thinking row.
export function noteThinkingTokens(list: AgentMsg[], tokens: number, uuid?: string, parentId?: string): AgentMsg[] {
  if (parentId) {
    const path = findToolPath(list, parentId)
    if (!path) return list
    return updateAtPath(list, path, (parent) => ({
      ...parent,
      children: noteThinkingTokens(parent.children ?? [], tokens, uuid),
    }))
  }
  const out = list.slice()
  const last = out[out.length - 1]
  const id = (uuid || '').trim()
  if (last && last.streaming && last.thinking) {
    // Token ticks are progress on the current thought. Claude Code mints a
    // unique uuid per system/thinking_tokens line — that is not a new frame.
    out[out.length - 1] = {
      ...last,
      tokens: Math.max(last.tokens ?? 0, tokens),
      providerUUID: last.providerUUID || id || undefined,
    }
  } else {
    closeOpenStreamsInPlace(out, 'thinking')
    if (out[out.length - 1]?.streaming) out[out.length - 1] = { ...out[out.length - 1], streaming: false }
    out.push({ thinking: true, streaming: true, tokens, providerUUID: id || undefined })
  }
  return out
}

function findToolPath(list: AgentMsg[], id: string): number[] | null {
  if (!id) return null
  for (let i = list.length - 1; i >= 0; i--) {
    const m = list[i]
    if (m.tool && m.toolId === id) return [i]
    if (m.children?.length) {
      const inner = findToolPath(m.children, id)
      if (inner) return [i, ...inner]
    }
  }
  return null
}

function updateAtPath(list: AgentMsg[], path: number[], fn: (m: AgentMsg) => AgentMsg): AgentMsg[] {
  const i = path[0]
  if (i == null || i < 0 || i >= list.length) return list
  const out = list.slice()
  if (path.length === 1) {
    out[i] = fn(out[i])
    return out
  }
  const cur = out[i]
  out[i] = { ...cur, children: updateAtPath(cur.children ?? [], path.slice(1), fn) }
  return out
}

function settleInFlight(m: AgentMsg): AgentMsg {
  const children = m.children?.map(settleInFlight)
  const next = m.streaming || m.resultPending
    ? { ...m, streaming: false, resultPending: false }
    : m
  return children ? { ...next, children } : next
}

/** Append a mid-turn user message (session.user). Settles the oldest unmatched
 *  optimistic row with this body; a real row of the same text is a new message. */
/** Engine-to-agent notices carry a hidden marker (Go displayHiddenSentinel):
 *  bare marker = hidden bubble; marker + label = a "Subagent" notice row. */
const NOTICE_MARKER = '\u2064'
export function subagentNoticeLabel(text: string): string | null {
  if (!text.startsWith(NOTICE_MARKER)) return null
  return text.slice(NOTICE_MARKER.length).trim()
}

export function addUser(list: AgentMsg[], text: string): AgentMsg[] {
  const label = subagentNoticeLabel(text)
  if (label !== null) {
    if (!label) return list
    const out = list.slice()
    closeOpenStreamsInPlace(out)
    return [...out, { note: true, noteLabel: 'Subagent', body: label, ts: Date.now() }]
  }
  const body = text.trim()
  if (!body) return list
  let turnStart = 0
  for (let i = 0; i < list.length; i++) {
    if (list[i].turnEnd) turnStart = i + 1
  }
  const out = list.slice()
  closeOpenStreamsInPlace(out)
  for (let i = turnStart; i < out.length; i++) {
    const m = out[i]
    if (m.me && m.optimistic && sameUserChatBody(m.body, body)) {
      out[i] = { ...m, optimistic: false, body, ts: m.ts ?? Date.now() }
      return out
    }
  }
  return [...out, { me: true, body, ts: Date.now() }]
}

// addTool upserts a live tool-call row. Cursor (and some models) re-report the
// same call as several session.step frames while it runs; appending each one
// stacked identical "Reading…" rows until the result (or a transcript resync)
// replaced them. Same provider id → update the still-open row in place (a
// settled match is a real second call, same as attachToolResult). Blank ids
// cannot be correlated across tools, so only the last in-flight row of that
// tool name with no id in this turn is reused — better a merged row than a
// swallowed call. Nested Task-subagent tools (parentId) land under the parent
// Task card instead of as sibling rows.
export function addTool(list: AgentMsg[], step: WorkspaceStep): AgentMsg[] {
  if (step.parentId) {
    const path = findToolPath(list, step.parentId)
    if (!path) return list
    const childStep = { ...step, parentId: undefined }
    return updateAtPath(list, path, (parent) => ({
      ...parent,
      children: addTool(parent.children ?? [], childStep),
    }))
  }
  const patch = (m: AgentMsg): AgentMsg => ({
    ...m,
    tool: step.tool || m.tool,
    toolInput: step.toolInput || m.toolInput,
    toolId: step.id || m.toolId,
    description: step.description || m.description,
    // Cursor re-reports the same call; a later stub must not freeze over a
    // richer body already shown (createPlan / partial args).
    input: richerJSON(step.input, m.input),
    resultPending: true,
  })
  if (step.id) {
    const path = findToolPath(list, step.id)
    if (path) {
      const cur = path.reduce((acc: AgentMsg | undefined, i, idx) => {
        const row = idx === 0 ? list[i] : acc?.children?.[i]
        return row
      }, undefined as AgentMsg | undefined)
      if (cur?.resultPending) {
        return updateAtPath(list, path, patch)
      }
    }
    // mapChatItems omits provider ids. After a remount fetch, the next
    // session.step would append a second card unless we adopt that row.
    for (let i = list.length - 1; i >= 0; i--) {
      const m = list[i]
      if ((m.me && !m.queued) || m.turnEnd) break
      if (m.tool === step.tool && m.resultPending && !m.toolId) {
        if (step.toolInput && m.toolInput && step.toolInput !== m.toolInput) continue
        const out = list.slice()
        out[i] = patch(m)
        return out
      }
    }
  } else {
    for (let i = list.length - 1; i >= 0; i--) {
      const m = list[i]
      if ((m.me && !m.queued) || m.turnEnd) break
      if (m.tool === step.tool && m.resultPending && !m.toolId) {
        const out = list.slice()
        out[i] = patch(m)
        return out
      }
    }
  }
  const child: AgentMsg = {
    tool: step.tool, toolInput: step.toolInput, toolId: step.id,
    description: step.description, input: step.input,
    providerUUID: step.uuid,
    // In flight until session.tool_result arrives. Tracked explicitly because an
    // empty result is legitimate (delete, TodoWrite, CreatePlan return nothing).
    resultPending: true,
  }
  const closed = list.map((m) => (m.streaming ? { ...m, streaming: false } : m))
  return [...closed, child]
}

// Steps {#each} key. toolId is not unique once addTool appends a second row
// for a settled id, so mix in the list index.
export function stepRowKey(toolId: string | undefined, i: number): string {
  return toolId ? 'id:' + toolId + ':' + i : 'i' + i
}

function richerJSON(next?: string, prev?: string): string | undefined {
  const n = next && next !== '{}' ? next : ''
  const p = prev && prev !== '{}' ? prev : ''
  if (!n) return prev
  if (!p) return next
  return n.length >= p.length ? next : prev
}

// attachToolResult fills a live tool call's result into its row (matched by the
// provider tool-use id, newest match first) so an expanded card streams in place.
// Optional input/toolInput refresh args that only arrived on completion (Codex MCP).
export function attachToolResult(
  list: AgentMsg[],
  id: string,
  result: string,
  isError: boolean,
  extras?: { input?: string; toolInput?: string },
): AgentMsg[] {
  let hit = -1
  for (let i = list.length - 1; i >= 0; i--) {
    if (list[i].tool && list[i].toolId === id) {
      hit = i
      break
    }
  }
  if (hit < 0) {
    for (let i = 0; i < list.length; i++) {
      const kids = list[i].children
      if (!kids?.length) continue
      const nested = attachToolResult(kids, id, result, isError, extras)
      if (nested !== kids) {
        const out = list.slice()
        out[i] = { ...list[i], children: nested }
        return out
      }
    }
    return list
  }
  const cur = list[hit]
  const settled: AgentMsg = {
    ...cur,
    result,
    resultError: isError,
    resultPending: false,
    input: extras?.input && extras.input !== '{}' ? extras.input : cur.input,
    toolInput: extras?.toolInput || cur.toolInput,
  }
  // Drop in-flight clones of this id (a live step that appended instead of
  // upserting). Keep any already-settled row — that would be a real second call.
  // Blank ids are not unique, so never use them as a drop key.
  const out: AgentMsg[] = []
  for (let i = 0; i < list.length; i++) {
    if (i === hit) {
      out.push(settled)
      continue
    }
    const m = list[i]
    if (id && m.tool && m.toolId === id && m.resultPending) continue
    out.push(m)
  }
  return out
}

/** Refresh tool args only (ask_user park backfill) — does not settle the result. */
export function patchToolInput(
  list: AgentMsg[],
  id: string,
  extras: { input?: string; toolInput?: string },
): AgentMsg[] {
  const input = extras.input && extras.input !== '{}' ? extras.input : undefined
  const toolInput = extras.toolInput || undefined
  if (!input && !toolInput) return list
  const path = findToolPath(list, id)
  if (!path) return list
  return updateAtPath(list, path, (m) => ({
    ...m,
    input: input || m.input,
    toolInput: toolInput || m.toolInput,
  }))
}

// endTurn appends the turn footer carrying the provider's own wall time, so a
// finished turn keeps the run time that was ticking while it ran instead of it
// vanishing. Settles in-flight rows so a leftover delta cannot open a second
// live-text key under the footer. A repeat footer returns the same array.
export function endTurn(list: AgentMsg[], durationMs: number): AgentMsg[] {
  if (!durationMs) return list
  const last = list[list.length - 1]
  if (last?.turnEnd) return list
  const closed = list.map(settleInFlight)
  return [...closed, { turnEnd: true, durationMs, ts: Date.now() }]
}

/**
 * True when the open turn already ends on a "completed in" footer.
 * sending / agentsRunning can stay lit after session.turn_end (Send RPC lag,
 * lost idle, Maestro still directing). A turn that started after that footer
 * — Retry and autonomous MA turns set turnStartedAt without a `me` bubble —
 * reopens the clock immediately. So does a new user / stream / tool row.
 * Callers that append live stream/tool rows must also consult
 * `dropLiveAfterFooter` so a leftover delta cannot reopen the elapsed clock,
 * while a new engine-driven turn (busy, stamp already cleared) can still land.
 */
export function currentTurnHasEnded(list: AgentMsg[], turnStartedAt = 0): boolean {
  for (let i = list.length - 1; i >= 0; i--) {
    const m = list[i]
    if (m.queued || m.log) continue
    if (!m.turnEnd) return false
    const footerAt = m.ts ?? 0
    // A stamp at or after the footer is a new turn (Retry / engine-driven). Persisted
    // footers may omit ts (mapChatItems uses `it.ts || 0`); any positive stamp
    // is still newer, otherwise the next turn cannot reopen past a ts=0 footer.
    if (turnStartedAt > 0 && turnStartedAt >= footerAt) return false
    return true
  }
  return false
}

/**
 * True when a live append (delta/step/thinking/compact) belongs to a turn that
 * already has its footer and must not grow a tail under it.
 *
 * Leftover events after session.turn_end keep the old turnStartedAt (Send /
 * agentsRunning lag) — drop those. After idle the stamp is cleared; if the
 * engine is busy again this is a new autonomous / reload turn, so let it
 * through. Retry already stamped turnStartedAt after the footer.
 */
export function dropLiveAfterFooter(
  list: AgentMsg[],
  turnStartedAt: number,
  engineBusy: boolean,
): boolean {
  if (!currentTurnHasEnded(list, turnStartedAt)) return false
  if (turnStartedAt > 0) return true
  return !engineBusy
}

/** Stamp (or replace) turnStartedAt when a running status arrives with no stamp,
 *  or with a stamp that still belongs to the turn whose footer is already on
 *  the tail (lost idle — leftover drop would otherwise swallow the next turn). */
export function shouldRefreshTurnStamp(list: AgentMsg[], turnStartedAt: number): boolean {
  return turnStartedAt <= 0 || currentTurnHasEnded(list, turnStartedAt)
}

/** Idle that reordered past the next turn's working/directing must not clear
 *  a stamp already newer than the footer, or resync-wipe that live turn. */
export function shouldIgnoreStaleIdle(list: AgentMsg[], turnStartedAt: number): boolean {
  if (turnStartedAt <= 0) return false
  for (let i = list.length - 1; i >= 0; i--) {
    const m = list[i]
    if (m.queued || m.log) continue
    if (!m.turnEnd) return false
    const footerAt = m.ts ?? 0
    return footerAt > 0 && turnStartedAt >= footerAt
  }
  return false
}

/**
 * Skip stamp-clear / resync on idle only for the first idle after we replaced
 * a stamp over a footer (`staleIdleArmed`): either that idle arrived while the
 * tail is still the footer, or while rows have already landed after it.
 * Later idles must settle so a turn that streamed and then ended without
 * `session.turn_end` still gets a store-authoritative resync.
 */
export function shouldSkipIdleTranscriptReset(
  list: AgentMsg[],
  turnStartedAt: number,
  staleIdleArmed: boolean,
): boolean {
  if (!staleIdleArmed) return false
  if (shouldIgnoreStaleIdle(list, turnStartedAt)) return true
  if (turnStartedAt <= 0) return false
  let footerAt = 0
  let rowsAfterFooter = false
  for (const m of list) {
    if (m.queued || m.log) continue
    if (m.turnEnd) {
      footerAt = m.ts ?? 0
      rowsAfterFooter = false
      continue
    }
    if (footerAt > 0) rowsAfterFooter = true
  }
  return rowsAfterFooter && turnStartedAt >= footerAt
}

/** Last assistant text bubble in the turn that turnEndIndex closes. */
export function lastAssistantTextInTurn(list: AgentMsg[], turnEndIndex: number): AgentMsg | null {
  if (turnEndIndex <= 0 || turnEndIndex >= list.length) return null
  for (let i = turnEndIndex - 1; i >= 0; i--) {
    const m = list[i]
    if (m.me || m.turnEnd) break
    if (m.log || m.tool || m.thinking || m.compact || m.note || m.fallback || m.stopped || m.error) continue
    if ((m.body ?? '').trim()) return m
  }
  return null
}

export type CompactEvt = {
  status: string
  summary?: string
  preTokens?: number
  trigger?: string
}

/** Insert or settle a context-compaction row in the live transcript. */
export function applyCompact(list: AgentMsg[], evt: CompactEvt): AgentMsg[] {
  const compacting = evt.status === 'started'
  const row: AgentMsg = {
    compact: true,
    compacting,
    body: evt.summary || '',
    tokens: evt.preTokens || 0,
  }
  const out = list.slice()
  const last = out[out.length - 1]
  if (last?.compacting) {
    out[out.length - 1] = { ...last, ...row, body: row.body || last.body }
    return out
  }
  if (last?.streaming) out[out.length - 1] = { ...last, streaming: false }
  out.push(row)
  return out
}

/** Live Recur status (drain, retry) — not assistant text and not persisted. */
export function applyNotice(list: AgentMsg[], text: string): AgentMsg[] {
  const t = text.trim()
  if (!t) return list
  const out = list.slice()
  const last = out[out.length - 1]
  if (last?.note) {
    out[out.length - 1] = { ...last, body: t }
    return out
  }
  if (last?.streaming) out[out.length - 1] = { ...last, streaming: false }
  out.push({ note: true, body: t })
  return out
}

/** Drop in-progress compaction rows (started is live-only; interrupted compact never persisted). */
export function dropOpenCompact(list: AgentMsg[]): AgentMsg[] {
  if (!list.some((m) => m.compacting)) return list
  return list.filter((m) => !m.compacting)
}

export type FallbackEvt = {
  from?: string
  to?: string
  fromLabel?: string
  toLabel?: string
  retracted?: string[]
}

/** Insert a model-fallback notice. Idempotent for the same serving model in this turn.
 *  An incomplete first hop (empty from) is enriched when a later audit names it.
 *  Retracted provider frame ids drop the refused leg's live rows. */
export function applyFallback(list: AgentMsg[], evt: FallbackEvt): AgentMsg[] {
  let out = evt.retracted?.length ? dropRetractedFrames(list, evt.retracted) : list.slice()
  const to = (evt.toLabel || evt.to || '').trim()
  if (!to) return out
  const from = (evt.fromLabel || evt.from || '').trim()
  for (let i = out.length - 1; i >= 0; i--) {
    const m = out[i]
    if (m.me || m.turnEnd) break
    if (m.fallback && (m.body ?? '') === to) {
      const have = (m.description ?? '').trim()
      if (!from || have === from) return out
      if (!have) {
        const next = out.slice()
        next[i] = { ...m, description: from }
        return next
      }
      return out
    }
  }
  const last = out[out.length - 1]
  if (last?.streaming) out[out.length - 1] = { ...last, streaming: false }
  const row = { fallback: true, body: to, description: from }
  if (evt.retracted?.length) {
    let start = 0
    for (let i = out.length - 1; i >= 0; i--) {
      if (out[i].me || out[i].turnEnd) {
        start = i + 1
        break
      }
    }
    out.splice(start, 0, row)
    return out
  }
  out.push(row)
  return out
}

function dropRetractedFrames(list: AgentMsg[], uuids: string[]): AgentMsg[] {
  const drop = new Set(uuids.map((u) => u.trim()).filter(Boolean))
  if (!drop.size) return list
  let start = 0
  for (let i = list.length - 1; i >= 0; i--) {
    if (list[i].me || list[i].turnEnd) {
      start = i + 1
      break
    }
  }
  const tail = list.slice(start)
  const matched = tail.some((m) => !!m.providerUUID && drop.has(m.providerUUID))
  const fb = tail.findIndex((m) => m.fallback)
  const kept: AgentMsg[] = []
  for (let i = 0; i < tail.length; i++) {
    const m = tail[i]
    if (m.providerUUID && drop.has(m.providerUUID)) continue
    if (!matched && !m.providerUUID && !m.fallback && (fb < 0 || i < fb) && (m.tool || m.thinking || m.body)) continue
    kept.push(m)
  }
  return [...list.slice(0, start), ...kept]
}

/** Stable prefix on backend-synthesized chat failure replies (matches Go). */
export const AGENT_ERROR_PREFIX = 'Agent error:'

/** Durable / live note when the user interrupts an in-flight chat turn. */
export const CHAT_STOPPED_REPLY = 'Stopped by user'

/**
 * UI-only display_text the chat Retry button sends (matches Go
 * chatRetryDisplaySentinel). Word joiner — hidden from the transcript so a
 * retry does not add a second user bubble.
 */
export const CHAT_RETRY_DISPLAY = '\u2060'

/**
 * SendWorkspaceMessage reply when a follow-up was injected into a live Cursor
 * turn. Matches Go chatInjectedSentinel — must not finishTurn / idle the tab.
 */
export const CHAT_INJECTED_REPLY = '\u2060inject'

export function isChatInjectedReply(text: string | null | undefined): boolean {
  return (text || '') === CHAT_INJECTED_REPLY
}

/** Drop an optimistic user row even if a turn footer already landed after it. */
export function dropOptimisticUser(
  list: AgentMsg[],
  display: string,
  pendingId?: string,
): AgentMsg[] {
  for (let i = list.length - 1; i >= 0; i--) {
    const m = list[i]
    if (!m.optimistic || !m.me) continue
    if (pendingId ? m.pendingId === pendingId : sameUserChatBody(m.body, display)) {
      return [...list.slice(0, i), ...list.slice(i + 1)]
    }
  }
  return list
}

/** Rejected steers in send order, then text typed after them.
 *  simplify: in-paragraph edits of a restored steer between rejections can
 *  duplicate. Upgrade: the caller keeps the restored prefix as one unit. */
export function mergeRejectedSteerDraft(failedMarked: string[], currentPlain: string): string {
  const steers = failedMarked.map((s) => s.trim()).filter(Boolean)
  let tail = currentPlain.trim()
  for (const steer of steers) {
    if (!tail) break
    if (tail === steer) {
      tail = ''
      continue
    }
    if (tail.startsWith(steer + '\n\n')) {
      tail = tail.slice(steer.length + 2).trim()
    }
  }
  return [...steers, tail].filter(Boolean).join('\n\n')
}

export function mergeRejectedSteerAttachments<T extends { id: string }>(
  failedDrafts: T[][],
  current: T[],
): T[] {
  const seen = new Set<string>()
  const out: T[] = []
  for (const group of [...failedDrafts, current]) {
    for (const a of group) {
      if (!a?.id || seen.has(a.id)) continue
      seen.add(a.id)
      out.push(a)
    }
  }
  return out
}

const GENERIC_AGENT_ERROR_REPLIES = new Set([
  'I hit an error trying to respond just now. Try again.',
  'The agent hit an error — try again.',
])

/** True when text is the user-interrupt note (current or legacy wording). */
export function isChatStoppedReply(text: string | null | undefined): boolean {
  const t = (text || '').trim()
  return t === CHAT_STOPPED_REPLY || t === 'Stopped.'
}

/** True when the send() reply is a provider/engine failure note to keep visible. */
export function isAgentErrorReply(text: string): boolean {
  const t = (text || '').trim()
  return t.startsWith(AGENT_ERROR_PREFIX) || GENERIC_AGENT_ERROR_REPLIES.has(t)
}

/** Drop CSI/SGR so colored CLI stderr can sit in a web error card. */
// simplify: SGR/C0 only; OSC/DCS stay in term.StripVT. Upgrade if a CLI dumps those into chat.
function stripVT(s: string): string {
  return s.replace(/\x1b\[[0-9;]*m/g, '').replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g, '')
}

/** Keep in sync with Go codexLoginExpiredMsg. */
const CODEX_LOGIN_EXPIRED = 'Codex login expired — run `codex login` in a terminal, then retry.'

function knownAgentError(s: string): string {
  const d = s.toLowerCase()
  if (d.includes('failed to refresh token') || (d.includes('codex_login') && d.includes('401'))) {
    return CODEX_LOGIN_EXPIRED
  }
  return ''
}

function stripCLILogPrefix(s: string): string {
  const i = s.indexOf(' - ')
  if (i <= 0) return s
  const head = s.slice(0, i)
  const u = ` ${head.toUpperCase()} `
  if (!/ ERROR | WARN | WARNING | INFO | DEBUG | TRACE /.test(u)) return s
  if (!head.includes('::') && !(head.includes('T') && head.includes('Z'))) return s
  return s.slice(i + 3).trim()
}

function tidyAgentError(s: string): string {
  s = stripVT(s)
  const known = knownAgentError(s)
  if (known) return known
  const stripped = stripCLILogPrefix(s)
  if (stripped !== s) {
    s = stripped
    const brace = s.indexOf('{')
    if (brace > 0) s = s.slice(0, brace)
  }
  return s.trim()
}

/** Provider/engine detail with the stable "Agent error:" prefix stripped. */
export function agentErrorDetail(text: string | null | undefined): string {
  let t = stripVT((text || '').trim())
  if (t.startsWith(AGENT_ERROR_PREFIX)) t = t.slice(AGENT_ERROR_PREFIX.length).trim()
  return tidyAgentError(t)
}

/** Card title for a failed chat turn — usage/rate-limit when the provider said so. */
export function agentErrorTitle(text: string | null | undefined): string {
  const d = agentErrorDetail(text).toLowerCase()
  if (
    /spend limit|usage limit|usage_limit|usage-credits|credit balance|insufficient credit|\bquota\b|\bbilling\b|upgrade your plan/.test(
      d,
    )
  ) {
    return 'Usage limit'
  }
  if (/rate limit|rate_limit|too many requests|\b429\b/.test(d)) return 'Rate limit'
  if (/codex login|refresh token|login expired/.test(d)) return 'Codex login'
  return 'Agent error'
}

function sameErrorDetail(a: string, b: string): boolean {
  return agentErrorDetail(a).toLowerCase() === agentErrorDetail(b).toLowerCase()
}

function isRetryDisplay(text: string | null | undefined): boolean {
  return (text || '') === CHAT_RETRY_DISPLAY
}

function isModeSwitchContinueDisplay(text: string | null | undefined): boolean {
  const t = (text || '').trim()
  return t.startsWith('Switched to ') && t.endsWith(': continuing')
}

/**
 * Fold a provider-echoed error line into the synthesized Agent error note, and
 * drop a duplicate error row. Keeps a turnEnd footer that landed between them.
 */
export function collapseAgentErrorDupes(list: AgentMsg[]): AgentMsg[] {
  const out: AgentMsg[] = []
  for (const m of list) {
    const asErr =
      !!m.error ||
      (!!m.body &&
        !m.me &&
        !m.tool &&
        !m.thinking &&
        !m.turnEnd &&
        !m.stopped &&
        isAgentErrorReply(m.body))
    if (!asErr) {
      out.push(m)
      continue
    }
    // Keep ts: the transcript {#each} keys text rows by ts+body, and two
    // identical provider errors in one chat (rate limit, retried) collide at 0.
    const err: AgentMsg = { body: m.body, error: true, ts: m.ts || 0 }
    const te = out.length && out[out.length - 1]?.turnEnd ? out.pop() : undefined
    const last = out[out.length - 1]
    if (last?.error && sameErrorDetail(last.body || '', err.body || '')) {
      if (te) out.push(te)
      continue
    }
    if (
      last &&
      !last.me &&
      !last.tool &&
      !last.thinking &&
      !last.turnEnd &&
      !last.error &&
      last.body &&
      sameErrorDetail(last.body, err.body || '')
    ) {
      out.pop()
    }
    out.push(err)
    if (te) out.push(te)
  }
  return out
}

/** True when the fallback must stay visible even if some assistant text streamed. */
export function shouldKeepFallbackReply(text: string): boolean {
  return isAgentErrorReply(text) || isChatStoppedReply(text)
}

/** Claude Code Task / Cursor task / occasional Agent alias. */
export function isTaskTool(tool: string | undefined): boolean {
  return isSubagentTool(tool) || tool === 'task'
}

/**
 * Claude Code's background-Task ack. Delegates to isSubagentLaunchAck, which
 * prefix-matches the same phrase as engine isSubagentLaunchMeta.
 */
export function isSubagentLaunchMeta(text: string | undefined): boolean {
  return isSubagentLaunchAck(text ?? '')
}

// finishTurn stops all streaming rows; if the turn produced no assistant text, it
// appends the returned reply (or an error/"Done.") so a turn never ends silent.
// Provider error notes (and user-stop) are appended even when some text already
// streamed — otherwise a partial reply hides the real ending until resync.
// durationMs (optional) stamps the same "completed in …" footer finished turns
// get — used for user-stop, where the provider often never emits turn_end.
export function finishTurn(
  list: AgentMsg[],
  fallback: string,
  turnGotText: boolean,
  durationMs = 0,
): AgentMsg[] {
  // A finished turn leaves nothing in flight: any row still marked pending never
  // got a result, so stop showing it as running.
  const closed = list.map(settleInFlight)
  const fb = (fallback || '').trim()
  if (!fb) return closed
  if (turnGotText && !shouldKeepFallbackReply(fb)) return closed
  const last = closed[closed.length - 1]
  if (last && !last.me && !last.tool && !last.thinking && !last.turnEnd && last.body === fb) {
    if (isChatStoppedReply(fb)) return endTurn(closed, durationMs)
    if (isAgentErrorReply(fb)) {
      const marked = last.error ? closed : [...closed.slice(0, -1), { ...last, error: true }]
      return endTurn(collapseAgentErrorDupes(marked), durationMs)
    }
    return closed
  }
  if (isChatStoppedReply(fb)) {
    return endTurn([...closed, { body: CHAT_STOPPED_REPLY, stopped: true }], durationMs)
  }
  if (isAgentErrorReply(fb)) {
    return endTurn(collapseAgentErrorDupes([...closed, { body: fb, error: true, ts: Date.now() }]), durationMs)
  }
  return [...closed, { body: fb }]
}
