/** @vitest-environment happy-dom */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { get } from 'svelte/store'
import type { Project, Workspace } from '../../bindings/recur/internal/domain/models'
import {
  commandsOpen,
  newTaskOpen,
  openInOpen,
  paletteOpen,
  projectFormId,
  projects,
  selectedWorkspaceId,
  shortcutsOpen,
  toasts,
  view,
  workspaces,
  workspaceSeeds,
  takeWorkspaceSeed,
} from './stores'

const api = vi.hoisted(() => ({
  create: vi.fn(),
  upsert: vi.fn(),
  listBranchesForPath: vi.fn(),
  refreshProjects: vi.fn(),
  listChats: vi.fn(),
  remember: vi.fn(),
  send: vi.fn(),
}))

vi.mock('./api', () => ({
  WorkspacesService: {
    CreateWorkspace: (...args: unknown[]) => api.create(...args),
    ListWorkspaceChats: (...args: unknown[]) => api.listChats(...args),
    RememberWorkspaceChatSelection: (...args: unknown[]) => api.remember(...args),
    SendWorkspaceMessage: (...args: unknown[]) => api.send(...args),
  },
  ProjectsService: {
    ListBranchesForPath: (...args: unknown[]) => api.listBranchesForPath(...args),
  },
  upsertWorkspace: (...args: unknown[]) => api.upsert(...args),
  refreshProjects: () => api.refreshProjects(),
  call: async (p: Promise<unknown>) => {
    try { return await p } catch { return null }
  },
  bridgeErrorMessage: (err: unknown) => err instanceof Error ? err.message : String(err ?? ''),
}))

import { createBlankWorkspace, eagerCreateInput, endEagerCreate, openingChatSeed, openingSeedText, revealCreatedWorkspace, startEagerCreate, startParkedOpening, takeEagerWorkspace } from './new-workspace'
import { attachmentMarker } from './agentchat'

function gitProject(id: string, extra: Partial<Project> = {}): Project {
  return {
    id,
    name: id,
    pic: id[0]!.toUpperCase(),
    color: 'var(--cyan)',
    open: true,
    idle: false,
    repoPath: `~/code/${id}`,
    baseBranch: 'main',
    setupCmd: '',
    teardownCmd: '',
    appURL: '',
    prOnGithub: false,
    isGit: true,
    ...extra,
  } as Project
}

function ws(id: string, projectId: string): Workspace {
  return { id, projectId } as Workspace
}

beforeEach(() => {
  api.create.mockReset()
  api.upsert.mockReset()
  api.listBranchesForPath.mockReset()
  api.refreshProjects.mockReset()
  api.listChats.mockReset()
  api.remember.mockReset()
  api.send.mockReset()
  api.create.mockResolvedValue({ id: 'WS-NEW', projectId: 'storefront' })
  api.listChats.mockResolvedValue([])
  api.remember.mockResolvedValue(undefined)
  api.send.mockResolvedValue('')
  api.refreshProjects.mockResolvedValue(undefined)
  toasts.set([])
  api.listBranchesForPath.mockResolvedValue([
    { name: 'main', group: 'Local' },
    { name: 'develop', group: 'Local' },
  ])
  projects.set([])
  workspaces.set([])
  selectedWorkspaceId.set('')
  projectFormId.set('')
  view.set('briefing')
  newTaskOpen.set(false)
  paletteOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  commandsOpen.set(false)
  workspaceSeeds.set({})
  localStorage.setItem('recur.newWorkspaceMode', 'plan')
})

