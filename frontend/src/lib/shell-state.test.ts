import { describe, expect, test } from 'vitest'
import {
  applyShellRestore,
  resolveShellRestore,
  snapshotShell,
  type ShellRestore,
} from './shell-state'

const live = {
  workspaceIds: new Set(['WS-1', 'WS-2']),
  prIds: new Set(['pr-9']),
  projectIds: new Set(['storefront']),
}

describe('snapshotShell', () => {
  test('keeps only the ids that belong to the view', () => {
    expect(snapshotShell('detail', 'WS-1', 'pr-9', 'storefront')).toEqual({
      view: 'detail',
      workspaceId: 'WS-1',
    })
    expect(snapshotShell('inbox', 'WS-1', 'pr-9', 'storefront')).toEqual({ view: 'inbox' })
    expect(snapshotShell('schedules', 'WS-1', 'pr-9', 'storefront')).toEqual({ view: 'schedules' })
    expect(snapshotShell('briefing', 'WS-1', 'pr-9', 'storefront')).toEqual({ view: 'briefing' })
    expect(snapshotShell('prdetail', 'WS-1', 'pr-9', 'storefront')).toEqual({
      view: 'prdetail',
      prId: 'pr-9',
    })
    expect(snapshotShell('project', 'WS-1', 'pr-9', 'storefront')).toEqual({
      view: 'project',
      projectId: 'storefront',
    })
  })
})

describe('resolveShellRestore', () => {
  test('empty or unknown saved state stays on the default', () => {
    expect(resolveShellRestore(null, live)).toEqual({ action: 'none' })
    expect(resolveShellRestore({}, live)).toEqual({ action: 'none' })
    expect(resolveShellRestore({ view: 'nope' }, live)).toEqual({ action: 'none' })
  })

  test('restores a live workspace (detail or escalation)', () => {
    expect(resolveShellRestore({ view: 'detail', workspaceId: 'WS-1' }, live)).toEqual({
      action: 'workspace',
      id: 'WS-1',
    })
    expect(resolveShellRestore({ view: 'escalation', workspaceId: 'WS-2' }, live)).toEqual({
      action: 'workspace',
      id: 'WS-2',
    })
  })

  test('drops a workspace that is gone or archived', () => {
    expect(resolveShellRestore({ view: 'detail', workspaceId: 'gone' }, live)).toEqual({
      action: 'none',
    })
    expect(resolveShellRestore({ view: 'detail' }, live)).toEqual({ action: 'none' })
  })

  test('restores a PR, or the PR list when that PR is gone', () => {
    expect(resolveShellRestore({ view: 'prdetail', prId: 'pr-9' }, live)).toEqual({
      action: 'pr',
      id: 'pr-9',
    })
    expect(resolveShellRestore({ view: 'prdetail', prId: 'gone' }, live)).toEqual({
      action: 'view',
      view: 'pr',
    })
  })

  test('restores inbox / briefing / settings / pr list / project; welcome is first-run only', () => {
    expect(resolveShellRestore({ view: 'inbox' }, live)).toEqual({ action: 'view', view: 'inbox' })
    expect(resolveShellRestore({ view: 'briefing' }, live)).toEqual({ action: 'view', view: 'briefing' })
    expect(resolveShellRestore({ view: 'settings' }, live)).toEqual({ action: 'view', view: 'settings' })
    expect(resolveShellRestore({ view: 'pr' }, live)).toEqual({ action: 'view', view: 'pr' })
    expect(resolveShellRestore({ view: 'schedules' }, live)).toEqual({ action: 'view', view: 'schedules' })
    expect(resolveShellRestore({ view: 'issues' }, live)).toEqual({ action: 'view', view: 'issues' })
    expect(resolveShellRestore({ view: 'welcome' }, live)).toEqual({ action: 'none' })
    expect(resolveShellRestore({ view: 'project', projectId: 'storefront' }, live)).toEqual({
      action: 'project',
      id: 'storefront',
    })
    expect(resolveShellRestore({ view: 'project', projectId: 'gone' }, live)).toEqual({
      action: 'none',
    })
  })
})

describe('applyShellRestore', () => {
  test('dispatches to the matching nav helper', () => {
    const calls: string[] = []
    const nav = {
      openWorkspace: (id: string) => calls.push(`ws:${id}`),
      openPR: (id: string) => calls.push(`pr:${id}`),
      openProject: (id: string) => calls.push(`proj:${id}`),
      go: (v: string) => calls.push(`go:${v}`),
    }
    const run = (r: ShellRestore) => {
      calls.length = 0
      applyShellRestore(r, nav)
      return [...calls]
    }
    expect(run({ action: 'none' })).toEqual([])
    expect(run({ action: 'workspace', id: 'WS-1' })).toEqual(['ws:WS-1'])
    expect(run({ action: 'pr', id: 'pr-9' })).toEqual(['pr:pr-9'])
    expect(run({ action: 'project', id: 'storefront' })).toEqual(['proj:storefront'])
    expect(run({ action: 'view', view: 'settings' })).toEqual(['go:settings'])
  })
})
