import { describe, expect, it } from 'vitest'
import { expandedFromIds, idsFromExpanded } from './schedule-expanded'

describe('schedule expanded ids', () => {
  it('defaults to none expanded', () => {
    expect(expandedFromIds(null)).toEqual({})
    expect(expandedFromIds(undefined)).toEqual({})
    expect(expandedFromIds([])).toEqual({})
  })

  it('round-trips open ids and drops collapsed ones', () => {
    expect(expandedFromIds(['a', '', 'c'])).toEqual({ a: true, c: true })
    expect(idsFromExpanded({ a: true, b: false, c: true })).toEqual(['a', 'c'])
  })
})
