type PRRef = { id?: string; workspaceId?: string } | null | undefined
type HostWorkspace = { id: string; hostPrId?: string; archived?: boolean }

/** Workspace the PR-detail agent dock should bind to. */
export function prAgentWorkspaceId(
  pr: PRRef,
  workspaces: HostWorkspace[],
): string {
  const prId = (pr?.id ?? '').trim()
  // A leftover chat host stays bound until hoist archives it, so an in-flight
  // PR-detail turn is not abandoned when a worktree gets linked.
  if (prId) {
    const host = workspaces.find((w) => !w.archived && (w.hostPrId ?? '').trim() === prId)
    if (host) return host.id
  }
  const linked = (pr?.workspaceId ?? '').trim()
  if (linked) {
    const ws = workspaces.find((w) => w.id === linked)
    if (ws && !ws.archived) return linked
  }
  return ''
}

/** Linked Recur worktree that can run Agent (live, not a PR chat host). */
export function prEditableWorkspaceId(
  pr: PRRef,
  workspaces: HostWorkspace[],
): string {
  const id = (pr?.workspaceId ?? '').trim()
  if (!id) return ''
  const ws = workspaces.find((w) => w.id === id)
  if (!ws || ws.archived || (ws.hostPrId ?? '').trim()) return ''
  return id
}

function isArchivedChatHost(ws: HostWorkspace | undefined): boolean {
  return !!ws?.archived && !!(ws.hostPrId ?? '').trim()
}

/** True when this PR's dock workspace (linked worktree or hidden chat host) has a live agent. */
export function prAgentIsRunning(
  pr: PRRef,
  workspaces: HostWorkspace[],
  running: ReadonlySet<string>,
): boolean {
  const id = prAgentWorkspaceId(pr, workspaces)
  if (!id || !running.has(id)) return false
  const ws = workspaces.find((w) => w.id === id)
  return !isArchivedChatHost(ws)
}

/** How many hidden PR chat hosts currently have a live agent. */
export function prChatHostRunningCount(
  workspaces: HostWorkspace[],
  running: ReadonlySet<string>,
): number {
  const seen = new Set<string>()
  let n = 0
  for (const w of workspaces) {
    if (w.archived) continue
    if (!(w.hostPrId ?? '').trim()) continue
    if (!running.has(w.id) || seen.has(w.id)) continue
    seen.add(w.id)
    n++
  }
  return n
}

/** Unseen finished chat turns across hidden PR chat hosts (no sidebar row of their own). */
export function prChatHostUnseenCount(
  workspaces: HostWorkspace[],
  byWorkspace: Record<string, number>,
): number {
  const seen = new Set<string>()
  let n = 0
  for (const w of workspaces) {
    if (w.archived) continue
    if (!(w.hostPrId ?? '').trim()) continue
    if (seen.has(w.id)) continue
    seen.add(w.id)
    n += byWorkspace[w.id] ?? 0
  }
  return n
}

/** Inbox `chat:` targets on a hidden PR host should open the PR, not the host workspace. */
export function prIdForChatWorkspace(
  workspaceId: string,
  workspaces: HostWorkspace[],
): string {
  const id = workspaceId.trim()
  if (!id) return ''
  return (workspaces.find((w) => w.id === id)?.hostPrId ?? '').trim()
}

/** Unseen finished chat turns on this PR's dock workspace (linked or hidden host). */
export function prUnseenCount(
  pr: PRRef,
  workspaces: HostWorkspace[],
  byWorkspace: Record<string, number>,
): number {
  const id = prAgentWorkspaceId(pr, workspaces)
  if (!id) return 0
  const ws = workspaces.find((w) => w.id === id)
  if (isArchivedChatHost(ws)) return 0
  return byWorkspace[id] ?? 0
}
