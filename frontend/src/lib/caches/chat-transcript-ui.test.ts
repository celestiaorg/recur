import { beforeEach, describe, expect, test } from 'vitest'
import {
  claimTranscriptUI,
  clearTranscriptUI,
  dropTranscriptUI,
  emptyTranscriptUI,
  expansionStillValid,
  loadTranscriptUI,
  saveTranscriptUI,
  saveTranscriptUIScroll,
  setTranscriptUIMaxBytesForTests,
  scrollRestoreReady,
  uniqueBlockKeys,
  nextNearBottom,
  scrollerNearBottom,
  snapshotTranscriptUI,
  transcriptBlockKey,
  transcriptMessageBlockLocal,
  transcriptSessionBlockLocal,
  transcriptStepBlockLocal,
  transcriptFingerprint,
  restoreBlockHeights,
} from './chat-transcript-ui'

const expanded = {
  openGroups: { 2: true },
  openRows: { 3: true, 4: false },
  findingAccordOpen: { f1: true },
  scrollTop: 480,
  nearBottom: false,
  rowCount: 12,
  fingerprint: 'u|a|t:Bash',
  blockHeights: { 'ws-a\tMA:m:1': 240 },
}

describe('chat-transcript-ui', () => {
  beforeEach(() => {
    clearTranscriptUI()
  })

  test('live row keys stay distinct across chats', () => {
    expect(transcriptBlockKey('ws-a\tPA', 'm:live-text')).not.toBe(
      transcriptBlockKey('ws-b\tPA', 'm:live-text'),
    )
    expect(transcriptBlockKey('ws-b\tPA', 's:live')).toBe('ws-b\tPA:s:live')
    expect(transcriptBlockKey('', 'm:live-text')).toBe('_:m:live-text')
  })

  test('uniqueBlockKeys suffixes repeated keys', () => {
    expect(uniqueBlockKeys(['k:m:0:Hi', 'k:me:1:x', 'k:m:0:Hi', 'k:m:0:Hi']))
      .toEqual(['k:m:0:Hi', 'k:me:1:x', 'k:m:0:Hi#1', 'k:m:0:Hi#2'])
  })

  test('step groups that share a timestamp stay unique', () => {
    const ts = 1786586607416
    const a = transcriptStepBlockLocal(19, { ts })
    const b = transcriptStepBlockLocal(21, { ts })
    expect(a).toBe(`s:19:${ts}`)
    expect(b).toBe(`s:21:${ts}`)
    expect(a).not.toBe(b)
    expect(transcriptBlockKey('buttery-risotto\tPA', a)).not.toBe(
      transcriptBlockKey('buttery-risotto\tPA', b),
    )
    expect(transcriptStepBlockLocal(19, { ts }, true)).toBe('s:live')
  })

  test('tool-led step groups stay stable when a row is spliced above', () => {
    expect(transcriptStepBlockLocal(12, { toolId: 'tool_abc123', ts: 100 })).toBe(
      's:tool_abc123',
    )
    expect(transcriptStepBlockLocal(18, { toolId: 'tool_abc123', ts: 100 })).toBe(
      's:tool_abc123',
    )
  })

  test('specialist card keys stay stable when a sibling joins the group', () => {
    const one = { kind: 'session' as const, id: 's-co#0', code: 'CO' }
    const group = {
      kind: 'sessions' as const,
      id: 's-co#0',
      sessions: [one, { id: 's-rv#0' }],
    }
    expect(transcriptSessionBlockLocal(one)).toBe('sess:s-co#0')
    expect(transcriptSessionBlockLocal(group)).toBe('sess:s-co#0')
    expect(transcriptBlockKey('ws\tMA', transcriptSessionBlockLocal(one))).toBe(
      transcriptBlockKey('ws\tMA', transcriptSessionBlockLocal(group)),
    )
  })

  test('message keys stay stable when a row is spliced above', () => {
    const user = { me: true, ts: 100, body: 'hello' }
    const asst = { ts: 200, body: 'ok' }
    expect(transcriptMessageBlockLocal(3, user)).toBe('me:100:hello')
    expect(transcriptMessageBlockLocal(5, user)).toBe('me:100:hello')
    expect(transcriptMessageBlockLocal(4, asst)).toBe('m:200:ok')
    expect(transcriptMessageBlockLocal(6, asst)).toBe('m:200:ok')
    expect(transcriptMessageBlockLocal(0, { me: true })).toBe('me:0')
    expect(transcriptMessageBlockLocal(2, {})).toBe('m:0:2')
  })

  test('round-trips chrome per chat key', () => {
    saveTranscriptUI('ws-a\tMA', expanded)
    saveTranscriptUI('ws-a\tPA', emptyTranscriptUI())
    expect(loadTranscriptUI('ws-a\tMA')).toEqual(expanded)
    expect(loadTranscriptUI('ws-a\tPA')).toEqual(emptyTranscriptUI())
    expect(loadTranscriptUI('ws-b\tMA')).toBeUndefined()
  })

  test('ignores empty keys so SessionCard instances do not share a bucket', () => {
    saveTranscriptUI('', expanded)
    expect(loadTranscriptUI('')).toBeUndefined()
  })

  test('load returns a clone so later toggles do not mutate the cache', () => {
    saveTranscriptUI('k', expanded)
    const a = loadTranscriptUI('k')!
    a.openRows[3] = false
    a.openGroups[2] = false
    a.blockHeights['ws-a\tMA:m:1'] = 1
    expect(loadTranscriptUI('k')?.openRows[3]).toBe(true)
    expect(loadTranscriptUI('k')?.openGroups[2]).toBe(true)
    expect(loadTranscriptUI('k')?.blockHeights['ws-a\tMA:m:1']).toBe(240)
  })

  test('dropTranscriptUI removes one key and leaves others', () => {
    saveTranscriptUI('a', expanded)
    saveTranscriptUI('b', emptyTranscriptUI())
    dropTranscriptUI('a')
    expect(loadTranscriptUI('a')).toBeUndefined()
    expect(loadTranscriptUI('b')).toEqual(emptyTranscriptUI())
  })

  test('drop tombstones so a late save cannot resurrect the key', () => {
    saveTranscriptUI('a', expanded)
    dropTranscriptUI('a')
    saveTranscriptUI('a', { ...expanded, scrollTop: 1 })
    expect(loadTranscriptUI('a')).toBeUndefined()
  })

  test('claim after drop lets a reused tab code persist again', () => {
    saveTranscriptUI('a', expanded)
    dropTranscriptUI('a')
    expect(claimTranscriptUI('a')).toBeUndefined()
    saveTranscriptUI('a', expanded)
    expect(loadTranscriptUI('a')).toEqual(expanded)
  })

  test('holds more than 64 chats under the default byte budget', () => {
    for (let i = 0; i < 80; i++) {
      saveTranscriptUI(`k${i}`, { ...emptyTranscriptUI(), scrollTop: i })
    }
    expect(loadTranscriptUI('k0')?.scrollTop).toBe(0)
    expect(loadTranscriptUI('k79')?.scrollTop).toBe(79)
  })

  test('evicts the oldest key once the byte budget is exceeded', () => {
    setTranscriptUIMaxBytesForTests(1)
    saveTranscriptUI('k0', { ...emptyTranscriptUI(), scrollTop: 0 })
    saveTranscriptUI('k1', { ...emptyTranscriptUI(), scrollTop: 1 })
    saveTranscriptUI('k2', { ...emptyTranscriptUI(), scrollTop: 2 })
    expect(loadTranscriptUI('k0')).toBeUndefined()
    expect(loadTranscriptUI('k2')?.scrollTop).toBe(2)
  })

  test('snapshot keeps fingerprint when the scroller is unmeasured, not the live pin', () => {
    const hidden = snapshotTranscriptUI(
      {
        openGroups: { 1: false },
        openRows: {},
        findingAccordOpen: {},
        scrollTop: 90,
        nearBottom: true,
        scrollerHidden: true,
        rowCount: 0,
        fingerprint: '',
      },
      expanded,
    )
    expect(hidden.openGroups[1]).toBe(false)
    expect(hidden.scrollTop).toBe(90)
    expect(hidden.nearBottom).toBe(true)
    expect(hidden.rowCount).toBe(12)
    expect(hidden.fingerprint).toBe('u|a|t:Bash')

    const leavingCounts = snapshotTranscriptUI(
      {
        openGroups: {},
        openRows: {},
        findingAccordOpen: {},
        scrollTop: 90,
        nearBottom: false,
        scrollerHidden: true,
        rowCount: 8,
        fingerprint: 'u|a',
      },
      expanded,
    )
    expect(leavingCounts.rowCount).toBe(8)
    expect(leavingCounts.fingerprint).toBe('u|a')

    const mismatched = snapshotTranscriptUI(
      {
        openGroups: {},
        openRows: {},
        findingAccordOpen: {},
        scrollTop: 90,
        nearBottom: false,
        scrollerHidden: true,
        rowCount: 50,
        fingerprint: '',
      },
      expanded,
    )
    expect(mismatched.rowCount).toBe(12)
    expect(mismatched.fingerprint).toBe('u|a|t:Bash')

    const visible = snapshotTranscriptUI({
      openGroups: {},
      openRows: { 8: true },
      findingAccordOpen: {},
      scrollTop: 12,
      nearBottom: false,
      scrollerHidden: false,
      rowCount: 3,
      fingerprint: 'u|a',
    })
    expect(visible).toEqual({
      openGroups: {},
      openRows: { 8: true },
      findingAccordOpen: {},
      scrollTop: 12,
      nearBottom: false,
      rowCount: 3,
      fingerprint: 'u|a',
      blockHeights: {},
    })
  })

  test('snapshot keeps measured heights when teardown has none', () => {
    const hidden = snapshotTranscriptUI(
      {
        openGroups: {},
        openRows: {},
        findingAccordOpen: {},
        scrollTop: 90,
        nearBottom: false,
        scrollerHidden: true,
        blockHeights: {},
      },
      expanded,
    )
    expect(hidden.blockHeights).toEqual(expanded.blockHeights)

    const live = snapshotTranscriptUI({
      openGroups: {},
      openRows: {},
      findingAccordOpen: {},
      scrollTop: 12,
      nearBottom: false,
      scrollerHidden: false,
      blockHeights: { 'k:m:2': 180 },
    })
    expect(live.blockHeights).toEqual({ 'k:m:2': 180 })
  })

  test('tombstones share the byte budget so the oldest close expires', () => {
    setTranscriptUIMaxBytesForTests(1)
    for (const k of ['k0', 'k1', 'k2']) {
      saveTranscriptUI(k, expanded)
      dropTranscriptUI(k)
    }
    // k0's tombstone was evicted — a late save is accepted.
    saveTranscriptUI('k0', expanded)
    expect(loadTranscriptUI('k0')).toEqual(expanded)
    // newest close is still tombstoned.
    saveTranscriptUI('k2', expanded)
    expect(loadTranscriptUI('k2')).toBeUndefined()
  })
})

