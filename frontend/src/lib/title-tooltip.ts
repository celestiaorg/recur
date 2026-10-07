/**
 * Native `title` tooltips are the webview's, so they ignore the app theme and
 * sit on a long OS delay. These helpers steal the attribute on hover/focus and
 * feed a custom overlay; markup stays `title="…"`.
 */

export const UI_TIP_ID = 'ui-tip'
export const UI_TIP_ATTR = 'data-ui-title'
export const UI_TIP_CLASS = 'ui-tip'

/** First show — short enough to feel instant, long enough to cross icon rows. */
export const SHOW_DELAY_MS = 160
/** After a recent hide, skip the delay so chaining titled controls feels continuous. */
export const RESHOW_WINDOW_MS = 480

export const TIP_GAP = 6
export const TIP_PAD = 8
/** Prefer the pointer as the X anchor once the target is this wide. */
export const TIP_CURSOR_ANCHOR_PX = 160

const TITLE_SEL = `[title], [${UI_TIP_ATTR}]`
/** Overlay itself, explicit opt-out, sidebar rows, pane resize bars, and iframes
 *  (iframe `title` is an accessible name for the embedded document, not a hover tip). */
const SKIP_SEL = `.${UI_TIP_CLASS}, [data-ui-tip="off"], .tloop, .vsplit, .hsplit, iframe`
/** Hover actions inside a skipped `.tloop` still want themed tooltips. */
const SKIP_EXCEPT_SEL = '.tloop-acts'

export type TipPlacement = 'above' | 'below'

export type TipPlace = {
  x: number
  y: number
  placement: TipPlacement
}

export function tooltipText(el: Element): string {
  const raw = el.getAttribute('title') ?? el.getAttribute(UI_TIP_ATTR) ?? ''
  return raw.trim()
}

export function titledFromElement(start: Element | null): HTMLElement | null {
  if (!start) return null
  if (start.closest(SKIP_SEL) && !start.closest(SKIP_EXCEPT_SEL)) return null
  const el = start.closest<HTMLElement>(TITLE_SEL)
  if (!el || (el.closest(SKIP_SEL) && !el.closest(SKIP_EXCEPT_SEL))) return null
  if (!tooltipText(el)) return null
  return el
}

/**
 * Hit-test the *topmost* element only, then walk ancestors.
 * Disabled controls are still the topmost hit (`elementsFromPoint` includes
 * them; they just don't fire events). Walking the full stack would leak
 * tooltips for controls hidden under a modal/palette backdrop.
 */
export function titledFromPoint(x: number, y: number): HTMLElement | null {
  if (typeof document === 'undefined') return null
  const top = document.elementFromPoint(x, y)
  return titledFromElement(top instanceof Element ? top : null)
}

