import { describe, expect, test } from 'vitest'
import { applyChatTabOrder, insertIndexFromPointerX, reorderByInsert, restoreStripOrder } from './chat-tab-reorder'

describe('reorderByInsert', () => {
  test('moves an item to a gap in the remaining list', () => {
    expect(reorderByInsert(['a', 'b', 'c'], 2, 0)).toEqual(['c', 'a', 'b'])
    expect(reorderByInsert(['a', 'b', 'c'], 0, 2)).toEqual(['b', 'c', 'a'])
    expect(reorderByInsert(['a', 'b', 'c'], 1, 1)).toEqual(['a', 'b', 'c'])
  })

  test('clamps a past-the-end insert and ignores a bad fromIndex', () => {
    expect(reorderByInsert(['a', 'b'], 0, 9)).toEqual(['b', 'a'])
    expect(reorderByInsert(['a', 'b'], -1, 0)).toEqual(['a', 'b'])
    expect(reorderByInsert(['a', 'b'], 2, 0)).toEqual(['a', 'b'])
  })
})

describe('insertIndexFromPointerX', () => {
  test('picks the first midpoint to the right of the pointer', () => {
    expect(insertIndexFromPointerX([10, 30, 50], 5)).toBe(0)
    expect(insertIndexFromPointerX([10, 30, 50], 30)).toBe(2)
    expect(insertIndexFromPointerX([10, 30, 50], 29)).toBe(1)
    expect(insertIndexFromPointerX([10, 30, 50], 80)).toBe(3)
  })

  test('appends when there are no other chips', () => {
    expect(insertIndexFromPointerX([], 40)).toBe(0)
  })
})

describe('restoreStripOrder', () => {
  const key = (s: string) => s

  test('restores snapshot order for keys that are still live', () => {
    expect(restoreStripOrder(['a', 'c', 'b'], ['c', 'a', 'b'], key)).toEqual(['c', 'a', 'b'])
  })

  test('drops snapshot keys that left and appends keys that arrived', () => {
    expect(restoreStripOrder(['a', 'd'], ['c', 'a', 'b'], key)).toEqual(['a', 'd'])
    expect(restoreStripOrder(['c', 'a', 'b', 'd'], ['c', 'a', 'b'], key)).toEqual(['c', 'a', 'b', 'd'])
  })

  test('captured-owner restore does not write into a different workspace list', () => {
    const byWs: Record<string, string[]> = {
      W1: ['PA', 'MA', 'PA2'],
      W2: ['MA', 'PA'],
    }
    const ownerId = 'W1'
    byWs[ownerId] = restoreStripOrder(byWs[ownerId]!, ['MA', 'PA', 'PA2'], key)
    expect(byWs.W1).toEqual(['MA', 'PA', 'PA2'])
    expect(byWs.W2).toEqual(['MA', 'PA'])
  })
})

describe('applyChatTabOrder', () => {
  const ma = { code: 'MA' }
  const pa = { code: 'PA' }
  const pa2 = { code: 'PA2' }
  const pa3 = { code: 'PA3' }
  const def = [ma, pa, pa2, pa3]

  test('empty order keeps default', () => {
    expect(applyChatTabOrder(def, []).map((t) => t.code)).toEqual(['MA', 'PA', 'PA2', 'PA3'])
  })

  test('single tab ignores order', () => {
    expect(applyChatTabOrder([pa], ['PA2', 'PA']).map((t) => t.code)).toEqual(['PA'])
  })

  test('saved permutation', () => {
    expect(applyChatTabOrder(def, ['PA2', 'MA', 'PA3', 'PA']).map((t) => t.code))
      .toEqual(['PA2', 'MA', 'PA3', 'PA'])
  })

  test('closed codes are skipped', () => {
    expect(applyChatTabOrder([ma, pa, pa3], ['PA2', 'PA3', 'MA', 'PA']).map((t) => t.code))
      .toEqual(['PA3', 'MA', 'PA'])
  })

  test('new pair tab appends', () => {
    expect(applyChatTabOrder(def, ['PA2', 'MA', 'PA']).map((t) => t.code))
      .toEqual(['PA2', 'MA', 'PA', 'PA3'])
  })

  test('new Loop still leads', () => {
    expect(applyChatTabOrder(def, ['PA2', 'PA', 'PA3']).map((t) => t.code))
      .toEqual(['MA', 'PA2', 'PA', 'PA3'])
  })

  test('duplicate order codes are ignored', () => {
    expect(applyChatTabOrder([pa, pa2], ['PA2', 'PA2', 'PA']).map((t) => t.code))
      .toEqual(['PA2', 'PA'])
  })
})
