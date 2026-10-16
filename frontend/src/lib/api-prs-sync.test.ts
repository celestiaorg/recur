import { beforeEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import type { PR } from '../../bindings/recur/internal/domain/models'
import { prs } from './stores'
import { mergeProjectPRsSync } from './api'

function pr(id: string, extra: Partial<PR> = {}): PR {
  return { id, projectId: 'p', ...extra } as PR
}

beforeEach(() => {
  prs.set([])
})

describe('mergeProjectPRsSync', () => {
  it('patches list fields, keeps heavy fields, appends new rows, leaves others alone', () => {
    const files = [{ path: 'a.go' }] as PR['files']
    const commits = [{ hash: 'abc' }] as PR['commits']
    prs.set([pr('a', { title: 'old', files, commits }), pr('b', { title: 'other' })])
    mergeProjectPRsSync([
      pr('a', { title: 'new', files: null as any, commits: null as any, comments: null as any }),
      pr('c', { title: 'fresh' }),
    ])
    const got = get(prs)
    expect(got.map((p) => p.id)).toEqual(['a', 'b', 'c'])
    expect(got[0].title).toBe('new')
    expect(got[0].files).toBe(files)
    expect(got[0].commits).toBe(commits)
    expect(got[1].title).toBe('other')
  })
})
