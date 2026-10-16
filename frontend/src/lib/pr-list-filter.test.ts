import { describe, expect, test } from 'vitest'
import {
  comparePRList,
  prCreatedMs,
  prListGroup,
  prListTitleCounts,
  PR_LIST_SORTS,
  prMatchesFacet,
  prMatchesLife,
  prMatchesListFilter,
  prUpdatedMs,
  sortPRList,
} from './pr-list-filter'
import { icons } from './icons'

const openReview = {
  group: 'you',
  state: 'In review',
  reviewRequested: true,
  checksOk: true,
  hasChecks: true,
  checks: [{ name: 'test' }],
}

describe('prMatchesLife', () => {
  test('Open hides drafts, merged, and closed', () => {
    expect(prMatchesLife(openReview, 'open')).toBe(true)
    expect(prMatchesLife({ ...openReview, mergeQueued: true }, 'open')).toBe(true)
    expect(prMatchesLife({ ...openReview, draft: true }, 'open')).toBe(false)
    expect(prMatchesLife({ group: 'merged', state: 'Merged' }, 'open')).toBe(false)
    expect(prMatchesLife({ state: 'Closed' }, 'open')).toBe(false)
    expect(prMatchesLife({ draft: true, state: 'In review' }, 'draft')).toBe(true)
    expect(prMatchesLife({ state: 'Closed' }, 'closed')).toBe(true)
    expect(prMatchesLife({ group: 'merged', state: 'Merged' }, 'all')).toBe(true)
  })
})

describe('prMatchesFacet', () => {
  test('review and check facets read the stored flags', () => {
    expect(prMatchesFacet(openReview, 'review_requested')).toBe(true)
    expect(prMatchesFacet(openReview, 'in_review')).toBe(true)
    expect(prMatchesFacet(openReview, 'checks_passing')).toBe(true)
    expect(prMatchesFacet(openReview, 'checks_failing')).toBe(false)
    expect(prMatchesFacet({ state: 'Changes requested' }, 'changes_requested')).toBe(true)
    expect(prMatchesFacet({ attentionReasons: ['assigned'] }, 'assigned')).toBe(true)
  })

  test('pending GitHub checks are not failing', () => {
    const pending = {
      ...openReview,
      checksOk: false,
      checks: [{ name: 'build', passed: false, status: 'pending' }],
    }
    expect(prMatchesFacet(pending, 'checks_failing')).toBe(false)
    expect(prMatchesFacet(pending, 'checks_passing')).toBe(false)
    expect(prMatchesListFilter(pending, 'open', ['checks_failing'])).toBe(false)
  })
})

describe('prMatchesListFilter', () => {
  test('ANDs status facets on top of the lifecycle cut', () => {
    expect(prMatchesListFilter(openReview, 'open', ['review_requested', 'checks_passing'])).toBe(true)
    expect(prMatchesListFilter(openReview, 'open', ['checks_failing'])).toBe(false)
    expect(prMatchesListFilter({ ...openReview, draft: true }, 'open', ['review_requested'])).toBe(false)
    expect(prMatchesListFilter({ ...openReview, draft: true }, 'draft', ['in_review'])).toBe(true)
  })
})

describe('prListTitleCounts', () => {
  test('counts live PRs and omits loop as its own tally', () => {
    expect(prListTitleCounts([
      openReview,
      { group: 'loop', state: 'Loop responding' },
      { group: 'merged', state: 'Merged' },
      { group: 'you', draft: true, state: 'In review' },
      { state: 'Closed' },
    ])).toEqual({ you: 1, open: 3, merged: 1 })
  })
})

describe('prListGroup', () => {
  test('moves GitHub drafts out of Needs your attention', () => {
    expect(prListGroup({ group: 'you', draft: true })).toBe('open')
    expect(prListGroup({ group: 'you', reviewRequested: true })).toBe('you')
    expect(prListGroup({ group: 'loop', draft: true })).toBe('loop')
  })
})

describe('PR_LIST_SORTS', () => {
  test('each option has a registered icon', () => {
    expect(PR_LIST_SORTS.map((s) => s.icon)).toEqual([
      'clock-arrow-down',
      'clock-arrow-up',
      'calendar-arrow-down',
      'calendar-arrow-up',
      'arrow-up-az',
    ])
    for (const s of PR_LIST_SORTS) {
      expect(icons[s.icon]?.body).toBeTruthy()
    }
  })
})

describe('prCreatedMs', () => {
  test('parses RFC3339 and treats blanks as oldest', () => {
    expect(prCreatedMs('2026-08-28T12:00:00Z')).toBe(Date.parse('2026-08-28T12:00:00Z'))
    expect(prCreatedMs('')).toBe(Number.NEGATIVE_INFINITY)
    expect(prCreatedMs('nope')).toBe(Number.NEGATIVE_INFINITY)
  })
})

describe('sortPRList', () => {
  const zebra = {
    id: 'a', number: 10, title: 'Zebra', when: 'now',
    createdAt: '2026-08-28T12:00:00Z', updatedAt: '2026-08-26T12:00:00Z',
  }
  const alpha = {
    id: 'b', number: 20, title: 'Alpha', when: '2d',
    createdAt: '2026-08-01T12:00:00Z', updatedAt: '2026-08-28T11:00:00Z',
  }
  const mid = {
    id: 'c', number: 15, title: 'mid', when: 'now',
    createdAt: '2026-08-15T12:00:00Z', updatedAt: '2026-08-28T11:00:00Z',
  }

  test('default is recently updatedAt, not a frozen when=now', () => {
    expect(sortPRList([zebra, alpha, mid]).map((p) => p.id)).toEqual(['c', 'b', 'a'])
    expect(comparePRList(alpha, zebra, 'updated')).toBeLessThan(0)
    expect(prUpdatedMs(alpha)).toBeGreaterThan(prUpdatedMs(zebra))
  })

  test('least recently updated flips updatedAt and ties by older createdAt', () => {
    expect(sortPRList([alpha, zebra], 'updated-asc').map((p) => p.id)).toEqual(['a', 'b'])
    expect(sortPRList([alpha, mid], 'updated-asc').map((p) => p.id)).toEqual(['b', 'c'])
  })

  test('newest / oldest use createdAt, not PR number', () => {
    expect(sortPRList([zebra, alpha, mid], 'newest').map((p) => p.id)).toEqual(['a', 'c', 'b'])
    expect(sortPRList([zebra, alpha, mid], 'oldest').map((p) => p.id)).toEqual(['b', 'c', 'a'])
  })

  test('title is case-insensitive A–Z', () => {
    expect(sortPRList([zebra, alpha, mid], 'title').map((p) => p.title)).toEqual(['Alpha', 'mid', 'Zebra'])
  })

  test('does not mutate the input', () => {
    const list = [zebra, alpha]
    sortPRList(list, 'oldest')
    expect(list.map((p) => p.id)).toEqual(['a', 'b'])
  })
})
