import { describe, expect, it } from 'vitest'
import { simplifyEnabled, simplifyFromSelection } from './simplify'

describe('simplifyEnabled', () => {
  it('defaults on when settings omit the field', () => {
    expect(simplifyEnabled(undefined)).toBe(true)
    expect(simplifyEnabled(null)).toBe(true)
    expect(simplifyEnabled({})).toBe(true)
  })

  it('respects an explicit boolean', () => {
    expect(simplifyEnabled({ simplify: true })).toBe(true)
    expect(simplifyEnabled({ simplify: false })).toBe(false)
  })
})

describe('simplifyFromSelection', () => {
  it('uses the session flag when the chip was set', () => {
    expect(simplifyFromSelection({ simplify: false }, { simplify: true })).toBe(false)
    expect(simplifyFromSelection({ simplify: true }, { simplify: false })).toBe(true)
  })

  it('falls back to the Settings default when the session never chose', () => {
    expect(simplifyFromSelection({}, { simplify: false })).toBe(false)
    expect(simplifyFromSelection(undefined, undefined)).toBe(true)
    expect(simplifyFromSelection({ simplify: null }, { simplify: false })).toBe(false)
  })
})
