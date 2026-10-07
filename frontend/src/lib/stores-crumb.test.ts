import { afterEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import type { Workspace } from '../../bindings/recur/internal/domain/models'
import { crumb, go, selectedWorkspaceId, view, workspaces } from './stores'

function ws(partial: Partial<Workspace> & Pick<Workspace, 'id'>): Workspace {
  return { projectId: 'storefront', ...partial } as Workspace
}

describe('workspace titlebar crumb', () => {
  afterEach(() => {
    workspaces.set([])
    selectedWorkspaceId.set('')
    go('briefing')
  })

  it('uses name and branch for a running loop, not the id', () => {
    workspaces.set([ws({
      id: 'LOOP-2229',
      short: 'Migrate analytics to PG16',
      branch: 'recur/pg16',
      type: 'loop',
      runStarted: true,
    })])
    selectedWorkspaceId.set('LOOP-2229')
    go('detail')
    expect(get(view)).toBe('detail')
    expect(get(crumb)).toContain('Migrate analytics to PG16')
    expect(get(crumb)).toContain('crumb-branch')
    expect(get(crumb)).toContain('recur/pg16')
    expect(get(crumb)).not.toContain('LOOP-2229')
  })
})
