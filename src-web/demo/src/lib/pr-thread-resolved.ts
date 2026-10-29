import { get } from 'svelte/store'
import type { PRComment } from '../../bindings/recur/internal/domain/models'
import { PRsService, call } from './api'
import { applyReactionHolds, commentReactionHold } from './comment-reactions'
import { applyThreadResolved, applyThreadResolvedHolds, keepPendingReplies, lineThreadKey } from './comment-thread'
import { prs, pushToast, selectedPRId } from './stores'

/** In-flight resolve toggles so a stale GetPR cannot flip the button back. */
export const threadResolvedHold: Record<string, boolean> = {}

export function resetThreadResolvedHolds() {
  for (const k of Object.keys(threadResolvedHold)) delete threadResolvedHold[k]
}

function paintComments(prId: string, key: string, resolved: boolean) {
  prs.update((arr) => arr.map((p) => (p.id !== prId ? p : { ...p, comments: applyThreadResolved(p.comments ?? [], key, resolved) })))
}

/** Optimistic resolve/unresolve. Returns false when the write did not stick. */
export async function setPRThreadResolved(c: PRComment, resolved: boolean): Promise<boolean> {
  const key = lineThreadKey(c)
  if (!key || threadResolvedHold[key] === resolved) return false
  const prId = get(selectedPRId)
  if (!prId) return false
  threadResolvedHold[key] = resolved
  paintComments(prId, key, resolved)
  const updated = await call(PRsService.SetPRThreadResolved(prId, key, resolved), 'Could not update conversation')
  if (threadResolvedHold[key] !== resolved) return false
  if (!updated) {
    delete threadResolvedHold[key]
    paintComments(prId, key, !resolved)
    return false
  }
  prs.update((arr) => arr.map((p) => {
    if (p.id !== updated.id) return p
    return { ...updated, comments: applyReactionHolds(applyThreadResolvedHolds(keepPendingReplies(p.comments ?? [], updated.comments ?? []), threadResolvedHold), commentReactionHold) }
  }))
  pushToast('success', resolved ? 'Conversation resolved' : 'Conversation reopened')
  window.setTimeout(() => {
    if (threadResolvedHold[key] === resolved) delete threadResolvedHold[key]
  }, 120_000)
  return true
}
