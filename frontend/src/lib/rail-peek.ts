/**
 * Left-edge flyout for a collapsed app rail.
 *
 * The docked rail still toggles with ⌘B / the titlebar. When it is hidden,
 * hovering (or tapping) the window's left edge temporarily overlays it so
 * navigation stays reachable on a laptop-width frame.
 */

import { get, writable } from 'svelte/store'

export const RAIL_PEEK_EDGE_PX = 8
export const RAIL_PEEK_COARSE_EDGE_PX = 16
/** Extra px past the edge strip before pointermove skips layout work. */
export const RAIL_PEEK_MOVE_SLACK_PX = 16
export const RAIL_PEEK_SHOW_MS = 90
export const RAIL_PEEK_HIDE_MS = 320

/** Hit targets that keep a peeked rail open (including portaled menus). */
export const RAIL_PEEK_SURFACE_SEL = '.rail, .rail-hotzone, .rail-move-menu'

/** Ephemeral overlay; not persisted. Esc-to-back reads this instead of
 *  swallowing the keystroke in a window capture listener. */
export const railPeek = writable(false)
/** True while a flyout drag or move-menu is in progress — hide paths must not yank the rail. */
export const railPeekLocked = writable(false)

export function openRailPeek() {
  railPeek.set(true)
}

export function closeRailPeek(force = false) {
  if (!get(railPeek)) return
  if (!force && get(railPeekLocked)) return
  parkFocusFromRail()
  railPeek.set(false)
}

export type RailPeekRect = {
  left: number
  top: number
  right: number
  bottom: number
}

/** True when the pointer is on the body's left edge strip (below the titlebar). */
export function pointerInLeftEdge(
  x: number,
  y: number,
  body: RailPeekRect,
  edgePx = RAIL_PEEK_EDGE_PX,
): boolean {
  return x >= body.left && x <= body.left + edgePx && y >= body.top && y <= body.bottom
}

/**
 * True when a collapsed, idle flyout can skip getBoundingClientRect: the pointer
 * is well right of the edge strip, so this move cannot open the overlay.
 */
export function pointerFarFromRailEdge(
  clientX: number,
  opts: { peeking: boolean; showPending: boolean; locked: boolean },
  slackPx = RAIL_PEEK_EDGE_PX + RAIL_PEEK_MOVE_SLACK_PX,
  bodyLeft = 0,
): boolean {
  if (opts.peeking || opts.showPending || opts.locked) return false
  return clientX > bodyLeft + slackPx
}

/**
 * True when a persistent pointer left the window. Touch/pen also fire
 * `pointerout` with a null relatedTarget after every tap (the pointer is gone),
 * so those must not count as a window exit.
 */
export function pointerLeftDocument(e: {
  relatedTarget: EventTarget | null
  pointerType?: string
}): boolean {
  return e.pointerType === 'mouse' && e.relatedTarget === null
}

export function isRailPeekSurface(el: EventTarget | null): boolean {
  return el instanceof Element && !!el.closest(RAIL_PEEK_SURFACE_SEL)
}

/** Menu or drag should keep the flyout open without a pointer. */
export function railPeekHeld(flags: {
  collapsed: boolean
  menuOpen: boolean
  dragging: boolean
}): boolean {
  if (!flags.collapsed) return false
  return flags.menuOpen || flags.dragging
}

/** Move focus into the content column before `inert` would drop it on <body>. */
export function parkFocusFromRail(): void {
  const active = document.activeElement
  if (!(active instanceof HTMLElement) || !active.closest('.rail')) return
  const main = document.querySelector<HTMLElement>('.main')
  if (main) {
    if (!main.hasAttribute('tabindex')) main.tabIndex = -1
    main.focus({ preventScroll: true })
    return
  }
  active.blur()
}
