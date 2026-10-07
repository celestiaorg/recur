import { describe, expect, it } from 'vitest'
import { ByteLru, estimateBytes, stringBytes } from './byte-lru'

describe('estimateBytes', () => {
  it('counts UTF-16 strings and JSON-like trees', () => {
    expect(stringBytes('ab')).toBe(24 + 4)
    expect(estimateBytes('ab')).toBe(stringBytes('ab'))
    expect(estimateBytes(1)).toBe(8)
    expect(estimateBytes({ a: 'x', b: [1, 2] })).toBeGreaterThan(stringBytes('a') + stringBytes('x'))
  })

  it('does not loop on cycles', () => {
    const a: { n?: unknown } = {}
    a.n = a
    expect(estimateBytes(a)).toBeGreaterThan(0)
  })
})

describe('ByteLru', () => {
  it('promotes on get and evicts the oldest when over the byte budget', () => {
    const lru = new ByteLru<string>({ maxBytes: 6, sizeOf: (s) => s.length })
    lru.set('a', 'aa')
    lru.set('b', 'bb')
    lru.set('c', 'cc')
    lru.set('d', 'dd')
    expect(lru.peek('a')).toBeUndefined()
    expect(lru.get('b')).toBe('bb')
    lru.set('e', 'ee')
    expect(lru.peek('c')).toBeUndefined()
    expect(lru.peek('b')).toBe('bb')
    expect(lru.peek('d')).toBe('dd')
  })

  it('keeps a single oversized newest entry', () => {
    const lru = new ByteLru<string>({ maxBytes: 4, sizeOf: (s) => s.length })
    lru.set('a', 'aa')
    lru.set('big', 'zzzzzzzz')
    expect(lru.peek('a')).toBeUndefined()
    expect(lru.peek('big')).toBe('zzzzzzzz')
    expect(lru.bytes).toBe(8)
  })

  it('skips inserts over entryMaxBytes and drops the previous value', () => {
    const lru = new ByteLru<string>({
      maxBytes: 100,
      entryMaxBytes: 4,
      sizeOf: (s) => s.length,
    })
    lru.set('k', 'ab')
    expect(lru.set('k', 'too-big')).toEqual([])
    expect(lru.peek('k')).toBeUndefined()
  })

  it('take transfers ownership without promoting siblings', () => {
    const lru = new ByteLru<string>({ maxBytes: 5, sizeOf: (s) => s.length })
    lru.set('a', 'aa')
    lru.set('b', 'bb')
    expect(lru.take('a')).toBe('aa')
    expect(lru.take('a')).toBeUndefined()
    lru.set('c', 'cc')
    lru.set('d', 'dd')
    expect(lru.peek('b')).toBeUndefined()
    expect(lru.peek('c')).toBe('cc')
    expect(lru.peek('d')).toBe('dd')
  })

  it('refresh re-measures in place so prune can shrink without promoting', () => {
    const box = { n: 'aa' }
    const lru = new ByteLru<{ n: string }>({ maxBytes: 100, sizeOf: (v) => v.n.length })
    lru.set('a', box)
    lru.set('b', { n: 'bb' })
    expect(lru.bytes).toBe(4)
    box.n = ''
    lru.refresh('a')
    expect(lru.bytes).toBe(2)
    expect([...lru.keys()]).toEqual(['a', 'b'])
  })

  it('prune drops rejected keys and shrinking maxBytes evicts', () => {
    const lru = new ByteLru<string>({ maxBytes: 100, sizeOf: (s) => s.length })
    lru.set('keep', 'aa')
    lru.set('drop', 'bb')
    expect(lru.prune((k) => k === 'keep')).toEqual(['bb'])
    expect(lru.peek('drop')).toBeUndefined()
    lru.set('two', 'cc')
    lru.maxBytes = 2
    expect(lru.peek('keep')).toBeUndefined()
    expect(lru.peek('two')).toBe('cc')
  })
})
