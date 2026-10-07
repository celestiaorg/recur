/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { fitTextareaHeight } from './autosize-textarea'

function mockArea(scrollHeight: number): HTMLTextAreaElement {
  const el = document.createElement('textarea')
  Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => scrollHeight })
  return el
}

describe('fitTextareaHeight', () => {
  test('grows up to max, then scrolls', () => {
    const tall = mockArea(200)
    fitTextareaHeight(tall, { minPx: 64, maxPx: 136 })
    expect(tall.style.height).toBe('136px')
    expect(tall.style.overflowY).toBe('auto')

    const mid = mockArea(90)
    fitTextareaHeight(mid, { minPx: 64, maxPx: 136 })
    expect(mid.style.height).toBe('90px')
    expect(mid.style.overflowY).toBe('hidden')

    const short = mockArea(20)
    fitTextareaHeight(short, { minPx: 64, maxPx: 136 })
    expect(short.style.height).toBe('64px')
    expect(short.style.overflowY).toBe('hidden')
  })
})
