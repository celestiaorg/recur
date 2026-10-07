import { afterEach, expect, test, vi } from 'vitest'
import { SessionFeed } from './session-feed.svelte'

const { listeners, workspaceSessions } = vi.hoisted(() => ({
  listeners: new Map<string, (...args: any[]) => void>(),
  workspaceSessions: vi.fn(),
}))
vi.mock('./api', () => ({
  WorkspacesService: { WorkspaceSessions: workspaceSessions, WorkspaceFacts: async () => [] },
  isAgentRunningStatus: (status: string) => status === 'working',
  ...Object.fromEntries([
    'onSessionDelta', 'onSessionStep', 'onSessionThinking', 'onSessionToolResult',
    'onSessionToolInput', 'onSessionTurnEnd', 'onSessionCompact', 'onSessionNotice',
    'onSessionUser', 'onSessionFallback', 'onAgent', 'onFeed',
  ].map((name) => [name, (fn: (...args: any[]) => void) => {
    listeners.set(name, fn)
    return () => { listeners.delete(name) }
  }])),
}))

afterEach(() => { listeners.clear(); workspaceSessions.mockReset() })

function session(code: string, text: string, ownerCode = 'PA2') {
  return { id: code, code, ownerCode, createdAt: 1, items: [{ kind: 'text', role: 'assistant', text }] }
}

test('loads only the selected chat and refreshes only the completed specialist', async () => {
  workspaceSessions.mockResolvedValueOnce([session('CO', 'coding'), session('RV', 'reviewing')])
  const feed = new SessionFeed()
  feed.attach('ws', 'PA2')
  await vi.waitFor(() => expect(feed.sessions).toHaveLength(2))
  expect(workspaceSessions).toHaveBeenCalledExactlyOnceWith('ws', 'PA2', '')

  workspaceSessions.mockResolvedValueOnce([session('CO', 'finished')])
  listeners.get('onAgent')!('ws', { kind: 'updated', agentId: 'ws-CO', status: 'idle' })
  await vi.waitFor(() => expect(feed.sessions[0].messages[0].body).toBe('finished'))
  expect(workspaceSessions).toHaveBeenLastCalledWith('ws', 'PA2', 'CO')
  expect(feed.sessions[1].messages[0].body).toBe('reviewing')
  feed.detach()
})

test('late scoped loads cannot overwrite a newer visit to the same chat', async () => {
  const pending: ((sessions: ReturnType<typeof session>[]) => void)[] = []
  workspaceSessions.mockImplementation(() => new Promise((resolve) => pending.push(resolve)))
  const feed = new SessionFeed()
  feed.attach('ws', 'PA2')
  feed.attach('ws', 'PA3')
  feed.attach('ws', 'PA2')
  pending[2]([session('CO', 'latest')])
  await vi.waitFor(() => expect(feed.sessions[0]?.messages[0].body).toBe('latest'))
  pending[0]([session('CO', 'stale')])
  pending[1]([session('CO', 'other chat', 'PA3')])
  await Promise.resolve()
  await Promise.resolve()
  expect(feed.sessions[0].messages[0].body).toBe('latest')
  feed.detach()
})