describe('createBlankWorkspace', () => {
  it('opens the create modal when there is no git project', async () => {
    projects.set([gitProject('notes', { isGit: false })])
    commandsOpen.set(true)
    await createBlankWorkspace()
    expect(api.create).not.toHaveBeenCalled()
    expect(get(newTaskOpen)).toBe(true)
    expect(get(commandsOpen)).toBe(false)
    expect(api.refreshProjects).toHaveBeenCalled()
  })

  it('creates an Agent workspace with an empty prompt in the first git project', async () => {
    projects.set([gitProject('storefront'), gitProject('payments', { baseBranch: 'develop' })])
    await createBlankWorkspace()
    expect(api.create).toHaveBeenCalledWith('storefront', '', 'main', 'agent', '', '', null)
    expect(api.upsert).toHaveBeenCalledWith({ id: 'WS-NEW', projectId: 'storefront' })
    expect(get(workspaceSeeds)['WS-NEW']).toEqual({ id: 'WS-NEW', text: '', mode: 'agent' })
    expect(get(selectedWorkspaceId)).toBe('')
    expect(get(view)).toBe('briefing')
    expect(get(newTaskOpen)).toBe(false)
  })

  it('stays on the current workspace when creating from the workbench', async () => {
    projects.set([gitProject('storefront'), gitProject('payments', { baseBranch: 'develop' })])
    workspaces.set([ws('ws-pay', 'payments')])
    selectedWorkspaceId.set('ws-pay')
    view.set('detail')
    await createBlankWorkspace()
    expect(api.create).toHaveBeenCalled()
    expect(get(workspaceSeeds)['WS-NEW']).toEqual({ id: 'WS-NEW', text: '', mode: 'agent' })
    expect(get(selectedWorkspaceId)).toBe('ws-pay')
    expect(get(view)).toBe('detail')
  })

  it('stays on a PR when creating from PR detail', async () => {
    projects.set([gitProject('storefront')])
    workspaces.set([ws('ws-cur', 'storefront')])
    selectedWorkspaceId.set('ws-cur')
    view.set('prdetail')
    await createBlankWorkspace()
    expect(get(selectedWorkspaceId)).toBe('ws-cur')
    expect(get(view)).toBe('prdetail')
  })

  it('targets the selected workspace\'s git project', async () => {
    projects.set([gitProject('storefront'), gitProject('payments', { baseBranch: 'develop' })])
    workspaces.set([ws('ws-pay', 'payments')])
    selectedWorkspaceId.set('ws-pay')
    await createBlankWorkspace()
    expect(api.create).toHaveBeenCalledWith('payments', '', 'develop', 'agent', '', '', null)
  })

  it('opens the modal instead of creating in another repo when the focused project is not git', async () => {
    projects.set([
      gitProject('notes', { isGit: false }),
      gitProject('storefront'),
    ])
    workspaces.set([ws('ws-notes', 'notes')])
    selectedWorkspaceId.set('ws-notes')
    await createBlankWorkspace()
    expect(api.create).not.toHaveBeenCalled()
    expect(get(newTaskOpen)).toBe(true)
  })

  it('uses the first listed branch when the configured base is gone', async () => {
    projects.set([gitProject('storefront', { baseBranch: 'master' })])
    // ListBranchesForPath does not inject a missing preferred (foldMissing=false).
    api.listBranchesForPath.mockResolvedValue([
      { name: 'main', group: 'Local' },
      { name: 'feature/auth', group: 'Local' },
    ])
    await createBlankWorkspace()
    expect(api.listBranchesForPath).toHaveBeenCalledWith('~/code/storefront', 'master')
    expect(api.create).toHaveBeenCalledWith('storefront', '', 'main', 'agent', '', '', null)
  })

  it('trims a padded baseBranch so it still matches a live tip', async () => {
    projects.set([gitProject('storefront', { baseBranch: '  main  ' })])
    api.listBranchesForPath.mockResolvedValue([{ name: 'main', group: 'Local' }])
    await createBlankWorkspace()
    expect(api.listBranchesForPath).toHaveBeenCalledWith('~/code/storefront', 'main')
    expect(api.create).toHaveBeenCalledWith('storefront', '', 'main', 'agent', '', '', null)
  })

  it('keeps the configured base when the repo has no tips yet', async () => {
    projects.set([gitProject('storefront', { baseBranch: 'main' })])
    api.listBranchesForPath.mockResolvedValue([])
    await createBlankWorkspace()
    expect(api.create).toHaveBeenCalledWith('storefront', '', 'main', 'agent', '', '', null)
  })

  it('keeps the configured base when ListBranchesForPath fails', async () => {
    projects.set([gitProject('storefront', { baseBranch: 'develop' })])
    api.listBranchesForPath.mockRejectedValue(new Error('offline'))
    await createBlankWorkspace()
    expect(api.create).toHaveBeenCalledWith('storefront', '', 'develop', 'agent', '', '', null)
  })

  it('ignores the create-modal\'s last-used mode', async () => {
    projects.set([gitProject('storefront')])
    localStorage.setItem('recur.newWorkspaceMode', 'loop')
    await createBlankWorkspace()
    expect(api.create.mock.calls[0][3]).toBe('agent')
    expect(get(workspaceSeeds)['WS-NEW']?.mode).toBe('agent')
  })

  it('does not create when the backend rejects', async () => {
    projects.set([gitProject('storefront')])
    api.create.mockRejectedValue(new Error('nope'))
    await createBlankWorkspace()
    expect(api.upsert).not.toHaveBeenCalled()
    expect(get(selectedWorkspaceId)).toBe('')
    expect(get(workspaceSeeds)['WS-NEW']).toBeUndefined()
  })
})

