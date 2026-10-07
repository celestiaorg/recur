/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { Chunk, unifiedMergeView } from '@codemirror/merge'
import { EditorState, Text } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { lineOrientedDiff } from './cm-line-diff'
import {
  lineFullyCoveredByChanges,
  suppressFullLineWordMarks,
} from './cm-merge-full-line-marks'

describe('lineFullyCoveredByChanges', () => {
  it('is true when one change covers the whole line', () => {
    // line at [10, 20), chunk starts at 10, change relative 0..10
    expect(
      lineFullyCoveredByChanges(10, [{ fromB: 0, toB: 10 }], 10, 20),
    ).toBe(true)
  })

  it('is false when a middle token is marked', () => {
    expect(
      lineFullyCoveredByChanges(10, [{ fromB: 3, toB: 7 }], 10, 20),
    ).toBe(false)
  })

  it('is true when several changes union to the whole line', () => {
    expect(
      lineFullyCoveredByChanges(
        0,
        [
          { fromB: 0, toB: 4 },
          { fromB: 4, toB: 10 },
        ],
        0,
        10,
      ),
    ).toBe(true)
  })

  it('is false when a gap remains', () => {
    expect(
      lineFullyCoveredByChanges(
        0,
        [
          { fromB: 0, toB: 4 },
          { fromB: 6, toB: 10 },
        ],
        0,
        10,
      ),
    ).toBe(false)
  })

  it('is false for empty lines', () => {
    expect(lineFullyCoveredByChanges(0, [{ fromB: 0, toB: 0 }], 5, 5)).toBe(false)
  })

  it('clips changes that extend past the line bounds', () => {
    // Change relative spans before and after the line; intersection still covers it.
    expect(
      lineFullyCoveredByChanges(10, [{ fromB: -5, toB: 20 }], 10, 20),
    ).toBe(true)
  })

  it('ignores changes that do not intersect the line', () => {
    expect(
      lineFullyCoveredByChanges(10, [{ fromB: 20, toB: 30 }], 10, 20),
    ).toBe(false)
  })

  it('handles overlapping intervals without double-counting gaps', () => {
    expect(
      lineFullyCoveredByChanges(
        0,
        [
          { fromB: 0, toB: 6 },
          { fromB: 3, toB: 10 },
        ],
        0,
        10,
      ),
    ).toBe(true)
  })

  it('with lineOrientedDiff chunks: token refine is not covered; whole-line replace is', () => {
    // Integration with Chunk.build (merge package) — belongs here, not in line-diff tests.
    const pad = ['p1', 'p2', 'p3', 'p4']
    const base = ['keep', 'alpha = 1', ...pad, 'zzzzzz', 'tail'].join('\n') + '\n'
    const cur = ['keep', 'alpha = 2', ...pad, 'yyyyyy', 'tail'].join('\n') + '\n'
    const tb = Text.of(cur.split('\n'))
    const chunks = Chunk.build(Text.of(base.split('\n')), tb, { override: lineOrientedDiff })
    expect(chunks).toHaveLength(2)

    const tokenLine = tb.lineAt(chunks[0].fromB)
    expect(tokenLine.text).toBe('alpha = 2')
    expect(
      lineFullyCoveredByChanges(chunks[0].fromB, chunks[0].changes, tokenLine.from, tokenLine.to),
    ).toBe(false)

    const wholeLine = tb.lineAt(chunks[1].fromB)
    expect(wholeLine.text).toBe('yyyyyy')
    expect(
      lineFullyCoveredByChanges(chunks[1].fromB, chunks[1].changes, wholeLine.from, wholeLine.to),
    ).toBe(true)
  })

  it('with lineOrientedDiff chunks: pure inserts are full-line covered', () => {
    const base = 'a\nc\n'
    const cur = 'a\nNEW\nc\n'
    const tb = Text.of(cur.split('\n'))
    const chunks = Chunk.build(Text.of(base.split('\n')), tb, { override: lineOrientedDiff })
    expect(chunks).toHaveLength(1)
    const line = tb.lineAt(chunks[0].fromB)
    expect(line.text).toBe('NEW')
    expect(
      lineFullyCoveredByChanges(chunks[0].fromB, chunks[0].changes, line.from, line.to),
    ).toBe(true)
  })
})

describe('suppressFullLineWordMarks (mounted merge view)', () => {
  function mount(original: string, doc: string) {
    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc,
        extensions: [
          unifiedMergeView({
            original,
            mergeControls: false,
            gutter: true,
            highlightChanges: true,
            diffConfig: { override: lineOrientedDiff },
          }),
          suppressFullLineWordMarks(),
        ],
      }),
    })
    return { view, parent }
  }

  function lineEls(parent: HTMLElement) {
    return [...parent.querySelectorAll('.cm-line')] as HTMLElement[]
  }

  it('tags wholly-changed lines with cm-fullLineChange, not token edits', () => {
    const pad = ['p1', 'p2', 'p3', 'p4']
    const original = ['keep', 'alpha = 1', ...pad, 'zzzzzz', 'tail'].join('\n') + '\n'
    const doc = ['keep', 'alpha = 2', ...pad, 'yyyyyy', 'tail'].join('\n') + '\n'
    const { view, parent } = mount(original, doc)

    const lines = lineEls(parent)
    const byText = (t: string) => lines.find((el) => el.textContent === t)

    const tokenLine = byText('alpha = 2')
    const wholeLine = byText('yyyyyy')
    const keep = byText('keep')

    expect(tokenLine).toBeTruthy()
    expect(wholeLine).toBeTruthy()
    expect(tokenLine!.classList.contains('cm-fullLineChange')).toBe(false)
    expect(wholeLine!.classList.contains('cm-fullLineChange')).toBe(true)
    expect(keep!.classList.contains('cm-fullLineChange')).toBe(false)

    // Token edit still has a word mark; whole-line replace should too in the DOM,
    // but CSS clears the fill when cm-fullLineChange is present.
    expect(tokenLine!.querySelector('.cm-changedText')).toBeTruthy()

    view.destroy()
    parent.remove()
  })

  it('tags pure inserted lines as full-line changes', () => {
    const original = 'a\nc\n'
    const doc = 'a\nNEW\nc\n'
    const { view, parent } = mount(original, doc)

    const inserted = lineEls(parent).find((el) => el.textContent === 'NEW')
    expect(inserted).toBeTruthy()
    expect(inserted!.classList.contains('cm-fullLineChange')).toBe(true)
    expect(inserted!.classList.contains('cm-changedLine')).toBe(true)

    view.destroy()
    parent.remove()
  })
})
