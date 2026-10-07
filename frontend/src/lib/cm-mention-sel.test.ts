import { describe, expect, test } from 'vitest'
import { EditorSelection, EditorState, Text } from '@codemirror/state'
import { mentionRangeFromLoc, mentionSelectionFromState } from './cm-mention-sel'

/** Three lines: alpha / beta / gamma (newline after alpha at 5, beta at 10). */
const DOC = 'alpha\nbeta\ngamma'

function stateAt(from: number, to: number): EditorState {
  return EditorState.create({
    doc: DOC,
    selection: EditorSelection.range(from, to),
  })
}

describe('mentionSelectionFromState', () => {
  test('collapsed caret attaches nothing (whole file)', () => {
    const state = EditorState.create({
      doc: DOC,
      selection: EditorSelection.cursor(2),
    })
    expect(mentionSelectionFromState(state)).toBeNull()
  })

  test('full line without newline is a line chip, not columns', () => {
    expect(mentionSelectionFromState(stateAt(0, 5))).toEqual({
      startLine: 1,
      endLine: 1,
    })
  })

  test('full line including newline still omits columns', () => {
    expect(mentionSelectionFromState(stateAt(0, 6))).toEqual({
      startLine: 1,
      endLine: 1,
    })
  })

  test('full multi-line range omits columns', () => {
    expect(mentionSelectionFromState(stateAt(0, 11))).toEqual({
      startLine: 1,
      endLine: 2,
    })
  })

  test('partial single line records 1-based inclusive columns', () => {
    // 'lph' in alpha (positions 1..4 exclusive 4)
    expect(mentionSelectionFromState(stateAt(1, 4))).toEqual({
      startLine: 1,
      endLine: 1,
      startCol: 2,
      endCol: 4,
    })
  })

  test('single character is a same-line column span', () => {
    expect(mentionSelectionFromState(stateAt(2, 3))).toEqual({
      startLine: 1,
      endLine: 1,
      startCol: 3,
      endCol: 3,
    })
  })

  test('partial span across lines records start and end columns', () => {
    // 'pha\nbe' — start col 3 of alpha through col 2 of beta
    expect(mentionSelectionFromState(stateAt(2, 8))).toEqual({
      startLine: 1,
      endLine: 2,
      startCol: 3,
      endCol: 2,
    })
  })

  test('selection ending at column 0 of the next line excludes that line', () => {
    expect(mentionSelectionFromState(stateAt(6, 11))).toEqual({
      startLine: 2,
      endLine: 2,
    })
  })
})

describe('mentionRangeFromLoc', () => {
  const doc = Text.of(['alpha', 'beta', 'gamma'])

  test('whole line selects from start to end of line', () => {
    expect(mentionRangeFromLoc(doc, { startLine: 2 })).toEqual({
      from: 6,
      to: 10,
    })
  })

  test('line span selects through the last line', () => {
    expect(mentionRangeFromLoc(doc, { startLine: 1, endLine: 2 })).toEqual({
      from: 0,
      to: 10,
    })
  })

  test('column span is exclusive at the end so one char is from..from+1', () => {
    expect(mentionRangeFromLoc(doc, {
      startLine: 1,
      startCol: 3,
      endCol: 3,
    })).toEqual({ from: 2, to: 3 })
  })

  test('cross-line column span maps both ends', () => {
    expect(mentionRangeFromLoc(doc, {
      startLine: 1,
      endLine: 2,
      startCol: 3,
      endCol: 2,
    })).toEqual({ from: 2, to: 8 })
  })
})
