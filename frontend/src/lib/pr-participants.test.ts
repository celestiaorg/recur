import { describe, expect, it } from 'vitest'
import {
  PR_FACE_CAP,
  participantAvatarUrl,
  participantInitials,
  participantLabel,
  prFaceSlots,
  prParticipants,
  prPeopleWithReviews,
  reviewStateLabel,
  reviewerSummary,
} from './pr-participants'

describe('participantLabel', () => {
  it('prefers @login over display name', () => {
    expect(participantLabel({ login: 'avery', name: 'Avery Rourke' })).toBe('@avery')
    expect(participantLabel({ name: 'Coder' })).toBe('Coder')
    expect(participantLabel({ login: '@sam' })).toBe('@sam')
  })
})

describe('participantInitials', () => {
  it('uses the agent code, then login/name', () => {
    expect(participantInitials({ code: 'CO', login: 'coder' })).toBe('CO')
    expect(participantInitials({ login: 'avery' })).toBe('AV')
    expect(participantInitials({ name: 'PR Responder' })).toBe('PR')
  })
})

describe('participantAvatarUrl', () => {
  it('uses a GitHub png when only a login is known', () => {
    expect(participantAvatarUrl({ avatarUrl: 'https://ex/a.png' })).toBe('https://ex/a.png')
    expect(participantAvatarUrl({ login: 'avery' })).toBe('https://github.com/avery.png?size=40')
    expect(participantAvatarUrl({ login: 'dependabot[bot]' })).toBe('https://github.com/dependabot.png?size=40')
    expect(participantAvatarUrl({ login: 'loopbot', avatarCss: 'background:red' })).toBe('')
    expect(participantAvatarUrl({ name: 'Coder' })).toBe('')
  })
})

describe('prFaceSlots', () => {
  it(`shows everyone at ${PR_FACE_CAP} and a +N chip past that`, () => {
    const six = Array.from({ length: 6 }, (_, i) => ({ login: `u${i}` }))
    expect(prFaceSlots(six)).toEqual({ shown: six, extra: 0, rest: [] })
    const eight = Array.from({ length: 8 }, (_, i) => ({ login: `u${i}` }))
    const slots = prFaceSlots(eight)
    expect(slots.shown).toHaveLength(5)
    expect(slots.extra).toBe(3)
    expect(slots.rest.map((p) => p.login)).toEqual(['u5', 'u6', 'u7'])
  })
})

describe('prParticipants', () => {
  it('uses the stored slice when present', () => {
    expect(prParticipants({
      participants: [{ login: 'sam' }],
      authorLogin: 'other',
      comments: [{ authorLogin: 'avery' }],
    })).toEqual([{ login: 'sam' }])
  })

  it('falls back to author then unique commenters', () => {
    expect(prParticipants({
      authorLogin: 'loopbot',
      comments: [
        { kind: 'description', authorLogin: 'loopbot', author: 'loopbot' },
        { authorLogin: 'avery', author: 'Avery Rourke', code: 'AR' },
        { kind: 'hidden', authorLogin: 'ghost' },
        { author: 'Coder', code: 'CO' },
      ],
    })).toEqual([
      { login: 'loopbot' },
      { login: 'avery', name: 'Avery Rourke', avatarUrl: undefined, avatarCss: undefined, code: 'AR' },
      { login: undefined, name: 'Coder', avatarUrl: undefined, avatarCss: undefined, code: 'CO' },
    ])
  })
})

describe('reviewStateLabel', () => {
  it('names GitHub-style review states', () => {
    expect(reviewStateLabel('approved')).toBe('Approved')
    expect(reviewStateLabel('pending')).toBe('Review requested')
    expect(reviewStateLabel('changes_requested')).toBe('Changes requested')
    expect(reviewStateLabel('commented')).toBe('Commented')
    expect(reviewStateLabel('')).toBe('')
  })
})

describe('prPeopleWithReviews', () => {
  it('puts reviewers first and stamps their state', () => {
    const people = prPeopleWithReviews({
      participants: [{ login: 'loopbot' }, { login: 'avery' }, { name: 'Coder', code: 'CO' }],
      reviewers: [
        { login: 'sam', state: 'approved' },
        { login: 'avery', state: 'pending' },
      ],
    })
    expect(people.map((p) => `${p.login || p.name}:${p.reviewState || ''}`)).toEqual([
      'avery:pending',
      'sam:approved',
      'loopbot:',
      'Coder:',
    ])
  })
})

describe('reviewerSummary', () => {
  it('names who approved and who is still waiting', () => {
    expect(reviewerSummary([
      { login: 'sam', state: 'approved' },
      { login: 'avery', state: 'pending' },
      { login: 'lee', state: 'changes_requested' },
    ])).toBe('Approved by @sam · Changes requested by @lee · Waiting on @avery')
  })
})
