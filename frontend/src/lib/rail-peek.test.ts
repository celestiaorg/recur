/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import { get } from 'svelte/store'
import {
  RAIL_PEEK_EDGE_PX,
  closeRailPeek,
  isRailPeekSurface,
  openRailPeek,
  parkFocusFromRail,
  pointerFarFromRailEdge,
  pointerInLeftEdge,
  pointerLeftDocument,
  railPeek,
  railPeekHeld,
  railPeekLocked,
} from './rail-peek'

afterEach(() => {
  document.body.replaceChildren()
  railPeek.set(false)
  railPeekLocked.set(false)
})

const body = { left: 0, top: 52, right: 1200, bottom: 800 }

describe('pointerInLeftEdge', () => {
  test('hits the left strip inside the body', () => {
    expect(pointerInLeftEdge(0, 60, body)).toBe(true)
    expect(pointerInLeftEdge(RAIL_PEEK_EDGE_PX, 400, body)).toBe(true)
  })

  test('ignores points past the strip, in the titlebar, or below the body', () => {
    expect(pointerInLeftEdge(RAIL_PEEK_EDGE_PX + 1, 400, body)).toBe(false)
    expect(pointerInLeftEdge(4, 20, body)).toBe(false)
    expect(pointerInLeftEdge(4, 820, body)).toBe(false)
  })

  test('respects a custom strip width', () => {
    expect(pointerInLeftEdge(12, 60, body, 16)).toBe(true)
    expect(pointerInLeftEdge(12, 60, body, 8)).toBe(false)
  })
})

describe('isRailPeekSurface', () => {
  test('matches the rail, hotzone, and move menu (including nested controls)', () => {
    document.body.replaceChildren()
    const rail = document.createElement('aside')
    rail.className = 'rail'
    const btn = document.createElement('button')
    rail.appendChild(btn)
    const hot = document.createElement('div')
    hot.className = 'rail-hotzone'
    const menu = document.createElement('div')
    menu.className = 'rail-move-menu'
    const item = document.createElement('button')
    menu.appendChild(item)
    const main = document.createElement('main')
    main.className = 'main'
    document.body.append(rail, hot, menu, main)

    expect(isRailPeekSurface(btn)).toBe(true)
    expect(isRailPeekSurface(hot)).toBe(true)
    expect(isRailPeekSurface(item)).toBe(true)
    expect(isRailPeekSurface(main)).toBe(false)
    expect(isRailPeekSurface(null)).toBe(false)
  })
})

describe('pointerFarFromRailEdge', () => {
  const idle = { peeking: false, showPending: false, locked: false }

  test('skips layout when the pointer is well inside the content column', () => {
    expect(pointerFarFromRailEdge(200, idle)).toBe(true)
    expect(pointerFarFromRailEdge(RAIL_PEEK_EDGE_PX + 16, idle)).toBe(false)
  })

  test('does not skip when the flyout is open, a show is pending, or a drag holds it', () => {
    expect(pointerFarFromRailEdge(400, { ...idle, peeking: true })).toBe(false)
    expect(pointerFarFromRailEdge(400, { ...idle, showPending: true })).toBe(false)
    expect(pointerFarFromRailEdge(400, { ...idle, locked: true })).toBe(false)
  })

  test('measures slack from body.left, not the viewport origin', () => {
    expect(pointerFarFromRailEdge(200, idle, undefined, 100)).toBe(true)
    expect(pointerFarFromRailEdge(120, idle, undefined, 100)).toBe(false)
    expect(pointerFarFromRailEdge(24, idle, undefined, 100)).toBe(false)
  })
})

describe('pointerLeftDocument', () => {
  test('is true only for mouse leaving the document', () => {
    expect(pointerLeftDocument({ relatedTarget: null, pointerType: 'mouse' })).toBe(true)
    expect(pointerLeftDocument({ relatedTarget: document.body, pointerType: 'mouse' })).toBe(false)
  })

  test('ignores the trailing pointerout after a touch or pen tap', () => {
    expect(pointerLeftDocument({ relatedTarget: null, pointerType: 'touch' })).toBe(false)
    expect(pointerLeftDocument({ relatedTarget: null, pointerType: 'pen' })).toBe(false)
  })
})

describe('parkFocusFromRail', () => {
  test('moves focus from a rail control onto .main', () => {
    const main = document.createElement('main')
    main.className = 'main'
    const rail = document.createElement('aside')
    rail.className = 'rail'
    const btn = document.createElement('button')
    rail.appendChild(btn)
    document.body.append(rail, main)
    btn.focus()
    expect(document.activeElement).toBe(btn)
    parkFocusFromRail()
    expect(document.activeElement).toBe(main)
    expect(main.tabIndex).toBe(-1)
  })

  test('leaves focus alone when it is not inside the rail', () => {
    const main = document.createElement('main')
    main.className = 'main'
    main.tabIndex = -1
    document.body.appendChild(main)
    main.focus()
    parkFocusFromRail()
    expect(document.activeElement).toBe(main)
  })
})

describe('railPeek store', () => {
  test('closeRailPeek is a no-op when already closed', () => {
    railPeek.set(false)
    closeRailPeek()
    expect(get(railPeek)).toBe(false)
  })

  test('closeRailPeek leaves the flyout open while locked unless forced', () => {
    openRailPeek()
    railPeekLocked.set(true)
    closeRailPeek()
    expect(get(railPeek)).toBe(true)
    closeRailPeek(true)
    expect(get(railPeek)).toBe(false)
  })

  test('openRailPeek then closeRailPeek parks rail focus', () => {
    const main = document.createElement('main')
    main.className = 'main'
    const rail = document.createElement('aside')
    rail.className = 'rail'
    const btn = document.createElement('button')
    rail.appendChild(btn)
    document.body.append(rail, main)
    openRailPeek()
    btn.focus()
    closeRailPeek()
    expect(get(railPeek)).toBe(false)
    expect(document.activeElement).toBe(main)
  })
})

describe('railPeekHeld', () => {
  test('never holds when the rail is docked', () => {
    expect(railPeekHeld({
      collapsed: false,
      menuOpen: true,
      dragging: true,
    })).toBe(false)
  })

  test('holds while a menu or a drag is active', () => {
    const base = { collapsed: true, menuOpen: false, dragging: false }
    expect(railPeekHeld(base)).toBe(false)
    expect(railPeekHeld({ ...base, menuOpen: true })).toBe(true)
    expect(railPeekHeld({ ...base, dragging: true })).toBe(true)
  })
})
