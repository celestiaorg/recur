import { describe, expect, test } from 'vitest'
import {
  canDisconnectCursor,
  cursorUsageVisible,
  formatResetIn,
  formatUsagePct,
  modelBuckets,
  providerChips,
  snapshotHasData,
  snapshotHasProviderError,
} from './providerUsage'
import { Snapshot } from '../../bindings/recur/internal/usage/models'

function snap(partial: ConstructorParameters<typeof Snapshot>[0]): Snapshot {
  return new Snapshot(partial)
}

describe('providerChips', () => {
  test('one chip per provider; stacks session rings; skips extra', () => {
    const chips = providerChips(snap({
      providers: [
        {
          id: 'claude-code',
          name: 'Claude',
          windows: [
            { id: 'five_hour', label: '5-hour', usedPct: 0 },
            { id: 'seven_day', label: '7-day', usedPct: 34 },
            { id: 'seven_day_fable', label: 'Fable (7-day)', usedPct: 12 },
            { id: 'extra', label: 'Extra', usedPct: 100 },
          ],
        },
        { id: 'cursor', name: 'Cursor', email: 'dev@example.com', windows: [{ id: 'billing', label: 'Included usage', usedPct: 27.4 }] },
      ],
    }))
    expect(chips.map((c) => c.id)).toEqual(['claude-code', 'cursor'])
    expect(chips[0].rings.map((r) => r.id)).toEqual(['five_hour', 'seven_day', 'seven_day_fable'])
    expect(chips[0].headline).toBe(34)
    expect(chips[1].headline).toBe(27.4)
  })

  test('Codex monthly credits is a chip ring after session windows', () => {
    const chips = providerChips(snap({
      providers: [{
        id: 'codex',
        name: 'Codex',
        windows: [
          { id: 'five_hour', label: '5-hour', usedPct: 100 },
          { id: 'seven_day', label: '7-day', usedPct: 16 },
          { id: 'billing', label: 'Monthly', usedPct: 100 },
          { id: 'extra', label: 'Credits', usedPct: 0, detail: '45.25' },
        ],
      }],
    }))
    expect(chips).toHaveLength(1)
    expect(chips[0].rings.map((r) => r.id)).toEqual(['five_hour', 'seven_day', 'billing'])
    expect(chips[0].headline).toBe(100)
  })

  test('skips providers that are off in Settings', () => {
    const chips = providerChips(snap({
      providers: [
        { id: 'claude-code', name: 'Claude', windows: [{ id: 'five_hour', label: '5-hour', usedPct: 10 }] },
        { id: 'cursor', name: 'Cursor', email: 'a@b.c', windows: [{ id: 'billing', label: 'Included usage', usedPct: 20 }] },
      ],
    }), ['cursor'])
    expect(chips.map((c) => c.id)).toEqual(['claude-code'])
  })
})

describe('cursorUsageVisible', () => {
  test('hides leftover Cursor windows without an account', () => {
    expect(cursorUsageVisible({ id: 'cursor' })).toBe(false)
    expect(cursorUsageVisible({ id: 'cursor', email: 'a@b.c' })).toBe(true)
    expect(cursorUsageVisible({ id: 'cursor', sourceId: 'cursor_connect' })).toBe(true)
    expect(cursorUsageVisible({ id: 'claude-code' })).toBe(true)
  })
})

describe('snapshotHasData', () => {
  test('true when a provider has an error or window', () => {
    expect(snapshotHasData(snap({ providers: [{ id: 'cursor', name: 'Cursor', error: 'no' }] }))).toBe(true)
    expect(snapshotHasData(snap({ providers: [{ id: 'cursor', name: 'Cursor', sourceId: 'cursor_connect', hint: 'off' }] }))).toBe(true)
    expect(snapshotHasData(snap({ providers: [] }))).toBe(false)
    expect(snapshotHasData(snap({
      providers: [{ id: 'cursor', name: 'Cursor', sourceId: 'cursor_connect', hint: 'off' }],
    }), ['cursor'])).toBe(false)
  })
})

describe('snapshotHasProviderError', () => {
  test('true only for recoverable Claude auth errors on enabled rows', () => {
    expect(snapshotHasProviderError(null)).toBe(false)
    expect(snapshotHasProviderError(snap({ providers: [{ id: 'claude-code', name: 'Claude', windows: [{ id: 'five_hour', label: '5-hour', usedPct: 1 }] }] }))).toBe(false)
    expect(snapshotHasProviderError(snap({ providers: [{ id: 'claude-code', name: 'Claude', error: 'Claude Code login expired' }] }))).toBe(true)
    expect(snapshotHasProviderError(snap({ providers: [{ id: 'cursor', name: 'Cursor', error: 'No Cursor usage session' }] }))).toBe(false)
    expect(snapshotHasProviderError(snap({ providers: [{ id: 'codex', name: 'Codex', error: 'Not signed in to Codex' }] }))).toBe(false)
    expect(snapshotHasProviderError(snap({ providers: [{ id: 'claude-code', name: 'Claude', error: 'Claude Code login expired' }] }), ['claude-code'])).toBe(false)
    expect(snapshotHasProviderError(snap({ providers: [{ id: 'claude-code', name: 'Claude', error: 'Usage endpoint is rate limited' }] }))).toBe(false)
  })
})

describe('formatResetIn', () => {
  test('formats future resets', () => {
    const now = Date.parse('2026-08-20T12:00:00Z')
    expect(formatResetIn(now + 90_000, now)).toBe('resets in 1m')
    expect(formatResetIn(now + 3 * 3600_000, now)).toBe('resets in 3h')
  })
})

describe('canDisconnectCursor', () => {
  test('only Recur Connect', () => {
    expect(canDisconnectCursor(snap({ providers: [{ id: 'cursor', name: 'Cursor', sourceId: 'cursor_connect' }] }))).toBe(true)
    expect(canDisconnectCursor(snap({ providers: [{ id: 'cursor', name: 'Cursor' }] }))).toBe(false)
  })
})

describe('formatUsagePct', () => {
  test('one decimal like the dashboard', () => {
    expect(formatUsagePct(27.14)).toBe('27.1%')
    expect(formatUsagePct(0)).toBe('0.0%')
  })
})

describe('modelBuckets', () => {
  test('groups Cursor vs Other and sums tokens and plan %', () => {
    const [cursor, other] = modelBuckets([
      { name: 'cursor-grok-4.6-high-fast', tokens: 590_200_000, usedPct: 22.7, bucket: 'cursor' },
      { name: 'claude-4.6-opus', tokens: 12_000, usedPct: 1.1, bucket: 'other' },
      { name: 'composer-2.5-fast', tokens: 4_300_000, usedPct: 0.1, bucket: 'cursor' },
    ])
    expect(cursor.label).toBe('Cursor Models')
    expect(cursor.tokens).toBe(594_500_000)
    expect(cursor.usedPct).toBeCloseTo(22.8)
    expect(cursor.models).toHaveLength(2)
    expect(other.label).toBe('Other Models')
    expect(other.tokens).toBe(12_000)
    expect(other.usedPct).toBeCloseTo(1.1)
  })
})
