import { describe, expect, test } from 'vitest'
import {
  chatHasStarted,
  defaultModelForMode,
  isModeDefaultSelection,
  pickerAfterModeChange,
  resolvedDefaultForMode,
  wsCreating,
  wsCreateFailed,
  wsChatHeld,
  wsMainBlocked,
  wsSetupStatus,
  wsRunActive,
  workspaceCanRemove,
  workspaceIsRunning,
  workspaceShowsRemove,
  parkedNeedsYouByWorkspace,
  sidebarUnreadTitle,
  type ModeModelSettings,
} from './modes'
import type { CatalogModel } from './models'

const set: ModeModelSettings = {
  modelAsk: 'claude-code/haiku',
  modelPlan: 'claude-code/opus',
  modelAgent: 'claude-code/sonnet',
  modelReview: 'claude-code/fable',
  modelLoop: 'claude-code/opus',
  flagsAsk: { effort: 'low' },
  flagsPlan: { effort: 'high' },
  flagsAgent: { effort: 'medium' },
  flagsReview: { effort: 'high' },
  flagsLoop: { effort: 'high' },
}

const catalog: CatalogModel[] = [
  {
    ref: 'claude-code/opus',
    label: 'Opus',
    group: 'Claude',
    provider: 'claude-code',
    flags: [{ id: 'effort', label: 'Effort', options: [
      { value: 'low', label: 'Low' },
      { value: 'medium', label: 'Medium' },
      { value: 'high', label: 'High', default: true },
    ] }],
  },
  {
    ref: 'claude-code/sonnet',
    label: 'Sonnet',
    group: 'Claude',
    provider: 'claude-code',
    flags: [{ id: 'effort', label: 'Effort', options: [
      { value: 'low', label: 'Low' },
      { value: 'medium', label: 'Medium', default: true },
      { value: 'high', label: 'High' },
    ] }],
  },
  {
    ref: 'claude-code/fable',
    label: 'Fable',
    group: 'Claude',
    provider: 'claude-code',
    flags: [{ id: 'effort', label: 'Effort', options: [
      { value: 'low', label: 'Low' },
      { value: 'medium', label: 'Medium' },
      { value: 'high', label: 'High', default: true },
    ] }],
  },
  {
    ref: 'claude-code/haiku',
    label: 'Haiku',
    group: 'Claude',
    provider: 'claude-code',
    flags: [{ id: 'effort', label: 'Effort', options: [
      { value: 'low', label: 'Low', default: true },
      { value: 'medium', label: 'Medium' },
      { value: 'high', label: 'High' },
    ] }],
  },
]

const cursorCatalog: CatalogModel[] = [
  {
    ref: 'cursor/composer-2.5',
    label: 'Composer 2.5',
    group: 'Cursor',
    provider: 'cursor',
    flags: [{ id: 'fast', label: 'Fast', options: [
      { value: 'true', label: 'On', default: true },
      { value: 'false', label: 'Off' },
    ] }],
  },
]

describe('defaultModelForMode', () => {
  test('maps each chat mode to its own Settings slot', () => {
    expect(defaultModelForMode('ask', set).model).toBe('claude-code/haiku')
    expect(defaultModelForMode('plan', set).model).toBe('claude-code/opus')
    expect(defaultModelForMode('agent', set).model).toBe('claude-code/sonnet')
    expect(defaultModelForMode('review', set).model).toBe('claude-code/fable')
    expect(defaultModelForMode('loop', set).model).toBe('claude-code/opus')
  })
})