describe('revealCreatedWorkspace', () => {
  it('opens from welcome', () => {
    view.set('welcome')
    revealCreatedWorkspace({ id: 'WS-NEW', text: '', mode: 'agent' })
    expect(get(selectedWorkspaceId)).toBe('WS-NEW')
    expect(get(view)).toBe('detail')
    expect(get(workspaceSeeds)['WS-NEW']).toEqual({ id: 'WS-NEW', text: '', mode: 'agent' })
    expect(api.send).not.toHaveBeenCalled()
  })

  it('stays on inbox', () => {
    view.set('briefing')
    revealCreatedWorkspace({ id: 'WS-NEW', text: '', mode: 'agent' })
    expect(get(selectedWorkspaceId)).toBe('')
    expect(get(view)).toBe('briefing')
    expect(api.send).not.toHaveBeenCalled()
  })

  it('stays on settings', () => {
    view.set('settings')
    revealCreatedWorkspace({ id: 'WS-NEW', text: '', mode: 'agent' })
    expect(get(selectedWorkspaceId)).toBe('')
    expect(get(view)).toBe('settings')
  })

  it('parks an opening prompt from the workbench', async () => {
    workspaces.set([
      ws('ws-cur', 'storefront'),
      { id: 'WS-NEW', projectId: 'storefront', worktreePath: '/tmp/WS-NEW' } as Workspace,
    ])
    selectedWorkspaceId.set('ws-cur')
    view.set('detail')
    await revealCreatedWorkspace({ id: 'WS-NEW', text: 'fix the flaky test', mode: 'agent', model: 'sonnet' })
    expect(get(selectedWorkspaceId)).toBe('ws-cur')
    expect(get(view)).toBe('detail')
    expect(get(workspaceSeeds)['WS-NEW']).toBeUndefined()
    expect(api.remember).toHaveBeenCalledWith('WS-NEW', 'PA', 'sonnet', {}, 'agent', null)
    expect(api.send).toHaveBeenCalled()
  })

  it('parks an opening prompt from PR detail', async () => {
    workspaces.set([
      ws('ws-cur', 'storefront'),
      { id: 'WS-NEW', projectId: 'storefront', worktreePath: '/tmp/WS-NEW' } as Workspace,
    ])
    selectedWorkspaceId.set('ws-cur')
    view.set('prdetail')
    await revealCreatedWorkspace({ id: 'WS-NEW', text: 'review the auth flow', mode: 'agent' })
    expect(get(selectedWorkspaceId)).toBe('ws-cur')
    expect(get(view)).toBe('prdetail')
    expect(get(workspaceSeeds)['WS-NEW']).toBeUndefined()
    expect(api.send).toHaveBeenCalled()
  })

  it('does not switch for a loop started from the workbench', () => {
    workspaces.set([ws('ws-cur', 'storefront')])
    selectedWorkspaceId.set('ws-cur')
    view.set('detail')
    revealCreatedWorkspace({ id: 'WS-NEW', text: 'keep shipping', mode: 'loop' }, { loop: true })
    expect(get(selectedWorkspaceId)).toBe('ws-cur')
    expect(get(view)).toBe('detail')
    expect(api.send).not.toHaveBeenCalled()
  })
})

