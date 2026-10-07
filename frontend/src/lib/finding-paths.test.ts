import { describe, expect, it } from 'vitest'
import type { Finding } from '../../bindings/recur/internal/domain/models'
import { extraHtmlWithoutFindingFlags, findingCoversLine, findingsCoveringLine, findingsEndingLine, findingsForPath, findingsOnLine, matchFindingPath, rightDiffHasRange } from './finding-paths'

function f(over: Partial<Finding> & { line: number }): Finding {
  return { id: over.id ?? `f-${over.line}`, file: 'a.ts', message: '', status: 'open', ...over } as Finding
}

describe('matchFindingPath', () => {
  const files = ['src/a.ts', 'lib/b.ts']

  it('prefers an exact match', () => {
    expect(matchFindingPath('src/a.ts', files)).toBe('src/a.ts')
  })

  it('falls back to a slash-anchored suffix', () => {
    expect(matchFindingPath('a.ts', files)).toBe('src/a.ts')
    expect(matchFindingPath('/abs/repo/lib/b.ts', files)).toBe('lib/b.ts')
  })

  it('does not match an unanchored suffix', () => {
    expect(matchFindingPath('api.ts', ['src/openapi.ts'])).toBe('')
    expect(matchFindingPath('in.go', ['src/main.go'])).toBe('')
  })

  it('returns empty when nothing matches', () => {
    expect(matchFindingPath('c.ts', files)).toBe('')
  })

  it('rejects an ambiguous suffix rather than picking the first hit', () => {
    expect(matchFindingPath('index.ts', ['src/index.ts', 'test/index.ts'])).toBe('')
  })
})

describe('findingsForPath', () => {
  const byPath = new Map<string, Finding[]>([
    ['src/a.ts', [f({ line: 1, file: 'src/a.ts' })]],
  ])

  it('returns the exact bucket first', () => {
    expect(findingsForPath(byPath, 'src/a.ts')).toHaveLength(1)
  })

  it('falls back to a slash-anchored suffix match', () => {
    expect(findingsForPath(byPath, 'a.ts')[0]?.file).toBe('src/a.ts')
  })

  it('does not match an unanchored suffix', () => {
    const byApi = new Map<string, Finding[]>([
      ['api.ts', [f({ line: 1, file: 'api.ts' })]],
    ])
    expect(findingsForPath(byApi, 'src/openapi.ts')).toEqual([])
  })
})

describe('findingsOnLine', () => {
  const notes = [f({ id: 'a', line: 12 }), f({ id: 'b', line: 40, endLine: 44 })]

  it('matches the new line number on unified add/context rows', () => {
    expect(findingsOnLine(notes, { new: '12', type: 'add' }).map((x) => x.id)).toEqual(['a'])
    expect(findingsOnLine(notes, { new: '40', type: 'ctx' }).map((x) => x.id)).toEqual(['b'])
  })

  it('excludes deleted rows even when new is populated', () => {
    expect(findingsOnLine(notes, { new: '12', type: 'del' })).toEqual([])
  })

  it('excludes rows without a new line (split-left / del)', () => {
    expect(findingsOnLine(notes, { new: '', type: 'ctx' })).toEqual([])
    expect(findingsOnLine(notes, { new: '0' })).toEqual([])
  })

  it('matches split-right the same as unified (new line only)', () => {
    expect(findingsOnLine(notes, { new: '12' }).map((x) => x.id)).toEqual(['a'])
  })

  it('does not place a range card on interior or end lines', () => {
    expect(findingsOnLine(notes, { new: '42' })).toEqual([])
    expect(findingsOnLine(notes, { new: '44' })).toEqual([])
  })
})

describe('findingCoversLine', () => {
  it('treats EndLine 0 or equal as a single line', () => {
    expect(findingCoversLine({ line: 12 }, 12)).toBe(true)
    expect(findingCoversLine({ line: 12, endLine: 12 }, 12)).toBe(true)
    expect(findingCoversLine({ line: 12 }, 13)).toBe(false)
  })

  it('includes the inclusive span', () => {
    const f = { line: 40, endLine: 44 }
    expect(findingCoversLine(f, 39)).toBe(false)
    expect(findingCoversLine(f, 40)).toBe(true)
    expect(findingCoversLine(f, 42)).toBe(true)
    expect(findingCoversLine(f, 44)).toBe(true)
    expect(findingCoversLine(f, 45)).toBe(false)
  })
})

describe('findingsCoveringLine / findingsEndingLine', () => {
  const notes = [f({ id: 'a', line: 12 }), f({ id: 'b', line: 40, endLine: 44 })]

  it('highlights every new-side row in the span', () => {
    expect(findingsCoveringLine(notes, { new: '12', type: 'add' }).map((x) => x.id)).toEqual(['a'])
    expect(findingsCoveringLine(notes, { new: '40' }).map((x) => x.id)).toEqual(['b'])
    expect(findingsCoveringLine(notes, { new: '42', type: 'ctx' }).map((x) => x.id)).toEqual(['b'])
    expect(findingsCoveringLine(notes, { new: '44' }).map((x) => x.id)).toEqual(['b'])
    expect(findingsCoveringLine(notes, { new: '45' })).toEqual([])
  })

  it('marks note-end on the last line of the span', () => {
    expect(findingsEndingLine(notes, { new: '12' }).map((x) => x.id)).toEqual(['a'])
    expect(findingsEndingLine(notes, { new: '40' })).toEqual([])
    expect(findingsEndingLine(notes, { new: '44' }).map((x) => x.id)).toEqual(['b'])
  })

  it('skips deleted rows', () => {
    expect(findingsCoveringLine(notes, { new: '12', type: 'del' })).toEqual([])
    expect(findingsEndingLine(notes, { new: '44', type: 'del' })).toEqual([])
  })
})

describe('rightDiffHasRange', () => {
  const lines = [
    { new: '10', type: 'ctx' },
    { new: '11', type: 'add' },
    { old: '12', new: '', type: 'del' },
    { new: '12', type: 'add' },
  ]

  it('accepts RIGHT add/context lines and rejects LEFT-only / missing lines', () => {
    expect(rightDiffHasRange(lines, 10, 12)).toBe(true)
    expect(rightDiffHasRange(lines, 11, 11)).toBe(true)
    expect(rightDiffHasRange(lines, 10, 13)).toBe(false)
    expect(rightDiffHasRange([{ old: '12', new: '', type: 'del' }], 12, 12)).toBe(false)
    expect(rightDiffHasRange([], 1, 1)).toBe(false)
  })
})

describe('extraHtmlWithoutFindingFlags', () => {
  it('strips projected .flag blocks and keeps other ExtraHTML', () => {
    const flag = '<div class="flag"><svg></svg><div><div class="ft">t</div><div class="fm">msg</div></div></div>'
    const other = '<div class="icmt">note</div>'
    expect(extraHtmlWithoutFindingFlags(flag + other)).toBe(other)
    expect(extraHtmlWithoutFindingFlags(other)).toBe(other)
    expect(extraHtmlWithoutFindingFlags(undefined)).toBe('')
  })
})
