import { describe, expect, test } from 'vitest'
import { prAgentIsRunning, prAgentWorkspaceId, prChatHostRunningCount, prChatHostUnseenCount, prEditableWorkspaceId, prIdForChatWorkspace, prUnseenCount } from './pr-workspace'

const host = { id: 'ws-host', hostPrId: 'pr-1' }
const linked = { id: 'ws-linked' }
const otherHost = { id: 'ws-host-2', hostPrId: 'pr-2' }

describe('prAgentWorkspaceId', () => {
  test('prefers a live chat host over the linked workspace', () => {
    expect(prAgentWorkspaceId({ id: 'pr-1', workspaceId: 'ws-linked' }, [host, linked])).toBe('ws-host')
  })

  test('falls back to the chat host when the PR has no workspace', () => {
    expect(prAgentWorkspaceId({ id: 'pr-1', workspaceId: '' }, [host])).toBe('ws-host')
  })

  test('skips an archived chat host', () => {
    expect(
      prAgentWorkspaceId({ id: 'pr-1', workspaceId: '' }, [{ ...host, archived: true }]),
    ).toBe('')
  })

  test('falls back to a live host when the linked workspace is archived', () => {
    expect(
      prAgentWorkspaceId(
        { id: 'pr-1', workspaceId: 'ws-linked' },
        [host, { ...linked, archived: true }],
      ),
    ).toBe('ws-host')
  })

  test('uses the linked workspace once the chat host is archived', () => {
    expect(
      prAgentWorkspaceId(
        { id: 'pr-1', workspaceId: 'ws-linked' },
        [{ ...host, archived: true }, linked],
      ),
    ).toBe('ws-linked')
  })
})

describe('prEditableWorkspaceId', () => {
  test('returns the linked worktree', () => {
    expect(prEditableWorkspaceId({ id: 'pr-1', workspaceId: 'ws-linked' }, [host, linked])).toBe('ws-linked')
  })

  test('skips a workspace that is still a chat host', () => {
    expect(prEditableWorkspaceId({ id: 'pr-1', workspaceId: 'ws-host' }, [host])).toBe('')
  })

  test('is empty when the PR has no linked workspace', () => {
    expect(prEditableWorkspaceId({ id: 'pr-1', workspaceId: '' }, [host])).toBe('')
  })

  test('is empty when the linked workspace is missing', () => {
    expect(prEditableWorkspaceId({ id: 'pr-1', workspaceId: 'ws-gone' }, [host])).toBe('')
  })

  test('is empty when the linked workspace is archived', () => {
    expect(prEditableWorkspaceId({ id: 'pr-1', workspaceId: 'ws-linked' }, [host, { ...linked, archived: true }])).toBe('')
  })
})

describe('prAgentIsRunning', () => {
  test('is true when the hidden chat host is running', () => {
    expect(prAgentIsRunning({ id: 'pr-1', workspaceId: '' }, [host], new Set(['ws-host']))).toBe(true)
  })

  test('is true when the linked workspace is running', () => {
    expect(
      prAgentIsRunning({ id: 'pr-1', workspaceId: 'ws-linked' }, [linked], new Set(['ws-linked'])),
    ).toBe(true)
  })

  test('still reports a leftover host that is running after the PR is linked', () => {
    expect(
      prAgentIsRunning({ id: 'pr-1', workspaceId: 'ws-linked' }, [host, linked], new Set(['ws-host'])),
    ).toBe(true)
  })

  test('is false when nothing bound to the PR is running', () => {
    expect(prAgentIsRunning({ id: 'pr-1', workspaceId: '' }, [host], new Set(['ws-other']))).toBe(false)
    expect(prAgentIsRunning({ id: 'pr-1', workspaceId: '' }, [host], new Set())).toBe(false)
  })

  test('ignores an archived chat host still listed as running', () => {
    expect(
      prAgentIsRunning({ id: 'pr-1', workspaceId: '' }, [{ ...host, archived: true }], new Set(['ws-host'])),
    ).toBe(false)
  })

  test('still reports a live linked workspace', () => {
    expect(
      prAgentIsRunning(
        { id: 'pr-1', workspaceId: 'ws-linked' },
        [{ ...host, archived: true }, linked],
        new Set(['ws-linked']),
      ),
    ).toBe(true)
  })
})

