import { describe, expect, test } from 'vitest'
import {
  hasPRDescription,
  prChecksPending,
  prCommitsPending,
  prConvPending,
  prFilesPending,
} from './pr-detail-pending'

describe('prConvPending', () => {
  test('waits when the PR is missing', () => {
    expect(prConvPending(undefined)).toBe(true)
  })

  test('local PRs render immediately', () => {
    expect(prConvPending({ source: 'local', comments: [] })).toBe(false)
  })

  test('GitHub PRs wait until a description comment exists', () => {
    expect(prConvPending({ source: 'gh', comments: [] })).toBe(true)
    expect(prConvPending({ source: 'gh', comments: [{ kind: 'comment' }] })).toBe(true)
    expect(prConvPending({ source: 'gh', comments: [{ kind: 'description' }] })).toBe(false)
  })

  test('give-up stops waiting after a failed snapshot', () => {
    expect(prConvPending({ source: 'gh' }, true)).toBe(false)
    expect(prConvPending(undefined, true)).toBe(false)
  })
})

describe('prFilesPending', () => {
  test('waits for gh pr diff when the list already knows there are files', () => {
    expect(prFilesPending({ source: 'gh', files: [], fileCount: 4 })).toBe(true)
    expect(prFilesPending({ source: 'gh', files: [{ path: 'a.ts' }], fileCount: 4 })).toBe(false)
  })

  test('empty GitHub PRs are not pending after the first fetch', () => {
    expect(prFilesPending({ source: 'gh', files: [], fileCount: 0 }, false)).toBe(false)
    expect(prFilesPending({ source: 'gh', files: [], fileCount: 0 }, true)).toBe(true)
  })
})

describe('local PR list rows without diff fields', () => {
  test('pending only while GetPR is in flight', () => {
    expect(prFilesPending({ source: 'local', files: null as any }, true)).toBe(true)
    expect(prCommitsPending({ source: 'local', commits: null as any }, true)).toBe(true)
    expect(prFilesPending({ source: 'local', files: null as any }, false)).toBe(false)
    expect(prCommitsPending({ source: 'local', commits: null as any }, false)).toBe(false)
    expect(prFilesPending({ source: 'local', files: [] }, true)).toBe(false)
    expect(prCommitsPending({ source: 'local', commits: [] }, true)).toBe(false)
  })
})

describe('prCommitsPending / prChecksPending', () => {
  test('commits skeleton only while GetPR is in flight', () => {
    expect(prCommitsPending({ source: 'gh', commits: [] }, true)).toBe(true)
    expect(prCommitsPending({ source: 'gh', commits: [] }, false)).toBe(false)
    expect(prCommitsPending({ source: 'gh', commits: [{ hash: 'abc' }] }, true)).toBe(false)
  })

  test('checks skeleton while rows are still coming', () => {
    expect(prChecksPending({ source: 'gh', checks: [] }, true)).toBe(true)
    expect(prChecksPending({ source: 'gh', hasChecks: true, checks: [] }, false)).toBe(true)
    expect(prChecksPending({ source: 'gh', checks: [] }, false)).toBe(false)
    expect(prChecksPending({ source: 'gh', checks: [{ name: 'ci' }] }, true)).toBe(false)
  })
})

describe('hasPRDescription', () => {
  test('is true only for a description comment', () => {
    expect(hasPRDescription({ comments: [{ kind: 'description' }] })).toBe(true)
    expect(hasPRDescription({ comments: [] })).toBe(false)
  })
})
