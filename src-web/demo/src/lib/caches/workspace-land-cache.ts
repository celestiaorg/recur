/**
 * Last titlebar git-land snapshot per workspace. Switching A→B used to blank
 * Commit / Update / Merge / Push until WorkspaceLandState returned, so the
 * bar jumped. Recalling the last snapshot (disabled until git confirms)
 * keeps the chrome width stable.
 */

import { ByteLru, CHROME_CACHE_MAX_BYTES, estimateBytes } from './byte-lru'

export type WorkspaceLandSnap = {
  behindBase: number
  aheadBase: number
  dirty: boolean
  unmerged: boolean
  base: string
  remote: string
  prId: string
  prNumber: number
  prSource: string
  prDraft: boolean
  aheadRemote: number
  behindRemote: number
  requirePR?: boolean
}

export const EMPTY_WORKSPACE_LAND: WorkspaceLandSnap = {
  behindBase: 0,
  aheadBase: 0,
  dirty: false,
  unmerged: false,
  base: '',
  remote: '',
  prId: '',
  prNumber: 0,
  prSource: '',
  prDraft: false,
  aheadRemote: 0,
  behindRemote: 0,
}

export const WORKSPACE_LAND_MAX_BYTES = CHROME_CACHE_MAX_BYTES

const cache = new ByteLru<WorkspaceLandSnap>({
  maxBytes: WORKSPACE_LAND_MAX_BYTES,
  sizeOf: estimateBytes,
})

export function workspaceLandFrom(
  st: Partial<WorkspaceLandSnap> | null | undefined,
): WorkspaceLandSnap {
  return {
    behindBase: st?.behindBase ?? 0,
    aheadBase: st?.aheadBase ?? 0,
    dirty: !!st?.dirty,
    unmerged: !!st?.unmerged,
    base: st?.base ?? '',
    remote: st?.remote ?? '',
    prId: st?.prId ?? '',
    prNumber: st?.prNumber ?? 0,
    prSource: st?.prSource ?? '',
    prDraft: !!st?.prDraft,
    aheadRemote: st?.aheadRemote ?? 0,
    behindRemote: st?.behindRemote ?? 0,
    requirePR: st?.requirePR,
  }
}

export function getWorkspaceLand(id: string): WorkspaceLandSnap | null {
  if (!id) return null
  const hit = cache.get(id)
  return hit ? { ...hit } : null
}

export function putWorkspaceLand(id: string, snap: WorkspaceLandSnap): void {
  if (!id) return
  cache.set(id, { ...snap })
}

export function dropWorkspaceLand(id: string): void {
  cache.delete(id)
}

export function pruneWorkspaceLand(liveWorkspaceIds: Iterable<string>): void {
  const live = new Set(liveWorkspaceIds)
  cache.prune((id) => live.has(id))
}

export function setWorkspaceLandMaxBytesForTests(n: number): void {
  cache.maxBytes = n
}
