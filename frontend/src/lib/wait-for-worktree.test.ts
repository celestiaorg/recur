import { describe, expect, it } from 'vitest'
import type { Workspace } from '../../bindings/recur/internal/domain/models'
import { workspaces } from './stores'
import { waitForWorktree } from './wait-for-worktree'

function ws(id: string, extra: Partial<Workspace> = {}): Workspace {
  return { id, projectId: 'p', ...extra } as Workspace
}

describe('waitForWorktree', () => {
  it('settles when the waited record is dropped and a ready replacement is published', async () => {
    workspaces.set([ws('old', { worktreePath: '' })])
    const p = waitForWorktree('old', new AbortController().signal)
    workspaces.set([ws('new', { worktreePath: '/tmp/wt' })])
    await expect(p).resolves.toBeNull()
  })

  it('waits while a published path is still in phase creating', async () => {
    workspaces.set([ws('WS-NEW', { worktreePath: '/tmp/wt', phase: 'creating' })])
    const p = waitForWorktree('WS-NEW', new AbortController().signal)
    let settled: Workspace | null | undefined
    void p.then((v) => { settled = v })
    await Promise.resolve()
    expect(settled).toBeUndefined()
    workspaces.set([ws('WS-NEW', { worktreePath: '/tmp/wt', phase: 'editing' })])
    await expect(p).resolves.toMatchObject({ id: 'WS-NEW', worktreePath: '/tmp/wt' })
  })

  it('returns null when setup fails after the path is published', async () => {
    workspaces.set([ws('WS-NEW', { worktreePath: '/tmp/wt', phase: 'creating' })])
    const p = waitForWorktree('WS-NEW', new AbortController().signal)
    workspaces.set([ws('WS-NEW', {
      worktreePath: '/tmp/wt',
      phase: 'failed',
      lastError: 'setup command failed',
    })])
    await expect(p).resolves.toBeNull()
  })
})
