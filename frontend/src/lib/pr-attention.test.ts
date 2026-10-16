import { describe, expect, it } from 'vitest'
import {
  ATTENTION_CHECKS_FAILING,
  ATTENTION_REVIEW_REQUESTED,
  prAttentionCodes,
  prAttentionLabels,
  prChecksPassing,
  prChecksRollup,
  prHasCheckStatus,
  prHasReviewRequest,
} from './pr-attention'

describe('prAttentionCodes', () => {
  it('puts review requested first and de-dupes the bool + reasons slice', () => {
    expect(prAttentionCodes({
      reviewRequested: true,
      attentionReasons: [ATTENTION_REVIEW_REQUESTED, ATTENTION_CHECKS_FAILING],
    })).toEqual([ATTENTION_REVIEW_REQUESTED, ATTENTION_CHECKS_FAILING])
  })

  it('falls back for Needs-you rows that have no stored reason', () => {
    expect(prAttentionCodes({ group: 'you' })).toEqual([ATTENTION_REVIEW_REQUESTED])
    expect(prAttentionCodes({ group: 'open' })).toEqual([])
  })

  it('treats legacy awaiting_review as a review request', () => {
    expect(prAttentionCodes({ attentionReasons: ['awaiting_review'] })).toEqual([ATTENTION_REVIEW_REQUESTED])
  })

  it('does not invent a review request for GitHub drafts', () => {
    expect(prAttentionCodes({
      group: 'you',
      draft: true,
      reviewRequested: true,
      attentionReasons: [ATTENTION_REVIEW_REQUESTED, ATTENTION_CHECKS_FAILING],
    })).toEqual([])
  })
})

describe('prAttentionLabels', () => {
  it('maps codes to dashboard copy and omits checks-failing (shown as an icon)', () => {
    expect(prAttentionLabels({
      reviewRequested: true,
      attentionReasons: ['assigned', ATTENTION_CHECKS_FAILING],
    })).toEqual(['Needs your review', 'Assigned to you'])
    expect(prAttentionLabels({ group: 'you' })).toEqual(['Needs your review'])
  })

  it('flags GitHub merge conflicts, drafts included', () => {
    expect(prAttentionLabels({ mergeable: 'CONFLICTING' })).toEqual(['Merge conflicts'])
    expect(prAttentionLabels({ draft: true, mergeStateStatus: 'DIRTY' })).toEqual(['Merge conflicts'])
    expect(prAttentionLabels({ mergeable: 'MERGEABLE', mergeStateStatus: 'CLEAN' })).toEqual([])
  })
})

describe('prHasReviewRequest', () => {
  it('is true from the bool, reasons slice, or a local Needs-you row', () => {
    expect(prHasReviewRequest({ reviewRequested: true })).toBe(true)
    expect(prHasReviewRequest({ attentionReasons: [ATTENTION_REVIEW_REQUESTED] })).toBe(true)
    expect(prHasReviewRequest({ attentionReasons: ['awaiting_review'] })).toBe(true)
    expect(prHasReviewRequest({ group: 'you' })).toBe(true)
    expect(prHasReviewRequest({ attentionReasons: [ATTENTION_CHECKS_FAILING] })).toBe(false)
    expect(prHasReviewRequest({ group: 'open' })).toBe(false)
    expect(prHasReviewRequest({ group: 'you', draft: true, reviewRequested: true })).toBe(false)
  })
})

describe('prHasCheckStatus', () => {
  it('is true when listing, the checks array, or a failing rollup says so', () => {
    expect(prHasCheckStatus({ hasChecks: true, checksOk: true })).toBe(true)
    expect(prHasCheckStatus({ checks: [{ name: 'test' }], checksOk: true })).toBe(true)
    expect(prHasCheckStatus({ checksOk: false })).toBe(true)
    expect(prHasCheckStatus({ checksOk: true })).toBe(false)
  })
})

describe('prChecksRollup', () => {
  it('treats in-progress GitHub checks as pending, not failing', () => {
    const pending = { checks: [{ passed: false, status: 'pending' }], checksOk: false, hasChecks: true }
    expect(prChecksRollup(pending).kind).toBe('pending')
    expect(prChecksPassing(pending)).toBe(false)
    expect(prChecksRollup({ checksOk: false, hasChecks: true }).kind).toBe('fail')
    expect(prChecksPassing({ hasChecks: true, checksOk: true })).toBe(true)
  })
})
