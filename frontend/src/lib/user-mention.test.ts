/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test, vi } from 'vitest'
import { get } from 'svelte/store'

vi.mock('../../bindings/recur/internal/app', () => ({
  PRsService: { SearchPRMentionableUsers: vi.fn().mockResolvedValue([]) },
}))

import type { PR } from '../../bindings/recur/internal/domain/models'
import { applyUserMention, mentionQueryAt, mergeMentionHits, suggestUserMentions, userMentionSuggest, userMentions } from './user-mention'
import type { PRPerson } from './pr-participants'
import { prs, selectedPRId } from './stores'

const people: PRPerson[] = [
  { login: 'avery', name: 'Avery Rourke' },
  { login: 'loopbot', name: 'loopbot' },
  { login: 'sam', name: 'sam' },
  { name: 'Coder', code: 'CO' },
  { name: 'PR Responder', code: 'PR' },
]

describe('mentionQueryAt', () => {
  test('matches @query after whitespace and at start', () => {
    expect(mentionQueryAt('see @quer', 9)).toEqual({ start: 4, end: 9, query: 'quer' })
    expect(mentionQueryAt('@src', 4)).toEqual({ start: 0, end: 4, query: 'src' })
  })

  test('rejects mid-token @ (email)', () => {
    expect(mentionQueryAt('john@acme', 9)).toBeNull()
  })

  test('matches empty @', () => {
    expect(mentionQueryAt('@', 1)).toEqual({ start: 0, end: 1, query: '' })
    expect(mentionQueryAt('see @', 5)).toEqual({ start: 4, end: 5, query: '' })
  })
})

describe('suggestUserMentions', () => {
  test('empty query returns people with logins, capped', () => {
    expect(suggestUserMentions(people, '').map((p) => p.login)).toEqual(['avery', 'loopbot', 'sam'])
    const many = Array.from({ length: 12 }, (_, i) => ({ login: `u${i}` }))
    expect(suggestUserMentions(many, '').map((p) => p.login)).toEqual(
      Array.from({ length: 8 }, (_, i) => `u${i}`),
    )
  })

  test('filters by login and name; excludes no-login people', () => {
    expect(suggestUserMentions(people, 'av').map((p) => p.login)).toEqual(['avery'])
    expect(suggestUserMentions(people, 'rourke').map((p) => p.login)).toEqual(['avery'])
    expect(suggestUserMentions(people, 'Coder')).toEqual([])
  })
})

describe('mergeMentionHits', () => {
  test('local first, case-insensitive dedupe, cap', () => {
    const local: PRPerson[] = [
      { login: 'avery', name: 'Avery' },
      { login: 'loopbot' },
    ]
    const remote: PRPerson[] = [
      { login: 'Avery', name: 'Avery Rourke' },
      { login: 'octocat', name: 'The Octocat' },
      { login: 'hubot' },
    ]
    expect(mergeMentionHits(local, remote, 3).map((p) => p.login)).toEqual(['avery', 'loopbot', 'octocat'])
    expect(mergeMentionHits(local, null).map((p) => p.login)).toEqual(['avery', 'loopbot'])
    expect(mergeMentionHits([], remote).map((p) => p.login)).toEqual(['Avery', 'octocat', 'hubot'])
  })
})

describe('applyUserMention', () => {
  test('replaces @av with @avery and caret after the space', () => {
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      cb(0)
      return 0
    })
    const el = document.createElement('textarea')
    el.value = 'hi @av'
    el.setSelectionRange(6, 6)
    document.body.appendChild(el)
    let next = ''
    applyUserMention(
      { el, start: 3, query: 'av', hits: [], sel: 0, set: (v) => { next = v; el.value = v } },
      { login: 'avery', name: 'Avery Rourke' },
    )
    expect(next).toBe('hi @avery ')
    expect(el.selectionStart).toBe(10)
    expect(el.selectionEnd).toBe(10)
    expect(document.activeElement).toBe(el)
  })

  test('refuses a stale range after the caret leaves the query', () => {
    const el = document.createElement('textarea')
    el.value = 'Please review @av'
    el.setSelectionRange(0, 0)
    document.body.appendChild(el)
    let next = ''
    applyUserMention(
      { el, start: 14, query: 'av', hits: [], sel: 0, set: (v) => { next = v; el.value = v } },
      { login: 'avery' },
    )
    expect(next).toBe('')
    expect(el.value).toBe('Please review @av')
    expect(get(userMentionSuggest)).toBeNull()
  })
})

describe('userMentions', () => {
  afterEach(() => {
    userMentionSuggest.set(null)
    selectedPRId.set('')
    prs.set([])
    document.body.replaceChildren()
  })

  test('caret movement closes the picker and Enter does not rewrite the draft', () => {
    selectedPRId.set('pr-1')
    prs.set([{ id: 'pr-1', source: 'gh', participants: [{ login: 'avery', name: 'Avery Rourke' }] } as PR])
    const el = document.createElement('textarea')
    document.body.appendChild(el)
    el.focus()
    let draft = ''
    const stop = userMentions((v) => { draft = v; el.value = v })(el)

    el.value = 'Please review @av'
    el.setSelectionRange(18, 18)
    el.dispatchEvent(new Event('input'))
    expect(get(userMentionSuggest)?.query).toBe('av')

    el.setSelectionRange(0, 0)
    document.dispatchEvent(new Event('selectionchange'))
    expect(get(userMentionSuggest)).toBeNull()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }))
    expect(draft).toBe('')
    expect(el.value).toBe('Please review @av')
    stop?.()
  })
})
