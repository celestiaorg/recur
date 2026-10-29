import { get } from 'svelte/store'
import { inbox } from './stores'
import { InboxService } from './api'

/**
 * Opening a workspace's Briefing card marks its notifications read (gmail
 * semantics: opening the thread clears unread). Routed through the service so
 * chat rows keep their mark-seen/remove behavior; the local store is updated
 * optimistically and reconciled on the next inbox refresh.
 */
export function markWorkspaceInboxRead(workspaceId: string) {
  if (!workspaceId) return
  const items = get(inbox).filter((i) => i.workspaceId === workspaceId && i.unread)
  if (!items.length) return
  for (const it of items) InboxService.MarkInboxRead(it.id)
  inbox.update((arr) => arr.map((i) => (i.workspaceId === workspaceId ? { ...i, unread: false } : i)))
}

/** One notification row/line: mark read (same service path as above). */
export function markInboxItemRead(id: string) {
  InboxService.MarkInboxRead(id)
  inbox.update((arr) => arr.map((i) => (i.id === id ? { ...i, unread: false } : i)))
}

export function markAllInboxRead() {
  InboxService.MarkAllInboxRead()
  inbox.update((arr) => arr.map((i) => ({ ...i, unread: false })))
}
