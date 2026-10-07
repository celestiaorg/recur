import { describe, expect, test } from 'vitest'
import { isReservedAppChord, parseKeyEquivalent, RESERVED_APP_CHORDS, chordFromKeyboardEvent, formatKeyEquivalentDisplay } from './keyequiv'

describe('RESERVED_APP_CHORDS', () => {
  test('reserves cmd+l for add-file-to-chat', () => {
    expect(RESERVED_APP_CHORDS.has('cmd+l')).toBe(true)
    expect(isReservedAppChord('cmd+l')).toBe(true)
  })

  test('reserves cmd+r for the run menu', () => {
    expect(RESERVED_APP_CHORDS.has('cmd+r')).toBe(true)
    expect(isReservedAppChord('cmd+r')).toBe(true)
  })

  test('reserves cmd+o for open in', () => {
    expect(RESERVED_APP_CHORDS.has('cmd+o')).toBe(true)
    expect(isReservedAppChord('cmd+o')).toBe(true)
  })

  test('reserves ctrl+shift+up/down for add-cursor', () => {
    expect(isReservedAppChord('ctrl+shift+up')).toBe(true)
    expect(isReservedAppChord('ctrl+shift+down')).toBe(true)
    expect(isReservedAppChord('ctrl+opt+up')).toBe(false)
  })

  test('reserves opt+cmd+up/down for workspace cycling', () => {
    expect(isReservedAppChord('opt+cmd+up')).toBe(true)
    expect(isReservedAppChord('opt+cmd+down')).toBe(true)
  })

  test('reserves shift+cmd+v and ctrl+shift+v for cycle editor view', () => {
    expect(RESERVED_APP_CHORDS.has('shift+cmd+v')).toBe(true)
    expect(isReservedAppChord('shift+cmd+v')).toBe(true)
    expect(RESERVED_APP_CHORDS.has('ctrl+shift+v')).toBe(true)
    expect(isReservedAppChord('ctrl+shift+v')).toBe(true)
  })

  test('reserves opt+cmd+[ / ] for editor theme cycling', () => {
    expect(isReservedAppChord('opt+cmd+[')).toBe(true)
    expect(isReservedAppChord('opt+cmd+]')).toBe(true)
  })

  test('reserves opt+cmd+z for revert diff hunk', () => {
    expect(isReservedAppChord('opt+cmd+z')).toBe(true)
  })
})

describe('parseKeyEquivalent', () => {
  test('canonicalizes cmd+l / command+L', () => {
    expect(parseKeyEquivalent('cmd+l')).toBe('cmd+l')
    expect(parseKeyEquivalent('command+L')).toBe('cmd+l')
  })

  test('accepts ctrl+< (TextMate Insert Tag Pair)', () => {
    expect(parseKeyEquivalent('ctrl+<')).toBe('ctrl+<')
    expect(formatKeyEquivalentDisplay('ctrl+<')).toBe('⌃<')
  })
})

describe('chordFromKeyboardEvent', () => {
  test('maps control+shift+comma to ctrl+<', () => {
    const e = {
      key: ',',
      code: 'Comma',
      ctrlKey: true,
      shiftKey: true,
      altKey: false,
      metaKey: false,
    } as KeyboardEvent
    expect(chordFromKeyboardEvent(e)).toBe('ctrl+<')
  })

  test('does not treat typing < as a chord', () => {
    const e = {
      key: '<',
      code: 'Comma',
      ctrlKey: false,
      shiftKey: true,
      altKey: false,
      metaKey: false,
    } as KeyboardEvent
    expect(chordFromKeyboardEvent(e)).toBeNull()
  })
})
