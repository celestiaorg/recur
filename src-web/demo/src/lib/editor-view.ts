/**
 * FileAtBase text for the SCM gutter.
 *
 * `null` = not loaded / non-git / scratch — no marks. Empty string means the
 * path did not exist at the spin-off revision and is painted as a full-file
 * add, but only for real SCM additions (`u`/`a`). Gitignored files also miss
 * that revision (porcelain skips `!!`) and must not get a green wall.
 */
export function scmGutterBase(cached: string | null, gitStatus: string): string | null {
  if (cached == null) return null
  if (cached === '' && gitStatus !== 'u' && gitStatus !== 'a') return null
  return cached
}
