import { describe, expect, it, beforeEach } from 'vitest'
import {
  SessionTabPark,
  sessionTabPark,
  resetSessionTabParksForTests,
  pruneAllSessionTabParks,
  pruneSessionTabParks,
} from './cm-session-tabs'

describe('SessionTabPark', () => {
  it('round-trips a tab map and transfers ownership on take', () => {
    const park = new SessionTabPark<number>()
    park.park('ws-a', new Map([['a.go', 1], ['b.go', 2]]))
    const taken = park.take('ws-a')
    expect([...taken!]).toEqual([['a.go', 1], ['b.go', 2]])
    expect(park.take('ws-a')).toBeUndefined()
  })

  it('does not clobber an existing snapshot with an empty park', () => {
    const park = new SessionTabPark<number>()
    park.park('ws-a', new Map([['a.go', 1]]))
    park.park('ws-a', new Map())
    expect(park.take('ws-a')?.get('a.go')).toBe(1)
  })

  it('copies on park so later mutation of the live map is invisible', () => {
    const park = new SessionTabPark<number>()
    const live = new Map([['a.go', 1]])
    park.park('ws-a', live)
    live.set('a.go', 99)
    live.set('b.go', 2)
    expect(park.take('ws-a')?.get('a.go')).toBe(1)
  })

  it('evicts the least-recently parked session past the byte budget', () => {
    const park = new SessionTabPark<number>({ maxBytes: 2, sizeOf: () => 1 })
    park.park('a', new Map([['a.go', 1]]))
    park.park('b', new Map([['b.go', 1]]))
    park.park('c', new Map([['c.go', 1]]))
    expect(park.take('a')).toBeUndefined()
    expect(park.take('b')?.get('b.go')).toBe(1)
    expect(park.take('c')?.get('c.go')).toBe(1)
  })

  it('re-parking a session refreshes LRU order', () => {
    const park = new SessionTabPark<number>({ maxBytes: 2, sizeOf: () => 1 })
    park.park('a', new Map([['a.go', 1]]))
    park.park('b', new Map([['b.go', 1]]))
    park.park('a', new Map([['a.go', 2]]))
    park.park('c', new Map([['c.go', 1]]))
    expect(park.take('b')).toBeUndefined()
    expect(park.take('a')?.get('a.go')).toBe(2)
  })

  it('prune drops sessions the keep callback rejects', () => {
    const park = new SessionTabPark<number>()
    park.park('keep', new Map([['a.go', 1]]))
    park.park('drop', new Map([['b.go', 1]]))
    park.prune((session) => session === 'keep')
    expect(park.take('drop')).toBeUndefined()
    expect(park.take('keep')?.get('a.go')).toBe(1)
  })
})

describe('pruneTabs', () => {
  it('drops matching tabs and any session left empty', () => {
    const park = new SessionTabPark<{ dirty: boolean }>()
    park.park('ws-a', new Map([['a.go', { dirty: false }], ['b.go', { dirty: true }]]))
    park.park('ws-b', new Map([['c.go', { dirty: false }]]))
    // Theme change: clean tabs are rebuilt from scratch, dirty ones kept.
    park.pruneTabs((tab) => tab.dirty)
    expect([...park.take('ws-a')!.keys()]).toEqual(['b.go'])
    expect(park.take('ws-b')).toBeUndefined()
  })

  it('prunes across every pane park', () => {
    resetSessionTabParksForTests()
    sessionTabPark<{ dirty: boolean }>('main').park('ws', new Map([['a.go', { dirty: false }]]))
    sessionTabPark<{ dirty: boolean }>('split').park('ws', new Map([['b.go', { dirty: false }]]))
    pruneAllSessionTabParks<{ dirty: boolean }>((tab) => tab.dirty)
    expect(sessionTabPark<{ dirty: boolean }>('main').take('ws')).toBeUndefined()
    expect(sessionTabPark<{ dirty: boolean }>('split').take('ws')).toBeUndefined()
  })
})

describe('sessionTabPark', () => {
  beforeEach(() => {
    resetSessionTabParksForTests()
  })

  it('shares one LRU per pane across callers', () => {
    const a = sessionTabPark<number>('main')
    a.park('ws', new Map([['a.go', 1]]))
    expect(sessionTabPark<number>('main').take('ws')?.get('a.go')).toBe(1)
    expect(sessionTabPark<number>('split').take('ws')).toBeUndefined()
  })

  it('pruneSessionTabParks drops sessions whose workspace id is gone', () => {
    const main = sessionTabPark<number>('main')
    const split = sessionTabPark<number>('split')
    main.park('proj\0live', new Map([['a.go', 1]]))
    main.park('proj\0gone', new Map([['b.go', 1]]))
    split.park('proj\0gone', new Map([['c.go', 1]]))
    pruneSessionTabParks(['live'])
    expect(main.take('proj\0live')?.get('a.go')).toBe(1)
    expect(main.take('proj\0gone')).toBeUndefined()
    expect(split.take('proj\0gone')).toBeUndefined()
  })
})
