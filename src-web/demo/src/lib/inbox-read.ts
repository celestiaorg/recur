import { inbox } from './stores'

/** Opening a workspace's Briefing card marks its notifications read. */
export function markWorkspaceInboxRead(workspaceId: string) {
  inbox.update((arr) => arr.map((i) => (i.workspaceId === workspaceId ? { ...i, unread: false } : i)))
}