describe('resolvedDefaultForMode', () => {
  test('skips a Settings default whose provider is turned off', () => {
    const mixed = [...catalog, ...cursorCatalog]
    const off: ModeModelSettings = { ...set, disabledProviders: ['claude-code'] }
    const got = resolvedDefaultForMode('agent', off, mixed)
    expect(got.model).toBe('cursor/composer-2.5')
    expect(got.flags).toEqual({ fast: 'true' })
  })

  test('uses a remapped Settings slot even when Claude is off', () => {
    const off: ModeModelSettings = {
      ...set,
      modelAgent: 'cursor/composer-2.5',
      flagsAgent: { fast: 'false' },
      disabledProviders: ['claude-code'],
    }
    const got = resolvedDefaultForMode('agent', off, cursorCatalog)
    expect(got.model).toBe('cursor/composer-2.5')
    expect(got.flags).toEqual({ fast: 'false' })
  })
})

describe('pickerAfterModeChange', () => {
  test('unhydrated pickers take the new mode default (not agent fallback)', () => {
    // Mirrors WorkspaceView before loadChatPickers: model falls back to agent,
    // flags are {} — must not look like an explicit override.
    const next = pickerAfterModeChange(
      'agent',
      'review',
      set.modelAgent!,
      {},
      set,
      catalog,
      false,
      false,
    )
    const want = resolvedDefaultForMode('review', set, catalog)
    expect(next).toEqual(want)
  })

  test('hydrated mode-default selection follows the new mode default on an unused chat', () => {
    const from = resolvedDefaultForMode('agent', set, catalog)
    expect(isModeDefaultSelection('agent', from.model, from.flags, set, catalog)).toBe(true)
    const next = pickerAfterModeChange(
      'agent',
      'review',
      from.model,
      from.flags,
      set,
      catalog,
      true,
      false,
    )
    expect(next).toEqual(resolvedDefaultForMode('review', set, catalog))
  })

  test('hydrated mode-default selection is kept once the session has started', () => {
    const from = resolvedDefaultForMode('agent', set, catalog)
    const next = pickerAfterModeChange(
      'agent',
      'ask',
      from.model,
      from.flags,
      set,
      catalog,
      true,
      true,
    )
    expect(next).toEqual({ model: from.model, flags: from.flags })
  })

  test('hydrated explicit pick is kept across modes', () => {
    const next = pickerAfterModeChange(
      'agent',
      'review',
      'claude-code/opus',
      { effort: 'low' },
      set,
      catalog,
      true,
      false,
    )
    expect(next).toEqual({ model: 'claude-code/opus', flags: { effort: 'low' } })
  })

  test('started explicit pick is kept even when pickers are hydrated', () => {
    const next = pickerAfterModeChange(
      'agent',
      'plan',
      'claude-code/opus',
      { effort: 'low' },
      set,
      catalog,
      true,
      true,
    )
    expect(next).toEqual({ model: 'claude-code/opus', flags: { effort: 'low' } })
  })

  test('plan and ask prompt-action targets also get mode defaults when unhydrated', () => {
    expect(
      pickerAfterModeChange('agent', 'plan', set.modelAgent!, {}, set, catalog, false, false),
    ).toEqual(resolvedDefaultForMode('plan', set, catalog))
    expect(
      pickerAfterModeChange('agent', 'ask', set.modelAgent!, {}, set, catalog, false, false),
    ).toEqual(resolvedDefaultForMode('ask', set, catalog))
  })
})

describe('chatHasStarted', () => {
  test('empty unused tabs have not started', () => {
    expect(chatHasStarted(undefined, undefined)).toBe(false)
    expect(chatHasStarted('', [])).toBe(false)
    expect(chatHasStarted('New chat', [])).toBe(false)
    expect(chatHasStarted('New chat', [{ queued: true }])).toBe(false)
  })

  test('a transcript row or a real tab title means the session has started', () => {
    expect(chatHasStarted('New chat', [{ queued: false }])).toBe(true)
    expect(chatHasStarted('Fix the dock', [])).toBe(true)
    expect(chatHasStarted('Image', undefined)).toBe(true)
  })
})

