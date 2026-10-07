import { afterEach, describe, expect, test } from 'vitest'
import {
  concatHoistList,
  concatHoistText,
  destChatKey,
  hoistCacheBustCodes,
  liveChatKey,
  liveChatTarget,
  noteHoistChatAlias,
  noteHoistDestBust,
  remapPrefixedRecord,
  resetHoistChatKeysForTests,
  stashHoistDraft,
  stashHoistPending,
  stashHoistSending,
  takeHoistDrafts,
  takeHoistPending,
  takeHoistSending,
  absorbHoistSending,
  clearHoistSending,
  hoistSendCompleteApplies,
  migrateEvictedHoistDrafts,
  noteHoistLiveSource,
  onHoistSendComplete,
  bustHoistedDestHeap,
  takeHoistDestBust,
} from './hoist-chat-keys'
import { getChatHeap, putChatHeap, resetChatHeapsForTests } from './caches/chat-heap-cache'

afterEach(() => {
  resetHoistChatKeysForTests()
  resetChatHeapsForTests()
})

describe('hoist chat keys', () => {
  test('ordinary activation does not bust picker caches', () => {
    expect(hoistCacheBustCodes(['PA', 'PA2'])).toEqual([])
    expect(hoistCacheBustCodes(['PA'], null)).toEqual([])
  })

  test('collision remap busts only the dest codes that received a session', () => {
    expect(hoistCacheBustCodes(['PA', 'PA2', 'PA3'], { PA: 'PA2' })).toEqual(['PA2'])
  })

  test('mixed hoist includes identity-moved dest codes', () => {
    expect(hoistCacheBustCodes(['PA', 'PA2', 'PA3'], { PA: 'PA3' }, ['PA3', 'PA2'])).toEqual(['PA3', 'PA2'])
  })

  test('identity hoist busts every dest tab (remap does not name them)', () => {
    expect(hoistCacheBustCodes(['PA', 'PA2'], {})).toEqual(['PA', 'PA2'])
  })

  test('evicted dest remount drops identity-moved picker heap', () => {
    putChatHeap('dest', {
      chat: {
        'dest\tPA': [{ body: 'keep' }],
        'dest\tPA2': [{ body: 'placeholder' }],
      },
      pickers: {
        'dest\tPA': { model: 'claude-code/sonnet', flags: {}, mode: 'agent' },
        'dest\tPA2': { model: 'claude-code/sonnet', flags: {}, mode: 'agent' },
      },
      input: { 'dest\tPA': 'leave me', 'dest\tPA2': 'typed on dest' },
      attachments: { 'dest\tPA2': [{ id: 'd1' }] },
    })
    stashHoistDraft('dest\tPA2', 'from host', [{ id: 'h1' }])
    const codes = hoistCacheBustCodes([], { PA: 'PA3' }, ['PA3', 'PA2'])
    noteHoistDestBust('dest', codes)
    bustHoistedDestHeap('dest', takeHoistDestBust('dest'))
    expect(getChatHeap('dest')?.pickers?.['dest\tPA2']).toBeUndefined()
    expect(getChatHeap('dest')?.chat['dest\tPA2']).toBeUndefined()
    expect(getChatHeap('dest')?.pickers?.['dest\tPA']?.mode).toBe('agent')
    expect(getChatHeap('dest')?.chat['dest\tPA']).toEqual([{ body: 'keep' }])
    expect(getChatHeap('dest')?.input?.['dest\tPA']).toBe('leave me')
    expect(takeHoistDrafts('dest')).toEqual({
      'dest\tPA2': {
        input: 'typed on dest\n\nfrom host',
        attachments: [{ id: 'd1' }, { id: 'h1' }],
      },
    })
    expect(takeHoistDestBust('dest')).toEqual([])
  })

  test('remaps host keys onto dest, merging drafts', () => {
    const rec = {
      'host\tPA': 'from-host',
      'dest\tPA': 'on-dest',
      'other\tPA': 'keep',
    }
    const got = remapPrefixedRecord(rec, 'host', 'dest', { PA: 'PA2' }, concatHoistText)
    expect(got).toEqual({
      'dest\tPA': 'on-dest',
      'dest\tPA2': 'from-host',
      'other\tPA': 'keep',
    })
    expect(liveChatKey('host\tPA')).toBe('dest\tPA2')
    expect(liveChatTarget('host', 'PA')).toEqual({ id: 'dest', code: 'PA2', key: 'dest\tPA2' })
  })

  test('identity remap still retargets the workspace id', () => {
    const pending = { 'host\tPA': [{ id: 'q' }] }
    const got = remapPrefixedRecord(pending, 'host', 'dest', {}, concatHoistList)
    expect(got).toEqual({ 'dest\tPA': [{ id: 'q' }] })
    expect(destChatKey('host', 'PA', 'dest', {})).toBe('dest\tPA')
  })

  test('stashes drafts for a dest that is not yet mounted', () => {
    stashHoistDraft('dest\tPA2', 'follow up', [{ id: 'a1' }])
    expect(takeHoistDrafts('host')).toEqual({})
    expect(takeHoistDrafts('dest')).toEqual({
      'dest\tPA2': { input: 'follow up', attachments: [{ id: 'a1' }] },
    })
    expect(takeHoistDrafts('dest')).toEqual({})
  })

  test('evicted source shell remaps heap drafts onto dest', () => {
    putChatHeap('host', {
      chat: { 'host\tPA': [{ body: 'hi' }] },
      input: { 'host\tPA': 'follow up' },
      attachments: { 'host\tPA': [{ id: 'a1' }], 'host\tPB': [{ id: 'a2' }] },
    })
    migrateEvictedHoistDrafts('host', 'dest', { PA: 'PA2' })
    expect(takeHoistDrafts('dest')).toEqual({
      'dest\tPA2': { input: 'follow up', attachments: [{ id: 'a1' }] },
      'dest\tPB': { input: '', attachments: [{ id: 'a2' }] },
    })
    expect(getChatHeap('host')?.input).toEqual({})
    expect(getChatHeap('host')?.attachments).toEqual({})
    expect(getChatHeap('host')?.chat['host\tPA']).toEqual([{ body: 'hi' }])
    expect(liveChatKey('host\tPA')).toBe('dest\tPA2')
    expect(takeHoistDrafts('dest')).toEqual({})
  })

  test('mounted source keeps heap drafts for applySourceHoist', () => {
    putChatHeap('host', {
      chat: { 'host\tPA': [{ body: 'hi' }] },
      input: { 'host\tPA': 'stale' },
    })
    const stop = noteHoistLiveSource('host')
    migrateEvictedHoistDrafts('host', 'dest', { PA: 'PA2' })
    stop()
    expect(takeHoistDrafts('dest')).toEqual({})
    expect(getChatHeap('host')?.input?.['host\tPA']).toBe('stale')
  })

  test('stashes pending queues for dest to absorb', () => {
    stashHoistPending('dest\tPA', [{ id: 'q1' }], false)
    stashHoistPending('dest\tPA', [{ id: 'q2' }], true)
    expect(takeHoistPending('host')).toEqual({})
    expect(takeHoistPending('dest')).toEqual({
      'dest\tPA': { items: [{ id: 'q1' }, { id: 'q2' }], hold: true },
    })
    expect(takeHoistPending('dest')).toEqual({})
  })

  test('transfers in-flight send ownership for dest to absorb', () => {
    stashHoistSending('dest\tPA', 2)
    stashHoistSending('dest\tPA', 1)
    stashHoistPending('dest\tPA', [{ id: 'q1' }])
    expect(takeHoistSending('host')).toEqual({})
    expect(takeHoistSending('dest')).toEqual({ 'dest\tPA': 2 })
    expect(takeHoistSending('dest')).toEqual({})
    expect(takeHoistPending('dest')).toEqual({
      'dest\tPA': { items: [{ id: 'q1' }], hold: false },
    })
  })

  test('drops unclaimed send ownership when the source send finishes', () => {
    stashHoistSending('dest\tPA', 3)
    clearHoistSending('dest\tPA', 2)
    expect(takeHoistSending('dest')).toEqual({ 'dest\tPA': 3 })
    stashHoistSending('dest\tPA', 3)
    clearHoistSending('dest\tPA', 3)
    expect(takeHoistSending('dest')).toEqual({})
  })

  function destComplete(
    sendingByKey: Record<string, boolean>,
    sendGenByKey: Record<string, number>,
    hoistSendGenByKey: Record<string, number>,
  ) {
    const flushed: string[] = []
    const stop = onHoistSendComplete((key, gen) => {
      if (!key.startsWith('dest\t')) return
      if (!hoistSendCompleteApplies(gen, sendGenByKey[key] ?? 0, hoistSendGenByKey[key])) return
      sendingByKey = { ...sendingByKey, [key]: false }
      flushed.push(key)
    })
    return {
      flushed,
      stop,
      sending: () => sendingByKey,
    }
  }

  test('source RPC completion releases dest after absorb, with no idle event', () => {
    stashHoistSending('dest\tPA', 2)
    stashHoistPending('dest\tPA', [{ id: 'q1' }])
    noteHoistChatAlias('host\tPA', 'dest\tPA')

    const inflight = takeHoistSending('dest')
    expect(inflight).toEqual({ 'dest\tPA': 2 })
    const qs = takeHoistPending('dest')
    expect(qs['dest\tPA']?.items).toEqual([{ id: 'q1' }])

    const next = absorbHoistSending({}, {}, {}, inflight)
    const watch = destComplete(next.sendingByKey, next.sendGenByKey, next.hoistSendGenByKey)

    const live = liveChatTarget('host', 'PA')
    clearHoistSending('host\tPA', 2)
    clearHoistSending(live.key, 2)

    expect(watch.sending()['dest\tPA']).toBe(false)
    expect(watch.flushed).toEqual(['dest\tPA'])
    watch.stop()
  })

  test('source completion releases dest when dest generation is already higher', () => {
    stashHoistSending('dest\tPA', 1)
    stashHoistPending('dest\tPA', [{ id: 'q1' }])
    noteHoistChatAlias('host\tPA', 'dest\tPA')

    // Leftover dest counter (empty prompt / pre-turn failure), dest not sending.
    const next = absorbHoistSending({}, { 'dest\tPA': 3 }, {}, takeHoistSending('dest'))
    expect(next.sendGenByKey['dest\tPA']).toBe(3)
    expect(next.hoistSendGenByKey['dest\tPA']).toBe(1)

    const watch = destComplete(next.sendingByKey, next.sendGenByKey, next.hoistSendGenByKey)
    const live = liveChatTarget('host', 'PA')
    clearHoistSending('host\tPA', 1)
    clearHoistSending(live.key, 1)

    expect(watch.sending()['dest\tPA']).toBe(false)
    expect(watch.flushed).toEqual(['dest\tPA'])
    watch.stop()
  })

  test('source completion does not release dest when dest is already sending', () => {
    stashHoistSending('dest\tPA', 1)
    const next = absorbHoistSending(
      { 'dest\tPA': true },
      { 'dest\tPA': 3 },
      {},
      takeHoistSending('dest'),
    )
    expect(next.hoistSendGenByKey['dest\tPA']).toBeUndefined()

    const watch = destComplete(next.sendingByKey, next.sendGenByKey, next.hoistSendGenByKey)
    clearHoistSending('dest\tPA', 1)
    expect(watch.sending()['dest\tPA']).toBe(true)
    expect(watch.flushed).toEqual([])
    watch.stop()
  })

  test('source completion does not release dest when both shells are at generation 1', () => {
    stashHoistSending('dest\tPA', 1)
    const next = absorbHoistSending(
      { 'dest\tPA': true },
      { 'dest\tPA': 1 },
      {},
      takeHoistSending('dest'),
    )
    expect(next.hoistSendGenByKey['dest\tPA']).toBeUndefined()
    expect(next.sendGenByKey['dest\tPA']).toBe(1)

    const watch = destComplete(next.sendingByKey, next.sendGenByKey, next.hoistSendGenByKey)
    clearHoistSending('dest\tPA', 1)
    expect(watch.sending()['dest\tPA']).toBe(true)
    expect(watch.flushed).toEqual([])
    watch.stop()
  })

  test('same-instance leftover dest gen still releases after remap made dest look busy', () => {
    // applySourceHoist remaps sending before absorb; reservation is noted first.
    const hoistSendGenByKey = { 'dest\tPA': 1 }
    stashHoistSending('dest\tPA', 1)
    const next = absorbHoistSending(
      { 'dest\tPA': true },
      { 'dest\tPA': 3 },
      hoistSendGenByKey,
      takeHoistSending('dest'),
    )
    expect(next.hoistSendGenByKey['dest\tPA']).toBe(1)

    const watch = destComplete(next.sendingByKey, next.sendGenByKey, next.hoistSendGenByKey)
    clearHoistSending('dest\tPA', 1)
    expect(watch.sending()['dest\tPA']).toBe(false)
    expect(watch.flushed).toEqual(['dest\tPA'])
    watch.stop()
  })

  test('dest dispatch after absorb supersedes the transferred reservation', () => {
    expect(hoistSendCompleteApplies(1, 3, 1)).toBe(true)
    expect(hoistSendCompleteApplies(1, 4, undefined)).toBe(false)
    expect(hoistSendCompleteApplies(1, 3, undefined)).toBe(false)
    expect(hoistSendCompleteApplies(1, 1, undefined)).toBe(false)
  })
})
