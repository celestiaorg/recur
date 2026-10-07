import { describe, expect, it } from 'vitest'
import { attachChatSurface, chatSurfaceForView, peelChatSurface } from './chat-surface'

describe('chatSurfaceForView', () => {
  it('uses the workspace surface away from PR detail', () => {
    expect(chatSurfaceForView('detail', { id: 'gh-1', source: 'gh', number: 1 })).toEqual({
      surface: 'workspace',
    })
    expect(chatSurfaceForView('inbox')).toEqual({ surface: 'workspace' })
  })

  it('uses github_pr on GitHub PR detail', () => {
    expect(
      chatSurfaceForView('prdetail', {
        id: 'gh-acme-9',
        number: 9,
        source: 'gh',
        url: 'https://github.com/acme/r/pull/9',
        title: 'Imported',
        base: 'main',
        branch: 'feat',
      }),
    ).toEqual({
      surface: 'github_pr',
      prId: 'gh-acme-9',
      prNumber: 9,
      prSource: 'gh',
      prUrl: 'https://github.com/acme/r/pull/9',
      prTitle: 'Imported',
      prBase: 'main',
      prBranch: 'feat',
    })
  })

  it('uses local_pr on Local PR detail', () => {
    expect(chatSurfaceForView('prdetail', { id: '7', number: 7, source: 'local', title: 'WIP' })).toEqual({
      surface: 'local_pr',
      prId: '7',
      prNumber: 7,
      prSource: 'local',
      prUrl: '',
      prTitle: 'WIP',
      prBase: '',
      prBranch: '',
    })
  })
})

describe('attachChatSurface', () => {
  it('prefixes a peelable marker', () => {
    const wire = attachChatSurface('please review', { surface: 'github_pr', prNumber: 9, prId: 'gh-9' })
    expect(wire.startsWith('⟦recur-surface⟧')).toBe(true)
    expect(wire.endsWith('\nplease review')).toBe(true)
    expect(wire).toContain('"surface":"github_pr"')
    expect(wire).toContain('"prNumber":9')
    expect(peelChatSurface(wire)).toBe('please review')
  })

  it('leaves unmarked and invalid wires alone', () => {
    expect(peelChatSurface('plain')).toBe('plain')
    expect(peelChatSurface('⟦recur-surface⟧not-json⟦/recur-surface⟧\nhi')).toBe(
      '⟦recur-surface⟧not-json⟦/recur-surface⟧\nhi',
    )
  })
})
