import { describe, expect, test } from 'vitest'
import { createHoverGate, hoverGateAllows, resetHoverGate } from './hoverGate'

describe('hoverGate', () => {
  test('ignores first position (menu opened under idle cursor)', () => {
    const g = createHoverGate()
    expect(hoverGateAllows(g, 100, 200)).toBe(false)
    expect(hoverGateAllows(g, 100, 200)).toBe(false)
    expect(g.armed).toBe(false)
  })

  test('ignores sub-threshold jitter', () => {
    const g = createHoverGate()
    expect(hoverGateAllows(g, 10, 10)).toBe(false)
    expect(hoverGateAllows(g, 12, 11)).toBe(false)
    expect(g.armed).toBe(false)
  })

  test('arms after real movement past threshold and allows thereafter', () => {
    const g = createHoverGate()
    expect(hoverGateAllows(g, 10, 10)).toBe(false)
    expect(hoverGateAllows(g, 15, 10)).toBe(true)
    expect(g.armed).toBe(true)
    expect(hoverGateAllows(g, 15, 10)).toBe(true)
    expect(hoverGateAllows(g, 50, 50)).toBe(true)
  })

  test('reset clears arm so a reopened menu ignores idle cursor again', () => {
    const g = createHoverGate()
    hoverGateAllows(g, 0, 0)
    hoverGateAllows(g, 10, 0)
    expect(g.armed).toBe(true)
    resetHoverGate(g)
    expect(g.armed).toBe(false)
    expect(hoverGateAllows(g, 10, 0)).toBe(false)
    expect(hoverGateAllows(g, 10, 0)).toBe(false)
    expect(hoverGateAllows(g, 14, 0)).toBe(true)
  })
})
