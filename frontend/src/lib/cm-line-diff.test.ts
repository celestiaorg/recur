import { describe, expect, it } from 'vitest'
import { Change, diff } from '@codemirror/merge'
import {
  exclusiveLineAt,
  lineHunks,
  lineIndexAt,
  lineIndexToOffset,
  lineOrientedDiff,
  refineForHighlights,
  splitLines,
  toCharChanges,
} from './cm-line-diff'

/** Assert helpers — thin views over Change[], not a parallel API under test. */
function slices(a: string, b: string, changes: readonly Change[]) {
  return changes.map((ch) => ({
    a: a.slice(ch.fromA, ch.toA),
    b: b.slice(ch.fromB, ch.toB),
  }))
}

function changeSpan(ch: Change) {
  return { a: ch.toA - ch.fromA, b: ch.toB - ch.fromB }
}

describe('splitLines / lineIndexToOffset / toCharChanges', () => {
  it('records start offsets including the final empty segment after a trailing newline', () => {
    const s = splitLines('ab\nc\n')
    expect(s.lines).toEqual(['ab', 'c', ''])
    expect(s.starts).toEqual([0, 3, 5])
    expect(lineIndexToOffset(s, 0, s.starts[s.starts.length - 1]!)).toBe(0)
    expect(lineIndexToOffset(s, 1, 5)).toBe(3)
    expect(lineIndexToOffset(s, 2, 5)).toBe(5)
    expect(lineIndexToOffset(s, 99, 5)).toBe(5)
    expect(lineIndexAt(s, 0, 5)).toBe(0)
    expect(lineIndexAt(s, 3, 5)).toBe(1)
    expect(lineIndexAt(s, 5, 5)).toBe(3)
    expect(exclusiveLineAt(s, 3, 5)).toBe(1)
    expect(exclusiveLineAt(s, 4, 5)).toBe(2)
  })

  it('maps line-index Changes back to character ranges', () => {
    const a = 'aa\nbb\ncc\n'
    const b = 'aa\nXX\ncc\n'
    const A = splitLines(a)
    const B = splitLines(b)
    // Line 1 replaced (line indices 1..2).
    const expanded = toCharChanges([new Change(1, 2, 1, 2)], A, B, a.length, b.length)
    expect(expanded).toHaveLength(1)
    expect(a.slice(expanded[0].fromA, expanded[0].toA)).toBe('bb\n')
    expect(b.slice(expanded[0].fromB, expanded[0].toB)).toBe('XX\n')
  })
})

describe('refineForHighlights', () => {
  it('leaves pure inserts and deletes untouched', () => {
    const a = 'a\nc\n'
    const b = 'a\nIN\nc\n'
    const lineChanges = [new Change(2, 2, 2, 5)] // insert "IN\n"
    expect(refineForHighlights(a, b, lineChanges)).toEqual(lineChanges)
  })

  it('narrows a whole-line replace to the edited token', () => {
    const a = 'keep\nalpha = 1\ntail\n'
    const b = 'keep\nalpha = 2\ntail\n'
    const lineChanges = [new Change(5, 15, 5, 15)] // "alpha = 1\n" / "alpha = 2\n"
    const refined = refineForHighlights(a, b, lineChanges)
    expect(slices(a, b, refined)).toEqual([{ a: '1', b: '2' }])
  })

  it('skips refine when the hunk is huge', () => {
    const a = 'x'.repeat(12_000)
    const b = 'y'.repeat(12_000)
    const lineChanges = [new Change(0, a.length, 0, b.length)]
    expect(refineForHighlights(a, b, lineChanges)).toEqual(lineChanges)
  })
})

describe('lineHunks', () => {
  it('returns no hunks for identical documents', () => {
    expect(lineHunks('a\nb\n', 'a\nb\n')).toEqual([])
  })

  it('reports inserted lines as an empty-A span', () => {
    expect(lineHunks('a\nc\n', 'a\nIN\nc\n')).toEqual([
      { fromA: 1, toA: 1, fromB: 1, toB: 2 },
    ])
  })

  it('reports deleted lines as an empty-B span', () => {
    expect(lineHunks('a\nX\nY\nc\n', 'a\nc\n')).toEqual([
      { fromA: 1, toA: 3, fromB: 1, toB: 1 },
    ])
  })

  it('reports a replaced line without collapsing neighbors', () => {
    expect(lineHunks('keep\nalpha = 1\ntail\n', 'keep\nalpha = 2\ntail\n')).toEqual([
      { fromA: 1, toA: 2, fromB: 1, toB: 2 },
    ])
  })

  it('returns no hunks when unique lines exhaust the encode alphabet', () => {
    const a = Array.from({ length: 8000 }, (_, i) => `L${i}_aaaa`).join('\n')
    const b = a.replace('L50_aaaa', 'L50_EDIT')
    expect(lineHunks(a, b)).toEqual([])
  })
})

