/** @vitest-environment happy-dom */
import { describe, expect, test } from 'vitest'
import { revealUnderSticky, syncTreeStickyFades, TREE_FADE_UNDER, TREE_IS_STUCK } from './tree-sticky'

function rect(top: number): DOMRect {
  return { top, bottom: top + 24, left: 0, right: 100, width: 100, height: 24, x: 0, y: top, toJSON() { return {} } }
}

function header(depth: number, top: number): HTMLElement {
  const el = document.createElement('div')
  el.className = 'h'
  el.style.setProperty('--d', String(depth))
  el.getBoundingClientRect = () => rect(top)
  return el
}

function scroller(scrollTop: number, ...kids: HTMLElement[]): HTMLElement {
  const el = document.createElement('div')
  Object.defineProperty(el, 'scrollTop', { value: scrollTop, writable: true })
  Object.defineProperty(el, 'scrollHeight', { value: 1000 })
  Object.defineProperty(el, 'clientHeight', { value: 200 })
  el.getBoundingClientRect = () => ({ ...rect(0), bottom: 200, height: 200 })
  for (const k of kids) el.append(k)
  return el
}

describe('syncTreeStickyFades', () => {
  test('fades nothing at the top of the list', () => {
    const a = header(0, 0)
    syncTreeStickyFades(scroller(0, a), '.h', 24)
    expect(a.classList.contains(TREE_IS_STUCK)).toBe(false)
    expect(a.classList.contains(TREE_FADE_UNDER)).toBe(false)
  })

  test('fades only the deepest stuck header', () => {
    const a = header(0, 0)
    const b = header(1, 24)
    const root = scroller(40, a, b)
    syncTreeStickyFades(root, '.h', 24)
    expect(a.classList.contains(TREE_IS_STUCK)).toBe(true)
    expect(b.classList.contains(TREE_IS_STUCK)).toBe(true)
    expect(a.classList.contains(TREE_FADE_UNDER)).toBe(false)
    expect(b.classList.contains(TREE_FADE_UNDER)).toBe(true)
    syncTreeStickyFades(root, '.h', 24)
    expect(a.classList.contains(TREE_FADE_UNDER)).toBe(false)
    expect(b.classList.contains(TREE_FADE_UNDER)).toBe(true)
  })

  test('does not fade over a selected row', () => {
    const a = header(0, 0)
    const on = document.createElement('button')
    on.className = 'on'
    on.getBoundingClientRect = () => rect(24)
    const root = scroller(40, a, on)
    syncTreeStickyFades(root, '.h', 24)
    expect(a.classList.contains(TREE_IS_STUCK)).toBe(true)
    expect(a.classList.contains(TREE_FADE_UNDER)).toBe(false)
  })
})

describe('revealUnderSticky', () => {
  test('scrolls a focused row out from under stuck headers', () => {
    const a = header(0, 0)
    const file = document.createElement('button')
    file.getBoundingClientRect = () => rect(8)
    const root = scroller(40, a, file)
    revealUnderSticky(root, '.h', file, 24)
    expect(root.scrollTop).toBe(24)
  })

  test('does not scroll a focused stuck header', () => {
    const a = header(0, 0)
    const root = scroller(40, a)
    revealUnderSticky(root, '.h', a, 24)
    expect(root.scrollTop).toBe(40)
  })
})