describe('transcriptFingerprint / expansionStillValid', () => {
  const rows = [
    { me: true },
    { },
    { tool: 'Bash' },
  ]
  const fp = transcriptFingerprint(rows)

  test('encodes role / tool kind, not payload text', () => {
    expect(fp).toBe('u|a|t:Bash')
  })

  test('keeps maps while the transcript is still empty (hydrate)', () => {
    expect(expansionStillValid({ rowCount: 3, fingerprint: fp }, '', 0)).toBe(true)
  })

  test('keeps maps when the list is unchanged or only appended to', () => {
    expect(expansionStillValid({ rowCount: 3, fingerprint: fp }, fp, 3)).toBe(true)
    const grown = transcriptFingerprint([...rows, { me: true }])
    expect(expansionStillValid({ rowCount: 3, fingerprint: fp }, grown, 4)).toBe(true)
  })

  test('a prefix-limited fingerprint is enough to accept an append', () => {
    const grown = [...rows, { me: true }, { tool: 'Read' }]
    const prefix = transcriptFingerprint(grown, 3)
    expect(prefix).toBe(fp)
    expect(expansionStillValid({ rowCount: 3, fingerprint: fp }, prefix, grown.length)).toBe(true)
  })

  test('drops maps when count matches but the signature drifted', () => {
    expect(expansionStillValid({ rowCount: 3, fingerprint: 'u|a' }, fp, 3)).toBe(false)
  })

  test('drops maps when a refetch shortens or rewrites the prefix', () => {
    expect(expansionStillValid({ rowCount: 3, fingerprint: fp }, 'u|a', 2)).toBe(false)
    expect(expansionStillValid({ rowCount: 3, fingerprint: fp }, 'u|a|t:Read', 3)).toBe(false)
  })
})

