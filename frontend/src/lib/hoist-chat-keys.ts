/** Client key remap when a PR chat host is hoisted onto a live worktree. */

import { dropChatHeapKey, getChatHeap, takeChatHeapDrafts } from './caches/chat-heap-cache'

export function destChatCode(code: string, remap?: Record<string, string> | null): string {
  return remap?.[code] || code
}

/** Dest tab codes whose parked caches belong to a different session after hoist.
 *  `remap == null` is not a hoist (ordinary activation / focus) — keep local picker edits.
 *  Collision remaps omit identity-moved dests; `movedCodes` is the complete set. */
export function hoistCacheBustCodes(
  tabCodes: readonly string[],
  remap?: Record<string, string> | null,
  movedCodes?: readonly string[] | null,
): string[] {
  if (remap == null) return []
  const named = [...new Set([
    ...Object.values(remap).filter(Boolean),
    ...(movedCodes ?? []).filter(Boolean),
  ])]
  return named.length ? named : [...tabCodes]
}

export function chatTabKey(workspaceId: string, code: string): string {
  return `${workspaceId}\t${code}`
}

export function destChatKey(
  fromId: string,
  code: string,
  toId: string,
  remap?: Record<string, string> | null,
): string {
  return chatTabKey(toId, destChatCode(code, remap))
}

const aliases = new Map<string, string>()

export function noteHoistChatAlias(fromKey: string, toKey: string): void {
  if (!fromKey || !toKey || fromKey === toKey) return
  aliases.set(fromKey, toKey)
}

export function liveChatKey(key: string): string {
  return aliases.get(key) || key
}

export function liveChatTarget(id: string, code: string): { id: string; code: string; key: string } {
  const key = liveChatKey(chatTabKey(id, code))
  const i = key.indexOf('\t')
  if (i <= 0) return { id, code, key: chatTabKey(id, code) }
  return { id: key.slice(0, i), code: key.slice(i + 1), key }
}

export function concatHoistText(dest: string | undefined, src: string): string {
  const a = dest?.trim() ?? ''
  const b = src.trim()
  if (!b) return dest ?? ''
  if (!a) return src
  return `${dest}\n\n${src}`
}

export function concatHoistList<T>(dest: T[] | undefined, src: T[]): T[] {
  if (!src.length) return dest ?? []
  if (!dest?.length) return src
  return [...dest, ...src]
}

/** Move fromId-prefixed keys onto toId, applying code remap. Dest values are merged. */
export function remapPrefixedRecord<T>(
  rec: Record<string, T>,
  fromId: string,
  toId: string,
  remap: Record<string, string> | null | undefined,
  merge: (dest: T | undefined, src: T) => T,
): Record<string, T> {
  const prefix = fromId + '\t'
  let changed = false
  const out: Record<string, T> = { ...rec }
  for (const [k, v] of Object.entries(rec)) {
    if (!k.startsWith(prefix)) continue
    const destKey = destChatKey(fromId, k.slice(prefix.length), toId, remap)
    noteHoistChatAlias(k, destKey)
    delete out[k]
    out[destKey] = merge(out[destKey], v)
    changed = true
  }
  return changed ? out : rec
}

export type HoistDraft<A = unknown> = { input?: string; attachments?: A[] }

const drafts = new Map<string, HoistDraft>()

export function stashHoistDraft<A>(toKey: string, input?: string, attachments?: A[]): void {
  if (!toKey) return
  if (!(input || '').trim() && !attachments?.length) return
  const prev = drafts.get(toKey) as HoistDraft<A> | undefined
  drafts.set(toKey, {
    input: concatHoistText(prev?.input, input ?? ''),
    attachments: concatHoistList(prev?.attachments, attachments ?? []),
  })
}

export function takeHoistDrafts<A>(workspaceId: string): Record<string, HoistDraft<A>> {
  if (!workspaceId) return {}
  const prefix = workspaceId + '\t'
  const out: Record<string, HoistDraft<A>> = {}
  for (const [k, v] of [...drafts.entries()]) {
    if (!k.startsWith(prefix)) continue
    drafts.delete(k)
    out[k] = v as HoistDraft<A>
  }
  return out
}

const liveSources = new Set<string>()
const pendingDestBust = new Map<string, Set<string>>()

/** Dest heap keys to drop on remount — onDestroy can rewrite after a live hoist. */
export function noteHoistDestBust(toId: string, codes: readonly string[]): void {
  if (!toId) return
  let set = pendingDestBust.get(toId)
  if (!set) {
    set = new Set()
    pendingDestBust.set(toId, set)
  }
  for (const c of codes) if (c) set.add(c)
}

export function takeHoistDestBust(toId: string): string[] {
  const set = pendingDestBust.get(toId)
  if (!set) return []
  pendingDestBust.delete(toId)
  return [...set]
}

export function bustHoistedDestHeap(toId: string, codes: readonly string[]): void {
  for (const code of codes) {
    if (!code) continue
    const key = chatTabKey(toId, code)
    const heap = getChatHeap(toId)
    const destInput = heap?.input?.[key]
    const destAtt = heap?.attachments?.[key]
    dropChatHeapKey(toId, key)
    const incoming = drafts.get(key)
    drafts.delete(key)
    stashHoistDraft(key, destInput, destAtt)
    if (incoming) stashHoistDraft(key, incoming.input, incoming.attachments)
  }
}

