import { afterEach, describe, expect, it } from 'vitest'
import type { ObservatorySnapshot } from './observatory-cache'
import {
  dropObservatorySnapshot,
  getObservatorySnapshot,
  observatoryCacheKey,
  pruneObservatorySnapshots,
  putObservatorySnapshot,
  resetObservatoryCacheForTests,
  setObservatoryCacheMaxBytesForTests,
} from './observatory-cache'

function snap(selectedPath: string): ObservatorySnapshot {
  return { selectedPath } as ObservatorySnapshot
}

afterEach(() => {
  resetObservatoryCacheForTests()
})

describe('observatory cache', () => {
  it('round-trips a snapshot', () => {
    const key = observatoryCacheKey('p', 'ws')
    putObservatorySnapshot(key, snap('a.go'))
    expect(getObservatorySnapshot(key)?.selectedPath).toBe('a.go')
  })

  it('evicts the oldest when over the byte budget', () => {
    setObservatoryCacheMaxBytesForTests(1)
    putObservatorySnapshot(observatoryCacheKey('p', 'a'), snap('a.go'))
    putObservatorySnapshot(observatoryCacheKey('p', 'b'), snap('b.go'))
    putObservatorySnapshot(observatoryCacheKey('p', 'c'), snap('c.go'))
    expect(getObservatorySnapshot(observatoryCacheKey('p', 'a'))).toBeNull()
    expect(getObservatorySnapshot(observatoryCacheKey('p', 'c'))?.selectedPath).toBe('c.go')
  })

  it('prunes and drops by workspace id', () => {
    putObservatorySnapshot(observatoryCacheKey('p', 'keep'), snap('k.go'))
    putObservatorySnapshot(observatoryCacheKey('p', 'gone'), snap('g.go'))
    pruneObservatorySnapshots(['keep'])
    expect(getObservatorySnapshot(observatoryCacheKey('p', 'gone'))).toBeNull()
    dropObservatorySnapshot(observatoryCacheKey('p', 'keep'))
    expect(getObservatorySnapshot(observatoryCacheKey('p', 'keep'))).toBeNull()
  })
})
