/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import {
  clampPage, discardPageCanvas, fitWidthScale, pageAtScroll, pageOriginInScroller,
  pdfjsAssetDirs, zoomStep,
} from './pdf-preview'

describe('clampPage', () => {
  test('clamps to 1..pages', () => {
    expect(clampPage(0, 10)).toBe(1)
    expect(clampPage(11, 10)).toBe(10)
    expect(clampPage(3.7, 10)).toBe(4)
    expect(clampPage(3.2, 10)).toBe(3)
    expect(clampPage(5, 5)).toBe(5)
  })

  test('rejects non-finite or empty documents', () => {
    expect(clampPage(NaN, 10)).toBe(1)
    expect(clampPage(Infinity, 10)).toBe(1)
    expect(clampPage(5, 0)).toBe(1)
    expect(clampPage(5, -2)).toBe(1)
  })
})

describe('zoomStep', () => {
  test('multiplies by 1.25 and clamps to 0.1–8', () => {
    expect(zoomStep(1, 1)).toBe(1.25)
    expect(zoomStep(1, -1)).toBe(0.8)
    expect(zoomStep(8, 1)).toBe(8)
    expect(zoomStep(0.1, -1)).toBe(0.1)
  })
})

describe('fitWidthScale', () => {
  test('viewport / page width, clamped', () => {
    expect(fitWidthScale(100, 200)).toBe(2)
    expect(fitWidthScale(100, 1000)).toBe(8)
    expect(fitWidthScale(1000, 50)).toBe(0.1)
    expect(fitWidthScale(0, 100)).toBe(1)
    expect(fitWidthScale(100, 0)).toBe(1)
  })
})

describe('pageAtScroll', () => {
  const gap = 8
  const even = [100, 100, 100]

  test('empty document is page 1', () => {
    expect(pageAtScroll(0, [], gap)).toBe(1)
  })

  test('slot that contains the viewport top', () => {
    expect(pageAtScroll(0, even, gap)).toBe(1)
    expect(pageAtScroll(99, even, gap)).toBe(1)
    expect(pageAtScroll(108, even, gap)).toBe(2)
    expect(pageAtScroll(216, even, gap)).toBe(3)
  })

  test('gap picks the nearer slot; ties stay on the earlier page', () => {
    expect(pageAtScroll(103, even, gap)).toBe(1)
    expect(pageAtScroll(104, even, gap)).toBe(1)
    expect(pageAtScroll(105, even, gap)).toBe(2)
  })

  test('non-finite scroll stays on page 1', () => {
    expect(pageAtScroll(NaN, even, gap)).toBe(1)
  })

  test('pinned end keeps a later page that cannot align to the top', () => {
    const heights = [400, 400]
    const pad = 12
    const g = 12
    const viewportH = 700
    const maxScroll = pad + heights[0]! + g + heights[1]! + pad - viewportH
    const y = maxScroll - pad
    expect(maxScroll).toBe(136)
    expect(pageAtScroll(y, heights, g)).toBe(1)
    expect(pageAtScroll(y, heights, g, y, 2)).toBe(2)
    expect(pageAtScroll(y, heights, g, y, 1)).toBe(1)
    expect(pageAtScroll(0, heights, g, y, 2)).toBe(1)
  })
})

describe('pageOriginInScroller', () => {
  function box(top: number): Pick<DOMRect, 'top'> {
    return { top }
  }

  test('next-page scroll uses viewport origin, not preview offset that includes the toolbar', () => {
    const toolbarH = 40
    const pad = 12
    const gap = 12
    const heights = [800, 800]
    const previewOrigin = toolbarH + pad
    const nextScrollTop = pad + heights[0]! + gap
    expect(pageAtScroll(nextScrollTop - previewOrigin, heights, gap)).toBe(1)

    const scroller = {
      scrollTop: nextScrollTop,
      getBoundingClientRect: () => box(toolbarH) as DOMRect,
    }
    const slot = {
      getBoundingClientRect: () => box(toolbarH + pad - nextScrollTop) as DOMRect,
    }
    const origin = pageOriginInScroller(scroller, slot, pad)
    expect(origin).toBe(pad)
    expect(pageAtScroll(nextScrollTop - origin, heights, gap)).toBe(2)
  })

  test('falls back when the first slot is missing', () => {
    const scroller = { scrollTop: 0, getBoundingClientRect: () => box(0) as DOMRect }
    expect(pageOriginInScroller(scroller, null, 12)).toBe(12)
  })
})

describe('discardPageCanvas', () => {
  test('clears the backing store so a later visit must paint again', () => {
    const canvas = document.createElement('canvas')
    canvas.width = 1200
    canvas.height = 1600
    discardPageCanvas(canvas)
    expect(canvas.width).toBe(0)
    expect(canvas.height).toBe(0)
  })
})

describe('pdfjsAssetDirs', () => {
  test('emits trailing-slash URLs under /pdfjs', () => {
    expect(pdfjsAssetDirs('http://127.0.0.1:9245/')).toEqual({
      cMapUrl: 'http://127.0.0.1:9245/pdfjs/cmaps/',
      standardFontDataUrl: 'http://127.0.0.1:9245/pdfjs/standard_fonts/',
      wasmUrl: 'http://127.0.0.1:9245/pdfjs/wasm/',
    })
  })
})