describe('scrollRestoreReady', () => {
  const mid = { scrollTop: 480, nearBottom: false, rowCount: 20 }

  test('waits for a measurable scroller', () => {
    expect(scrollRestoreReady(mid, { clientHeight: 0, scrollHeight: 0, rowCount: 20 })).toBe('wait')
  })

  test('pins immediately when the saved view was at the bottom or top', () => {
    const geom = { clientHeight: 400, scrollHeight: 400, rowCount: 0 }
    expect(scrollRestoreReady({ scrollTop: 0, nearBottom: true, rowCount: 20 }, geom)).toBe('apply')
    expect(scrollRestoreReady({ scrollTop: 0, nearBottom: false, rowCount: 20 }, geom)).toBe('apply')
  })

  test('waits while content has not overflowed and rows are still arriving', () => {
    expect(scrollRestoreReady(mid, { clientHeight: 400, scrollHeight: 400, rowCount: 0 })).toBe('wait')
  })

  test('abandons a mid-transcript restore once the hydrated list fits the scroller', () => {
    expect(scrollRestoreReady(mid, { clientHeight: 400, scrollHeight: 400, rowCount: 20 })).toBe('abandon')
  })

  test('applies once the offset is reachable, or once a shorter transcript has laid out', () => {
    expect(scrollRestoreReady(mid, { clientHeight: 400, scrollHeight: 1200, rowCount: 20 })).toBe('apply')
    expect(scrollRestoreReady(mid, { clientHeight: 400, scrollHeight: 500, rowCount: 20 })).toBe('apply')
  })

  test('abandons an unreachable offset even when the saved entry has no rowCount', () => {
    const noCount = { scrollTop: 900, nearBottom: false, rowCount: 0 }
    expect(scrollRestoreReady(noCount, { clientHeight: 400, scrollHeight: 400, rowCount: 8 })).toBe('abandon')
    expect(scrollRestoreReady(noCount, { clientHeight: 400, scrollHeight: 600, rowCount: 8 })).toBe('apply')
    expect(scrollRestoreReady(noCount, { clientHeight: 400, scrollHeight: 400, rowCount: 0 })).toBe('wait')
  })
})

