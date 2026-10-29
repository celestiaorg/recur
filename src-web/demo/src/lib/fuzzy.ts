/** Simple fuzzy ranker for Quick Open / Go to Symbol. */

export type FuzzyHit<T> = { item: T; score: number; label: string }

/** An item with its label pre-lowercased and masked — see prepareFuzzy. */
export type FuzzyItem<T> = { item: T; label: string; lower: string; mask: number }

/**
 * Bit 0-25 = 'a'-'z' present, bit 26 = digit, bit 27 = any other character.
 * A query can only be a subsequence of a text if every bit it sets is also set
 * in the text — one integer AND rejects most of a repo index before the
 * character-by-character scan runs.
 */
function charMask(s: string): number {
  let m = 0
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i)
    if (c >= 97 && c <= 122) m |= 1 << (c - 97)
    else if (c >= 48 && c <= 57) m |= 1 << 26
    else m |= 1 << 27
  }
  return m
}

/**
 * Score `lowerText` (already lowercased) against `lowerQuery` (already trimmed
 * and lowercased). Split out from fuzzyScore so the hot path can hoist both
 * normalizations out of the per-item loop — Quick Open re-scores the entire
 * repo file index on every keystroke, and `toLowerCase()` per item per key was
 * the single largest allocation source in the app.
 */
function scoreLower(lowerText: string, lowerQuery: string): number | null {
  const t = lowerText
  const q = lowerQuery
  if (!q) return 0
  if (t === q) return 10_000
  const idx = t.indexOf(q)
  if (idx >= 0) {
    // Prefer basename / earlier matches
    const slash = t.lastIndexOf('/')
    const base = slash >= 0 ? t.slice(slash + 1) : t
    const baseBonus = base.startsWith(q) ? 500 : base.includes(q) ? 200 : 0
    return 1000 - idx + baseBonus + Math.max(0, 100 - t.length)
  }
  let qi = 0
  let score = 0
  let prev = -2
  for (let i = 0; i < t.length && qi < q.length; i++) {
    if (t[i] === q[qi]) {
      score += i === prev + 1 ? 8 : 3
      if (i === 0 || t[i - 1] === '/' || t[i - 1] === '_' || t[i - 1] === '-') score += 12
      prev = i
      qi++
    }
  }
  if (qi < q.length) return null
  return score - Math.floor(t.length / 4)
}

/** Higher score is better. Returns null when no match. */
export function fuzzyScore(text: string, query: string): number | null {
  return scoreLower(text.toLowerCase(), query.trim().toLowerCase())
}

/**
 * Pre-lowercase a candidate list once so repeated queries against it don't pay
 * for it again. Callers should memoize this on the source list, not the query.
 */
export function prepareFuzzy<T>(items: T[], labelOf: (item: T) => string): FuzzyItem<T>[] {
  const out: FuzzyItem<T>[] = new Array(items.length)
  for (let i = 0; i < items.length; i++) {
    const label = labelOf(items[i])
    const lower = label.toLowerCase()
    out[i] = { item: items[i], label, lower, mask: charMask(lower) }
  }
  return out
}

/**
 * Rank `prepared` against `query`, keeping only the best `limit`.
 *
 * Ordering is score desc, then label asc — same as before. Two deliberate
 * changes for speed on large repos:
 *
 *   - Results are collected into a bounded, always-sorted array of at most
 *     `limit` entries instead of pushing every match and sorting the lot. A
 *     candidate that can't beat the current worst is dropped without
 *     allocating, so a 20k-file index costs O(n log limit) rather than a full
 *     O(n log n) sort. Ties still fall through to the label comparison, so an
 *     alphabetically-earlier path correctly displaces a later one.
 *   - A character-set mask rejects non-matching candidates with one integer AND
 *     before the subsequence scan (see charMask).
 *
 * Tie-breaking still goes through a collator so the order is identical to the
 * previous `localeCompare` implementation — a plain `<` looks equivalent but
 * isn't, because code-point order sorts `/` before `_` while the collator
 * weights letters ahead of punctuation, which visibly reorders repo paths.
 */
const labelCollator = new Intl.Collator()

export function fuzzyFilterPrepared<T>(
  prepared: readonly FuzzyItem<T>[],
  query: string,
  limit = 50,
): FuzzyHit<T>[] {
  const q = query.trim().toLowerCase()
  const out: FuzzyHit<T>[] = []
  if (limit <= 0) return out
  const qMask = charMask(q)
  const cmp = labelCollator.compare

  for (let i = 0; i < prepared.length; i++) {
    const p = prepared[i]
    if ((qMask & ~p.mask) !== 0) continue
    const score = scoreLower(p.lower, q)
    if (score == null) continue
    // Full and no better than the current tail — skip without allocating.
    if (out.length === limit) {
      const worst = out[limit - 1]
      if (score < worst.score) continue
      if (score === worst.score && cmp(p.label, worst.label) >= 0) continue
    }
    // Binary search for the insertion point (score desc, then label asc).
    let lo = 0
    let hi = out.length
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      const m = out[mid]
      if (m.score > score || (m.score === score && cmp(m.label, p.label) < 0)) lo = mid + 1
      else hi = mid
    }
    out.splice(lo, 0, { item: p.item, score, label: p.label })
    if (out.length > limit) out.pop()
  }
  return out
}
