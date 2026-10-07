import { afterEach, describe, expect, it, vi } from 'vitest'
import { System } from '@wailsio/runtime'
import { isCmdOrCtrl, isLetterKey, paletteRowDigit } from './shortcut'

vi.mock('@wailsio/runtime', () => ({
  System: { IsMac: vi.fn(() => false) },
}))

afterEach(() => {
  vi.mocked(System.IsMac).mockReturnValue(false)
})

describe('isCmdOrCtrl', () => {
  it('is metaKey on macOS', () => {
    vi.mocked(System.IsMac).mockReturnValue(true)
    expect(isCmdOrCtrl({ metaKey: true, ctrlKey: false })).toBe(true)
    expect(isCmdOrCtrl({ metaKey: false, ctrlKey: true })).toBe(false)
  })

  it('is ctrlKey off macOS', () => {
    expect(isCmdOrCtrl({ metaKey: false, ctrlKey: true })).toBe(true)
    expect(isCmdOrCtrl({ metaKey: true, ctrlKey: false })).toBe(false)
  })
})

describe('isLetterKey', () => {
  it('matches e.code even when Option turns the letter into a mark', () => {
    expect(isLetterKey({ key: '®', code: 'KeyR' }, 'r')).toBe(true)
    expect(isLetterKey({ key: 'å', code: 'KeyA' }, 'a')).toBe(true)
  })

  it('falls back to e.key when code is not Key*', () => {
    expect(isLetterKey({ key: 'r', code: '' }, 'r')).toBe(true)
    expect(isLetterKey({ key: 'R', code: '' }, 'r')).toBe(true)
  })

  it('rejects a different letter', () => {
    expect(isLetterKey({ key: '®', code: 'KeyR' }, 'b')).toBe(false)
    expect(isLetterKey({ key: 'b', code: 'KeyB' }, 'r')).toBe(false)
  })

  it('prefers e.code over e.key when both are letters (Dvorak KeyR is p)', () => {
    expect(isLetterKey({ key: 'p', code: 'KeyR' }, 'r')).toBe(true)
    expect(isLetterKey({ key: 'p', code: 'KeyR' }, 'p')).toBe(false)
  })

  it('rejects AltGraph so AltGr typing is not an Option+letter chord', () => {
    const altGr = (k: string) => k === 'AltGraph'
    expect(isLetterKey({ key: 'ó', code: 'KeyO', getModifierState: altGr }, 'o')).toBe(false)
    expect(isLetterKey({ key: '€', code: 'KeyE', getModifierState: altGr }, 'e')).toBe(false)
    expect(isLetterKey({ key: 'e', code: 'KeyE', getModifierState: altGr }, 'e')).toBe(true)
    expect(isLetterKey({ key: '®', code: 'KeyR', getModifierState: () => false }, 'r')).toBe(true)
  })

  it('does not treat macOS Option as AltGraph', () => {
    vi.mocked(System.IsMac).mockReturnValue(true)
    expect(isLetterKey({ key: '®', code: 'KeyR', getModifierState: (k) => k === 'AltGraph' }, 'r')).toBe(true)
  })
})

describe('paletteRowDigit', () => {
  function key(partial: Partial<KeyboardEvent>): KeyboardEvent {
    return {
      key: '',
      code: '',
      metaKey: false,
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
      ...partial,
    } as KeyboardEvent
  }

  it('reads 1–9 from e.key', () => {
    expect(paletteRowDigit(key({ key: '1', code: 'Digit1' }))).toBe(1)
    expect(paletteRowDigit(key({ key: '9', code: 'Digit9' }))).toBe(9)
    expect(paletteRowDigit(key({ key: '0', code: 'Digit0' }))).toBeNull()
  })

  it('reads DigitN when Shift turns 1 into ! if shiftDigits is on', () => {
    expect(paletteRowDigit(key({ key: '!', code: 'Digit1', shiftKey: true }), { shiftDigits: true })).toBe(1)
  })

  it('lets shifted symbols through without shiftDigits (Open in / Run palettes)', () => {
    expect(paletteRowDigit(key({ key: '!', code: 'Digit1', shiftKey: true }))).toBeNull()
  })

  it('ignores chords with cmd/ctrl/alt', () => {
    expect(paletteRowDigit(key({ key: '1', code: 'Digit1', metaKey: true }))).toBeNull()
  })
})
