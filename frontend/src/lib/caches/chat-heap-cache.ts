/**
 * Agent transcripts across workspace remount.
 *
 * The editor paints from observatory-cache + parked EditorState on the first
 * frame. Chat used to start empty, wait on WorkspaceChatItems, then mount
 * every bubble. Byte-capped like the Observatory snapshot, not a 24-root cap.
 *
 * Composer drafts (`input` / `attachments`), queued unsent turns (`pending`),
 * dock pickers (mode / model / flags), Simplify, and the context-usage ring ride the
 * same snapshot so a warm remount paints the dock immediately.
 */

import { ByteLru, WARM_CACHE_MAX_BYTES, estimateBytes } from './byte-lru'
import { contextUsageReady, type ContextUsage } from '../contextUsage'
import type { ChatMode } from '../modes'

export type ChatPickerSnap = {
  model: string
  flags: Record<string, string>
  mode: ChatMode
}

export type ChatHeapSnapshot<M = unknown, A = unknown> = {
  chat: Record<string, M[]>
  usage?: Record<string, ContextUsage>
  input?: Record<string, string>
  attachments?: Record<string, A[]>
  /** Wall-clock start of an in-flight turn — elapsed must not reset on remount. */
  turnStartedAt?: Record<string, number>
  streamSeq?: Record<string, number>
  turnGotText?: Record<string, boolean>
  /** Per-tab dock selection (`workspaceId\\tcode`). */
  pickers?: Record<string, ChatPickerSnap>
  /** Queued unsent turns (`workspaceId\\tcode`). */
  pending?: Record<string, unknown[]>
  holdPending?: Record<string, boolean>
  /** Per-tab Simplify selection (`workspaceId\\tcode`). */
  simplify?: Record<string, boolean>
}

export const CHAT_HEAP_MAX_BYTES = WARM_CACHE_MAX_BYTES

const cache = new ByteLru<ChatHeapSnapshot>({
  maxBytes: CHAT_HEAP_MAX_BYTES,
  sizeOf: estimateBytes,
})

function prefix(workspaceId: string): string {
  return `${workspaceId}\t`
}

function scoped<T>(workspaceId: string, rec: Record<string, T> | undefined): Record<string, T> {
  const p = prefix(workspaceId)
  const out: Record<string, T> = {}
  if (!rec) return out
  for (const [k, v] of Object.entries(rec)) {
    if (k.startsWith(p)) out[k] = v
  }
  return out
}

function scopedUsage(
  workspaceId: string,
  rec: Record<string, ContextUsage> | undefined,
): Record<string, ContextUsage> {
  const p = prefix(workspaceId)
  const out: Record<string, ContextUsage> = {}
  if (!rec) return out
  for (const [k, v] of Object.entries(rec)) {
    if (!k.startsWith(p) || !v || !contextUsageReady(v)) continue
    out[k] = {
      inputTokens: v.inputTokens || 0,
      cacheCreationTokens: v.cacheCreationTokens || 0,
      cacheReadTokens: v.cacheReadTokens || 0,
      contextWindow: v.contextWindow || 0,
      compacting: !!v.compacting,
      compacted: !!v.compacted,
    }
  }
  return out
}

function scopedPickers(
  workspaceId: string,
  rec: Record<string, ChatPickerSnap> | undefined,
): Record<string, ChatPickerSnap> {
  const p = prefix(workspaceId)
  const out: Record<string, ChatPickerSnap> = {}
  if (!rec) return out
  for (const [k, v] of Object.entries(rec)) {
    if (!k.startsWith(p) || !v?.model) continue
    out[k] = { model: v.model, flags: { ...v.flags }, mode: v.mode }
  }
  return out
}

function scopedSimplify(
  workspaceId: string,
  rec: Record<string, boolean> | undefined,
): Record<string, boolean> {
  const p = prefix(workspaceId)
  const out: Record<string, boolean> = {}
  if (!rec) return out
  for (const [k, v] of Object.entries(rec)) {
    if (k.startsWith(p) && typeof v === 'boolean') out[k] = v
  }
  return out
}

