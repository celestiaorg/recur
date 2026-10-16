import { describe, expect, it } from 'vitest'
import { keepMergedPR, prChromeColor, prConversationLocked, prForWorkspace, prIconName, prLifecycle, prStatusLabel, projectColors, projectPicFromName } from './colors'

describe('projectPicFromName', () => {
  it('takes the first letter, uppercased', () => {
    expect(projectPicFromName('storefront')).toBe('S')
    expect(projectPicFromName('  payments')).toBe('P')
  })

  it('uses the first code point, not the first byte', () => {
    expect(projectPicFromName('éclair')).toBe('É')
  })

  it('is empty when the name is blank', () => {
    expect(projectPicFromName('')).toBe('')
    expect(projectPicFromName('   ')).toBe('')
  })
})

describe('projectColors', () => {
  it('matches the backend create-project palette', () => {
    expect([...projectColors]).toEqual([
      'var(--cyan)',
      'var(--amber)',
      'var(--indigo)',
      'var(--emerald)',
      'var(--coral)',
    ])
  })
})

describe('prLifecycle', () => {
  it('is null without a PR', () => {
    expect(prLifecycle(null)).toBeNull()
    expect(prLifecycle(undefined)).toBeNull()
  })

  it('treats merged group or state as merged', () => {
    expect(prLifecycle({ group: 'merged', state: 'In review' })).toBe('merged')
    expect(prLifecycle({ group: 'you', state: 'Merged' })).toBe('merged')
  })

  it('treats closed as closed', () => {
    expect(prLifecycle({ group: 'you', state: 'Closed' })).toBe('closed')
    expect(prLifecycle({ group: 'merged', state: 'Closed' })).toBe('closed')
  })

  it('locks comments on merged and closed PRs', () => {
    expect(prConversationLocked({ group: 'merged', state: 'Merged' })).toBe(true)
    expect(prConversationLocked({ state: 'Closed' })).toBe(true)
    expect(prConversationLocked({ state: 'In review' })).toBe(false)
  })

  it('treats everything else as open', () => {
    expect(prLifecycle({ group: 'you', state: 'In review' })).toBe('open')
    expect(prLifecycle({ state: 'Approved' })).toBe('open')
  })

  it('treats an unmerged draft as draft', () => {
    expect(prLifecycle({ group: 'you', state: 'In review', draft: true })).toBe('draft')
    expect(prLifecycle({ group: 'you', state: 'In review', mergeStateStatus: 'DRAFT' })).toBe('draft')
    expect(prLifecycle({ group: 'merged', state: 'Merged', draft: true })).toBe('merged')
  })
})

describe('keepMergedPR', () => {
  it('does not let a stale open snapshot un-merge', () => {
    const prev = { state: 'Merged', group: 'merged', title: 'old' }
    const next = { state: 'In review', group: 'you', title: 'from GetPR' }
    expect(keepMergedPR(prev, next)).toEqual({ state: 'Merged', group: 'merged', title: 'from GetPR' })
  })

  it('passes through when the incoming record is already merged', () => {
    const prev = { state: 'Merged', group: 'merged' }
    const next = { state: 'Merged', group: 'merged', title: 'fresh' }
    expect(keepMergedPR(prev, next)).toEqual(next)
  })

  it('passes through when nothing was merged locally', () => {
    const next = { state: 'In review', group: 'you' }
    expect(keepMergedPR({ state: 'In review', group: 'you' }, next)).toEqual(next)
    expect(keepMergedPR(undefined, next)).toEqual(next)
  })

  it('lets a reopened closed PR update', () => {
    const prev = { state: 'Closed', group: 'merged', title: 'old' }
    const next = { state: 'Open', group: 'open', title: 'from GetPR' }
    expect(keepMergedPR(prev, next)).toEqual(next)
  })
})

describe('prStatusLabel', () => {
  it('shows Draft instead of the review state', () => {
    expect(prStatusLabel({ state: 'In review', draft: true })).toBe('Draft')
    expect(prStatusLabel({ state: 'In review', mergeStateStatus: 'DRAFT' })).toBe('Draft')
    expect(prStatusLabel({ state: 'In review' })).toBe('In review')
    expect(prStatusLabel({ state: 'Merged', group: 'merged', draft: true })).toBe('Merged')
  })
})

