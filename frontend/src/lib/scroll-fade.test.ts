/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import {
  fadeScrollEdges,
  isAwayFromBottom,
  isScrolledFromBottom,
  isScrolledFromTop,
  SCROLL_FADE_BOT_CLASS,
  SCROLL_FADE_READY_CLASS,
  SCROLL_FADE_TOP_CLASS,
  syncScrollEdgeFades,
  syncScrollFade,
} from './scroll-fade'

describe('isScrolledFromTop', () => {
  test('is false at the top, including 1px slack', () => {
    expect(isScrolledFromTop({ scrollTop: 0 })).toBe(false)
    expect(isScrolledFromTop({ scrollTop: 1 })).toBe(false)
  })

  test('is true once the scroller has moved', () => {
    expect(isScrolledFromTop({ scrollTop: 2 })).toBe(true)
    expect(isScrolledFromTop({ scrollTop: 80 })).toBe(true)
  })
})

describe('isScrolledFromBottom', () => {
  test('is false when content fits the viewport', () => {
    expect(isScrolledFromBottom({ scrollTop: 0, scrollHeight: 80, clientHeight: 80 })).toBe(false)
    expect(isScrolledFromBottom({ scrollTop: 0, scrollHeight: 81, clientHeight: 80 })).toBe(false)
  })

  test('is false at the tail, including 1px slack', () => {
    expect(isScrolledFromBottom({ scrollTop: 100, scrollHeight: 200, clientHeight: 100 })).toBe(false)
    expect(isScrolledFromBottom({ scrollTop: 99, scrollHeight: 200, clientHeight: 100 })).toBe(false)
  })

  test('is true when more content sits below', () => {
    expect(isScrolledFromBottom({ scrollTop: 0, scrollHeight: 200, clientHeight: 100 })).toBe(true)
    expect(isScrolledFromBottom({ scrollTop: 98, scrollHeight: 200, clientHeight: 100 })).toBe(true)
  })

  test('honors a larger tail slack', () => {
    const el = { scrollTop: 80, scrollHeight: 200, clientHeight: 100 }
    expect(isScrolledFromBottom(el, 24)).toBe(false)
    expect(isScrolledFromBottom({ ...el, scrollTop: 70 }, 24)).toBe(true)
  })
})

describe('isAwayFromBottom', () => {
  test('matches isScrolledFromBottom against a cached maxScroll', () => {
    expect(isAwayFromBottom(80, 100, 24)).toBe(false)
    expect(isAwayFromBottom(70, 100, 24)).toBe(true)
    expect(isAwayFromBottom(0, 1)).toBe(false)
  })
})

describe('syncScrollFade', () => {
  test('toggles fade-top with scrollTop', () => {
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 0, writable: true })
    syncScrollFade(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(false)

    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 12 })
    syncScrollFade(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(true)

    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 0 })
    syncScrollFade(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(false)
  })
})

describe('syncScrollEdgeFades', () => {
  function scroller(scrollTop: number, scrollHeight: number, clientHeight: number) {
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { configurable: true, value: scrollTop, writable: true })
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: scrollHeight })
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: clientHeight })
    return el
  }

  test('fades neither edge when content fits', () => {
    const el = scroller(0, 80, 80)
    syncScrollEdgeFades(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(false)
    expect(el.classList.contains(SCROLL_FADE_BOT_CLASS)).toBe(false)
  })

  test('fades only the bottom at scroll top', () => {
    const el = scroller(0, 200, 100)
    syncScrollEdgeFades(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(false)
    expect(el.classList.contains(SCROLL_FADE_BOT_CLASS)).toBe(true)
  })

  test('fades only the top at scroll bottom', () => {
    const el = scroller(100, 200, 100)
    syncScrollEdgeFades(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(true)
    expect(el.classList.contains(SCROLL_FADE_BOT_CLASS)).toBe(false)
  })

  test('fades both edges in the middle', () => {
    const el = scroller(40, 200, 100)
    syncScrollEdgeFades(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(true)
    expect(el.classList.contains(SCROLL_FADE_BOT_CLASS)).toBe(true)
  })

  test('keeps the tail crisp inside a larger bot slack', () => {
    const el = scroller(80, 200, 100)
    syncScrollEdgeFades(el, 1, 24)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(true)
    expect(el.classList.contains(SCROLL_FADE_BOT_CLASS)).toBe(false)
  })
})

describe('fadeScrollEdges', () => {
  test('pinnedToBottom does not mark fade-ready at scrollTop 0 while content overflows', () => {
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 0, writable: true })
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: 400 })
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: 100 })
    const destroy = fadeScrollEdges({ pinnedToBottom: () => true, botSlack: 24 })(el)
    expect(el.classList.contains(SCROLL_FADE_READY_CLASS)).toBe(false)
    destroy()
  })

  test('marks fade-ready on a short transcript that fits the viewport', () => {
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 0, writable: true })
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: 80 })
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: 100 })
    const destroy = fadeScrollEdges({ pinnedToBottom: () => true, botSlack: 24 })(el)
    expect(el.classList.contains(SCROLL_FADE_READY_CLASS)).toBe(true)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(false)
    destroy()
  })

  test('does not drop fade-top while the scroller is in #wsv-park', () => {
    const park = document.createElement('div')
    park.id = 'wsv-park'
    document.body.append(park)
    const el = document.createElement('div')
    el.classList.add(SCROLL_FADE_TOP_CLASS)
    park.append(el)
    Object.defineProperty(el, 'scrollTop', { configurable: true, value: 0, writable: true })
    Object.defineProperty(el, 'scrollHeight', { configurable: true, value: 400 })
    Object.defineProperty(el, 'clientHeight', { configurable: true, value: 100 })
    const destroy = fadeScrollEdges()(el)
    expect(el.classList.contains(SCROLL_FADE_TOP_CLASS)).toBe(true)
    expect(el.classList.contains(SCROLL_FADE_READY_CLASS)).toBe(false)
    destroy()
    park.remove()
  })
})
