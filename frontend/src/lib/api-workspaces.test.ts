import { beforeEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import type { Workspace } from '../../bindings/recur/internal/domain/models'
import { workspaces } from './stores'
import { dropWorkspace, setWorkspacesFromList, upsertWorkspace } from './api'

let n = 0
function ws(id: string, extra: Partial<Workspace> = {}): Workspace {
  return { id, projectId: 'p', ...extra } as Workspace
}
function freshId() {
  return `ws-reuse-${++n}`
}

beforeEach(() => {
  workspaces.set([])
})

describe('dropped workspace ids', () => {
  it('keeps a stale ListWorkspaces snapshot from resurrecting a deleted row', () => {
    const id = freshId()
    upsertWorkspace(ws(id))
    dropWorkspace(id)
    expect(get(workspaces).some((w) => w.id === id)).toBe(false)
    setWorkspacesFromList([ws(id)])
    expect(get(workspaces).some((w) => w.id === id)).toBe(false)
  })

  it('admits a recycled id after a confirmed create', () => {
    const id = freshId()
    upsertWorkspace(ws(id, { short: 'old' }))
    dropWorkspace(id)
    upsertWorkspace(ws(id, { short: 'new' }))
    expect(get(workspaces).find((w) => w.id === id)?.short).toBe('new')
    setWorkspacesFromList([ws(id, { short: 'listed' })])
    expect(get(workspaces).find((w) => w.id === id)?.short).toBe('listed')
  })
})
