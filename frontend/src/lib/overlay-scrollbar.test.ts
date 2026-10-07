/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { overlayScrollbar, overlayThumb, scrollFromTrack } from './overlay-scrollbar'

describe('overlayThumb', () => {
  test('fills the track when content fits', () => {
    const t = overlayThumb(0, 200, 200)
    expect(t.overflow).toBe(false)
    expect(t.h).toBe(200)
    expect(t.top).toBe(0)
  })

  test('scales and travels with scroll', () => {
    const mid = overlayThumb(200, 400, 200)
    expect(mid.overflow).toBe(true)
    expect(mid.h).toBe(100)
    expect(mid.top).toBe(100)
  })
})

describe('scrollFromTrack', () => {
  test('inverts overlayThumb along the travel', () => {
    const mid = overlayThumb(200, 400, 200)
    expect(scrollFromTrack(mid.top, 400, 200)).toBe(200)
    expect(scrollFromTrack(0, 400, 200)).toBe(0)
  })
})

describe('overlayScrollbar', () => {
  function mount(scrollHeight = 400, clientHeight = 200) {
    const shell = document.createElement('div')
    const el = document.createElement('div')
    Object.defineProperty(el, 'scrollTop', { value: 0, writable: true, configurable: true })
    Object.defineProperty(el, 'scrollHeight', { value: scrollHeight, configurable: true })
    Object.defineProperty(el, 'clientHeight', { value: clientHeight, configurable: true })
    shell.append(el)
    document.body.append(shell)
    const cleanup = overlayScrollbar()(el)!
    const sb = shell.querySelector('.overlay-sb') as HTMLElement
    sb.getBoundingClientRect = () => ({
      top: 0, bottom: 200, left: 0, right: 6, width: 6, height: 200, x: 0, y: 0, toJSON() { return {} },
    })
    return { el, sb, cleanup: () => { cleanup(); shell.remove() } }
  }

  test('track click and thumb drag scroll the pane', () => {
    const { el, sb, cleanup } = mount()
    expect(sb.classList.contains('live')).toBe(true)
    sb.dispatchEvent(new PointerEvent('pointerdown', { button: 0, clientY: 150 }))
    expect(el.scrollTop).toBe(200)
    sb.dispatchEvent(new PointerEvent('pointermove', { clientY: 50 }))
    expect(el.scrollTop).toBe(0)
    cleanup()
  })

  test('gains .live when content grows past the viewport', async () => {
    const { el, sb, cleanup } = mount(200, 200)
    expect(sb.classList.contains('live')).toBe(false)
    Object.defineProperty(el, 'scrollHeight', { value: 400, configurable: true })
    el.append(document.createElement('div'))
    await Promise.resolve()
    expect(sb.classList.contains('live')).toBe(true)
    cleanup()
  })
})