describe('wsSetupStatus', () => {
  test('shows setup while the worktree is allocating', () => {
    expect(wsSetupStatus({ phase: 'creating' })).toBe('Setting up the worktree…')
    expect(wsSetupStatus({ phase: 'creating', worktreePath: '/tmp/wt' })).toBe('Setting up the worktree…')
  })

  test('shows a loop-slot wait even before the worktree exists', () => {
    expect(wsSetupStatus({ phase: 'waiting for a slot' })).toBe('Waiting for a loop slot…')
    expect(wsSetupStatus({ phase: 'waiting for a slot', worktreePath: '/tmp/wt' })).toBe('Waiting for a loop slot…')
    expect(wsSetupStatus({ phase: 'starting', worktreePath: '/tmp/wt' })).toBe('')
  })
})

describe('wsChatHeld', () => {
  test('holds composer send until setup finishes, even after the worktree lands', () => {
    expect(wsChatHeld({ phase: 'creating' })).toBe(true)
    expect(wsChatHeld({ phase: 'creating', worktreePath: '/tmp/wt' })).toBe(true)
    expect(wsChatHeld({ phase: 'editing' })).toBe(true)
    expect(wsChatHeld({ phase: 'editing', worktreePath: '/tmp/wt' })).toBe(false)
  })

  test('does not hold the main working copy or a failed create', () => {
    expect(wsChatHeld({ isMain: true, phase: 'creating' })).toBe(false)
    expect(wsChatHeld({
      phase: 'failed',
      lastError: 'git failed',
      statusLabel: "Couldn't create",
      type: 'solo',
    })).toBe(false)
  })
})

describe('wsMainBlocked', () => {
  test('blocks eager create and slot wait without a worktree', () => {
    expect(wsMainBlocked({ phase: 'creating' })).toBe(true)
    expect(wsMainBlocked({ phase: 'waiting for a slot' })).toBe(true)
    expect(wsMainBlocked({ phase: 'waiting for a slot', worktreePath: '/tmp/wt' })).toBe(false)
  })

  test('unblocks the editor once the worktree lands, even while setup still runs', () => {
    expect(wsMainBlocked({ phase: 'creating', worktreePath: '/tmp/wt' })).toBe(false)
    expect(wsCreating({ phase: 'creating', worktreePath: '/tmp/wt' })).toBe(true)
  })

  test('blocks a failed create with no worktree', () => {
    expect(wsMainBlocked({
      phase: 'failed',
      lastError: 'git failed',
      statusLabel: "Couldn't create",
      type: 'solo',
    })).toBe(true)
    expect(wsCreateFailed({
      phase: 'failed',
      lastError: 'git failed',
      statusLabel: "Couldn't create",
      type: 'solo',
    })).toBe(true)
    expect(wsMainBlocked({ phase: 'editing', lastError: 'later', worktreePath: '/tmp/wt' })).toBe(false)
  })

  test('surfaces setup failure without overlaying a published worktree', () => {
    const w = {
      phase: 'failed',
      lastError: 'setup command failed',
      worktreePath: '/tmp/wt',
      statusLabel: "Couldn't create" as const,
      type: 'solo',
    }
    expect(wsCreateFailed(w)).toBe(true)
    expect(wsMainBlocked(w)).toBe(false)
    expect(wsCreating(w)).toBe(false)
  })

  test('does not treat a failed loop as a failed create', () => {
    const w = {
      phase: 'failed',
      lastError: 'agent died',
      worktreePath: '/tmp/wt',
      statusLabel: 'Failed',
      runStarted: true,
    }
    expect(wsCreateFailed(w)).toBe(false)
    expect(wsMainBlocked(w)).toBe(false)
  })

  test('does not treat a failed loop dispatch as a failed create', () => {
    const w = {
      phase: 'failed',
      lastError: 'dispatch failed',
      worktreePath: '/tmp/wt',
      statusLabel: "Couldn't create" as const,
      runStarted: true,
    }
    expect(wsCreateFailed(w)).toBe(false)
    expect(wsMainBlocked(w)).toBe(false)
    expect(wsCreating(w)).toBe(false)
  })

  test('overlays a loop whose worktree allocation failed', () => {
    const w = {
      phase: 'failed',
      lastError: 'allocate worktree: bad base',
      statusLabel: "Couldn't create" as const,
      runStarted: true,
    }
    expect(wsCreateFailed(w)).toBe(false)
    expect(wsMainBlocked(w)).toBe(true)
    expect(wsCreating(w)).toBe(false)
  })

  test('leaves a ready workspace unblocked', () => {
    expect(wsMainBlocked({ phase: 'editing', worktreePath: '/tmp/wt' })).toBe(false)
    expect(wsMainBlocked(null)).toBe(false)
  })
})