export function pointInRect(
  r: { left: number; right: number; top: number; bottom: number },
  x: number,
  y: number,
): boolean {
  return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

/** True when this scroll actually moved `watched`. Unrelated overflow (chat
 *  auto-follow while hovering a sidebar control) must not dismiss the tip. */
export function scrollHidesTip(scroller: EventTarget | null, watched: Node | null): boolean {
  if (!watched) return false
  if (!(scroller instanceof Node)) return true
  return scroller.contains(watched)
}

/** Copy `title` aside and strip it so the OS tooltip never appears. */
export function holdTitle(el: Element): string | null {
  const next = el.getAttribute('title')
  if (next != null) {
    const trimmed = next.trim()
    if (!trimmed) {
      el.removeAttribute('title')
      el.removeAttribute(UI_TIP_ATTR)
      return null
    }
    el.setAttribute(UI_TIP_ATTR, next)
    el.removeAttribute('title')
    return trimmed
  }
  const held = el.getAttribute(UI_TIP_ATTR)
  if (held == null) return null
  const trimmed = held.trim()
  return trimmed || null
}

/** Put `title` back unless the framework already wrote a newer one. */
export function releaseTitle(el: Element): void {
  const held = el.getAttribute(UI_TIP_ATTR)
  el.removeAttribute(UI_TIP_ATTR)
  if (held == null) return
  if (el.hasAttribute('title')) return
  if (!held.trim()) return
  el.setAttribute('title', held)
}

/** Re-steal `title` if Svelte (or anyone) writes it back while hovered. */
export function syncHeldTitle(el: Element): string | null {
  if (!el.hasAttribute(UI_TIP_ATTR) && !el.hasAttribute('title')) return null
  return holdTitle(el)
}

export function showDelayMs(now: number, lastHideAt: number, alreadyVisible: boolean): number {
  if (alreadyVisible) return 0
  if (lastHideAt > 0 && now - lastHideAt < RESHOW_WINDOW_MS) return 0
  return SHOW_DELAY_MS
}

export type TipHideReason = 'leave' | 'dismiss' | 'interrupt'

/**
 * Click/Escape suppress the copy that was showing. A different title on the
 * same node (armed confirm, live discard risk) is a new tip and should show
 * even if the pointer never left.
 */
export function shouldReshowAfterDismiss(dismissedText: string, nextText: string | null): boolean {
  return !!nextText && nextText !== dismissedText
}

/** Show/hide fields the overlay drives. Tokens are identity-compared (`===`). */
export type TipCore<T> = {
  target: T | null
  pending: T | null
  dismissed: T | null
  visible: boolean
  lastHideAt: number
}

export function idleTip<T>(): TipCore<T> {
  return { target: null, pending: null, dismissed: null, visible: false, lastHideAt: 0 }
}

export type TipAction<T> =
  | { kind: 'noop' }
  | { kind: 'hide'; reason: TipHideReason }
  | { kind: 'reveal'; el: T }
  | { kind: 'schedule'; el: T; delay: number }

export function applyHide<T>(s: TipCore<T>, reason: TipHideReason, now: number): TipCore<T> {
  return {
    target: null,
    pending: null,
    dismissed: reason === 'dismiss' ? (s.target ?? s.pending ?? s.dismissed) : s.dismissed,
    visible: false,
    lastHideAt: reason === 'leave' && (s.visible || !!s.target || !!s.pending) ? now : s.lastHideAt,
  }
}

export function applyConsider<T>(s: TipCore<T>, el: T | null, now: number): { next: TipCore<T>; action: TipAction<T> } {
  if (el && el === s.dismissed) return { next: s, action: { kind: 'noop' } }
  const dismissed = s.dismissed && el !== s.dismissed ? null : s.dismissed
  const cur = { ...s, dismissed }
  if (el && (el === cur.target || el === cur.pending)) return { next: cur, action: { kind: 'noop' } }
  if (!el) {
    const next = applyHide(cur, 'leave', now)
    return { next, action: { kind: 'hide', reason: 'leave' } }
  }
  const delay = showDelayMs(now, cur.lastHideAt, cur.visible)
  if (delay <= 0) {
    return { next: { ...cur, pending: null }, action: { kind: 'reveal', el } }
  }
  return { next: { ...cur, pending: el }, action: { kind: 'schedule', el, delay } }
}

export function applyTimer<T>(s: TipCore<T>): { next: TipCore<T>; action: TipAction<T> } {
  const el = s.pending
  if (!el) return { next: s, action: { kind: 'noop' } }
  return { next: { ...s, pending: null }, action: { kind: 'reveal', el } }
}

export function placeTooltip(
  target: { left: number; top: number; right: number; bottom: number; width: number },
  tip: { width: number; height: number },
  view: { width: number; height: number },
  cursorX?: number,
  gap = TIP_GAP,
  pad = TIP_PAD,
): TipPlace {
  const tw = tip.width
  const th = tip.height
  const vw = view.width
  const vh = view.height

  let anchorX = target.left + target.width / 2
  if (
    cursorX != null &&
    Number.isFinite(cursorX) &&
    target.width >= TIP_CURSOR_ANCHOR_PX &&
    cursorX >= target.left &&
    cursorX <= target.right
  ) {
    anchorX = cursorX
  }

  let x = anchorX - tw / 2
  const maxX = Math.max(pad, vw - tw - pad)
  x = clamp(x, pad, maxX)

  const below = target.bottom + gap
  const above = target.top - gap - th
  const belowFits = below + th + pad <= vh
  const aboveFits = above >= pad
  const placement: TipPlacement = belowFits || !aboveFits ? 'below' : 'above'
  let y = placement === 'below' ? below : above
  y = clamp(y, pad, Math.max(pad, vh - th - pad))

  return { x, y, placement }
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n))
}

export function describeWithTip(el: Element): void {
  if (!(el instanceof HTMLElement)) return
  const prev = el.getAttribute('aria-describedby')
  if (!prev) {
    el.setAttribute('aria-describedby', UI_TIP_ID)
    return
  }
  const ids = prev.split(/\s+/).filter(Boolean)
  if (ids.includes(UI_TIP_ID)) return
  el.setAttribute('data-ui-describedby', prev)
  el.setAttribute('aria-describedby', `${prev} ${UI_TIP_ID}`)
}

export function undescribeWithTip(el: Element): void {
  if (!(el instanceof HTMLElement)) return
  const saved = el.getAttribute('data-ui-describedby')
  el.removeAttribute('data-ui-describedby')
  if (saved != null) {
    el.setAttribute('aria-describedby', saved)
    return
  }
  const cur = el.getAttribute('aria-describedby')
  if (cur === UI_TIP_ID) el.removeAttribute('aria-describedby')
}
