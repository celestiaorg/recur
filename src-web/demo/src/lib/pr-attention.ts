import { checksRollup, type ChecksRollup } from './pr-checks'

/** GitHub / Recur codes stored on PR.attentionReasons. */
export const ATTENTION_REVIEW_REQUESTED = 'review_requested'
export const ATTENTION_ASSIGNED = 'assigned'
export const ATTENTION_CHANGES_REQUESTED = 'changes_requested'
export const ATTENTION_CHECKS_FAILING = 'checks_failing'
export const ATTENTION_AWAITING_REVIEW = 'awaiting_review'

const LABELS: Record<string, string> = {
  [ATTENTION_REVIEW_REQUESTED]: 'Needs your review',
  [ATTENTION_ASSIGNED]: 'Assigned to you',
  [ATTENTION_CHANGES_REQUESTED]: 'Changes requested',
  [ATTENTION_CHECKS_FAILING]: 'Checks failing',
  [ATTENTION_AWAITING_REVIEW]: 'Needs your review',
}

export type PRAttention = {
  group?: string
  reviewRequested?: boolean
  attentionReasons?: string[]
  checksOk?: boolean
  hasChecks?: boolean
  checks?: unknown[]
  draft?: boolean
  mergeable?: string
  mergeStateStatus?: string
}

function prIsDraft(pr: PRAttention | null | undefined): boolean {
  return !!pr?.draft || (pr?.mergeStateStatus ?? '').toUpperCase() === 'DRAFT'
}

function pushCode(out: string[], seen: Set<string>, code: string) {
  const c = code.trim()
  if (!c || seen.has(c)) return
  seen.add(c)
  out.push(c)
}

/** Stable reason codes for this PR, review-request first.
 *  Local "you" rows and legacy `awaiting_review` are the same as a GitHub review request.
 *  GitHub drafts are not an inbox — they are not ready for review. */
export function prAttentionCodes(pr: PRAttention | null | undefined): string[] {
  if (prIsDraft(pr)) return []
  const out: string[] = []
  const seen = new Set<string>()
  if (pr?.reviewRequested) pushCode(out, seen, ATTENTION_REVIEW_REQUESTED)
  for (const c of pr?.attentionReasons ?? []) {
    pushCode(out, seen, c === ATTENTION_AWAITING_REVIEW ? ATTENTION_REVIEW_REQUESTED : c)
  }
  if (out.length === 0 && pr?.group === 'you') {
    pushCode(out, seen, ATTENTION_REVIEW_REQUESTED)
  }
  return out
}

function prHasConflicts(pr: PRAttention | null | undefined): boolean {
  return (pr?.mergeable ?? '').toUpperCase() === 'CONFLICTING' || (pr?.mergeStateStatus ?? '').toUpperCase() === 'DIRTY'
}

/** Labels for the list/detail — CI pass/fail is a check/x, not copy. */
export function prAttentionLabels(pr: PRAttention | null | undefined): string[] {
  const labels = prAttentionCodes(pr)
    .filter((c) => c !== ATTENTION_CHECKS_FAILING)
    .map((c) => LABELS[c] ?? c.replaceAll('_', ' '))
  if (prHasConflicts(pr)) labels.push('Merge conflicts')
  return labels
}

export function prHasReviewRequest(pr: PRAttention | null | undefined): boolean {
  return prAttentionCodes(pr).includes(ATTENTION_REVIEW_REQUESTED)
}

/** True when this PR reported at least one check run. */
export function prHasCheckStatus(pr: PRAttention | null | undefined): boolean {
  if (!pr) return false
  if (pr.hasChecks) return true
  if ((pr.checks ?? []).length > 0) return true
  if (pr.checksOk === false) return true
  if ((pr.attentionReasons ?? []).includes(ATTENTION_CHECKS_FAILING)) return true
  return false
}

export function prChecksRollup(pr: PRAttention | null | undefined): ChecksRollup {
  const rows: { passed: boolean; status?: string; detail?: string }[] = []
  for (const row of pr?.checks ?? []) {
    if (!row || typeof row !== 'object') continue
    const c = row as { passed?: boolean; status?: string; detail?: string }
    if (typeof c.passed !== 'boolean' && !(c.status ?? '').trim()) continue
    rows.push({ passed: !!c.passed, status: c.status, detail: c.detail })
  }
  return checksRollup(rows, { checksOk: pr?.checksOk, hasChecks: pr?.hasChecks })
}
