import { describe, expect, it } from 'vitest'
import { completeStreamPrefix } from './stream-reveal'

describe('completeStreamPrefix', () => {
  it('paints a text block that has no newline yet', () => {
    expect(completeStreamPrefix('Hello world', 0)).toBe(11)
  })

  it('holds the trailing partial after a newline', () => {
    expect(completeStreamPrefix('one\ntwo\npartial', 0)).toBe(8)
  })

  it('flushes the trailing partial when the block ends', () => {
    expect(completeStreamPrefix('one\npartial', 0, true)).toBe(11)
  })

  it('does not rewind past what is already shown', () => {
    expect(completeStreamPrefix('one\ntwo', 6)).toBe(6)
  })

  it('clamps when the string shrinks', () => {
    expect(completeStreamPrefix('hi', 10)).toBe(2)
  })
})
