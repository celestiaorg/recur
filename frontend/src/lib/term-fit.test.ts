/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { termFitSize, termHostInnerSize, TERM_FIT_SCROLLBAR_WIDTH } from './term-fit'

describe('termFitSize', () => {
  test('uses the content box so parent padding cannot clip the last row', () => {
    // Border-box 100px with 6px top padding → 94px content. 20px cells.
    // FitAddon would floor(100/20)=5 and paint 100px into 94px.
    expect(termFitSize(200, 94, 10, 20, 0)).toEqual({ cols: 20, rows: 4 })
  })

  test('pins leftover sub-row pixels rather than rounding up a clipped row', () => {
    expect(termFitSize(80, 50, 8, 17, 0)).toEqual({ cols: 10, rows: 2 })
  })

  test('subtracts the xterm scrollbar from width like FitAddon', () => {
    expect(termFitSize(94, 80, 8, 16)).toEqual({
      cols: Math.floor((94 - TERM_FIT_SCROLLBAR_WIDTH) / 8),
      rows: 5,
    })
  })

  test('clamps empty or invalid metrics to the xterm minimum', () => {
    expect(termFitSize(0, 0, 8, 16, 0)).toEqual({ cols: 2, rows: 1 })
    expect(termFitSize(80, 80, 0, 0, 0)).toEqual({ cols: 2, rows: 1 })
  })

  test('termHostInnerSize subtracts the host padding FitAddon ignores', () => {
    const host = document.createElement('div')
    Object.defineProperty(host, 'clientWidth', { value: 200 })
    Object.defineProperty(host, 'clientHeight', { value: 100 })
    host.style.padding = '6px 0 0 8px'
    document.body.append(host)
    expect(termHostInnerSize(host)).toEqual({ width: 192, height: 94 })
    host.remove()
  })
})
