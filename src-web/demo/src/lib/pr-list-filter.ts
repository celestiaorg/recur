import type { IconName } from './icons'
import { prLifecycle, type PRLifecycle } from './colors'
import {
  ATTENTION_ASSIGNED,
  ATTENTION_CHANGES_REQUESTED,
  prChecksRollup,
  prHasReviewRequest,
  type PRAttention,
} from './pr-attention'

export type PRListLife = 'open' | 'draft' | 'merged' | 'closed' | 'all'

export const PR_LIST_SEGS: { id: PRListLife; label: string }[] = [
  { id: 'open', label: 'Open' },
  { id: 'draft', label: 'Draft' },
  { id: 'merged', label: 'Merged' },
  { id: 'closed', label: 'Closed' },
  { id: 'all', label: 'All' },
]

export const PR_LIST_FACETS: { value: string; label: string; group: string }[] = [
  { value: 'review_requested', label: 'Needs your review', group: 'Review' },
  { value: 'changes_requested', label: 'Changes requested', group: 'Review' },
  { value: 'assigned', label: 'Assigned to you', group: 'Review' },
  { value: 'in_review', label: 'In review', group: 'Review' },
  { value: 'checks_failing', label: 'Checks failing', group: 'Checks' },
  { value: 'checks_passing', label: 'Checks passing', group: 'Checks' },
]

export type PRListRow = PRAttention & {
  id?: string
  state?: string
  group?: string
  draft?: boolean
  mergeStateStatus?: string
  mergeQueued?: boolean
  when?: string
  createdAt?: string
  updatedAt?: string
  number?: number
  title?: string
}

export type PRListSort = 'updated' | 'updated-asc' | 'newest' | 'oldest' | 'title'

export const PR_LIST_SORT_DEFAULT: PRListSort = 'updated'

export const PR_LIST_SORTS: { id: PRListSort; label: string; icon: IconName }[] = [
  { id: 'updated', label: 'Recently updated', icon: 'clock-arrow-down' },
  { id: 'updated-asc', label: 'Least recently updated', icon: 'clock-arrow-up' },
  { id: 'newest', label: 'Newest', icon: 'calendar-arrow-down' },
  { id: 'oldest', label: 'Oldest', icon: 'calendar-arrow-up' },
  { id: 'title', label: 'Title A–Z', icon: 'arrow-up-az' },
]

/** Queued PRs are still open on GitHub, so they list and count as open. */
export function prListLifecycle(pr: PRListRow | null | undefined): PRLifecycle | null {
  const life = prLifecycle(pr)
  return life === 'queued' ? 'open' : life
}

export function prMatchesLife(pr: PRListRow | null | undefined, life: PRListLife): boolean {
  const got = prListLifecycle(pr)
  if (!got) return life === 'all'
  if (life === 'all') return true
  return got === life
}

function stateText(pr: PRListRow | null | undefined): string {
  return (pr?.state ?? '').toLowerCase()
}

export function prMatchesFacet(pr: PRListRow | null | undefined, facet: string): boolean {
  switch (facet) {
    case 'review_requested':
      return prHasReviewRequest(pr)
    case 'changes_requested':
      return (pr?.attentionReasons ?? []).includes(ATTENTION_CHANGES_REQUESTED)
        || stateText(pr).includes('changes requested')
    case 'assigned':
      return (pr?.attentionReasons ?? []).includes(ATTENTION_ASSIGNED)
    case 'in_review': {
      const s = stateText(pr)
      return s.includes('in review') || s.includes('review required')
    }
    case 'checks_failing':
      return prChecksRollup(pr).kind === 'fail'
    case 'checks_passing':
      return prChecksRollup(pr).kind === 'pass'
    default:
      return false
  }
}

/** Lifecycle first, then every selected status (AND). Empty facets = no extra cut. */
export function prMatchesListFilter(
  pr: PRListRow | null | undefined,
  life: PRListLife,
  facets: string[] = [],
): boolean {
  if (!prMatchesLife(pr, life)) return false
  for (const f of facets) {
    if (!prMatchesFacet(pr, f)) return false
  }
  return true
}

export const PR_LIST_GROUP_ORDER = ['you', 'loop', 'open', 'merged'] as const

/** Milliseconds since epoch from RFC3339 stamps. Missing / invalid sorts as oldest. */
export function prCreatedMs(createdAt: string | undefined): number {
  const s = (createdAt ?? '').trim()
  if (!s) return Number.NEGATIVE_INFINITY
  const t = Date.parse(s)
  return Number.isFinite(t) ? t : Number.NEGATIVE_INFINITY
}

/** Last-activity time: `updatedAt`, then `createdAt`. `when` is display-only. */
export function prUpdatedMs(pr: PRListRow): number {
  const u = prCreatedMs(pr.updatedAt)
  if (u !== Number.NEGATIVE_INFINITY) return u
  return prCreatedMs(pr.createdAt)
}

function tiePRList(a: PRListRow, b: PRListRow, createdDesc = true): number {
  const d = createdDesc
    ? prCreatedMs(b.createdAt) - prCreatedMs(a.createdAt)
    : prCreatedMs(a.createdAt) - prCreatedMs(b.createdAt)
  if (d) return d
  return (a.id ?? '').localeCompare(b.id ?? '')
}

export function comparePRList(a: PRListRow, b: PRListRow, sort: PRListSort = PR_LIST_SORT_DEFAULT): number {
  switch (sort) {
    case 'updated-asc': {
      const d = prUpdatedMs(a) - prUpdatedMs(b)
      return d || tiePRList(a, b, false)
    }
    case 'newest': {
      const d = prCreatedMs(b.createdAt) - prCreatedMs(a.createdAt)
      return d || (a.id ?? '').localeCompare(b.id ?? '')
    }
    case 'oldest': {
      const d = prCreatedMs(a.createdAt) - prCreatedMs(b.createdAt)
      return d || (a.id ?? '').localeCompare(b.id ?? '')
    }
    case 'title': {
      const t = (a.title ?? '').localeCompare(b.title ?? '', undefined, { sensitivity: 'base' })
      return t || tiePRList(a, b)
    }
    case 'updated':
    default: {
      const d = prUpdatedMs(b) - prUpdatedMs(a)
      return d || tiePRList(a, b)
    }
  }
}

/** Copy + sort. Default is recently updated, then later createdAt. */
export function sortPRList<T extends PRListRow>(list: T[], sort: PRListSort = PR_LIST_SORT_DEFAULT): T[] {
  return [...list].sort((a, b) => comparePRList(a, b, sort))
}

/** Drafts are not a needs-you inbox, even if GitHub still has a stale you-group. */
export function prListGroup(pr: PRListRow | null | undefined): string {
  const g = (pr?.group || 'open').trim() || 'open'
  if (g === 'you' && (pr?.draft || (pr?.mergeStateStatus ?? '').toUpperCase() === 'DRAFT')) {
    return 'open'
  }
  return g
}