describe('lineOrientedDiff', () => {
  it('returns no changes for identical documents', () => {
    const doc = 'a\nb\nc\n'
    expect(lineOrientedDiff(doc, doc)).toEqual([])
  })

  it('keeps distant one-line edits as separate changes (not one whole-file Change)', () => {
    const lines = Array.from({ length: 200 }, (_, i) => `func f${i}() { return ${i} }`)
    const base = lines.join('\n') + '\n'
    const cur =
      lines
        .map((l, i) => (i === 20 ? 'func f20() { return 999 }' : i === 150 ? 'func f150() { return -1 }' : l))
        .join('\n') + '\n'

    const changes = lineOrientedDiff(base, cur)
    expect(changes).toHaveLength(2)
    const hits = slices(base, cur, changes)
    // Word refine narrows to the return values, not the whole line.
    expect(hits).toEqual(
      expect.arrayContaining([
        { a: '20', b: '999' },
        { a: '150', b: '-1' },
      ]),
    )
    // Neither change should swallow most of the file.
    const docLen = base.length
    expect(changes.every((ch) => changeSpan(ch).a < docLen / 4 && changeSpan(ch).b < docLen / 4)).toBe(true)
  })

  it('reports a pure insertion as empty-A / inserted-B', () => {
    const base = ['a', 'b', 'c', 'd', 'e'].join('\n') + '\n'
    const cur = ['a', 'b', 'X', 'Y', 'c', 'd', 'e'].join('\n') + '\n'
    expect(slices(base, cur, lineOrientedDiff(base, cur))).toEqual([{ a: '', b: 'X\nY\n' }])
  })

  it('reports a pure deletion as deleted-A / empty-B', () => {
    const base = ['a', 'b', 'X', 'Y', 'c', 'd', 'e'].join('\n') + '\n'
    const cur = ['a', 'b', 'c', 'd', 'e'].join('\n') + '\n'
    expect(slices(base, cur, lineOrientedDiff(base, cur))).toEqual([{ a: 'X\nY\n', b: '' }])
  })

  it('stays interactive on large wholly-different inputs', () => {
    const a = Array.from({ length: 4000 }, (_, i) => `left_${i}_aaaaaaaa`).join('\n')
    const b = Array.from({ length: 4000 }, (_, i) => `right_${i}_bbbbbbbb`).join('\n')
    const t0 = performance.now()
    const changes = lineOrientedDiff(a, b)
    const ms = performance.now() - t0
    expect(changes.length).toBeGreaterThanOrEqual(1)
    expect(ms).toBeLessThan(500)
  })

  it('refines a one-line replace so highlightChanges can mark a token, not the whole line', () => {
    const base = ['keep', 'alpha = 1', 'tail'].join('\n') + '\n'
    const cur = ['keep', 'alpha = 2', 'tail'].join('\n') + '\n'
    expect(slices(base, cur, lineOrientedDiff(base, cur))).toEqual([{ a: '1', b: '2' }])
  })

  it('refines multiple token edits on one line into separate changes', () => {
    const base = 'foo = 1; bar = 2\n'
    const cur = 'foo = 9; bar = 8\n'
    const hits = slices(base, cur, lineOrientedDiff(base, cur))
    expect(hits.length).toBeGreaterThanOrEqual(2)
    expect(hits.some((h) => h.a === '1' && h.b === '9')).toBe(true)
    expect(hits.some((h) => h.a === '2' && h.b === '8')).toBe(true)
    expect(hits.some((h) => h.a.includes('foo') || h.b.includes('foo'))).toBe(false)
  })

  it('keeps a wholly-different line as one replace (no false shared tokens)', () => {
    const base = 'aaaaaa\n'
    const cur = 'bbbbbb\n'
    const hits = slices(base, cur, lineOrientedDiff(base, cur))
    expect(hits).toHaveLength(1)
    expect(hits[0].a.replace(/\n$/, '')).toBe('aaaaaa')
    expect(hits[0].b.replace(/\n$/, '')).toBe('bbbbbb')
  })

  it('does not refine pure inserts (whole inserted lines stay line-level)', () => {
    const base = 'a\nc\n'
    const cur = 'a\nINSERTED\nc\n'
    expect(slices(base, cur, lineOrientedDiff(base, cur))).toEqual([{ a: '', b: 'INSERTED\n' }])
  })

  it('avoids CM scanLimit:500 whole-file collapse on large edited files', () => {
    // Repro shape of TokenFactory.sol: short shared prefix, many unique lines,
    // scattered edits + inserts. Default merge scanLimit:500 → 1 mega-Change.
    const lines = ['hdr']
    for (let i = 0; i < 1200; i++) lines.push(`L${i}_${'x'.repeat(40)}`)
    const base = lines.join('\n') + '\n'
    const curLines = lines.map((l, i) => (i % 17 === 0 ? `${l}_EDIT` : l))
    for (let k = 0; k < 10; k++) {
      curLines.splice(50 + k * 100, 0, ...Array.from({ length: 20 }, (_, j) => `INS${k}_${j}`))
    }
    const cur = curLines.join('\n') + '\n'

    const crude = diff(base, cur, { scanLimit: 500 })
    expect(crude.length).toBe(1)

    const changes = lineOrientedDiff(base, cur)
    expect(changes.length).toBeGreaterThan(20)
    const maxTouched = Math.max(...changes.map((ch) => changeSpan(ch).a + changeSpan(ch).b))
    expect(maxTouched).toBeLessThan(base.length / 4)
  })
})
