/** Persist the active Agent-panel chat tab per workspace across reloads. */

const KEY = 'recur.activeChatTab.v1'

function loadAll(): Record<string, string> {
  try {
    const raw = localStorage.getItem(KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== 'object') return {}
    const out: Record<string, string> = {}
    for (const [id, code] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof code === 'string' && code) out[id] = code
    }
    return out
  } catch {
    return {}
  }
}

export function loadActiveChatCode(workspaceId: string): string | null {
  if (!workspaceId) return null
  return loadAll()[workspaceId] ?? null
}

function writeAll(all: Record<string, string>): void {
  if (Object.keys(all).length) localStorage.setItem(KEY, JSON.stringify(all))
  else localStorage.removeItem(KEY)
}

export function saveActiveChatCode(workspaceId: string, code: string): void {
  if (!workspaceId || !code) return
  try {
    const all = loadAll()
    if (all[workspaceId] === code) return
    all[workspaceId] = code
    writeAll(all)
  } catch {
    // quota / private mode — ignore
  }
}

export function clearActiveChatCode(workspaceId: string): void {
  if (!workspaceId) return
  try {
    const all = loadAll()
    if (!(workspaceId in all)) return
    delete all[workspaceId]
    writeAll(all)
  } catch {
    // quota / private mode — ignore
  }
}

/** Drop entries whose workspace is no longer in the live list. */
export function pruneActiveChatCodes(liveWorkspaceIds: Iterable<string>): void {
  try {
    const all = loadAll()
    const live = new Set(liveWorkspaceIds)
    let changed = false
    for (const id of Object.keys(all)) {
      if (live.has(id)) continue
      delete all[id]
      changed = true
    }
    if (!changed) return
    writeAll(all)
  } catch {
    // quota / private mode — ignore
  }
}

/** Prefer the saved code when it still exists in the strip; else the first tab. */
export function resolveActiveChatCode(
  tabs: { code: string }[],
  saved: string | null | undefined,
): string {
  if (saved && tabs.some((t) => t.code === saved)) return saved
  return tabs[0]?.code || 'PA'
}

export function restoredChatCode(workspaceId: string, tabs: { code: string }[]): string {
  return resolveActiveChatCode(tabs, loadActiveChatCode(workspaceId))
}
