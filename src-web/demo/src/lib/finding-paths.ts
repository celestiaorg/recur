import type { Finding } from './types'

/** Exact candidate first, then a unique slash-anchored suffix — a wrong-file hit is worse than a miss. */
export function matchFindingPath(path: string, candidates: readonly string[]): string {
  if (!path) return ''
  const exact = candidates.find((x) => x === path)
  if (exact) return exact
  const hits = candidates.filter((x) => x.endsWith('/' + path) || path.endsWith('/' + x))
  return hits.length === 1 ? hits[0] : ''
}

export function findingsForPath(
  byPath: ReadonlyMap<string, Finding[]> | undefined,
  path: string,
): Finding[] {
  if (!byPath || !path) return []
  const exact = byPath.get(path)
  if (exact?.length) return exact
  for (const [p, fs] of byPath) {
    if (!fs.length) continue
    if (p === path || path.endsWith('/' + p) || p.endsWith('/' + path)) {
      return fs
    }
  }
  return []
}