describe('wsRunActive', () => {
  test('in-flight phases own the worktree', () => {
    expect(wsRunActive({ state: 'coding' })).toBe(true)
    expect(wsRunActive({ state: 'queued' })).toBe(true)
  })

  test('escalated and human-paused still own the worktree', () => {
    expect(wsRunActive({ state: 'escalated', phase: 'reviewing iteration 2' })).toBe(true)
    expect(wsRunActive({ state: 'paused', phase: 'paused' })).toBe(true)
  })

  test('walker idle and artifact pauses do not', () => {
    expect(wsRunActive({ state: 'paused', phase: 'idle' })).toBe(false)
    expect(wsRunActive({ state: 'paused', phase: 'artifact' })).toBe(false)
    expect(wsRunActive({ state: 'paused', phase: 'phase missing' })).toBe(false)
    expect(wsRunActive({ state: 'paused', phase: 'loop refs missing' })).toBe(false)
    expect(wsRunActive({ state: 'pr_open' })).toBe(false)
    expect(wsRunActive({ state: 'stopped' })).toBe(false)
  })
})

describe('workspaceIsRunning', () => {
  test('a coding loop pulses with no live specialist', () => {
    expect(workspaceIsRunning({ id: 'child', state: 'coding' }, new Set())).toBe(true)
    expect(workspaceIsRunning({ id: 'parent', state: 'coding', phase: 'board: 2 child workspace(s) running' }, new Set())).toBe(true)
  })

  test('Maestro or any other live agent lights an otherwise idle workspace', () => {
    expect(workspaceIsRunning({ id: 'ws', state: 'paused', phase: 'idle' }, new Set(['ws']))).toBe(true)
    expect(workspaceIsRunning({ id: 'ws', state: 'paused', phase: 'idle' }, new Set(['other']))).toBe(false)
  })

  test('escalated is needs-you, not a running pulse', () => {
    expect(workspaceIsRunning({ id: 'ws', state: 'escalated' }, new Set())).toBe(false)
    expect(workspaceIsRunning({ id: 'ws', state: 'escalated' }, new Set(['ws']))).toBe(true)
  })

  test('human pauses and board-cap exits do not pulse without a live agent', () => {
    expect(workspaceIsRunning({ id: 'ws', state: 'paused', phase: 'paused' }, new Set())).toBe(false)
    expect(workspaceIsRunning({ id: 'ws', state: 'paused', phase: 'board event cap' }, new Set())).toBe(false)
    expect(workspaceIsRunning({ id: 'ws', state: 'paused', phase: 'paused' }, new Set(['ws']))).toBe(true)
    expect(workspaceIsRunning({ id: 'ws', state: 'paused', phase: 'board event cap' }, new Set(['ws']))).toBe(true)
  })
})