export function getChatHeap<M = unknown, A = unknown>(workspaceId: string): ChatHeapSnapshot<M, A> | null {
  if (!workspaceId) return null
  const hit = cache.get(workspaceId)
  return hit ? hit as ChatHeapSnapshot<M, A> : null
}

export function putChatHeap<M, A = unknown>(workspaceId: string, snap: ChatHeapSnapshot<M, A>): void {
  if (!workspaceId) return
  const chat = scoped(workspaceId, snap.chat)
  const pickers = scopedPickers(workspaceId, snap.pickers)
  const pending = scoped(workspaceId, snap.pending)
  const holdPending = scoped(workspaceId, snap.holdPending)
  const simplify = scopedSimplify(workspaceId, snap.simplify)
  if (!Object.keys(chat).length && !Object.keys(pickers).length) return
  // Picker-only put (mode change, no messages yet) must not wipe a transcript.
  if (!Object.keys(chat).length) {
    const prev = cache.peek(workspaceId)
    cache.set(workspaceId, {
      chat: prev?.chat ?? {},
      usage: prev?.usage,
      input: prev?.input,
      attachments: prev?.attachments,
      turnStartedAt: prev?.turnStartedAt,
      streamSeq: prev?.streamSeq,
      turnGotText: prev?.turnGotText,
      pickers,
      pending: prev?.pending,
      holdPending: prev?.holdPending,
      simplify: Object.keys(simplify).length ? simplify : prev?.simplify,
    })
    return
  }
  cache.set(workspaceId, {
    chat,
    usage: scopedUsage(workspaceId, snap.usage),
    input: scoped(workspaceId, snap.input),
    attachments: scoped(workspaceId, snap.attachments),
    turnStartedAt: scoped(workspaceId, snap.turnStartedAt),
    streamSeq: scoped(workspaceId, snap.streamSeq),
    turnGotText: scoped(workspaceId, snap.turnGotText),
    pickers,
    pending,
    holdPending,
    simplify,
  })
}

function omitHeapKey<T>(rec: Record<string, T> | undefined, key: string): Record<string, T> | undefined {
  if (!rec || !(key in rec)) return rec
  const { [key]: _, ...rest } = rec
  return rest
}

/** Take composer drafts off a workspace heap so a hoist can remap them. */
export function takeChatHeapDrafts<A = unknown>(workspaceId: string): {
  input: Record<string, string>
  attachments: Record<string, A[]>
} {
  const empty = { input: {} as Record<string, string>, attachments: {} as Record<string, A[]> }
  if (!workspaceId) return empty
  const prev = cache.peek(workspaceId)
  if (!prev) return empty
  const input = prev.input ?? {}
  const attachments = (prev.attachments ?? {}) as Record<string, A[]>
  if (!Object.keys(input).length && !Object.keys(attachments).length) return empty
  cache.set(workspaceId, { ...prev, input: {}, attachments: {} })
  return { input, attachments }
}

/** Drop one chat key from a workspace heap so a later remount cannot seed it. */
export function dropChatHeapKey(workspaceId: string, key: string): void {
  if (!workspaceId || !key) return
  const prev = cache.peek(workspaceId)
  if (!prev) return
  cache.set(workspaceId, {
    chat: omitHeapKey(prev.chat, key) ?? {},
    usage: omitHeapKey(prev.usage, key),
    input: omitHeapKey(prev.input, key),
    attachments: omitHeapKey(prev.attachments, key),
    turnStartedAt: omitHeapKey(prev.turnStartedAt, key),
    streamSeq: omitHeapKey(prev.streamSeq, key),
    turnGotText: omitHeapKey(prev.turnGotText, key),
    pickers: omitHeapKey(prev.pickers, key),
    pending: omitHeapKey(prev.pending, key),
    holdPending: omitHeapKey(prev.holdPending, key),
    simplify: omitHeapKey(prev.simplify, key),
  })
}

export function pruneChatHeaps(liveWorkspaceIds: Iterable<string>): void {
  const live = new Set(liveWorkspaceIds)
  cache.prune((id) => live.has(id))
}

export function resetChatHeapsForTests(): void {
  cache.clear()
  cache.maxBytes = CHAT_HEAP_MAX_BYTES
}

export function setChatHeapMaxBytesForTests(n: number): void {
  cache.maxBytes = n
}
