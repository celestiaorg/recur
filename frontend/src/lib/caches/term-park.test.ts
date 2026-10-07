/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import {
  TERM_PARK_ENTRY_BYTES,
  discardParkedTerminal,
  pruneParkedTerminals,
  parkedTerminalCount,
  resetTermParkForTests,
  setTermParkMaxBytesForTests,
  shouldDisposeTerminal,
  stashParkedTerminal,
  takeParkedTerminal,
  type ParkedTerm,
} from './term-park'

function fake(id: string): ParkedTerm {
  const element = document.createElement('div')
  element.dataset.term = id
  return {
    term: { element, dispose() {} },
    fit: { fit() {} },
    writer: { dispose() {} },
    disposers: [],
    lastCols: 80,
    lastRows: 24,
  }
}

afterEach(() => {
  resetTermParkForTests()
})

describe('term park', () => {
  test('stash then take returns the same widget and reparents the screen', () => {
    const p = fake('a')
    expect(stashParkedTerminal('a', p)).toEqual([])
    expect(parkedTerminalCount()).toBe(1)
    expect(p.term.element?.parentNode).toBeTruthy()
    expect(p.term.element?.isConnected).toBe(false)
    expect(takeParkedTerminal('a')).toBe(p)
    expect(parkedTerminalCount()).toBe(0)
  })

  test('evicts the oldest over the byte budget and returns it for dispose', () => {
    setTermParkMaxBytesForTests(TERM_PARK_ENTRY_BYTES * 2)
    const first = fake('0')
    expect(stashParkedTerminal('0', first)).toEqual([])
    stashParkedTerminal('1', fake('1'))
    const evicted = stashParkedTerminal('2', fake('2'))
    expect(evicted.map((e) => e.id)).toEqual(['0'])
    expect(parkedTerminalCount()).toBe(2)
    expect(takeParkedTerminal('0')).toBeUndefined()
  })

  test('discard takes a parked widget and flags a live pane to dispose', () => {
    stashParkedTerminal('a', fake('a'))
    expect(discardParkedTerminal('a')?.term.element?.dataset.term).toBe('a')
    expect(parkedTerminalCount()).toBe(0)
    discardParkedTerminal('live')
    expect(shouldDisposeTerminal('live')).toBe(true)
    expect(shouldDisposeTerminal('live')).toBe(false)
  })

  test('pruneParkedTerminals drops widgets whose workspace is gone', () => {
    stashParkedTerminal('keep', { ...fake('keep'), workspaceId: 'ws-a' })
    stashParkedTerminal('gone', { ...fake('gone'), workspaceId: 'ws-b' })
    stashParkedTerminal('orphan', fake('orphan'))
    const dropped = pruneParkedTerminals(['ws-a'])
    expect(dropped.map((d) => d.id)).toEqual(['gone'])
    expect(parkedTerminalCount()).toBe(2)
    expect(takeParkedTerminal('keep')).toBeTruthy()
    expect(takeParkedTerminal('orphan')).toBeTruthy()
  })
})