describe('prChatHostRunningCount', () => {
  test('counts only hidden hosts, not linked workspaces', () => {
    expect(prChatHostRunningCount([host, linked, otherHost], new Set(['ws-host', 'ws-linked']))).toBe(1)
  })

  test('is zero when only visible workspaces are running', () => {
    expect(prChatHostRunningCount([host, linked], new Set(['ws-linked']))).toBe(0)
  })

  test('counts every running host', () => {
    expect(prChatHostRunningCount([host, otherHost], new Set(['ws-host', 'ws-host-2']))).toBe(2)
  })

  test('skips archived hosts', () => {
    expect(
      prChatHostRunningCount(
        [{ ...host, archived: true }, otherHost],
        new Set(['ws-host', 'ws-host-2']),
      ),
    ).toBe(1)
  })

  test('counts a duplicate host id once', () => {
    expect(prChatHostRunningCount([host, { ...host }], new Set(['ws-host']))).toBe(1)
  })
})

describe('prChatHostUnseenCount', () => {
  test('sums unseen across hidden hosts', () => {
    expect(prChatHostUnseenCount([host, linked, otherHost], { 'ws-host': 2, 'ws-host-2': 1, 'ws-linked': 9 })).toBe(3)
  })

  test('ignores linked workspaces', () => {
    expect(prChatHostUnseenCount([host, linked], { 'ws-linked': 4 })).toBe(0)
  })

  test('skips archived hosts', () => {
    expect(prChatHostUnseenCount([{ ...host, archived: true }, otherHost], { 'ws-host': 2, 'ws-host-2': 1 })).toBe(1)
  })

  test('counts a duplicate host id once', () => {
    expect(prChatHostUnseenCount([host, { ...host }], { 'ws-host': 3 })).toBe(3)
  })

  test('is zero when byWorkspace is empty', () => {
    expect(prChatHostUnseenCount([host, otherHost], {})).toBe(0)
  })

  test('counts down when one host is no longer unseen', () => {
    expect(prChatHostUnseenCount([host, otherHost], { 'ws-host': 2, 'ws-host-2': 1 })).toBe(3)
    expect(prChatHostUnseenCount([host, otherHost], { 'ws-host-2': 1 })).toBe(1)
  })
})

describe('prIdForChatWorkspace', () => {
  test('returns the hosted PR for a chat host', () => {
    expect(prIdForChatWorkspace('ws-host', [host, linked])).toBe('pr-1')
  })

  test('is empty for a linked workspace', () => {
    expect(prIdForChatWorkspace('ws-linked', [host, linked])).toBe('')
  })
})

describe('prUnseenCount', () => {
  test('reads unseen from the hidden chat host', () => {
    expect(prUnseenCount({ id: 'pr-1', workspaceId: '' }, [host], { 'ws-host': 2 })).toBe(2)
  })

  test('reads unseen from the leftover host while it is still live', () => {
    expect(prUnseenCount({ id: 'pr-1', workspaceId: 'ws-linked' }, [host, linked], { 'ws-linked': 1, 'ws-host': 9 })).toBe(9)
  })

  test('is zero when the bound workspace has no unseen', () => {
    expect(prUnseenCount({ id: 'pr-1', workspaceId: '' }, [host], { 'ws-other': 3 })).toBe(0)
    expect(prUnseenCount({ id: 'pr-1', workspaceId: '' }, [host], {})).toBe(0)
  })

  test('ignores an archived chat host while attention still lists it', () => {
    expect(
      prUnseenCount(
        { id: 'pr-1', workspaceId: '' },
        [{ ...host, archived: true }],
        { 'ws-host': 2 },
      ),
    ).toBe(0)
  })

  test('still reads unseen from a live linked workspace', () => {
    expect(
      prUnseenCount(
        { id: 'pr-1', workspaceId: 'ws-linked' },
        [{ ...host, archived: true }, linked],
        { 'ws-linked': 1, 'ws-host': 9 },
      ),
    ).toBe(1)
  })
})
