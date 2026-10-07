import type { Commit } from '../../bindings/recur/internal/domain/models'

/** Keep already-loaded per-commit diffs when a GetPR snapshot still has Files empty. */
export function keepCommitFiles(prev: Commit[] | undefined, next: Commit[]): Commit[] {
  if (!prev?.length) return next
  return next.map((c) => {
    if (c.files?.length) return c
    const old = prev.find((p) => commitKeysMatch(p, c))
    return old?.files?.length ? { ...c, files: old.files } : c
  })
}

export function commitKeysMatch(a: Commit, b: Commit): boolean {
  const ah = (a.hash || '').toLowerCase()
  const bh = (b.hash || '').toLowerCase()
  const as = (a.sha || '').toLowerCase()
  const bs = (b.sha || '').toLowerCase()
  if (ah && bh && (ah === bh || ah.startsWith(bh) || bh.startsWith(ah))) return true
  if (as && bs && (as === bs || as.startsWith(bs) || bs.startsWith(as))) return true
  if (ah && bs && (bs.startsWith(ah) || ah.startsWith(bs))) return true
  if (as && bh && (as.startsWith(bh) || bh.startsWith(as))) return true
  return false
}
