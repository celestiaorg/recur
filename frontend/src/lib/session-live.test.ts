import { describe, expect, test } from 'vitest'
import { SessionLiveFlags } from './session-live'

describe('SessionLiveFlags', () => {
  test('stream events light a card before agent.updated working', () => {
    const f = new SessionLiveFlags()
    expect(f.touchFromStream('CO')).toBe(true)
    expect([...f.snapshot()]).toEqual(['CO'])
    expect(f.touchFromStream('CO')).toBe(false)
  })

  test('idle then late stream / turn_end must not re-light', () => {
    const f = new SessionLiveFlags()
    f.markRunning('CO')
    f.markSettled('CO')
    expect(f.snapshot().size).toBe(0)
    expect(f.touchFromStream('CO')).toBe(false)
    expect(f.clearLive('CO')).toBe(false)
    expect(f.markSettled('CO')).toBe(false)
    expect(f.snapshot().size).toBe(0)
  })

  test('turn_end without idle clears live; the next stream can re-light', () => {
    const f = new SessionLiveFlags()
    f.touchFromStream('CO')
    expect(f.clearLive('CO')).toBe(true)
    expect(f.snapshot().size).toBe(0)
    expect(f.touchFromStream('CO')).toBe(true)
    expect([...f.snapshot()]).toEqual(['CO'])
  })

  test('a later running status starts a new live turn', () => {
    const f = new SessionLiveFlags()
    f.markRunning('CO')
    f.markSettled('CO')
    expect(f.markRunning('CO')).toBe(true)
    expect([...f.snapshot()]).toEqual(['CO'])
    expect(f.touchFromStream('CO')).toBe(false)
  })

  test('reset drops live and settled so a re-attach starts clean', () => {
    const f = new SessionLiveFlags()
    f.markRunning('CO')
    f.markSettled('CO')
    f.reset()
    expect(f.touchFromStream('CO')).toBe(true)
    expect([...f.snapshot()]).toEqual(['CO'])
  })
})
