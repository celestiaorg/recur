/**
 * Frecency for the Run Command palette (⌘⇧P): most recently run × most often.
 * Persisted in localStorage like other chrome prefs.
 */

const STORAGE_KEY = 'recur.commandUsage'
const MAX_TRACKED = 80
const RECENT_SECTION_SIZE = 8
/** Half-life for recency decay (ms) — a week. */
const HALF_LIFE_MS = 7 * 24 * 60 * 60 * 1000

export type CommandUsage = {
  count: number
  lastUsed: number // epoch ms
}

type UsageMap = Record<string, CommandUsage>

function load(): UsageMap {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as UsageMap
    if (!parsed || typeof parsed !== 'object') return {}
    return parsed
  } catch {
    return {}
  }
}

function save(map: UsageMap) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map))
  } catch {
    /* ignore quota / private mode */
  }
}

/** Record one palette invocation of a command (by uuid). */
export function recordCommandUsage(id: string) {
  if (!id) return
  const map = load()
  const prev = map[id]
  map[id] = {
    count: (prev?.count ?? 0) + 1,
    lastUsed: Date.now(),
  }
  // Cap map size: drop lowest-scoring entries.
  const ids = Object.keys(map)
  if (ids.length > MAX_TRACKED) {
    const ranked = ids
      .map((k) => ({ id: k, score: frecencyScore(map[k]) }))
      .sort((a, b) => b.score - a.score)
    const keep = new Set(ranked.slice(0, MAX_TRACKED).map((r) => r.id))
    for (const k of ids) {
      if (!keep.has(k)) delete map[k]
    }
  }
  save(map)
}

export function frecencyScore(u: CommandUsage | undefined, now = Date.now()): number {
  if (!u || u.count <= 0) return 0
  const age = Math.max(0, now - u.lastUsed)
  // count × 0.5^(age / halfLife) — recent heavy use wins, stale use fades.
  return u.count * Math.pow(0.5, age / HALF_LIFE_MS)
}

/**
 * Recent section order: last-used always first, then frecency for the rest.
 * Cap is overall (last-used consumes one slot when present).
 */
export function rankedCommandIds(limit = RECENT_SECTION_SIZE): string[] {
  const map = load()
  const now = Date.now()
  const ids = Object.keys(map).filter((id) => frecencyScore(map[id], now) > 0)
  if (!ids.length) return []

  let lastId: string | null = null
  let lastTs = -1
  for (const id of ids) {
    const ts = map[id].lastUsed
    if (ts > lastTs) {
      lastTs = ts
      lastId = id
    }
  }

  const rest = ids
    .filter((id) => id !== lastId)
    .map((id) => ({ id, score: frecencyScore(map[id], now) }))
    .sort((a, b) => b.score - a.score || (map[b.id].lastUsed - map[a.id].lastUsed))
    .map((r) => r.id)

  const out = lastId ? [lastId, ...rest] : rest
  return out.slice(0, limit)
}

export const COMMAND_USAGE_RECENT_SIZE = RECENT_SECTION_SIZE