/** Mounted WorkspaceView owns live drafts; heap is a stale snapshot. */
export function noteHoistLiveSource(workspaceId: string): () => void {
  if (!workspaceId) return () => {}
  liveSources.add(workspaceId)
  return () => { liveSources.delete(workspaceId) }
}

/** Evicted host: remap heap drafts onto dest stash. Mounted source already did this. */
export function migrateEvictedHoistDrafts(
  fromId: string,
  toId: string,
  remap?: Record<string, string> | null,
): void {
  if (!fromId || !toId || fromId === toId) return
  if (liveSources.has(fromId)) return
  const { input, attachments } = takeChatHeapDrafts(fromId)
  const prefix = fromId + '\t'
  const seen = new Set<string>()
  for (const [k, v] of Object.entries(input)) {
    if (!k.startsWith(prefix)) continue
    seen.add(k)
    const destKey = destChatKey(fromId, k.slice(prefix.length), toId, remap)
    noteHoistChatAlias(k, destKey)
    stashHoistDraft(destKey, v, attachments[k])
  }
  for (const [k, v] of Object.entries(attachments)) {
    if (!k.startsWith(prefix) || seen.has(k)) continue
    const destKey = destChatKey(fromId, k.slice(prefix.length), toId, remap)
    noteHoistChatAlias(k, destKey)
    stashHoistDraft(destKey, '', v)
  }
}

export type HoistPending<T = unknown> = { items: T[]; hold?: boolean }

const queues = new Map<string, HoistPending>()

export function stashHoistPending<T>(toKey: string, items?: T[], hold?: boolean): void {
  if (!toKey) return
  if (!items?.length && !hold) return
  const prev = queues.get(toKey) as HoistPending<T> | undefined
  queues.set(toKey, {
    items: concatHoistList(prev?.items, items ?? []),
    hold: !!(prev?.hold || hold),
  })
}

export function takeHoistPending<T>(workspaceId: string): Record<string, HoistPending<T>> {
  if (!workspaceId) return {}
  const prefix = workspaceId + '\t'
  const out: Record<string, HoistPending<T>> = {}
  for (const [k, v] of [...queues.entries()]) {
    if (!k.startsWith(prefix)) continue
    queues.delete(k)
    out[k] = v as HoistPending<T>
  }
  return out
}

const sending = new Map<string, number>()
type HoistSendCompleteFn = (key: string, gen: number) => void
const sendComplete = new Set<HoistSendCompleteFn>()

export function stashHoistSending(toKey: string, gen = 1): void {
  if (!toKey || gen < 1) return
  sending.set(toKey, Math.max(sending.get(toKey) ?? 0, gen))
}

export function takeHoistSending(workspaceId: string): Record<string, number> {
  if (!workspaceId) return {}
  const prefix = workspaceId + '\t'
  const out: Record<string, number> = {}
  for (const [k, gen] of [...sending.entries()]) {
    if (!k.startsWith(prefix)) continue
    sending.delete(k)
    out[k] = gen
  }
  return out
}

export function onHoistSendComplete(fn: HoistSendCompleteFn): () => void {
  sendComplete.add(fn)
  return () => { sendComplete.delete(fn) }
}

export function clearHoistSending(key: string, gen?: number): void {
  if (!key) return
  const cur = sending.get(key)
  if (gen && cur !== undefined && cur !== gen) return
  sending.delete(key)
  if (gen) for (const fn of sendComplete) fn(key, gen)
}

/** Merge transferred in-flight sends onto dest. Records the source gen as the
 * reservation identity unless dest is already sending (newer local send). */
export function absorbHoistSending(
  sendingByKey: Record<string, boolean>,
  sendGenByKey: Record<string, number>,
  hoistSendGenByKey: Record<string, number>,
  inflight: Record<string, number>,
): {
  sendingByKey: Record<string, boolean>
  sendGenByKey: Record<string, number>
  hoistSendGenByKey: Record<string, number>
} {
  for (const [k, gen] of Object.entries(inflight)) {
    const destBusy = !!sendingByKey[k]
    sendingByKey = { ...sendingByKey, [k]: true }
    sendGenByKey = { ...sendGenByKey, [k]: Math.max(sendGenByKey[k] ?? 0, gen) }
    if (!destBusy) hoistSendGenByKey = { ...hoistSendGenByKey, [k]: gen }
  }
  return { sendingByKey, sendGenByKey, hoistSendGenByKey }
}

/** Source RPC completion matches the transferred reservation, not dest's leftover counter. */
export function hoistSendCompleteApplies(
  completeGen: number,
  _destGen: number,
  reservedGen?: number,
): boolean {
  if (!completeGen) return true
  return reservedGen === completeGen
}

export function resetHoistChatKeysForTests(): void {
  aliases.clear()
  drafts.clear()
  queues.clear()
  sending.clear()
  sendComplete.clear()
  liveSources.clear()
  pendingDestBust.clear()
}
