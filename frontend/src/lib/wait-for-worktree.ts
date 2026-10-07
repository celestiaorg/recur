import { get } from 'svelte/store'
import type { Workspace } from '../../bindings/recur/internal/domain/models'
import { wsCreating } from './modes'
import { workspaces } from './stores'

/** Wait until `id` has finished create/setup (not just a published path),
 *  fails, is aborted, or is gone from the store. */
export function waitForWorktree(id: string, signal: AbortSignal): Promise<Workspace | null> {
  const current = (): Workspace | null | undefined => {
    if (signal.aborted) return null
    const w = get(workspaces).find((x) => x.id === id)
    if (!w) return null
    if (w.lastError || w.phase === 'failed') return null
    if (wsCreating(w)) return undefined
    if ((w.worktreePath ?? '').trim()) return w
    return undefined
  }
  const now = current()
  if (now !== undefined) return Promise.resolve(now)
  return new Promise((resolve) => {
    let settled = false
    const finish = (v: Workspace | null) => {
      if (settled) return
      settled = true
      unsub()
      signal.removeEventListener('abort', onAbort)
      resolve(v)
    }
    const onAbort = () => finish(null)
    signal.addEventListener('abort', onAbort)
    const unsub = workspaces.subscribe(() => {
      const next = current()
      if (next !== undefined) finish(next)
    })
    const again = current()
    if (again !== undefined) finish(again)
  })
}