describe('startParkedOpening', () => {
  function parkedWs(extra: Partial<Workspace> = {}): Workspace {
    return { id: 'WS-NEW', projectId: 'storefront', worktreePath: '/tmp/WS-NEW', ...extra } as Workspace
  }

  it('does not send when the worktree never lands', async () => {
    workspaces.set([{ id: 'WS-NEW', projectId: 'storefront', phase: 'failed', lastError: 'nope' } as Workspace])
    await startParkedOpening({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
    expect(api.send).not.toHaveBeenCalled()
    expect(get(workspaceSeeds)['WS-NEW']).toEqual({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
  })

  it('does not send while setup is still running after the path lands', async () => {
    workspaces.set([parkedWs({ phase: 'creating' })])
    const p = startParkedOpening({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
    await vi.waitFor(() => expect(api.remember).toHaveBeenCalled())
    expect(api.send).not.toHaveBeenCalled()
    expect(get(workspaceSeeds)['WS-NEW']).toEqual({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
    workspaces.set([parkedWs({ phase: 'editing' })])
    await p
    expect(api.send).toHaveBeenCalled()
  })

  it('skips send when the workspace view claims the pending opening', async () => {
    workspaces.set([parkedWs({ phase: 'creating' })])
    const p = startParkedOpening({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
    await vi.waitFor(() => expect(get(workspaceSeeds)['WS-NEW']).toBeTruthy())
    expect(takeWorkspaceSeed('WS-NEW')).toEqual({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
    workspaces.set([parkedWs({ phase: 'editing' })])
    await p
    expect(api.send).not.toHaveBeenCalled()
  })

  it('persists simplify on PA before sending', async () => {
    workspaces.set([parkedWs()])
    await startParkedOpening({
      id: 'WS-NEW', text: 'fix it', mode: 'agent', model: 'sonnet', simplify: false,
    })
    expect(api.listChats).toHaveBeenCalledWith('WS-NEW')
    expect(api.remember).toHaveBeenCalledWith('WS-NEW', 'PA', 'sonnet', {}, 'agent', false)
    expect(api.send).toHaveBeenCalled()
  })

  it('keeps the opening seed when send returns a pre-turn failure', async () => {
    workspaces.set([parkedWs()])
    api.send.mockResolvedValue("I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP.")
    const seed = { id: 'WS-NEW', text: 'fix it', mode: 'agent' }
    await startParkedOpening(seed)
    expect(get(workspaceSeeds)['WS-NEW']).toEqual(seed)
    expect(get(toasts).some((t) => t.kind === 'error' && t.text.includes("couldn't save"))).toBe(true)
  })

  it('does not restore the seed after a successful nonempty reply', async () => {
    workspaces.set([parkedWs()])
    api.send.mockResolvedValue("I'll look at the flaky test.")
    await startParkedOpening({ id: 'WS-NEW', text: 'fix it', mode: 'agent' })
    expect(get(workspaceSeeds)['WS-NEW']).toBeUndefined()
    expect(get(toasts).some((t) => t.kind === 'error')).toBe(false)
  })

  it('keeps the opening seed when send rejects', async () => {
    workspaces.set([parkedWs()])
    api.send.mockRejectedValue(new Error('bridge down'))
    const seed = { id: 'WS-NEW', text: 'fix it', mode: 'agent' }
    await startParkedOpening(seed)
    expect(get(workspaceSeeds)['WS-NEW']).toEqual(seed)
    expect(get(toasts).some((t) => t.kind === 'error' && t.text === 'bridge down')).toBe(true)
  })

  it('keeps failed openings per workspace when a later create parks another seed', async () => {
    workspaces.set([
      parkedWs(),
      { id: 'WS-B', projectId: 'storefront', worktreePath: '/tmp/WS-B' } as Workspace,
    ])
    api.send.mockResolvedValue("I'm at capacity — try again in a moment.")
    const a = { id: 'WS-NEW', text: 'fix it', mode: 'agent' }
    await startParkedOpening(a)
    api.send.mockReset()
    api.send.mockResolvedValue('')
    await startParkedOpening({ id: 'WS-B', text: '', mode: 'agent' })
    expect(get(workspaceSeeds)['WS-NEW']).toEqual(a)
    expect(get(workspaceSeeds)['WS-B']).toEqual({ id: 'WS-B', text: '', mode: 'agent' })
  })

  it('keeps two rejected background openings', async () => {
    workspaces.set([
      parkedWs(),
      { id: 'WS-B', projectId: 'storefront', worktreePath: '/tmp/WS-B' } as Workspace,
    ])
    api.send.mockResolvedValue("I'm at capacity — try again in a moment.")
    const a = { id: 'WS-NEW', text: 'fix A', mode: 'agent' }
    const b = { id: 'WS-B', text: 'fix B', mode: 'agent' }
    await startParkedOpening(a)
    await startParkedOpening(b)
    expect(get(workspaceSeeds)['WS-NEW']).toEqual(a)
    expect(get(workspaceSeeds)['WS-B']).toEqual(b)
  })
})

describe('openingChatSeed', () => {
  const wire = 'Work on Linear issue ENG-1 in this workspace.\n\n## Description\nFull prose'
  const label = 'Linear ENG-1 · Fix the thing'

  it('keeps the ⌘K draft label instead of the wire', () => {
    expect(openingChatSeed(label, { wire, display: label })).toEqual({ text: wire, display: label })
  })

  it('uses ExpandGoal display when pasting a Linear URL', () => {
    expect(openingChatSeed('https://linear.app/acme/issue/ENG-1', { wire: '', display: '' }, {
      prompt: wire,
      display: label,
    })).toEqual({ text: wire, display: label })
  })

  it('leaves a plain goal unchanged', () => {
    expect(openingChatSeed('add dark mode', { wire: '', display: '' }, { prompt: 'add dark mode', display: '' }))
      .toEqual({ text: 'add dark mode', display: '' })
  })
})

describe('openingSeedText', () => {
  const a = attachmentMarker('imgA')
  const b = attachmentMarker('imgB')
  const drafts = [
    { id: 'imgA', name: 'a.png', mime: 'image/png', dataUrl: '' },
    { id: 'imgB', name: 'b.png', mime: 'image/png', dataUrl: '' },
  ]

  it('keeps inline marker layout when expansion did not rewrite the prompt', () => {
    const marked = `Before ${a}, after ${b}`
    expect(openingSeedText(marked, 'Before , after', drafts)).toBe(marked)
  })

  it('appends drafts when expansion replaced the prompt', () => {
    const marked = `https://linear.app/acme/issue/ENG-1 ${a}`
    const wire = 'Work on Linear issue ENG-1'
    expect(openingSeedText(marked, wire, drafts)).toBe(`${wire}\n${a}\n${b}`)
  })
})

describe('eagerCreateInput', () => {
  it('starts as soon as repo and base are known', () => {
    expect(eagerCreateInput(true, false, 'p1', 'main', false)).toEqual({ projectId: 'p1', branch: 'main' })
  })

  it('skips loop mode, a closed modal, a missing repo/base, and a claimed submit', () => {
    expect(eagerCreateInput(false, false, 'p1', 'main', false)).toBeNull()
    expect(eagerCreateInput(true, true, 'p1', 'main', false)).toBeNull()
    expect(eagerCreateInput(true, false, '', 'main', false)).toBeNull()
    expect(eagerCreateInput(true, false, 'p1', '', false)).toBeNull()
    expect(eagerCreateInput(true, false, 'p1', 'main', true)).toBeNull()
  })
})

describe('startEagerCreate', () => {
  it('abandons a workspace that resolves after cancel', async () => {
    let resolveWs!: (ws: Workspace) => void
    const create = () => new Promise<Workspace | null>((r) => { resolveWs = r })
    const ready: string[] = []
    const abandoned: string[] = []
    const handle = startEagerCreate(create, (ws) => ready.push(ws.id), (id) => abandoned.push(id))
    handle.cancel()
    resolveWs({ id: 'WS-1', projectId: 'p' } as Workspace)
    await expect(handle.promise).resolves.toBeNull()
    expect(ready).toEqual([])
    expect(abandoned).toEqual(['WS-1'])
  })

  it('abandons a workspace that was already painted', async () => {
    const create = () => Promise.resolve({ id: 'WS-1', projectId: 'p' } as Workspace)
    const ready: string[] = []
    const abandoned: string[] = []
    const handle = startEagerCreate(create, (ws) => ready.push(ws.id), (id) => abandoned.push(id))
    await expect(handle.promise).resolves.toMatchObject({ id: 'WS-1' })
    expect(ready).toEqual(['WS-1'])
    handle.cancel()
    expect(abandoned).toEqual(['WS-1'])
  })

  it('after a claimed create, submit waits for the next delayed workspace', async () => {
    const rows: Workspace[] = []
    const abandoned: string[] = []
    let pendingId = ''

    let resolveFirst!: (ws: Workspace | null) => void
    const first = startEagerCreate(
      () => new Promise<Workspace | null>((r) => { resolveFirst = r }),
      (ws) => { pendingId = ws.id; rows.push(ws) },
      (id) => abandoned.push(id),
    )
    resolveFirst({ id: 'WS-FIRST', projectId: 'p' } as Workspace)
    await expect(first.promise).resolves.toMatchObject({ id: 'WS-FIRST' })

    const closed = endEagerCreate(true, first.cancel)
    pendingId = closed.pendingId
    let pendingPromise: Promise<Workspace | null> | null = closed.pendingPromise
    expect(abandoned).toEqual([])
    expect(pendingId).toBe('')

    let resolveSecond!: (ws: Workspace | null) => void
    const second = startEagerCreate(
      () => new Promise<Workspace | null>((r) => { resolveSecond = r }),
      (ws) => { pendingId = ws.id; rows.push(ws) },
      (id) => abandoned.push(id),
    )
    pendingPromise = second.promise

    const taken = takeEagerWorkspace(pendingId, pendingPromise, rows)
    resolveSecond({ id: 'WS-SECOND', projectId: 'q' } as Workspace)
    await expect(taken).resolves.toMatchObject({ id: 'WS-SECOND' })
    expect(abandoned).toEqual([])
  })

  it('unclaimed cleanup abandons the pending workspace', () => {
    const cancel = vi.fn()
    expect(endEagerCreate(false, cancel)).toEqual({ pendingId: '', pendingPromise: null })
    expect(cancel).toHaveBeenCalledTimes(1)
  })
})
