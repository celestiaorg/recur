import { describe, expect, it } from 'vitest'
import { paneScrollTo } from './review-scroll'
import { applySlotPin, coverIndex, coverNear, DIFF_LINE_PX, estimateDiffHeight, headerIsStuck, inViewPaths, jumpLineWindow, lineWindow, pickHydrated, readSlotHeights, reservedCardHeight, samePathList, samePathSet, slotPad, windowAround } from './review-window'

describe('estimateDiffHeight', () => {
  it('is a short header when collapsed', () => {
    expect(estimateDiffHeight({ lines: new Array(400) }, true)).toBeLessThan(50)
  })

  it('grows with line count', () => {
    const small = estimateDiffHeight({ lines: new Array(10) }, false)
    const mid = estimateDiffHeight({ lines: new Array(500) }, false)
    const big = estimateDiffHeight({ lines: new Array(800) }, false)
    expect(big).toBeGreaterThan(small)
    expect(big).toBeGreaterThan(mid)
  })

  it('uses at least the CSS line-height (12px × 1.6)', () => {
    const one = estimateDiffHeight({ lines: new Array(1) }, false)
    const two = estimateDiffHeight({ lines: new Array(2) }, false)
    expect(two - one).toBeGreaterThanOrEqual(20)
  })

  it('falls back to add/del when lines are missing', () => {
    const fromStat = estimateDiffHeight({ diff: { add: 20, del: 4 } }, false)
    expect(fromStat).toBeGreaterThan(estimateDiffHeight({ diff: { add: 0, del: 0 } }, false))
    expect(fromStat).toBeGreaterThan(estimateDiffHeight({ lines: new Array(4) }, false))
  })
})

describe('reservedCardHeight', () => {
  it('does not pin a live card to its measured body, even when collapsed', () => {
    expect(reservedCardHeight({ lines: new Array(400) }, { live: true, collapsed: true, measured: 8000 })).toBeUndefined()
    expect(reservedCardHeight({ lines: new Array(400) }, { live: true, collapsed: false, measured: 8000 })).toBeUndefined()
  })

  it('lets a live expanded card size itself', () => {
    expect(reservedCardHeight({ lines: new Array(10) }, { live: true, collapsed: false, measured: 400 })).toBeUndefined()
  })

  it('keeps the measured body on an expanded stub', () => {
    expect(reservedCardHeight({ lines: new Array(10) }, { live: false, collapsed: false, measured: 400 })).toBe(400)
  })

  it('is header-only on a collapsed stub, ignoring measured height', () => {
    expect(reservedCardHeight({ lines: new Array(10) }, { live: false, collapsed: true, measured: 400 })).toBeLessThan(50)
  })

  it('estimates expanded height when a stub has never been measured', () => {
    const estimated = estimateDiffHeight({ lines: new Array(10) }, false)
    expect(reservedCardHeight({ lines: new Array(10) }, { live: false, collapsed: false })).toBe(estimated)
  })
})

describe('headerIsStuck', () => {
  it('is false while the card top is still in view', () => {
    expect(headerIsStuck(14, 400, 0)).toBe(false)
    expect(headerIsStuck(14, 400, 14)).toBe(false)
  })

  it('is true once the card top has scrolled past the port', () => {
    expect(headerIsStuck(14, 400, 15)).toBe(true)
    expect(headerIsStuck(14, 400, 200)).toBe(true)
  })

  it('is false after the card has left the top', () => {
    expect(headerIsStuck(14, 400, 400)).toBe(false)
    expect(headerIsStuck(14, 400, 500)).toBe(false)
  })
})

