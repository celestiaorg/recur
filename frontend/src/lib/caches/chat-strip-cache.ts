/**
 * Agent-panel tab strip across workspace remount.
 *
 * Before hot shells, one WorkspaceView kept `tabsByWs` for every id it had
 * visited, so A→B→A painted the strip immediately. Each hot shell has its
 * own maps; LRU evict drops them and ListWorkspaceChats fills in a frame
 * later — the header goes empty. This is the warm tier for that chrome.
 */

import type { WorkspaceChatTab } from '../../../bindings/recur/internal/domain/models'
import { ByteLru, CHROME_CACHE_MAX_BYTES, estimateBytes } from './byte-lru'

export type ChatStripSnapshot = {
  tabs: WorkspaceChatTab[]
  closed?: WorkspaceChatTab[]
  activeCode?: string
  /**
   * Agent-tab strip scrollLeft when the strip had live layout. Omitted when
   * the node is missing or parked (a fabricated 0 would pin restore and
   * suppress the reveal chase). Restored as-is on remount so returning to a
   * workspace does not chase the active chip.
   */
  tabScrollLeft?: number
}

export const CHAT_STRIP_MAX_BYTES = CHROME_CACHE_MAX_BYTES

const cache = new ByteLru<ChatStripSnapshot>({
  maxBytes: CHAT_STRIP_MAX_BYTES,
  sizeOf: estimateBytes,
})

export function getChatStrip(workspaceId: string): ChatStripSnapshot | null {
  if (!workspaceId) return null
  return cache.get(workspaceId) ?? null
}

export function putChatStrip(workspaceId: string, snap: ChatStripSnapshot): void {
  if (!workspaceId || !snap.tabs.length) return
  const prev = cache.peek(workspaceId)
  cache.set(workspaceId, {
    tabs: snap.tabs,
    closed: snap.closed ?? prev?.closed,
    activeCode: snap.activeCode || prev?.activeCode,
    tabScrollLeft: snap.tabScrollLeft ?? prev?.tabScrollLeft,
  })
}

export function pruneChatStrips(liveWorkspaceIds: Iterable<string>): void {
  const live = new Set(liveWorkspaceIds)
  cache.prune((id) => live.has(id))
}

export function resetChatStripsForTests(): void {
  cache.clear()
  cache.maxBytes = CHAT_STRIP_MAX_BYTES
}

export function setChatStripMaxBytesForTests(n: number): void {
  cache.maxBytes = n
}
