// Shared agent-chat transcript logic, used by both chat surfaces (the workspace
// coding agent and the loop Maestro) so they render the session store identically —
// text, reasoning, and tool calls, live and on reload. Pure list transforms: the
// component owns the per-workspace map + streaming flags and calls these.
import type { AgentMsg } from '../components/AgentChat.svelte'
import type { ChatItem } from './types'
type WorkspaceStep = { id: string; tool: string; toolInput: string; description: string; input: string; parentId?: string; uuid?: string }
import { isSubagentLaunchAck } from './subagent'

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

/** Plain text with attachment markers stripped (composer display helper). */
export function stripAttachmentMarkers(text: string): string {
  return text.replace(ATTACHMENT_MARKER_RE, '').replace(/\n{3,}/g, '\n\n')
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

/** Operator note shown on a turn-end digest finding. */
export type TurnDigestNote = {
  id?: string
  author?: string
  body: string
  createdAt?: string
}

/** Minimal finding fields the turn digest needs from the workspace store. */
export type DigestFindingRef = {
  id: string
  shortId?: string
  file?: string
  line?: number
  endLine?: number
  severity?: string
  message?: string
  summary?: string
  status?: string
  notes?: TurnDigestNote[]
}

/**
 * ask_user resume wire (engine formatAskUserResumePrompt). Prefer persisted
 * display_text; this fallback shortens full wire when display_text is empty.
 */
const ASK_USER_RESUME_RE =
  /^The user answered your earlier ask_user question\.\s*\n[\s\S]*?\nAnswer:\s*(.+?)(?:\nDetails:\s*(.+?))?(?:\n\nContinue with this answer[\s\S]*)?$/

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

function isSameSendReplay(prev: AgentMsg, next: AgentMsg): boolean {
  if (!sameUserChatBody(prev.body, next.body)) return false
  if (!prev.optimistic || next.optimistic) return false
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
const CODEX_LOGIN_EXPIRED = 'Codex login expired: sign in with ChatGPT in Settings → Providers → Codex, then retry.'

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
