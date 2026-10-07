import { get, writable } from 'svelte/store'
import { PRsService, call, upsertWorkspace } from './api'
import { requestPromptAction } from './promptActions'
import { prEditableWorkspaceId } from './pr-workspace'
import { prAgentDockOpen, prDetailTab, prs, workspaces } from './stores'

type PendingFixChecks = { prId: string; args: string }

/** Queue /fix-checks until CreateWorkspaceFromPR finishes allocating. */
export const pendingFixChecks = writable<PendingFixChecks | null>(null)

/** Fire a queued /fix-checks once the PR has an Agent-capable worktree. */
export function flushPendingFixChecks(): void {
  const pending = get(pendingFixChecks)
  if (!pending) return
  const pr = get(prs).find((p) => p.id === pending.prId)
  const wsId = prEditableWorkspaceId(pr, get(workspaces))
  if (!wsId) return
  pendingFixChecks.set(null)
  requestPromptAction(wsId, 'fix-checks', pending.args)
}

/**
 * Open the PR agent dock and invoke /fix-checks. Creates a workspace from
 * the PR first when the current dock is only a chat host.
 */
export async function requestFixChecks(prId: string, args = ''): Promise<void> {
  const id = prId.trim()
  if (!id) return
  prDetailTab.set('checks')
  prAgentDockOpen.set(true)
  const pr = get(prs).find((p) => p.id === id)
  let wsId = prEditableWorkspaceId(pr, get(workspaces))
  if (wsId) {
    requestPromptAction(wsId, 'fix-checks', args)
    return
  }
  pendingFixChecks.set({ prId: id, args })
  const ws = await call(PRsService.CreateWorkspaceFromPR(id), 'Could not create the workspace')
  if (!ws) {
    pendingFixChecks.set(null)
    return
  }
  upsertWorkspace(ws)
  workspaces.update((arr) =>
    arr.map((w) => (w.id === ws.id ? { ...w, ...ws, hostPrId: ws.hostPrId ?? '' } : w)),
  )
  const ready = !!(ws.worktreePath ?? '').trim() && !(ws.hostPrId ?? '').trim()
  if (ready) {
    prs.update((arr) => arr.map((p) => (p.id === id ? { ...p, workspaceId: ws.id, projectId: p.projectId || ws.projectId } : p)))
  }
  flushPendingFixChecks()
}
