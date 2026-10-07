import { beforeEach, describe, expect, test, vi } from 'vitest'
import { get } from 'svelte/store'
import { applyOpenPathResult } from './open-path'
import { observatoryJump, projects, selectedWorkspaceId, view, workspaces } from './stores'
import type { OpenPathResult } from '../../bindings/recur/internal/domain/models'

vi.mock('./api', async () => {
  const actual = await vi.importActual<typeof import('./api')>('./api')
  return {
    ...actual,
    refreshProjects: vi.fn(async () => {}),
  }
})

describe('applyOpenPathResult', () => {
  beforeEach(() => {
    projects.set([])
    workspaces.set([])
    selectedWorkspaceId.set('')
    observatoryJump.set(null)
  })

  test('inserts project+workspace, focuses the workspace, and jumps to the file', () => {
    const res = {
      path: '/tmp/repo/README.md',
      projectId: 'repo',
      workspaceId: 'ws-1',
      file: 'README.md',
      created: true,
      project: { id: 'repo', name: 'repo', repoPath: '/tmp/repo' },
      workspace: { id: 'ws-1', projectId: 'repo', isMain: true, short: 'Working copy' },
    } as OpenPathResult
    applyOpenPathResult(res)
    expect(get(projects).some((p) => p.id === 'repo')).toBe(true)
    expect(get(workspaces).some((w) => w.id === 'ws-1')).toBe(true)
    expect(get(selectedWorkspaceId)).toBe('ws-1')
    expect(get(view)).toBe('detail')
    expect(get(observatoryJump)).toEqual({ path: 'README.md', workspaceId: 'ws-1' })
  })
})
