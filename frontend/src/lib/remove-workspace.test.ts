/** @vitest-environment happy-dom */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'
import type { Workspace } from '../../bindings/recur/internal/domain/models'
import { selectedWorkspaceId, toasts, view, workspaces } from './stores'

const api = vi.hoisted(() => ({
  remove: vi.fn(),
  list: vi.fn(),
  prune: vi.fn(),
  upsert: vi.fn(),
  setFromList: vi.fn(),
}))

vi.mock('./api', () => ({
  WorkspacesService: {
    RemoveWorkspace: (...args: unknown[]) => api.remove(...args),
    ListWorkspaces: (...args: unknown[]) => api.list(...args),
  },
  call: async (p: Promise<unknown>) => {
    try { return await p } catch { return null }
  },
  pruneWorkspaceClientState: (...args: unknown[]) => api.prune(...args),
  upsertWorkspace: (...args: unknown[]) => api.upsert(...args),
  setWorkspacesFromList: (...args: unknown[]) => api.setFromList(...args),
}))

vi.mock('./editor-session', () => ({
  flushEditorSessionNow: vi.fn(),
}))

import { removeWorkspaces } from './remove-workspace'

function ws(id: string): Workspace {
  return { id, archived: false, worktreePath: `/tmp/${id}`, port: 1 } as Workspace
}

beforeEach(() => {
  api.remove.mockReset()
  api.list.mockReset()
  api.prune.mockReset()
  api.upsert.mockReset()
  api.setFromList.mockReset()
  workspaces.set([ws('a'), ws('b'), ws('c')])
  view.set('briefing')
  selectedWorkspaceId.set('')
  toasts.set([])
})

describe('removeWorkspaces', () => {
  it('archives every id before the first RemoveWorkspace RPC returns', async () => {
    let release!: (v: Workspace) => void
    const first = new Promise<Workspace>((r) => { release = r })
    let calls = 0
    api.remove.mockImplementation((id: string) => {
      calls++
      if (calls === 1) return first
      return Promise.resolve({ id, archived: true, worktreePath: '', port: 0 } as Workspace)
    })

    const done = removeWorkspaces(['a', 'b'])
    expect(get(workspaces).filter((w) => w.archived).map((w) => w.id).sort()).toEqual(['a', 'b'])
    expect(get(workspaces).find((w) => w.id === 'c')?.archived).toBe(false)
    expect(calls).toBe(1)

    release({ id: 'a', archived: true, worktreePath: '', port: 0 } as Workspace)
    expect(await done).toBe(2)
  })

  it('keeps queued ids archived when an earlier remove fails and ListWorkspaces restores the snapshot', async () => {
    let releaseB!: (v: Workspace) => void
    const second = new Promise<Workspace>((r) => { releaseB = r })
    api.remove.mockImplementation((id: string) => {
      if (id === 'a') return Promise.reject(new Error('fail'))
      return second
    })
    api.list.mockResolvedValue([ws('a'), ws('b'), ws('c')])
    api.setFromList.mockImplementation((list: Workspace[]) => {
      workspaces.set(list)
    })

    const done = removeWorkspaces(['a', 'b'])
    await vi.waitFor(() => expect(api.remove).toHaveBeenCalledWith('b'))
    expect(get(workspaces).find((w) => w.id === 'a')?.archived).toBe(false)
    expect(get(workspaces).find((w) => w.id === 'b')?.archived).toBe(true)
    expect(get(workspaces).find((w) => w.id === 'c')?.archived).toBe(false)

    releaseB({ id: 'b', archived: true, worktreePath: '', port: 0 } as Workspace)
    expect(await done).toBe(1)
  })
})
