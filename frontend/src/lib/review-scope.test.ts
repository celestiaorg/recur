import { describe, expect, it } from 'vitest'
import type { Commit } from '../../bindings/recur/internal/domain/models'
import { canStepOnTape, reviewTape, reviewTapeIndex, stepTape, tapeStepLabel } from './review-scope'

function commit(hash: string): Commit {
  return { hash, sha: hash, message: hash } as Commit
}

const a = commit('aaa')
const b = commit('bbb')

describe('reviewTape', () => {
  it('is just commits when the tree is clean', () => {
    expect(reviewTape(false, [a, b])).toEqual([
      { kind: 'commit', commit: a },
      { kind: 'commit', commit: b },
    ])
  })

  it('puts working first when dirty', () => {
    expect(reviewTape(true, [a])).toEqual([
      { kind: 'working' },
      { kind: 'commit', commit: a },
    ])
  })
})

describe('reviewTapeIndex / stepTape', () => {
  const dirty = reviewTape(true, [a, b])

  it('indexes working as 0 when open', () => {
    expect(reviewTapeIndex(dirty, true, '')).toBe(0)
  })

  it('steps older from working to the newest commit', () => {
    const idx = reviewTapeIndex(dirty, true, '')
    expect(stepTape(dirty, idx, -1)).toEqual({ kind: 'commit', commit: a })
    expect(canStepOnTape(idx, dirty.length)).toEqual({ older: true, newer: false })
  })

  it('steps newer from the newest commit back to working', () => {
    const idx = reviewTapeIndex(dirty, false, 'aaa')
    expect(stepTape(dirty, idx, 1)).toEqual({ kind: 'working' })
    expect(canStepOnTape(idx, dirty.length)).toEqual({ older: true, newer: true })
  })

  it('does not step newer from working', () => {
    expect(stepTape(dirty, 0, 1)).toBeUndefined()
  })

  it('clean tape matches commits-only stepping', () => {
    const tape = reviewTape(false, [a, b])
    expect(reviewTapeIndex(tape, false, 'aaa')).toBe(0)
    expect(stepTape(tape, 0, -1)).toEqual({ kind: 'commit', commit: b })
    expect(stepTape(tape, 0, 1)).toBeUndefined()
  })
})

describe('tapeStepLabel', () => {
  it('names the WIP item instead of calling it a commit', () => {
    expect(tapeStepLabel({ kind: 'working' }, 1)).toBe('Uncommitted changes')
    expect(tapeStepLabel({ kind: 'working' }, -1)).toBe('Uncommitted changes')
  })

  it('keeps previous/next commit when the neighbor is a commit', () => {
    expect(tapeStepLabel({ kind: 'commit', commit: a }, -1)).toBe('Previous commit')
    expect(tapeStepLabel({ kind: 'commit', commit: a }, 1)).toBe('Next commit')
  })
})
