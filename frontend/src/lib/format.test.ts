import { describe, expect, it } from 'vitest'
import { formatCompactCount, formatElapsed, formatRelativeAge } from './format'

describe('formatRelativeAge', () => {
  const now = Date.UTC(2026, 7, 18, 13, 0, 0)

  it('returns null for missing timestamps', () => {
    expect(formatRelativeAge(0, now)).toBeNull()
    expect(formatRelativeAge(Number.NaN, now)).toBeNull()
  })

  it('uses compact units', () => {
    expect(formatRelativeAge(now - 10_000, now)?.rel).toBe('just now')
    expect(formatRelativeAge(now - 12 * 60_000, now)?.rel).toBe('12m')
    expect(formatRelativeAge(now - 3 * 3_600_000, now)?.rel).toBe('3h')
    expect(formatRelativeAge(now - 2 * 86_400_000, now)?.rel).toBe('2d')
    expect(formatRelativeAge(now - 13 * 86_400_000, now)?.rel).toBe('13d')
    expect(formatRelativeAge(now - 14 * 86_400_000, now)?.rel).toBe('2w')
    expect(formatRelativeAge(now - 29 * 86_400_000, now)?.rel).toBe('4w')
    expect(formatRelativeAge(now - 40 * 86_400_000, now)?.rel).toBe('1mo')
    expect(formatRelativeAge(now - 400 * 86_400_000, now)?.rel).toBe('1y')
  })

  it('includes a full tooltip stamp', () => {
    const ms = now - 12 * 60_000
    const got = formatRelativeAge(ms, now)
    const expected = new Date(ms).toLocaleString(undefined, {
      month: 'short', day: 'numeric', year: 'numeric',
      hour: 'numeric', minute: '2-digit',
    })
    expect(got?.full).toBe(expected)
  })
})

describe('formatElapsed', () => {
  it('keeps a mm:ss clock under an hour', () => {
    expect(formatElapsed(0)).toBe('0:00')
    expect(formatElapsed(5)).toBe('0:05')
    expect(formatElapsed(65)).toBe('1:05')
    expect(formatElapsed(59 * 60 + 59)).toBe('59:59')
  })

  it('flips to h:mm:ss past 59:59', () => {
    expect(formatElapsed(3600)).toBe('1:00:00')
    expect(formatElapsed(3600 + 16)).toBe('1:00:16')
    expect(formatElapsed(117 * 60 + 16)).toBe('1:57:16')
    expect(formatElapsed(25 * 3600 + 3 * 60)).toBe('25:03:00')
  })
})

describe('formatCompactCount', () => {
  it('keeps small integers literal', () => {
    expect(formatCompactCount(0)).toBe('0')
    expect(formatCompactCount(999)).toBe('999')
  })

  it('uses one decimal for 1k–9.9k', () => {
    expect(formatCompactCount(1000)).toBe('1k')
    expect(formatCompactCount(1200)).toBe('1.2k')
    expect(formatCompactCount(9999)).toBe('10k')
  })

  it('rounds whole thousands above 10k', () => {
    expect(formatCompactCount(40_000)).toBe('40k')
    expect(formatCompactCount(184_200)).toBe('184k')
  })

  it('uses M above a million', () => {
    expect(formatCompactCount(1_000_000)).toBe('1M')
    expect(formatCompactCount(1_600_000)).toBe('1.6M')
  })
})
