import { get } from 'svelte/store'
import { WorkspacesService, call, pruneWorkspaceClientState, setWorkspacesFromList, upsertWorkspace } from './api'
import { flushEditorSessionNow } from './editor-session'
import { go, pushToast, selectedWorkspaceId, view, workspaces } from './stores'
import type { Workspace } from '../../bindings/recur/internal/domain/models'

export type RemoveWorkspaceOpts = {
  /** Skip the success toast (bulk remove sends one summary). */
  silent?: boolean
}

function markRemoved(w: Workspace): Workspace {
  return { ...w, archived: true, worktreePath: '', port: 0 }
}

function hideWorkspacesNow(ids: string[]): Map<string, Workspace> {
  const hide = new Set(ids.filter(Boolean))
  const originals = new Map<string, Workspace>()
  if (!hide.size) return originals
  const selected = get(selectedWorkspaceId)
  const viewing = get(view) === 'detail' && hide.has(selected)
  if (viewing) flushEditorSessionNow()
  workspaces.update((arr) => {
    for (const l of arr) {
      if (hide.has(l.id)) originals.set(l.id, l)
    }
    return arr.map((l) => (hide.has(l.id) ? markRemoved(l) : l))
  })
  if (viewing) go('briefing')
  return originals
}

async function finishRemove(id: string, original: Workspace | undefined, opts?: RemoveWorkspaceOpts, pending: string[] = []): Promise<boolean> {
  const removed = await call(WorkspacesService.RemoveWorkspace(id), 'Remove failed')
  if (removed) {
    workspaces.update((arr) => arr.map((l) => (l.id === removed.id ? removed : l)))
    pruneWorkspaceClientState(id)
    if (!opts?.silent) pushToast('success', 'Workspace removed')
    return true
  }
  const list = await WorkspacesService.ListWorkspaces().catch(() => null)
  if (list) {
    const hide = new Set(pending.filter(Boolean))
    setWorkspacesFromList(hide.size ? list.map((w) => (hide.has(w.id) ? markRemoved(w) : w)) : list)
  } else if (original) upsertWorkspace(original)
  return false
}

/** Archive a workspace (tear down its worktree, keep history). If the operator
 *  is looking at it, leave immediately so Observatory does not paint teardown. */
export async function removeWorkspace(id: string, opts?: RemoveWorkspaceOpts): Promise<boolean> {
  if (!id) return false
  const originals = hideWorkspacesNow([id])
  return finishRemove(id, originals.get(id), opts)
}

/** Archive several workspaces and toast once. */
export async function removeWorkspaces(ids: string[]): Promise<number> {
  const unique = [...new Set(ids.filter(Boolean))]
  if (unique.length === 1) return (await removeWorkspace(unique[0])) ? 1 : 0
  if (!unique.length) return 0
  const originals = hideWorkspacesNow(unique)
  const pending = new Set(unique)
  let n = 0
  for (const id of unique) {
    pending.delete(id)
    if (await finishRemove(id, originals.get(id), { silent: true }, [...pending])) n++
  }
  if (n === 1) pushToast('success', 'Workspace removed')
  else if (n > 1) pushToast('success', `${n} workspaces removed`)
  return n
}
