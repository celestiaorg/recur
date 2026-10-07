import { describe, expect, it } from 'vitest'
import { commentHeaderVerb, reviewDecision } from './comment-header'

describe('reviewDecision', () => {
  it('reads approve and request-changes from verb or action body', () => {
    expect(reviewDecision({ verb: 'approved' })).toBe('approved')
    expect(reviewDecision({ kind: 'action', bodyMd: 'approved' })).toBe('approved')
    expect(reviewDecision({ verb: 'requested changes' })).toBe('changes')
    expect(reviewDecision({ kind: 'action', bodyMd: 'requested changes' })).toBe('changes')
    expect(reviewDecision({ kind: 'action', bodyMd: 'force-pushed the branch' })).toBe('')
    expect(reviewDecision({ kind: 'comment', bodyMd: 'looks approved to me' })).toBe('')
  })
})

describe('commentHeaderVerb', () => {
  it('is empty on description and action rows', () => {
    expect(commentHeaderVerb({ kind: 'description' })).toBe('')
    expect(commentHeaderVerb({ kind: 'action', verb: 'approved' })).toBe('')
    expect(commentHeaderVerb(null)).toBe('')
  })

  it('uses the stored verb', () => {
    expect(commentHeaderVerb({ verb: 'approved' })).toBe('approved these changes')
    expect(commentHeaderVerb({ kind: 'comment', verb: 'requested changes' })).toBe('requested changes')
  })

  it('falls back to commented', () => {
    expect(commentHeaderVerb({})).toBe('commented')
    expect(commentHeaderVerb({ kind: 'comment', verb: '  ' })).toBe('commented')
  })
})
