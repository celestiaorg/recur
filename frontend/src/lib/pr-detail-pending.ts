/** GitHub conversation snapshots always include a description comment. */
export function hasPRDescription(
  pr: { comments?: Array<{ kind?: string }> } | null | undefined,
): boolean {
  return (pr?.comments ?? []).some((c) => c.kind === 'description')
}

export function prConvPending(
  pr: { source?: string; comments?: Array<{ kind?: string }> } | null | undefined,
  giveUp = false,
): boolean {
  if (!pr) return !giveUp
  if (pr.source !== 'gh') return false
  if (giveUp) return false
  return !hasPRDescription(pr)
}

export function prFilesPending(
  pr: { source?: string; files?: unknown[]; fileCount?: number } | null | undefined,
  refreshing = false,
  giveUp = false,
): boolean {
  if (!pr) return !giveUp
  if (pr.source !== 'gh') return false
  if ((pr.files?.length ?? 0) > 0) return false
  if (giveUp) return false
  return (pr.fileCount ?? 0) > 0 || refreshing
}

export function prCommitsPending(
  pr: { source?: string; commits?: unknown[] } | null | undefined,
  refreshing = false,
): boolean {
  if (!pr) return true
  if (pr.source !== 'gh') return false
  if ((pr.commits?.length ?? 0) > 0) return false
  return refreshing
}

export function prChecksPending(
  pr: { source?: string; checks?: unknown[]; hasChecks?: boolean } | null | undefined,
  refreshing = false,
): boolean {
  if (!pr) return true
  if (pr.source !== 'gh') return false
  if ((pr.checks?.length ?? 0) > 0) return false
  return refreshing || !!pr.hasChecks
}
