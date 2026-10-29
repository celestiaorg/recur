/**
 * In-memory transcript chrome per chat key (workspace + agent code).
 *
 * AgentChat is one instance for the whole Observatory: switching tabs or
 * workspaces used to wipe expand/collapse and pin scroll to the bottom.
 * Open tabs keep their live $state; this cache covers the swap so a return
 * restores the same groups, rows, finding accordions, stub heights, and scroll.
 *
 * Scroll must be written while the scroller still shows that chat — `$effect`
 * cleanup runs after the incoming transcript has already replaced the DOM.
 */

import { ByteLru, CHROME_CACHE_MAX_BYTES, estimateBytes } from './byte-lru'

/** Distance from the tail (px) that counts as stuck-to-bottom for auto-follow. */
export const NEAR_BOTTOM_PX = 24

export function scrollerNearBottom(
  el: Pick<HTMLElement, 'scrollHeight' | 'scrollTop' | 'clientHeight'>,
  slack = NEAR_BOTTOM_PX,
): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight < slack
}

/**
 * Stick-to-bottom after a `scroll` event. Fast streaming grows the canvas
 * between `pinBottom` and the event, so distance can exceed `NEAR_BOTTOM_PX`
 * without the user moving — that must not unpin. An upward movement
 * (scrollTop dropped) leaves the tail even inside the slack: checking slack
 * first keeps the pin armed through a wheel tick, and the hydrator snaps back.
 */
export function nextNearBottom(
  wasNear: boolean,
  prevTop: number,
  el: Pick<HTMLElement, 'scrollHeight' | 'scrollTop' | 'clientHeight'>,
  slack = NEAR_BOTTOM_PX,
): boolean {
  if (el.scrollTop < prevTop - 1) return false
  if (scrollerNearBottom(el, slack)) return true
  return wasNear
}

export type ChatTranscriptUIState = {
  openGroups: Record<number, boolean>
  openRows: Record<string, boolean>
  findingAccordOpen: Record<string, boolean>
  scrollTop: number
  nearBottom: boolean
  /** Transcript row count at capture — expansion maps are index-based. */
  rowCount: number
  /** Cheap row-kind signature; prefix-stable when the transcript only appends. */
  fingerprint: string
  /** Measured `.cblk` heights so a remount can stub at the same offsets. */
  blockHeights: Record<string, number>
}

export type TranscriptFingerprintRow = {
  tool?: string
  thinking?: boolean
  compact?: boolean
  note?: boolean
  fallback?: boolean
  me?: boolean
}

export const TRANSCRIPT_UI_MAX_BYTES = CHROME_CACHE_MAX_BYTES

const cache = new ByteLru<ChatTranscriptUIState>({
  maxBytes: TRANSCRIPT_UI_MAX_BYTES,
  sizeOf: estimateBytes,
})
/** Keys dropped by CloseWorkspaceChat — ignore late `$effect` cleanup saves. */
const closed = new ByteLru<true>({
  maxBytes: TRANSCRIPT_UI_MAX_BYTES,
  sizeOf: () => 64,
})

export function emptyTranscriptUI(): ChatTranscriptUIState {
  return {
    openGroups: {},
    openRows: {},
    findingAccordOpen: {},
    scrollTop: 0,
    nearBottom: true,
    rowCount: 0,
    fingerprint: '',
    blockHeights: {},
  }
}

function clone(state: ChatTranscriptUIState): ChatTranscriptUIState {
  return {
    openGroups: { ...state.openGroups },
    openRows: { ...state.openRows },
    findingAccordOpen: { ...state.findingAccordOpen },
    scrollTop: state.scrollTop,
    nearBottom: state.nearBottom,
    rowCount: state.rowCount,
    fingerprint: state.fingerprint,
    blockHeights: { ...(state.blockHeights ?? {}) },
  }
}

/** Copy saved stub heights into a fresh per-chat map (remount / first visit). */
export function restoreBlockHeights(
  into: Map<string, number>,
  saved?: Record<string, number>,
): void {
  if (!saved) return
  for (const [k, h] of Object.entries(saved)) {
    if (h > 0) into.set(k, h)
  }
}

