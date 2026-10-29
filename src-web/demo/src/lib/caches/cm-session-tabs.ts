/**
 * Park CodeMirror tab snapshots across workspace switches.
 *
 * CodeEditor already caches EditorState for in-workspace tab switches. The
 * session key used to wipe that cache so two worktrees could not share a
 * buffer — which also re-tokenized the visible file on every switch. A 5k-line
 * Svelte buffer made that uniquely slow. This LRU holds the per-workspace tab
 * map so returning to a root can view.setState(cached) instead of parsing again.
 *
 * Empty parks do not clobber a previous snapshot for that session. EditorState
 * is sized from doc.length — never walked (syntax trees cycle and are huge).
 */

import { ByteLru, WARM_CACHE_MAX_BYTES, estimateBytes, stringBytes } from './byte-lru'

export const EDITOR_SESSION_TAB_PARK_MAX_BYTES = WARM_CACHE_MAX_BYTES

/** UTF-16 source plus ~8× for the syntax tree / EditorState (budgeting). */
function parkedTabBytes(tab: unknown): number {
  if (tab == null) return 0
  if (typeof tab !== 'object') return estimateBytes(tab)
  const rec = tab as { state?: { doc?: { length?: number } }; clean?: { length?: number } }
  const doc = rec.state?.doc?.length
  if (typeof doc === 'number') {
    const clean = typeof rec.clean?.length === 'number' ? rec.clean.length : 0
    return 64 + (doc + clean) * 16
  }
  return estimateBytes(tab)
}

function parkedSessionBytes<T>(tabs: Map<string, T>): number {
  let n = 48
  for (const [path, tab] of tabs) n += stringBytes(path) + parkedTabBytes(tab)
  return n
}

export class SessionTabPark<T> {
  private readonly lru: ByteLru<Map<string, T>>

  constructor(opts?: { maxBytes?: number; sizeOf?: (tabs: Map<string, T>) => number }) {
    this.lru = new ByteLru({
      maxBytes: opts?.maxBytes ?? EDITOR_SESSION_TAB_PARK_MAX_BYTES,
      sizeOf: opts?.sizeOf ?? parkedSessionBytes,
    })
  }

  park(session: string, tabs: Map<string, T>): void {
    if (!session || tabs.size === 0) return
    this.lru.set(session, new Map(tabs))
  }

  take(session: string): Map<string, T> | undefined {
    if (!session) return undefined
    return this.lru.take(session)
  }

  get size(): number {
    return this.lru.size
  }

  /** Drop sessions for which `keep` returns false. */
  prune(keep: (session: string) => boolean): void {
    this.lru.prune((session) => keep(session))
  }

  /** Drop individual parked tabs for which `keep` returns false. */
  pruneTabs(keep: (tab: T) => boolean): void {
    for (const session of [...this.lru.keys()]) {
      const tabs = this.lru.peek(session)
      if (!tabs) continue
      for (const [path, tab] of [...tabs]) {
        if (!keep(tab)) tabs.delete(path)
      }
      if (tabs.size === 0) this.lru.delete(session)
      else this.lru.refresh(session)
    }
  }
}

const parks = new Map<string, SessionTabPark<unknown>>()

/** Process-wide park per editor pane so a CodeEditor remount still hits. */
export function sessionTabPark<T>(pane: string): SessionTabPark<T> {
  let park = parks.get(pane)
  if (!park) {
    park = new SessionTabPark<unknown>()
    parks.set(pane, park)
  }
  return park as SessionTabPark<T>
}

export function resetSessionTabParksForTests(): void {
  parks.clear()
}

/** Apply `keep` to every parked tab in every pane (theme change drops clean ones). */
export function pruneAllSessionTabParks<T>(keep: (tab: T) => boolean): void {
  for (const park of parks.values()) {
    ;(park as SessionTabPark<T>).pruneTabs(keep)
  }
}

/** Drop parked EditorStates whose workspace id is no longer live. */
export function pruneSessionTabParks(liveWorkspaceIds: Iterable<string>): void {
  const live = new Set(liveWorkspaceIds)
  for (const park of parks.values()) {
    park.prune((session) => {
      const ws = session.includes('\0') ? session.slice(session.indexOf('\0') + 1) : session
      return !ws || live.has(ws)
    })
  }
}