describe('pickHydrated', () => {
  const paths = Array.from({ length: 40 }, (_, i) => `f${i}.ts`)

  it('returns empty when there are no paths', () => {
    expect(pickHydrated([], '', [])).toEqual(new Set())
  })

  it('keeps the selected file and its neighbors', () => {
    const got = pickHydrated(paths, 'f10.ts', [], 8, 2)
    expect(got.has('f10.ts')).toBe(true)
    expect(got.has('f8.ts')).toBe(true)
    expect(got.has('f12.ts')).toBe(true)
    expect(got.size).toBeLessThanOrEqual(8)
  })

  it('caps even when many files are in view', () => {
    const got = pickHydrated(paths, 'f0.ts', paths, 6, 2)
    expect(got.size).toBe(6)
    expect(got.has('f0.ts')).toBe(true)
  })

  it('prefers files closer to the selection', () => {
    const got = pickHydrated(paths, 'f20.ts', ['f0.ts', 'f21.ts'], 3, 1)
    expect(got.has('f20.ts')).toBe(true)
    expect(got.has('f21.ts')).toBe(true)
    expect(got.has('f0.ts')).toBe(false)
  })

  it('still hydrates the viewport when the selection is far away', () => {
    const got = pickHydrated(paths, 'f0.ts', ['f30.ts', 'f31.ts', 'f32.ts'], 8, 2)
    expect(got.has('f0.ts')).toBe(true)
    expect(got.has('f30.ts')).toBe(true)
    expect(got.has('f31.ts')).toBe(true)
    expect(got.has('f32.ts')).toBe(true)
  })

  it('hydrates the true viewport before padded prefetch cards', () => {
    const ranges = [
      { path: 'f0.ts', top: 200, bottom: 240 },
      { path: 'f1.ts', top: 240, bottom: 280 },
      { path: 'f2.ts', top: 280, bottom: 320 },
      { path: 'f3.ts', top: 320, bottom: 360 },
      { path: 'f4.ts', top: 360, bottom: 400 },
      { path: 'f10.ts', top: 1000, bottom: 1400 },
      { path: 'f11.ts', top: 1400, bottom: 1800 },
    ]
    const padded = inViewPaths(ranges, 1000, 500, 900)
    const got = pickHydrated(paths, 'f0.ts', padded, 5, 2)
    expect(got.has('f0.ts')).toBe(true)
    expect(got.has('f10.ts')).toBe(true)
    expect(got.has('f11.ts')).toBe(true)
    expect(got.has('f3.ts')).toBe(false)
  })
})

describe('inViewPaths', () => {
  const ranges = [
    { path: 'a.ts', top: 0, bottom: 100 },
    { path: 'b.ts', top: 100, bottom: 400 },
    { path: 'c.ts', top: 400, bottom: 900 },
  ]

  it('includes files overlapping the padded viewport', () => {
    expect(inViewPaths(ranges, 0, 80, 50)).toEqual(['a.ts', 'b.ts'])
    expect(inViewPaths(ranges, 500, 80, 0)).toEqual(['c.ts'])
  })

  it('lists truly visible files before the prefetch pad', () => {
    const stacked = [
      { path: 'above.ts', top: 200, bottom: 250 },
      { path: 'vis.ts', top: 1000, bottom: 1400 },
      { path: 'below.ts', top: 2000, bottom: 2100 },
    ]
    expect(inViewPaths(stacked, 1000, 500, 900)).toEqual(['vis.ts', 'above.ts', 'below.ts'])
  })
})

describe('slotPad', () => {
  it('uses fallback for a missing heights list', () => {
    expect(slotPad(undefined, 0, 10, 20)).toBe(200)
    expect(slotPad([], 2, 5, 20)).toBe(60)
  })

  it('prefers measured heights and falls back per slot', () => {
    const h = [40, , 80] as number[]
    expect(slotPad(h, 0, 3, 20)).toBe(140)
  })
})

describe('readSlotHeights', () => {
  function node(hline: boolean, h: number) {
    return { classList: { contains: (name: string) => hline && name === 'hline' }, offsetHeight: h }
  }

  it('groups non-hline siblings onto the preceding slot', () => {
    const into: number[] = []
    const changed = readSlotHeights([
      node(true, 20),
      node(false, 60),
      node(true, 24),
    ], 2, into)
    expect(changed).toBe(true)
    expect(into[2]).toBe(80)
    expect(into[3]).toBe(24)
  })
})

describe('lineWindow', () => {
  it('renders everything when the file is short', () => {
    expect(lineWindow(40, 0, 0, 400)).toEqual({ start: 0, end: 40 })
  })

  it('keeps the viewport plus overscan', () => {
    const win = lineWindow(400, 0, 2000, 400, 20, 10)
    expect(win.start).toBeGreaterThan(0)
    expect(win.end).toBeLessThan(400)
    expect(win.end - win.start).toBeGreaterThan(20)
  })

  it('pins to the tail when the body is above the port', () => {
    expect(lineWindow(200, 0, 8000, 400, 20, 8)).toEqual({ start: 192, end: 200 })
  })

  it('pins to the head when the body is below the port', () => {
    expect(lineWindow(200, 8000, 0, 400, 20, 8)).toEqual({ start: 0, end: 8 })
  })

  it('walks measured heights so a tall prefix does not shrink the spacer', () => {
    const heights = Array.from({ length: 200 }, (_, i) => i < 10 ? 80 : 20)
    const win = lineWindow(200, 0, 800, 400, 20, 8, heights)
    expect(win.start).toBeGreaterThan(0)
    expect(slotPad(heights, 0, win.start)).toBeGreaterThan(win.start * 20)
  })
})