describe('scrollerNearBottom', () => {
  test('re-pins only when the tail is within 24px', () => {
    // distance = scrollHeight - scrollTop - clientHeight
    const el = { scrollHeight: 1000, clientHeight: 200, scrollTop: 776 }
    expect(scrollerNearBottom(el)).toBe(false) // 24px away
    expect(scrollerNearBottom({ ...el, scrollTop: 777 })).toBe(true) // 23px
  })
})

describe('nextNearBottom', () => {
  const box = { scrollHeight: 1000, clientHeight: 200 }

  test('stays pinned when a pin lands short of a fast-growing canvas', () => {
    // Was at the tail; pin jumped down but content grew another 80px first.
    expect(nextNearBottom(true, 700, { ...box, scrollTop: 720 })).toBe(true)
  })

  test('unpins only when scrollTop moves up away from the tail', () => {
    expect(nextNearBottom(true, 800, { ...box, scrollTop: 720 })).toBe(false)
  })

  test('unpins a wheel tick that is still inside the 24px slack', () => {
    // max scroll 800; 790 is 10px from the tail — must not stay pinned.
    expect(nextNearBottom(true, 800, { ...box, scrollTop: 790 })).toBe(false)
  })

  test('re-arms when the event lands within the 24px slack', () => {
    expect(nextNearBottom(false, 700, { ...box, scrollTop: 777 })).toBe(true)
  })
})

describe('saveTranscriptUIScroll', () => {
  beforeEach(() => {
    clearTranscriptUI()
  })

  test('patches scroll without dropping expansion maps', () => {
    saveTranscriptUI('k', expanded)
    saveTranscriptUIScroll('k', 12, true)
    expect(loadTranscriptUI('k')).toEqual({
      ...expanded,
      scrollTop: 12,
      nearBottom: true,
    })
  })

  test('does not create an empty entry when nothing has been saved yet', () => {
    saveTranscriptUIScroll('k', 12, false)
    expect(loadTranscriptUI('k')).toBeUndefined()
  })
})

describe('restoreBlockHeights', () => {
  test('fills a fresh map from the saved record', () => {
    const m = new Map<string, number>()
    restoreBlockHeights(m, { a: 240, b: 0, c: 56 })
    expect([...m.entries()]).toEqual([['a', 240], ['c', 56]])
  })

  test('is a no-op when nothing was saved', () => {
    const m = new Map<string, number>([['a', 1]])
    restoreBlockHeights(m)
    expect(m.get('a')).toBe(1)
  })
})
