/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test, vi } from 'vitest'
import { cancelStripScroll, captureStripScrollLeft, nextStripScrollLeft, scrollChildIntoStrip, stripScrollIsProgrammatic } from './tab-strip-scroll'

describe('nextStripScrollLeft', () => {
  test('returns the current offset when the chip is already in view', () => {
    expect(nextStripScrollLeft(80, 200, 600, 100, 60)).toBe(80)
  })

  test('scrolls left when the chip sits past the left edge', () => {
    expect(nextStripScrollLeft(80, 200, 600, 20, 40)).toBe(20)
  })

  test('scrolls right when the chip sits past the right edge', () => {
    expect(nextStripScrollLeft(0, 200, 600, 180, 40)).toBe(20)
  })

  test('clamps to the strip max so a wide chip cannot overscroll', () => {
    expect(nextStripScrollLeft(0, 200, 250, 200, 80)).toBe(50)
  })

  test('keeps scrollLeft at 0 when nothing overflows', () => {
    expect(nextStripScrollLeft(0, 400, 200, 0, 80)).toBe(0)
  })
})

describe('scrollChildIntoStrip', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  function fakeStrip(scrollLeft: number, clientWidth: number, scrollWidth: number) {
    const strip = document.createElement('div')
    Object.defineProperty(strip, 'clientWidth', { configurable: true, get: () => clientWidth })
    Object.defineProperty(strip, 'scrollWidth', { configurable: true, get: () => scrollWidth })
    strip.scrollLeft = scrollLeft
    strip.getBoundingClientRect = () => ({
      x: 0, y: 0, top: 0, left: 0, bottom: 20, right: clientWidth,
      width: clientWidth, height: 20, toJSON: () => ({}),
    })
    strip.scrollTo = vi.fn() as HTMLElement['scrollTo']
    return strip
  }

  function fakeChip(left: number, width: number) {
    const child = document.createElement('div')
    child.getBoundingClientRect = () => ({
      x: left, y: 0, top: 0, left, bottom: 20, right: left + width,
      width, height: 20, toJSON: () => ({}),
    })
    return child
  }

  function stubFrames() {
    const queued: FrameRequestCallback[] = []
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      queued.push(cb)
      return queued.length
    })
    vi.stubGlobal('cancelAnimationFrame', (id: number) => {
      queued[id - 1] = () => {}
    })
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: false,
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    return queued
  }

  test('smooth chase eases scrollLeft across frames without native scrollTo', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    const child = fakeChip(400, 80)
    scrollChildIntoStrip(strip, child, 'smooth')
    expect(strip.scrollTo).not.toHaveBeenCalled()
    expect(strip.scrollLeft).toBe(0)
    expect(queued).toHaveLength(1)

    queued[0](0)
    expect(strip.scrollLeft).toBe(0)

    queued[1](140)
    expect(strip.scrollLeft).toBeGreaterThan(180)
    expect(strip.scrollLeft).toBeLessThan(280)

    queued[2](280)
    expect(strip.scrollLeft).toBe(280)
  })

  test('a second smooth chase to the same target does not restart the ease', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    const child = fakeChip(400, 80)
    scrollChildIntoStrip(strip, child, 'smooth')
    const started = queued.length
    scrollChildIntoStrip(strip, child, 'smooth')
    expect(queued.length).toBe(started)
  })

  test('instant restore assigns scrollLeft and does not animate', () => {
    const queued = stubFrames()
    const strip = fakeStrip(0, 200, 600)
    const child = fakeChip(400, 80)
    scrollChildIntoStrip(strip, child, 'auto')
    expect(queued).toHaveLength(0)
    expect(strip.scrollTo).not.toHaveBeenCalled()
    expect(strip.scrollLeft).toBe(280)
  })

  test('cancelStripScroll stops an in-flight chase so restore can snap', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    scrollChildIntoStrip(strip, fakeChip(400, 80), 'smooth')
    queued[0](140)
    const mid = strip.scrollLeft
    expect(mid).toBeGreaterThan(0)
    cancelStripScroll(strip)
    strip.scrollLeft = 40
    queued[1](280)
    expect(strip.scrollLeft).toBe(40)
  })

  test('a user offset between ease frames cancels even if the scroll event has not fired yet', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    scrollChildIntoStrip(strip, fakeChip(400, 80), 'smooth')
    queued[0](0)
    strip.scrollLeft = 40
    queued[1](140)
    expect(strip.scrollLeft).toBe(40)
    queued[2]?.(280)
    expect(strip.scrollLeft).toBe(40)
  })

  test('a vertical hover-wheel that does not move the strip does not cancel the chase', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    scrollChildIntoStrip(strip, fakeChip(400, 80), 'smooth')
    queued[0](0)
    strip.dispatchEvent(new WheelEvent('wheel', { deltaY: 40 }))
    queued[1](280)
    expect(strip.scrollLeft).toBe(280)
  })

  test('treats an ease write as programmatic and a later user offset as not', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    scrollChildIntoStrip(strip, fakeChip(400, 80), 'smooth')
    queued[0](0)
    expect(stripScrollIsProgrammatic(strip)).toBe(true)
    strip.scrollLeft = 40
    expect(stripScrollIsProgrammatic(strip)).toBe(false)
  })

  test('treats a between-frame clamp to the new max as programmatic', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    scrollChildIntoStrip(strip, fakeChip(400, 80), 'smooth')
    queued[0](0)
    queued[1](140)
    Object.defineProperty(strip, 'scrollWidth', { configurable: true, get: () => 250 })
    strip.scrollLeft = 50
    expect(stripScrollIsProgrammatic(strip)).toBe(true)
    queued[2](280)
    expect(strip.scrollLeft).toBe(280)
  })

  test('a user scroll to the new max is still not programmatic', () => {
    const queued = stubFrames()
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const strip = fakeStrip(0, 200, 600)
    scrollChildIntoStrip(strip, fakeChip(400, 80), 'smooth')
    queued[0](0)
    Object.defineProperty(strip, 'scrollWidth', { configurable: true, get: () => 250 })
    strip.scrollLeft = 50
    expect(stripScrollIsProgrammatic(strip)).toBe(false)
  })
})

describe('captureStripScrollLeft', () => {
  test('returns undefined when the strip is missing or has no layout', () => {
    expect(captureStripScrollLeft(undefined)).toBeUndefined()
    const strip = document.createElement('div')
    Object.defineProperty(strip, 'clientWidth', { get: () => 0 })
    expect(captureStripScrollLeft(strip)).toBeUndefined()
  })

  test('records a real 0 when the strip is laid out at the start', () => {
    const strip = document.createElement('div')
    Object.defineProperty(strip, 'clientWidth', { get: () => 200 })
    Object.defineProperty(strip, 'scrollLeft', { get: () => 0, configurable: true })
    expect(captureStripScrollLeft(strip)).toBe(0)
  })
})
