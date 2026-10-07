/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { HOT_SHELL_MAX, hotShellsEqual, liveWorkspaceIds, touchHotShells } from './hot-shells'

describe('touchHotShells', () => {
  test('caps at two and keeps the previous id', () => {
    expect(HOT_SHELL_MAX).toBe(2)
    expect(touchHotShells([], 'a')).toEqual(['a'])
    expect(touchHotShells(['a'], 'b')).toEqual(['b', 'a'])
    expect(touchHotShells(['b', 'a'], 'c')).toEqual(['c', 'b'])
  })

  test('moving back to a parked id is a reorder, not a third slot', () => {
    expect(touchHotShells(['b', 'a'], 'a')).toEqual(['a', 'b'])
  })

  test('empty id does not shift the LRU', () => {
    expect(touchHotShells(['b', 'a'], '')).toEqual(['b', 'a'])
  })

  test('drops ids that are no longer live', () => {
    expect(touchHotShells(['b', 'a'], 'b', { live: new Set(['b']) })).toEqual(['b'])
    expect(touchHotShells(['gone'], 'a', { live: new Set(['a']) })).toEqual(['a'])
  })

  test('ignores a select of a workspace that is not live', () => {
    expect(touchHotShells(['a'], 'deleted', { live: new Set(['a']) })).toEqual(['a'])
  })
})

describe('liveWorkspaceIds', () => {
  test('drops archived and empty ids', () => {
    expect(liveWorkspaceIds([
      { id: 'a' },
      { id: 'b', archived: true },
      { id: '' },
      { id: 'c', archived: false },
    ])).toEqual(new Set(['a', 'c']))
  })
})

describe('hotShellsEqual', () => {
  test('compares by id order', () => {
    expect(hotShellsEqual(['a', 'b'], ['a', 'b'])).toBe(true)
    expect(hotShellsEqual(['a', 'b'], ['b', 'a'])).toBe(false)
    expect(hotShellsEqual(['a'], ['a', 'b'])).toBe(false)
  })
})