describe('coverIndex', () => {
  it('expands the slice to include a pinned row', () => {
    expect(coverIndex({ start: 40, end: 80 }, 10, 200)).toEqual({ start: 10, end: 80 })
    expect(coverIndex({ start: 40, end: 80 }, 90, 200)).toEqual({ start: 40, end: 91 })
  })

  it('ignores an out-of-range index', () => {
    expect(coverIndex({ start: 40, end: 80 }, -1, 200)).toEqual({ start: 40, end: 80 })
    expect(coverIndex({ start: 40, end: 80 }, 200, 200)).toEqual({ start: 40, end: 80 })
  })
})

describe('coverNear', () => {
  it('leaves a distant pin out of the viewport slice', () => {
    const view = { start: 0, end: 80 }
    expect(coverNear(view, 10_000, 12_000, 60)).toEqual(view)
  })

  it('expands only when the pin is already near the slice', () => {
    const view = { start: 40, end: 80 }
    expect(coverNear(view, 50, 200, 60)).toEqual(view)
    expect(coverNear(view, 90, 200, 60)).toEqual({ start: 40, end: 91 })
  })
})

describe('windowAround', () => {
  it('centers a jump without spanning the file', () => {
    const w = windowAround(10_000, 20_000, 60)
    expect(w.end - w.start).toBe(121)
    expect(w.start).toBe(9940)
    expect(w.end).toBe(10_061)
  })

  it('a later viewport does not snap back to a prior jump target', () => {
    const later = lineWindow(20_000, 0, 6000, 400, 20, 60)
    expect(later.end).toBeLessThan(1000)
    expect(coverNear(later, 10_000, 20_000, 60)).toEqual(later)
  })
})

describe('jumpLineWindow', () => {
  it('re-hydrates the same line after the highlight is cleared', () => {
    const count = 1000
    const header = lineWindow(count, 40, 0, 400)
    const slot = 799
    expect(header).toEqual({ start: 0, end: 78 })

    let lastJump = 0
    expect(jumpLineWindow(header, count, 800, lastJump, slot)).toEqual({ start: 739, end: 860 })
    lastJump = 800

    expect(jumpLineWindow(header, count, 0, lastJump, slot)).toEqual(header)
    expect(jumpLineWindow(header, count, 800, lastJump, slot)).toEqual(header)
    lastJump = 0

    expect(jumpLineWindow(header, count, 800, lastJump, slot)).toEqual({ start: 739, end: 860 })
  })
})

describe('applySlotPin', () => {
  it('corrects layout drift when the pane has not moved', () => {
    const pane = { scrollTop: 100 }
    applySlotPin(pane, { isConnected: true, getBoundingClientRect: () => ({ top: 50 }) }, 80, 100)
    expect(pane.scrollTop).toBe(70)
  })

  it('ignores a row whose DiffFile unmounted before the frame', () => {
    const pane = { scrollTop: 990049 }
    applySlotPin(pane, { isConnected: false, getBoundingClientRect: () => ({ top: 0 }) }, 608867, 990049)
    expect(pane.scrollTop).toBe(990049)
  })

  it('does not undo a jump whose old first row stays mounted', () => {
    const count = 500
    const viewH = 400
    const headH = 40
    const jumpSlot = 39
    const view = lineWindow(count, headH, 0, viewH, DIFF_LINE_PX)
    const next = windowAround(jumpSlot, count)
    expect(next.start).toBe(view.start)

    const pane = {
      scrollTop: 0,
      clientHeight: viewH,
      getBoundingClientRect: () => ({ top: 0 }),
      scrollTo(opts: { top: number }) { this.scrollTop = opts.top },
    }
    const pinY = headH
    const pinScroll = pane.scrollTop
    const target = { getBoundingClientRect: () => ({ top: headH + jumpSlot * DIFF_LINE_PX }) }
    paneScrollTo(pane as unknown as HTMLElement, target as HTMLElement)
    expect(pane.scrollTop).toBe(708)

    applySlotPin(pane, { isConnected: true, getBoundingClientRect: () => ({ top: pinY - pane.scrollTop }) }, pinY, pinScroll)
    expect(pane.scrollTop).toBe(708)
  })
})

describe('samePathList', () => {
  it('compares by index', () => {
    expect(samePathList(['a', 'b'], ['a', 'b'])).toBe(true)
    expect(samePathList(['a', 'b'], ['b', 'a'])).toBe(false)
  })
})

describe('samePathSet', () => {
  it('ignores insertion order', () => {
    expect(samePathSet(new Set(['a', 'b']), new Set(['b', 'a']))).toBe(true)
    expect(samePathSet(new Set(['a']), new Set(['a', 'b']))).toBe(false)
  })
})