describe('prIconName', () => {
  it('uses the draft glyph when draft is set', () => {
    expect(prIconName({ draft: true })).toBe('git-pull-request-draft')
    expect(prIconName({ mergeStateStatus: 'DRAFT' })).toBe('git-pull-request-draft')
    expect(prIconName({ draft: false })).toBe('git-pull-request')
    expect(prIconName(null)).toBe('git-pull-request')
  })

  it('uses git-merge once the PR is merged', () => {
    expect(prIconName({ group: 'merged', state: 'Merged' })).toBe('git-merge')
    expect(prIconName({ group: 'merged', draft: true })).toBe('git-merge')
  })

  it('uses the dashed git-merge while queued, git-merge once merged', () => {
    expect(prIconName({ state: 'In review', mergeQueued: true })).toBe('git-merge-queued')
    expect(prIconName({ state: 'Merged', group: 'merged', mergeQueued: true })).toBe('git-merge')
    expect(prChromeColor({ state: 'In review', mergeQueued: true })).toBe('var(--warn)')
    expect(prStatusLabel({ state: 'In review', mergeQueued: true })).toBe('Queued')
  })

  it('uses git-pull-request-closed when the PR is closed', () => {
    expect(prIconName({ state: 'Closed' })).toBe('git-pull-request-closed')
    expect(prIconName({ state: 'Closed', draft: true })).toBe('git-pull-request-closed')
  })
})

describe('prChromeColor', () => {
  it('tints draft cyan, open emerald, merged indigo', () => {
    expect(prChromeColor({ draft: true, state: 'In review' })).toBe('var(--cyan)')
    expect(prChromeColor({ mergeStateStatus: 'DRAFT', state: 'In review' })).toBe('var(--cyan)')
    expect(prChromeColor({ state: 'In review' })).toBe('var(--emerald)')
    expect(prChromeColor({ group: 'merged' })).toBe('var(--indigo)')
  })

  it('tints changes requested coral, not emerald', () => {
    expect(prChromeColor({ state: 'Changes requested' })).toBe('var(--coral)')
    expect(prChromeColor({ draft: true, state: 'Changes requested' })).toBe('var(--cyan)')
  })
})

describe('prForWorkspace', () => {
  const prs = [
    { workspaceId: 'ws-a', group: 'merged', state: 'Merged' },
    { workspaceId: 'ws-a', group: 'you', state: 'In review' },
    { workspaceId: 'ws-b', group: 'merged', state: 'Merged' },
  ]

  it('prefers an open PR on the same workspace', () => {
    expect(prForWorkspace(prs, 'ws-a')).toEqual(prs[1])
  })

  it('falls back to a merged or closed PR', () => {
    expect(prForWorkspace(prs, 'ws-b')).toEqual(prs[2])
  })

  it('is undefined when the workspace has no PR', () => {
    expect(prForWorkspace(prs, 'ws-none')).toBeUndefined()
  })

  it('prefers a merged GitHub PR over a leftover open Local PR', () => {
    const local = { workspaceId: 'ws-c', id: '4', source: 'local', group: 'you', state: 'In review' }
    const github = { workspaceId: 'ws-c', id: 'gh-156', source: 'gh', group: 'merged', state: 'Merged' }
    expect(prForWorkspace([local, github], 'ws-c')).toEqual(github)
    expect(prForWorkspace([github, local], 'ws-c')).toEqual(github)
  })

  it('prefers the newer PR on a tie', () => {
    const closed = { workspaceId: 'ws-d', id: 'gh-9', source: 'gh', group: 'merged', state: 'Closed' }
    const merged = { workspaceId: 'ws-d', id: 'gh-13', source: 'gh', group: 'merged', state: 'Merged' }
    expect(prForWorkspace([closed, merged], 'ws-d')).toEqual(merged)
  })

  it('prefers an open GitHub PR over an open Local PR', () => {
    const local = { workspaceId: 'ws-c', id: '4', source: 'local', group: 'you', state: 'In review' }
    const github = { workspaceId: 'ws-c', id: 'gh-156', source: 'gh', group: 'you', state: 'In review' }
    expect(prForWorkspace([local, github], 'ws-c')).toEqual(github)
  })
})
