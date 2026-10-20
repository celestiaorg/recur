import { beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'
import type { Workspace } from '../../bindings/recur/internal/domain/models'

vi.mock('../../bindings/recur/internal/app/workspacesservice', async (orig) => ({
  ...(await orig<object>()),
  SetWorkspaceIssue: (id: string) => Promise.resolve({ id }),
  ClearWorkspaceIssue: (id: string) => Promise.resolve({ id }),
}))

const { workspaces } = await import('./stores')
const { linkWorkspaceIssue, unlinkWorkspaceIssue } = await import('./api')

const issue = (id: string) => get(workspaces).find((w) => w.id === id)?.issue

beforeEach(() => {
  workspaces.set([
    { id: 'a', issue: { source: 'linear', id: 'L1', key: 'ENG-1' } },
    { id: 'b', issue: { source: 'linear', id: 'L2', key: 'ENG-2' } },
    { id: 'c' },
  ] as Workspace[])
})

describe('linkWorkspaceIssue', () => {
  it('moves the issue off its previous workspace and leaves others alone', async () => {
    await linkWorkspaceIssue('c', { source: 'linear', id: 'L1', key: 'ENG-1' })
    expect(issue('a')).toBeUndefined()
    expect(issue('b')?.id).toBe('L2')
    expect(issue('c')).toEqual({ source: 'linear', id: 'L1', key: 'ENG-1' })
  })
})

describe('unlinkWorkspaceIssue', () => {
  it('clears only the target workspace', async () => {
    await unlinkWorkspaceIssue('a')
    expect(issue('a')).toBeUndefined()
    expect(issue('b')?.id).toBe('L2')
  })
})
