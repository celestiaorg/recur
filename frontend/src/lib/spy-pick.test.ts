import { describe, expect, it } from 'vitest'
import { fileAtScrollTop, stepPath, topmostIntersecting, captureScrollAnchor, restoreScrollTop, captureViewportPin, applyViewportPin } from './spy-pick'

describe('fileAtScrollTop', () => {
  const files = [
    { path: 'a.ts', top: 0, bottom: 100 },
    { path: 'b.ts', top: 100, bottom: 400 },
    { path: 'c.ts', top: 400, bottom: 480 },
  ]

  it('returns empty when there are no files', () => {
    expect(fileAtScrollTop([], 0)).toBe('')
  })

  it('picks the file spanning the top edge', () => {
    expect(fileAtScrollTop(files, 0)).toBe('a.ts')
    expect(fileAtScrollTop(files, 99)).toBe('a.ts')
    expect(fileAtScrollTop(files, 100)).toBe('b.ts')
    expect(fileAtScrollTop(files, 250)).toBe('b.ts')
    expect(fileAtScrollTop(files, 399)).toBe('b.ts')
    expect(fileAtScrollTop(files, 400)).toBe('c.ts')
  })

  it('clamps past the last file to the last path', () => {
    expect(fileAtScrollTop(files, 999)).toBe('c.ts')
  })

  it('does not pick the last path when every card is still 0×0', () => {
    const unloaded = [
      { path: 'a.ts', top: 0, bottom: 0 },
      { path: 'b.ts', top: 0, bottom: 0 },
      { path: 'c.ts', top: 0, bottom: 0 },
    ]
    expect(fileAtScrollTop(unloaded, 0)).toBe('')
  })

  it('walks back over zero-height cards stacked on a laid-out file', () => {
    const mixed = [
      { path: 'a.ts', top: 0, bottom: 200 },
      { path: 'b.ts', top: 200, bottom: 200 },
      { path: 'c.ts', top: 200, bottom: 200 },
    ]
    expect(fileAtScrollTop(mixed, 200)).toBe('a.ts')
  })
})

describe('scroll anchor', () => {
  const files = [
    { path: 'a.ts', top: 0, bottom: 400 },
    { path: 'b.ts', top: 400, bottom: 900 },
    { path: 'c.ts', top: 900, bottom: 1100 },
  ]

  it('keeps the same offset into a file after content above shrinks', () => {
    const anchor = captureScrollAnchor(files, 1000)
    expect(anchor).toEqual({ path: 'c.ts', rel: -100 })
    const shrunk = [
      { path: 'a.ts', top: 0, bottom: 50 },
      { path: 'b.ts', top: 50, bottom: 120 },
      { path: 'c.ts', top: 120, bottom: 320 },
    ]
    expect(restoreScrollTop(shrunk, anchor!)).toBe(220)
  })

  it('returns null when the anchored path is gone', () => {
    const anchor = captureScrollAnchor(files, 100)
    expect(restoreScrollTop([{ path: 'z.ts', top: 0, bottom: 10 }], anchor!)).toBeNull()
  })
})

describe('viewport pin', () => {
  const files = [
    { path: 'a.ts', top: 0, bottom: 400 },
    { path: 'b.ts', top: 400, bottom: 900 },
    { path: 'c.ts', top: 900, bottom: 1100 },
  ]

  it('marks the user at the end so a growing last file can stick', () => {
    const pin = captureViewportPin(files, 900, 200, 1100)
    expect(pin.atEnd).toBe(true)
    expect(pin.anchor?.path).toBe('c.ts')
  })

  it('keeps a file-relative pin in the middle of the stack', () => {
    const pin = captureViewportPin(files, 100, 200, 1100)
    expect(pin.atEnd).toBe(false)
    expect(pin.anchor).toEqual({ path: 'a.ts', rel: -100 })
  })

  it('applyViewportPin snaps to the live end', () => {
    const root = { scrollTop: 200, scrollHeight: 4000, clientHeight: 500 } as HTMLElement
    applyViewportPin(root, files, { atEnd: true, anchor: null })
    expect(root.scrollTop).toBe(3500)
  })

  it('applyViewportPin restores the file offset when not at the end', () => {
    const root = { scrollTop: 0 } as HTMLElement
    const after = [
      { path: 'a.ts', top: 0, bottom: 50 },
      { path: 'b.ts', top: 50, bottom: 120 },
      { path: 'c.ts', top: 120, bottom: 320 },
    ]
    applyViewportPin(root, after, { atEnd: false, anchor: { path: 'c.ts', rel: -100 } })
    expect(root.scrollTop).toBe(220)
  })

  it('applyViewportPin skips when the viewport has moved since capture', () => {
    const root = { scrollTop: 400 } as HTMLElement
    applyViewportPin(root, files, { atEnd: false, anchor: { path: 'a.ts', rel: 0 } }, 100)
    expect(root.scrollTop).toBe(400)
  })
})

describe('stepPath', () => {
  const paths = ['a.ts', 'b.ts', 'c.ts']

  it('returns null when there are no paths', () => {
    expect(stepPath([], 'a.ts', 1)).toBeNull()
  })

  it('steps forward and back within the list', () => {
    expect(stepPath(paths, 'a.ts', 1)).toBe('b.ts')
    expect(stepPath(paths, 'b.ts', 1)).toBe('c.ts')
    expect(stepPath(paths, 'c.ts', -1)).toBe('b.ts')
    expect(stepPath(paths, 'b.ts', -1)).toBe('a.ts')
  })

  it('returns null at the ends', () => {
    expect(stepPath(paths, 'c.ts', 1)).toBeNull()
    expect(stepPath(paths, 'a.ts', -1)).toBeNull()
  })

  it('picks first or last when current is unknown', () => {
    expect(stepPath(paths, '', 1)).toBe('a.ts')
    expect(stepPath(paths, 'missing.ts', -1)).toBe('c.ts')
  })
})

describe('topmostIntersecting', () => {
  it('an observer delta of only the next file is not the spy target', () => {
    // IO reports b.ts entering the top band while a.ts still owns scrollTop.
    expect(topmostIntersecting([
      { path: 'b.ts', top: 40, intersecting: true },
    ])).toBe('b.ts')
    expect(fileAtScrollTop([
      { path: 'a.ts', top: 0, bottom: 400 },
      { path: 'b.ts', top: 400, bottom: 700 },
    ], 320)).toBe('a.ts')
  })

  it('returns the intersecting entry with the smallest top', () => {
    expect(topmostIntersecting([
      { path: 'b.ts', top: 40, intersecting: true },
      { path: 'a.ts', top: 8, intersecting: true },
      { path: 'c.ts', top: 2, intersecting: false },
    ])).toBe('a.ts')
  })

  it('skips empty paths and non-intersecting entries', () => {
    expect(topmostIntersecting([
      { path: '', top: 0, intersecting: true },
      { path: 'x.ts', top: 10, intersecting: false },
    ])).toBe('')
  })
})