describe('parkedNeedsYouByWorkspace', () => {
  test('counts escalated workspaces, parked questions, and permission prompts', () => {
    const n = parkedNeedsYouByWorkspace(
      [{ id: 'ask', state: 'escalated', group: 'needs' }, { id: 'idle', state: 'paused' }],
      [{ workspaceId: 'idle' }, { workspaceId: 'idle' }],
      [{ workspaceId: 'ask' }],
    )
    expect(n.get('ask')).toBe(2)
    expect(n.get('idle')).toBe(2)
  })

  test('counts GroupNeeds without a pending prompt (retryable fail, interrupted recover)', () => {
    const n = parkedNeedsYouByWorkspace([
      { id: 'fail', state: 'failed', group: 'needs' },
      { id: 'cut', state: 'paused', group: 'needs' },
      { id: 'rest', state: 'paused', group: 'active' },
    ])
    expect(n.get('fail')).toBe(1)
    expect(n.get('cut')).toBe(1)
    expect(n.has('rest')).toBe(false)
  })

  test('skips archived workspaces even when a question is still pending', () => {
    expect(parkedNeedsYouByWorkspace(
      [{ id: 'gone', state: 'escalated', archived: true }],
      [{ workspaceId: 'gone' }],
    ).size).toBe(0)
  })

  test('skips a hidden PR chat host even when a prompt is pending', () => {
    const n = parkedNeedsYouByWorkspace(
      [
        { id: 'host', hostPrId: 'pr-1', group: 'needs' },
        { id: 'task', group: 'needs' },
      ],
      [{ workspaceId: 'host' }, { workspaceId: 'task' }],
    )
    expect(n.has('host')).toBe(false)
    expect(n.get('task')).toBe(2)
  })
})

describe('sidebarUnreadTitle', () => {
  test('parked items keep a needs-you label; unseen-only stays a reply count', () => {
    expect(sidebarUnreadTitle(0, 1)).toBe('Needs you')
    expect(sidebarUnreadTitle(2, 1)).toBe('3 waiting')
    expect(sidebarUnreadTitle(2, 0)).toBe('2 unseen agent responses')
  })
})

describe('workspaceShowsRemove', () => {
  test('shows for hand workspaces and loops, including running ones', () => {
    expect(workspaceShowsRemove({ type: 'solo' })).toBe(true)
    expect(workspaceShowsRemove({ runStarted: true, state: 'coding' })).toBe(true)
    expect(workspaceShowsRemove({ state: 'merged' })).toBe(true)
  })

  test('hides for the main checkout, PR hosts, archived, and missing rows', () => {
    expect(workspaceShowsRemove({ isMain: true, type: 'solo' })).toBe(false)
    expect(workspaceShowsRemove({ hostPrId: '482', type: 'solo' })).toBe(false)
    expect(workspaceShowsRemove({ archived: true, type: 'solo' })).toBe(false)
    expect(workspaceShowsRemove(null)).toBe(false)
  })
})

describe('workspaceCanRemove', () => {
  test('allows a hand workspace and a settled loop', () => {
    expect(workspaceCanRemove({ type: 'solo' })).toBe(true)
    expect(workspaceCanRemove({ runStarted: true, state: 'pr_open' })).toBe(true)
    expect(workspaceCanRemove({ state: 'merged' })).toBe(true)
    expect(workspaceCanRemove({ runStarted: true, state: 'paused', phase: 'idle' })).toBe(true)
    expect(workspaceCanRemove({ runStarted: true, state: 'paused', phase: 'artifact' })).toBe(true)
  })

  test('refuses the main checkout, PR hosts, archived, running loops, and live agent turns', () => {
    expect(workspaceCanRemove({ isMain: true, type: 'solo' })).toBe(false)
    expect(workspaceCanRemove({ hostPrId: '482', type: 'solo' })).toBe(false)
    expect(workspaceCanRemove({ archived: true, type: 'solo' })).toBe(false)
    expect(workspaceCanRemove({ runStarted: true, state: 'coding' })).toBe(false)
    expect(workspaceCanRemove({ runStarted: true, state: 'paused', phase: 'paused' })).toBe(false)
    expect(workspaceCanRemove({ runStarted: true, state: 'paused', phase: 'interrupted' })).toBe(false)
    expect(workspaceCanRemove({ type: 'solo' }, true)).toBe(false)
    expect(workspaceCanRemove({ runStarted: true, state: 'paused', phase: 'idle' }, true)).toBe(false)
    expect(workspaceCanRemove(null)).toBe(false)
  })
})
