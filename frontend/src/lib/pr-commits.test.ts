import { describe, expect, test } from 'vitest'
import { commitKeysMatch, keepCommitFiles } from './pr-commits'
import type { Commit } from '../../bindings/recur/internal/domain/models'

function commit(over: Partial<Commit>): Commit {
  return {
    hash: '',
    message: '',
    authorRole: '',
    agentCode: '',
    color: '',
    iterLabel: '',
    diff: { add: 0, del: 0 },
    when: '',
    files: [],
    ...over,
  }
}

describe('keepCommitFiles', () => {
  test('keeps previously loaded diffs when the snapshot is still empty', () => {
    const prev = [commit({ hash: 'abc1234', sha: 'abc1234ffff', files: [{ path: 'a.ts', status: 'm', diff: { add: 1, del: 0 }, lines: [] }] })]
    const next = [commit({ hash: 'abc1234', sha: 'abc1234ffff', message: 'fresh subject', files: [] })]
    const got = keepCommitFiles(prev, next)
    expect(got[0].message).toBe('fresh subject')
    expect(got[0].files).toHaveLength(1)
    expect(got[0].files[0].path).toBe('a.ts')
  })

  test('does not overwrite a snapshot that already has files', () => {
    const prev = [commit({ hash: 'abc1234', files: [{ path: 'old.ts', status: 'm', diff: { add: 1, del: 0 }, lines: [] }] })]
    const next = [commit({ hash: 'abc1234', files: [{ path: 'new.ts', status: 'm', diff: { add: 2, del: 0 }, lines: [] }] })]
    expect(keepCommitFiles(prev, next)[0].files[0].path).toBe('new.ts')
  })
})

describe('commitKeysMatch', () => {
  test('matches short hash against full sha', () => {
    expect(commitKeysMatch(
      commit({ hash: 'abc1234', sha: 'abc1234ffff' }),
      commit({ hash: 'abc1234' }),
    )).toBe(true)
  })
})