/** Row-kind signature. `limit` fingerprints only a prefix (the expansion guard). */
export function transcriptFingerprint(messages: TranscriptFingerprintRow[], limit?: number): string {
  const end = limit == null ? messages.length : Math.min(Math.max(limit, 0), messages.length)
  const rows = end === messages.length ? messages : messages.slice(0, end)
  return rows.map((m) => (
    m.tool ? `t:${m.tool}` : m.thinking ? 'th' : m.compact ? 'c' : m.note ? 'n' : m.fallback ? 'fb' : m.me ? 'u' : 'a'
  )).join('|')
}

/**
 * Index-based expand maps are only safe for the transcript they were captured
 * against (or an append-only continuation). A shorter or rewritten list means
 * the indices point at different rows.
 */
export function expansionStillValid(
  saved: Pick<ChatTranscriptUIState, 'rowCount' | 'fingerprint'>,
  fingerprint: string,
  rowCount: number,
): boolean {
  if (rowCount === 0) return true
  if (!saved.fingerprint && !saved.rowCount) return true
  if (rowCount < saved.rowCount) return false
  if (rowCount === saved.rowCount) return fingerprint === saved.fingerprint
  return fingerprint === saved.fingerprint || fingerprint.startsWith(saved.fingerprint + '|')
}

/** Whether a pending scroll restore should apply, wait for layout, or be dropped. */
export type ScrollRestoreGate = 'wait' | 'apply' | 'abandon'

/** True once the scroller can honor a pending restore (or we know it never will). */
export function scrollRestoreReady(
  saved: Pick<ChatTranscriptUIState, 'scrollTop' | 'nearBottom' | 'rowCount'>,
  opts: { clientHeight: number; scrollHeight: number; rowCount: number },
): ScrollRestoreGate {
  if (opts.clientHeight <= 0) return 'wait'
  if (saved.nearBottom || saved.scrollTop <= 0) return 'apply'
  const maxScroll = Math.max(0, opts.scrollHeight - opts.clientHeight)
  if (maxScroll >= saved.scrollTop) return 'apply'
  // Content has laid out with overflow the saved offset cannot reach (shorter
  // transcript, taller dock). Clamp rather than wait for rows that will not come.
  if (maxScroll > 0) return 'apply'
  const expected = saved.rowCount ?? 0
  // Teardown-created entries often have rowCount 0; any hydrated list that still
  // does not overflow will never grow into the saved offset.
  if (opts.rowCount > 0 && (expected === 0 || opts.rowCount >= expected)) return 'abandon'
  return 'wait'
}

export function saveTranscriptUI(key: string, state: ChatTranscriptUIState): void {
  if (!key || closed.has(key)) return
  cache.set(key, clone(state))
}

/** Scroll-only write — reuses the cached maps/fingerprint (scrolling cannot change them). */
export function saveTranscriptUIScroll(key: string, scrollTop: number, nearBottom: boolean): void {
  if (!key || closed.has(key)) return
  const prev = cache.peek(key)
  if (!prev) return
  prev.scrollTop = scrollTop
  prev.nearBottom = nearBottom
  cache.set(key, prev)
}

export function loadTranscriptUI(key: string): ChatTranscriptUIState | undefined {
  if (!key) return undefined
  const saved = cache.peek(key)
  return saved ? clone(saved) : undefined
}

/** Re-open a previously dropped key (new tab reused the agent code). */
export function claimTranscriptUI(key: string): ChatTranscriptUIState | undefined {
  if (!key) return undefined
  closed.delete(key)
  return loadTranscriptUI(key)
}

export function dropTranscriptUI(key: string): void {
  if (!key) return
  cache.delete(key)
  closed.set(key, true)
}

/** Test helper — the cache is process-local. */
export function clearTranscriptUI(): void {
  cache.clear()
  closed.clear()
  cache.maxBytes = TRANSCRIPT_UI_MAX_BYTES
  closed.maxBytes = TRANSCRIPT_UI_MAX_BYTES
}

export function setTranscriptUIMaxBytesForTests(n: number): void {
  cache.maxBytes = n
  closed.maxBytes = n
}

/**
 * `{#each}` identity for a transcript row. Live rows used a global key
 * (`m:live-text`, `s:live`) so a new workspace that immediately streams
 * reused the previous chat's StreamingMarkdown / tool-row instance until
 * that turn ended. Prefix with the chat identity (workspace + agent code).
 */
export function transcriptBlockKey(resetKey: string, local: string): string {
  return `${resetKey || '_'}:${local}`
}

