import { describe, expect, it } from 'vitest'
import { normalizeUiScale } from './stores'

describe('normalizeUiScale', () => {
  it('snaps to 5% steps within 50-150', () => {
    expect(normalizeUiScale(100)).toBe(100)
    expect(normalizeUiScale(87)).toBe(85)
    expect(normalizeUiScale(10)).toBe(50)
    expect(normalizeUiScale(500)).toBe(150)
    expect(normalizeUiScale('abc')).toBe(100)
  })
})
