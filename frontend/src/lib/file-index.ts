/**
 * Workspace file search for Quick Open and @-mentions.
 *
 * Ranking lives in Go (CodeService.SearchFiles) against the rg-backed FileIndex.
 * files.exclude is applied at index build time (SetFileIndexExclude) so both
 * surfaces search the explorer-visible corpus with no per-query filter.
 */
import { get } from 'svelte/store'
import { CodeService, onWorkspaceFileIndex, bridgeErrorMessage } from './api'
import { explorerPrefs, pushToast } from './stores'

export type FileSearchHit = { item: string; score: number; label: string; dir: boolean }

export type FileSearchResult = {
  hits: FileSearchHit[]
  building: boolean
  ready: boolean
}

/** Returned when this call was cancelled by a newer searchFiles — callers must not apply it. */
export const SEARCH_CANCELLED = Symbol('search-cancelled')

type Status = { gen: number; ready: boolean; building: boolean }
type Listener = () => void

const statusByWs = new Map<string, Status>()
const listeners = new Map<string, Set<Listener>>()

/** Latest in-flight SearchFiles — cancelled when a newer query supersedes it. */
let inflightSearch: { cancel: (cause?: unknown) => void } | null = null

function notify(workspaceId: string) {
  const set = listeners.get(workspaceId)
  if (!set) return
  for (const fn of set) fn()
}

/** Subscribe to index readiness (no path payloads). */
export function subscribeFileIndexStatus(workspaceId: string, fn: Listener): () => void {
  if (!workspaceId) return () => {}
  let set = listeners.get(workspaceId)
  if (!set) {
    set = new Set()
    listeners.set(workspaceId, set)
  }
  set.add(fn)
  return () => {
    set!.delete(fn)
    if (!set!.size) listeners.delete(workspaceId)
  }
}

export function getFileIndexStatus(workspaceId: string): Status | undefined {
  return statusByWs.get(workspaceId)
}

let wired = false

/** Push files.exclude into Go so the index matches the explorer. */
export function syncFileIndexExclude(patterns: string[] = get(explorerPrefs).exclude) {
  void CodeService.SetFileIndexExclude(patterns).catch((err: unknown) => {
    // Non-fatal — search still works with always-on .git/node_modules filters.
    console.warn('SetFileIndexExclude failed', bridgeErrorMessage(err))
  })
}

/** Wire bus events once (call from app boot). */
export function wireFileIndexEvents() {
  if (wired) return
  wired = true
  onWorkspaceFileIndex((payload) => {
    statusByWs.set(payload.workspaceId, {
      gen: payload.gen,
      ready: payload.ready,
      building: payload.building,
    })
    notify(payload.workspaceId)
  })
  syncFileIndexExclude()
  explorerPrefs.subscribe((p) => syncFileIndexExclude(p.exclude))
}

function isCancelError(err: unknown): boolean {
  if (!err || typeof err !== 'object') return false
  const name = 'name' in err ? String((err as { name: unknown }).name) : ''
  if (name === 'CancelError' || name === 'CancelledRejectionError') return true
  return /cancel/i.test(bridgeErrorMessage(err))
}

/**
 * Rank project/workspace files for query via Go. Empty workspaceId searches
 * the project checkout (New-workspace @mentions). Cancels any prior in-flight
 * SearchFiles so keystroke storms don't pile up full-corpus scans on the Go
 * side. Cancelled calls resolve to SEARCH_CANCELLED (not empty hits).
 */
export async function searchFiles(
  projectId: string,
  workspaceId: string,
  query: string,
  limit = 50,
  filesOnly = false,
): Promise<FileSearchResult | typeof SEARCH_CANCELLED> {
  if (!projectId && !workspaceId) {
    return { hits: [], building: false, ready: false }
  }
  inflightSearch?.cancel()
  const pending = CodeService.SearchFiles(projectId, workspaceId, query, limit, filesOnly)
  inflightSearch = pending
  let res: Awaited<ReturnType<typeof CodeService.SearchFiles>> | null = null
  try {
    res = await pending
  } catch (err: unknown) {
    if (inflightSearch === pending) inflightSearch = null
    if (isCancelError(err)) {
      return SEARCH_CANCELLED
    }
    pushToast('error', `Search files failed: ${bridgeErrorMessage(err)}`)
    return { hits: [], building: false, ready: false }
  }
  if (inflightSearch === pending) inflightSearch = null
  if (!res) {
    return { hits: [], building: false, ready: false }
  }
  const key = workspaceId || projectId
  statusByWs.set(key, {
    gen: statusByWs.get(key)?.gen ?? 0,
    ready: !!res.ready,
    building: !!res.building,
  })
  const hits = mapFileSearchHits(res.hits)
  return { hits, building: !!res.building, ready: !!res.ready }
}

/** Normalize SearchFiles hits (dir flag, trailing slash, or parent of another hit). */
export function mapFileSearchHits(
  raw: ReadonlyArray<{ path?: string; score?: number; dir?: boolean }> | null | undefined,
): FileSearchHit[] {
  const rows = (raw ?? [])
    .map((h) => {
      const path = String(h.path ?? '').replace(/\/+$/, '')
      return {
        path,
        score: h.score ?? 0,
        flagged: !!h.dir || String(h.path ?? '').endsWith('/'),
      }
    })
    .filter((h) => h.path)
  const parents = new Set<string>()
  for (const h of rows) {
    let p = h.path
    while (true) {
      const i = p.lastIndexOf('/')
      if (i <= 0) break
      p = p.slice(0, i)
      parents.add(p)
    }
  }
  return rows.map((h) => {
    const dir = h.flagged || parents.has(h.path)
    return { item: h.path, score: h.score, label: h.path, dir }
  })
}
