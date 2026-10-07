import { describe, expect, test } from 'vitest'
import { orderOpenInTargets } from './open-in-pref'

describe('orderOpenInTargets', () => {
  const list = [{ id: 'finder' }, { id: 'cursor' }, { id: 'terminal' }]

  test('empty default keeps catalog order', () => {
    expect(orderOpenInTargets(list, '')).toEqual(list)
  })

  test('unknown default keeps catalog order', () => {
    expect(orderOpenInTargets(list, 'zed')).toEqual(list)
  })

  test('moves the default to the front', () => {
    expect(orderOpenInTargets(list, 'cursor').map((t) => t.id)).toEqual([
      'cursor',
      'finder',
      'terminal',
    ])
    expect(orderOpenInTargets(list, 'terminal').map((t) => t.id)).toEqual([
      'terminal',
      'finder',
      'cursor',
    ])
  })

  test('already-first default is unchanged', () => {
    expect(orderOpenInTargets(list, 'finder')).toEqual(list)
  })
})
