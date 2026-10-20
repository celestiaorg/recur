import { describe, expect, it } from 'vitest'
import { TmStackCache, tokenizeLineRange } from './cm-tm-stacks'
import type { StateStack } from 'vscode-textmate'

function stack(id: number): StateStack {
  return { id } as unknown as StateStack
}

describe('TmStackCache', () => {
  it('starts from line 1 with no checkpoints', () => {
    const cache = new TmStackCache()
    const r = cache.resumeAt(4000)
    expect(r.startLine).toBe(1)
  })

  it('resumes just after the last contiguous checkpoint', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 50; n++) cache.remember(n, stack(n))
    const r = cache.resumeAt(4980)
    expect(r.startLine).toBe(51)
    expect(r.stack).toEqual(stack(50))
  })

  it('a viewport near the end does not restart at line 1 after a fill', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 5000; n++) cache.remember(n, stack(n))
    expect(cache.resumeAt(4980).startLine).toBe(4980)
  })

  it('growing the cache makes a later resume skip the filled prefix', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 50; n++) cache.remember(n, stack(n))
    for (let n = 51; n <= 100; n++) cache.remember(n, stack(n))
    const r = cache.resumeAt(90)
    expect(r.startLine).toBe(90)
    expect(r.stack).toEqual(stack(89))
  })

  it('a splice keeps the stack entering the changed line', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 100; n++) cache.remember(n, stack(n))
    cache.splice(40, 1, 1)
    const r = cache.resumeAt(80)
    expect(r.startLine).toBe(40)
    expect(r.stack).toEqual(stack(39))
  })

  it('a splice at line 1 drops everything', () => {
    const cache = new TmStackCache()
    cache.remember(1, stack(1))
    cache.splice(1, 1, 1)
    expect(cache.resumeAt(10).startLine).toBe(1)
    expect(cache.filled).toBe(0)
  })

  it('splices a whole-document replace without exceeding the call stack', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 5; n++) cache.remember(n, stack(n), [0, n])
    // A live refresh replaces every line at once; spreading that many
    // arguments into splice throws RangeError on a large file.
    expect(() => cache.splice(1, 5, 200_000)).not.toThrow()
    expect(cache.filled).toBe(0)
  })
})

describe('tokenizeLineRange early-stop', () => {
  function fakeStack(id: number): StateStack {
    return {
      depth: id,
      clone() {
        return this
      },
      equals(other: StateStack) {
        return (other as unknown as { depth: number }).depth === id
      },
    } as unknown as StateStack
  }

  function tokenize(text: string, prev: StateStack | null) {
    const d = prev ? (prev as unknown as { depth: number }).depth : 0
    return { tokens: [0, text.length || 1], ruleStack: fakeStack(d + text.length) }
  }

  it('stops on the edited line when the end stack still matches', () => {
    const cache = new TmStackCache()
    const texts = ['aa', 'bb', 'cc', 'dd', 'ee']
    tokenizeLineRange({
      lineText: (n) => texts[n - 1],
      cache,
      tokenize,
      fromLine: 1,
      toLine: 5,
    })
    expect(cache.filled).toBe(5)
    cache.splice(3, 1, 1)
    const r = tokenizeLineRange({
      lineText: (n) => texts[n - 1],
      cache,
      tokenize,
      fromLine: 3,
      toLine: 5,
      stopWhenStable: true,
    })
    expect(r.tokenized).toBe(1)
    expect(r.stableAt).toBe(3)
    expect(cache.filled).toBe(5)
  })

  it('keeps stale stacks on a same-length splice so early-stop can match', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 5; n++) cache.remember(n, stack(n), [0, n])
    cache.splice(3, 1, 1)
    expect(cache.filled).toBe(2)
    expect(cache.stackAt(3)).toEqual(stack(3))
    expect(cache.tokensAt(4)).toEqual([0, 4])
  })

  it('re-tokenizes every replaced line when the first one re-converges', () => {
    const cache = new TmStackCache()
    let texts = ['aa', 'bb', 'cc', 'dd', 'ee']
    const run = () =>
      tokenizeLineRange({ lineText: (n) => texts[n - 1], cache, tokenize, fromLine: 1, toLine: 5, stopWhenStable: true })
    run()
    // Full-document replace with the same line count: line 1 is unchanged, so
    // its end stack matches, but lines 2..5 hold new text.
    texts = ['aa', 'bx', 'cc', 'dd', 'eeee']
    cache.splice(1, 5, 5)
    run()
    expect(cache.tokensAt(5)).toEqual([0, 4])
    expect(cache.filled).toBe(5)
  })

  it('shifts later checkpoints when a line is inserted', () => {
    const cache = new TmStackCache()
    for (let n = 1; n <= 4; n++) cache.remember(n, stack(n), [0, n])
    cache.splice(2, 0, 1)
    expect(cache.filled).toBe(1)
    expect(cache.stackAt(2)).toBeUndefined()
    expect(cache.stackAt(3)).toEqual(stack(2))
  })
})
