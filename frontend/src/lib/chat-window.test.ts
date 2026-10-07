import { describe, expect, it } from 'vitest'
import { estimateChatBlockHeight, reservedChatHeight } from './chat-window'

describe('estimateChatBlockHeight', () => {
  it('is a short header for a collapsed step group', () => {
    expect(estimateChatBlockHeight({ kind: 'steps' })).toBe(36)
  })

  it('sizes a user bubble smaller than a long assistant reply', () => {
    const me = estimateChatBlockHeight({ kind: 'msg', me: true, body: 'ok' })
    const long = estimateChatBlockHeight({ kind: 'msg', body: 'x'.repeat(400) })
    expect(long).toBeGreaterThan(me)
  })

  it('caps a huge markdown body so a stub cannot own the scroller', () => {
    expect(estimateChatBlockHeight({ kind: 'msg', body: 'x'.repeat(20_000) })).toBe(720)
  })
})

describe('reservedChatHeight', () => {
  it('lets a live block size itself', () => {
    expect(reservedChatHeight(true, 400, 36)).toBeUndefined()
  })

  it('prefers the measured height on a stub', () => {
    expect(reservedChatHeight(false, 240, 36)).toBe(240)
  })

  it('falls back to the estimate when never measured', () => {
    expect(reservedChatHeight(false, 0, 56)).toBe(56)
  })
})