/** Local `{#each}` id for a specialist card. One row and a packed group share
 *  this so a sibling starting does not remount the first SessionCard. */
export function transcriptSessionBlockLocal(log: {
  kind: 'session' | 'sessions'
  id?: string
  code?: string
  sessions?: { id?: string }[]
}): string {
  const id = log.kind === 'sessions' ? log.id || log.sessions?.[0]?.id || '' : log.id || log.code || ''
  return `sess:${id}`
}

/** Local `{#each}` id for a tool/thinking group. A tool-led group keys off
 *  its provider `toolId` so a mid-list splice (Loop facts / specialist cards)
 *  does not remount it. `start` is only mixed in when there is no toolId —
 *  several thinking-led groups in one turn can share a timestamp. */
export function transcriptStepBlockLocal(
  start: number,
  first?: { toolId?: string; ts?: number; body?: string },
  liveLast = false,
): string {
  if (liveLast) return 's:live'
  if (first?.toolId) return `s:${first.toolId}`
  return `s:${start}:${first?.ts || first?.body?.slice(0, 24) || 'x'}`
}

/** Suffix repeats so `{#each}` keys stay unique when two rows share a
 *  timestamp and body prefix (e.g. untimestamped replies to several steers). */
export function uniqueBlockKeys(keys: string[]): string[] {
  const seen = new Map<string, number>()
  return keys.map((k) => {
    const n = seen.get(k) ?? 0
    seen.set(k, n + 1)
    return n ? `${k}#${n}` : k
  })
}

/** Local `{#each}` id for a user/assistant text row. Timestamp + body/toolId
 *  so a mid-list splice (Loop facts / specialist cards) does not remount every
 *  following row. Index is only the last fallback when those fields are empty. */
export function transcriptMessageBlockLocal(
  index: number,
  m: { me?: boolean; ts?: number; toolId?: string; body?: string },
): string {
  if (m.me) {
    const body = m.body?.slice(0, 48) ?? ''
    if (m.ts || body) return `me:${m.ts || 0}:${body}`
    return `me:${index}`
  }
  return `m:${m.ts || 0}:${m.toolId || m.body?.slice(0, 32) || index}`
}

/**
 * Capture chrome for a chat key.
 * `scrollerHidden` means the DOM already shows another chat (or is detached):
 * keep the previous expansion pair unless the caller still has a matched
 * leaving-tab count+fingerprint. Scroll/pin come from the caller — they are
 * tracked eagerly and must not fall back to a stale cache entry.
 */
export function snapshotTranscriptUI(
  live: {
    openGroups: Record<number, boolean>
    openRows: Record<string, boolean>
    findingAccordOpen: Record<string, boolean>
    scrollTop: number
    nearBottom: boolean
    scrollerHidden: boolean
    rowCount?: number
    fingerprint?: string
    blockHeights?: Record<string, number>
  },
  previous?: ChatTranscriptUIState,
): ChatTranscriptUIState {
  return {
    openGroups: { ...live.openGroups },
    openRows: { ...live.openRows },
    findingAccordOpen: { ...live.findingAccordOpen },
    scrollTop: live.scrollTop,
    nearBottom: live.nearBottom,
    ...expansionPair(live, previous),
    blockHeights: heightPair(live, previous),
  }
}

/** rowCount and fingerprint are one guard — never take one without the other. */
function expansionPair(
  live: { scrollerHidden: boolean; rowCount?: number; fingerprint?: string },
  previous?: ChatTranscriptUIState,
): Pick<ChatTranscriptUIState, 'rowCount' | 'fingerprint'> {
  const rowCount = live.rowCount ?? 0
  const fingerprint = live.fingerprint ?? ''
  if (!live.scrollerHidden) return { rowCount, fingerprint }
  if (rowCount > 0 && fingerprint) return { rowCount, fingerprint }
  return { rowCount: previous?.rowCount ?? 0, fingerprint: previous?.fingerprint ?? '' }
}

function heightPair(
  live: { scrollerHidden: boolean; blockHeights?: Record<string, number> },
  previous?: ChatTranscriptUIState,
): Record<string, number> {
  const h = live.blockHeights
  if (h && Object.keys(h).length) return { ...h }
  if (live.scrollerHidden) return { ...(previous?.blockHeights ?? {}) }
  return {}
}
