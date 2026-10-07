/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import { dropChatHeapKey, getChatHeap, pruneChatHeaps, putChatHeap, resetChatHeapsForTests, setChatHeapMaxBytesForTests, takeChatHeapDrafts } from './chat-heap-cache'

afterEach(() => {
  resetChatHeapsForTests()
})

describe('chat heap cache', () => {
  test('round-trips messages for one workspace', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'hi' }] },
      usage: { 'ws\tPA': { inputTokens: 1, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 10 } },
      input: { 'ws\tPA': 'draft' },
      attachments: { 'ws\tPA': [{ id: 'a1' }] },
    })
    const hit = getChatHeap('ws')
    expect(hit?.chat['ws\tPA']).toEqual([{ body: 'hi' }])
    expect(hit?.usage?.['ws\tPA']).toEqual({
      inputTokens: 1, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 10,
      compacting: false, compacted: false,
    })
    expect(hit?.input?.['ws\tPA']).toBe('draft')
    expect(hit?.attachments?.['ws\tPA']).toEqual([{ id: 'a1' }])
  })

  test('round-trips a post-compact empty meter', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'hi' }] },
      usage: {
        'ws\tPA': {
          inputTokens: 0, cacheCreationTokens: 0, cacheReadTokens: 0,
          contextWindow: 200_000, compacting: false, compacted: true,
        },
      },
    })
    expect(getChatHeap('ws')?.usage?.['ws\tPA']).toEqual({
      inputTokens: 0, cacheCreationTokens: 0, cacheReadTokens: 0,
      contextWindow: 200_000, compacting: false, compacted: true,
    })
  })

  test('round-trips the in-flight turn clock', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'hi', streaming: true }] },
      turnStartedAt: { 'ws\tPA': 1_700_000_000_000 },
      streamSeq: { 'ws\tPA:text': 12 },
      turnGotText: { 'ws\tPA': true },
    })
    const hit = getChatHeap('ws')
    expect(hit?.turnStartedAt?.['ws\tPA']).toBe(1_700_000_000_000)
    expect(hit?.streamSeq?.['ws\tPA:text']).toBe(12)
    expect(hit?.turnGotText?.['ws\tPA']).toBe(true)
  })

  test('drops keys that belong to another workspace', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'a' }], 'other\tPA': [{ body: 'b' }] },
    })
    expect(Object.keys(getChatHeap('ws')?.chat ?? {})).toEqual(['ws\tPA'])
  })

  test('empty chat does not clobber', () => {
    putChatHeap('ws', { chat: { 'ws\tPA': [{ body: 'a' }] } })
    putChatHeap('ws', { chat: {} })
    expect(getChatHeap('ws')?.chat['ws\tPA']).toEqual([{ body: 'a' }])
  })

  test('prunes workspaces that are no longer live', () => {
    putChatHeap('a', { chat: { 'a\tPA': [{ body: 'a' }] } })
    putChatHeap('b', { chat: { 'b\tPA': [{ body: 'b' }] } })
    pruneChatHeaps(['a'])
    expect(getChatHeap('a')).toBeTruthy()
    expect(getChatHeap('b')).toBeNull()
  })

  test('evicts the oldest heap when over the byte budget', () => {
    setChatHeapMaxBytesForTests(1)
    putChatHeap('a', { chat: { 'a\tPA': [{ body: 'a' }] } })
    putChatHeap('b', { chat: { 'b\tPA': [{ body: 'b' }] } })
    putChatHeap('c', { chat: { 'c\tPA': [{ body: 'c' }] } })
    expect(getChatHeap('a')).toBeNull()
    expect(getChatHeap('c')?.chat['c\tPA']).toEqual([{ body: 'c' }])
  })

  test('round-trips queued unsent turns with the rest of the heap', () => {
    const pending = [{ id: 'p1', marked: 'fix it' }]
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'fix it', queued: true, pendingId: 'p1' }] },
      pending: { 'ws\tPA': pending },
      holdPending: { 'ws\tPA': true },
    })
    const hit = getChatHeap('ws')
    expect(hit?.pending?.['ws\tPA']).toEqual(pending)
    expect(hit?.holdPending?.['ws\tPA']).toBe(true)
  })

  test('picker-only put keeps a parked pending queue and simplify', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'fix it', queued: true, pendingId: 'p1' }] },
      pending: { 'ws\tPA': [{ id: 'p1' }] },
      holdPending: { 'ws\tPA': true },
      simplify: { 'ws\tPA': false },
    })
    putChatHeap('ws', {
      chat: {},
      pickers: { 'ws\tPA': { model: 'claude-code/opus', flags: {}, mode: 'agent' } },
    })
    const hit = getChatHeap('ws')
    expect(hit?.pending?.['ws\tPA']).toEqual([{ id: 'p1' }])
    expect(hit?.holdPending?.['ws\tPA']).toBe(true)
    expect(hit?.simplify?.['ws\tPA']).toBe(false)
  })

  test('round-trips Simplify with the rest of the heap', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'fix it', queued: true, pendingId: 'p1' }] },
      pending: { 'ws\tPA': [{ id: 'p1' }] },
      simplify: { 'ws\tPA': false },
    })
    expect(getChatHeap('ws')?.simplify?.['ws\tPA']).toBe(false)
  })

  test('transferred pending owner cannot drain after a late RPC completes', () => {
    const key = 'ws\tPA'
    const item = { id: 'p1' }
    let live: Record<string, { id: string }[]> = { [key]: [item] }
    putChatHeap('ws', {
      chat: { [key]: [{ body: 'fix it', queued: true, pendingId: 'p1' }] },
      pending: live,
    })
    live = {}
    const sent: string[] = []
    const flush = (pending: Record<string, { id: string }[]>) => {
      const q = pending[key]
      if (!q?.length) return
      pending[key] = []
      sent.push(...q.map((p) => p.id))
    }
    flush(live)
    expect(sent).toEqual([])
    flush({ ...(getChatHeap('ws')?.pending as Record<string, { id: string }[]>) })
    expect(sent).toEqual(['p1'])
  })

  test('round-trips dock pickers with the rest of the heap', () => {
    const pa = { model: 'claude-code/opus', flags: { effort: 'high' }, mode: 'review' as const }
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'hi' }] },
      input: { 'ws\tPA': 'draft' },
      pickers: { 'ws\tPA': pa },
    })
    const hit = getChatHeap('ws')
    expect(hit?.pickers?.['ws\tPA']).toEqual(pa)
    expect(hit?.input?.['ws\tPA']).toBe('draft')
  })

  test('drops picker keys that belong to another workspace', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'a' }] },
      pickers: {
        'ws\tPA': { model: 'claude-code/opus', flags: {}, mode: 'agent' },
        'other\tPA': { model: 'cursor/gpt', flags: {}, mode: 'ask' },
      },
    })
    expect(Object.keys(getChatHeap('ws')?.pickers ?? {})).toEqual(['ws\tPA'])
  })

  test('empty chat with pickers keeps the previous transcript', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'a' }] },
      input: { 'ws\tPA': 'draft' },
      usage: { 'ws\tPA': { inputTokens: 8, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 100 } },
    })
    putChatHeap('ws', {
      chat: {},
      pickers: { 'ws\tPA': { model: 'claude-code/opus', flags: {}, mode: 'review' } },
    })
    const hit = getChatHeap('ws')
    expect(hit?.chat['ws\tPA']).toEqual([{ body: 'a' }])
    expect(hit?.input?.['ws\tPA']).toBe('draft')
    expect(hit?.usage?.['ws\tPA'].inputTokens).toBe(8)
    expect(hit?.pickers?.['ws\tPA'].mode).toBe('review')
  })

  test('pickers persist when there are no messages yet', () => {
    putChatHeap('ws', {
      chat: {},
      pickers: { 'ws\tPA': { model: 'claude-code/opus', flags: {}, mode: 'ask' } },
    })
    expect(getChatHeap('ws')?.pickers?.['ws\tPA'].model).toBe('claude-code/opus')
    expect(getChatHeap('ws')?.chat).toEqual({})
  })

  test('skips picker stubs with no model', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'a' }] },
      pickers: { 'ws\tPA': { model: '', flags: {}, mode: 'agent' } },
    })
    expect(getChatHeap('ws')?.pickers).toEqual({})
  })

  test('clones usage so a later mutation does not leak into the heap', () => {
    const usage = { inputTokens: 10, cacheCreationTokens: 0, cacheReadTokens: 5, contextWindow: 100 }
    putChatHeap('ws', { chat: { 'ws\tPA': [{ body: 'a' }] }, usage: { 'ws\tPA': usage } })
    usage.inputTokens = 99
    expect(getChatHeap('ws')?.usage?.['ws\tPA'].inputTokens).toBe(10)
  })

  test('evicts one chat key so an empty remount put cannot restore it', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'keep' }], 'ws\tPA2': [{ body: 'gone' }] },
      input: { 'ws\tPA': 'a', 'ws\tPA2': 'b' },
      attachments: { 'ws\tPA2': [{ id: 'x' }] },
      pending: { 'ws\tPA': [{ id: 'keep' }], 'ws\tPA2': [{ id: 'gone' }] },
      holdPending: { 'ws\tPA2': true },
      simplify: { 'ws\tPA': false, 'ws\tPA2': true },
    })
    dropChatHeapKey('ws', 'ws\tPA2')
    expect(getChatHeap('ws')?.chat['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.input?.['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.attachments?.['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.pending?.['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.holdPending?.['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.simplify?.['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.chat['ws\tPA']).toEqual([{ body: 'keep' }])
    expect(getChatHeap('ws')?.pending?.['ws\tPA']).toEqual([{ id: 'keep' }])
    expect(getChatHeap('ws')?.simplify?.['ws\tPA']).toBe(false)
    putChatHeap('ws', { chat: {}, input: {}, attachments: {} })
    expect(getChatHeap('ws')?.chat['ws\tPA2']).toBeUndefined()
    expect(getChatHeap('ws')?.input?.['ws\tPA2']).toBeUndefined()
  })

  test('takes composer drafts without dropping the transcript', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'keep' }] },
      input: { 'ws\tPA': 'draft' },
      attachments: { 'ws\tPA': [{ id: 'a1' }] },
    })
    expect(takeChatHeapDrafts('ws')).toEqual({
      input: { 'ws\tPA': 'draft' },
      attachments: { 'ws\tPA': [{ id: 'a1' }] },
    })
    expect(getChatHeap('ws')?.chat['ws\tPA']).toEqual([{ body: 'keep' }])
    expect(getChatHeap('ws')?.input).toEqual({})
    expect(getChatHeap('ws')?.attachments).toEqual({})
    expect(takeChatHeapDrafts('ws')).toEqual({ input: {}, attachments: {} })
  })

  test('skips usage that cannot fill the ring', () => {
    putChatHeap('ws', {
      chat: { 'ws\tPA': [{ body: 'a' }] },
      usage: { 'ws\tPA': { inputTokens: 0, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 100 } },
    })
    expect(getChatHeap('ws')?.usage).toEqual({})
  })
})
