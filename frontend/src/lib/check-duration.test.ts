import { describe, expect, it } from 'vitest'
import {
  checkClockKey,
  formatCheckDuration,
  liveCheckDetail,
  parseCheckDuration,
  prunePendingClocks,
  type PendingClock,
} from './check-duration'

describe('formatCheckDuration', () => {
  it('matches the Go check duration labels', () => {
    expect(formatCheckDuration(0)).toBe('0s')
    expect(formatCheckDuration(1)).toBe('1s')
    expect(formatCheckDuration(45)).toBe('45s')
    expect(formatCheckDuration(60)).toBe('1m')
    expect(formatCheckDuration(64)).toBe('1m 4s')
    expect(formatCheckDuration(12 * 60)).toBe('12m')
    expect(formatCheckDuration(3600)).toBe('1h')
    expect(formatCheckDuration(3600 + 120)).toBe('1h 2m')
  })
})

describe('parseCheckDuration', () => {
  it('parses the labels formatCheckDuration emits', () => {
    expect(parseCheckDuration('45s')).toBe(45)
    expect(parseCheckDuration('1m')).toBe(60)
    expect(parseCheckDuration('1m 4s')).toBe(64)
    expect(parseCheckDuration('12m')).toBe(12 * 60)
    expect(parseCheckDuration('1h')).toBe(3600)
    expect(parseCheckDuration('1h 2m')).toBe(3600 + 120)
    expect(parseCheckDuration('pending')).toBeNull()
    expect(parseCheckDuration('skipped')).toBeNull()
    expect(parseCheckDuration('')).toBeNull()
  })
})

describe('liveCheckDetail', () => {
  const t0 = Date.parse('2026-08-21T12:00:00Z')

  it('ticks a pending check from startedAt', () => {
    const clocks = new Map<string, PendingClock>()
    const check = { name: 'build', status: 'pending', detail: '45s', startedAt: '2026-08-21T12:00:00Z' }
    expect(liveCheckDetail(check, t0 + 45_000, clocks)).toBe('45s')
    expect(liveCheckDetail(check, t0 + 50_000, clocks)).toBe('50s')
    expect(liveCheckDetail(check, t0 + 64_000, clocks)).toBe('1m 4s')
  })

  it('interpolates from the snapshot until a refresh replaces detail', () => {
    const clocks = new Map<string, PendingClock>()
    const key = checkClockKey({ name: 'build' })
    expect(liveCheckDetail({ name: 'build', status: 'pending', detail: '45s' }, t0, clocks)).toBe('45s')
    expect(liveCheckDetail({ name: 'build', status: 'pending', detail: '45s' }, t0 + 5_000, clocks)).toBe('50s')
    expect(clocks.get(key)?.detail).toBe('45s')
    expect(liveCheckDetail({ name: 'build', status: 'pending', detail: '50s' }, t0 + 5_000, clocks)).toBe('50s')
    expect(liveCheckDetail({ name: 'build', status: 'pending', detail: '50s' }, t0 + 8_000, clocks)).toBe('53s')
  })

  it('settles on the completed duration and drops the clock', () => {
    const clocks = new Map<string, PendingClock>()
    liveCheckDetail({ name: 'build', status: 'pending', detail: '45s' }, t0, clocks)
    expect(clocks.size).toBe(1)
    expect(liveCheckDetail({ name: 'build', status: 'pass', detail: '1m 4s' }, t0 + 20_000, clocks)).toBe('1m 4s')
    expect(clocks.size).toBe(0)
  })

  it('prunes clocks for checks that left the pending set', () => {
    const clocks = new Map<string, PendingClock>()
    liveCheckDetail({ name: 'build', status: 'pending', detail: '10s' }, t0, clocks)
    liveCheckDetail({ name: 'lint', status: 'pending', detail: '4s' }, t0, clocks)
    prunePendingClocks(clocks, [{ name: 'lint', status: 'pending', detail: '4s' }])
    expect([...clocks.keys()]).toEqual([checkClockKey({ name: 'lint' })])
  })
})
