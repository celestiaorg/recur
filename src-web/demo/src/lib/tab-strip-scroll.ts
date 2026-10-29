/**
 * Horizontal "keep this chip in view" for TabStrip.
 *
 * Native Element.scrollIntoView also scrolls ancestor overflow containers —
 * the editor, #v-detail — so a workspace restore that only meant to reveal
 * a file tab yanked the buffer the user had scrolled. Adjust scrollLeft on
 * the strip itself, the same way the sidebar uses scrollDeltaToReveal.
 *
 * In-workspace opens animate that offset with a rAF ease. WKWebView does not
 * reliably animate Element.scrollTo({ behavior: 'smooth' }) on overflow:auto
 * (unlike scrollIntoView, which it does animate — and which we cannot use).
 * Restore assigns scrollLeft instantly. User-driven movement (trackpad
 * mapped onto this overflow-x-only strip, keyboard, drag-select) cancels
 * an in-flight ease — sampled at the start of each ease frame, because a
 * coalesced `scroll` event lands after rAF and would otherwise see our
 * write as programmatic.
 */

import { scrollDeltaToReveal } from './workspace-nav'

/** Matches the feel of Safari's scrollIntoView smooth for a tab-strip hop. */
const SMOOTH_MS = 280

type StripAnim = { raf: number; to: number }

const anims = new WeakMap<HTMLElement, StripAnim>()
const lastWrite = new WeakMap<HTMLElement, number>()
const listening = new WeakSet<HTMLElement>()

/** Next strip.scrollLeft that puts [childLeft, childLeft+childWidth] in view. */
export function nextStripScrollLeft(
  scrollLeft: number,
  clientWidth: number,
  scrollWidth: number,
  childLeft: number,
  childWidth: number,
): number {
  const delta = scrollDeltaToReveal(
    childLeft,
    childLeft + childWidth,
    scrollLeft,
    scrollLeft + clientWidth,
    0,
    0,
  )
  const max = Math.max(0, scrollWidth - clientWidth)
  return Math.min(max, Math.max(0, scrollLeft + delta))
}

/** Stop an in-flight chase so restore can snap without fighting it. */
export function cancelStripScroll(strip: HTMLElement): void {
  const anim = anims.get(strip)
  if (!anim) return
  cancelAnimationFrame(anim.raf)
  anims.delete(strip)
}

function reduceMotion(): boolean {
  return typeof matchMedia === 'function'
    && matchMedia('(prefers-reduced-motion: reduce)').matches
}

function easeOutCubic(t: number): number {
  return 1 - (1 - t) ** 3
}

function writeScrollLeft(strip: HTMLElement, left: number, quiet = false): void {
  if (quiet) {
    const prev = strip.style.overflow
    strip.style.overflow = 'hidden'
    strip.scrollLeft = left
    lastWrite.set(strip, strip.scrollLeft)
    strip.style.overflow = prev
    return
  }
  strip.scrollLeft = left
  lastWrite.set(strip, strip.scrollLeft)
}

/**
 * True when the current scrollLeft is one we just wrote (ease or snap),
 * or a between-frame browser clamp to the new max (chip closed, strip
 * narrowed). A user/keyboard/WebKit-mapped wheel changes scrollLeft away
 * from both the last write and the max.
 */
export function stripScrollIsProgrammatic(strip: HTMLElement): boolean {
  const written = lastWrite.get(strip)
  if (written == null) return false
  if (Math.abs(strip.scrollLeft - written) < 1.5) return true
  const max = Math.max(0, strip.scrollWidth - strip.clientWidth)
  return strip.scrollLeft >= max - 1.5 && written > strip.scrollLeft + 1.5
}

function watchUserScroll(strip: HTMLElement): void {
  if (listening.has(strip)) return
  listening.add(strip)
  strip.addEventListener('scroll', () => {
    if (stripScrollIsProgrammatic(strip)) return
    cancelStripScroll(strip)
  }, { passive: true })
}

function animateScrollLeft(strip: HTMLElement, to: number): void {
  const running = anims.get(strip)
  if (running && Math.abs(running.to - to) < 1) return
  cancelStripScroll(strip)
  watchUserScroll(strip)
  lastWrite.set(strip, strip.scrollLeft)
  const from = strip.scrollLeft
  const t0 = performance.now()
  const tick = (now: number) => {
    // Sample before writing: a coalesced `scroll` event lands after rAF, so
    // writing first would make the user offset look programmatic.
    if (!stripScrollIsProgrammatic(strip)) {
      cancelStripScroll(strip)
      return
    }
    const t = Math.min(1, (now - t0) / SMOOTH_MS)
    writeScrollLeft(strip, from + (to - from) * easeOutCubic(t))
    if (t < 1) {
      anims.set(strip, { raf: requestAnimationFrame(tick), to })
      return
    }
    anims.delete(strip)
  }
  anims.set(strip, { raf: requestAnimationFrame(tick), to })
}

/** Instant scrollLeft (workspace restore). Cancels any in-flight chase. */
export function snapStripScroll(strip: HTMLElement, left: number): void {
  cancelStripScroll(strip)
  writeScrollLeft(strip, left, true)
}

/** Scroll `child` into view inside `strip` without touching ancestor scrollers. */
export function scrollChildIntoStrip(
  strip: HTMLElement,
  child: HTMLElement,
  behavior: ScrollBehavior = 'auto',
): void {
  watchUserScroll(strip)
  const stripRect = strip.getBoundingClientRect()
  const childRect = child.getBoundingClientRect()
  if (stripRect.width < 1) return
  if (childRect.width < 1 && childRect.height < 1) return
  const childLeft = childRect.left - stripRect.left + strip.scrollLeft
  const next = nextStripScrollLeft(
    strip.scrollLeft,
    strip.clientWidth,
    strip.scrollWidth,
    childLeft,
    childRect.width || child.offsetWidth,
  )
  if (Math.abs(next - strip.scrollLeft) < 1) {
    const running = anims.get(strip)
    if (running && Math.abs(running.to - next) >= 1) cancelStripScroll(strip)
    return
  }
  if (behavior === 'smooth' && !reduceMotion()) {
    animateScrollLeft(strip, next)
    return
  }
  snapStripScroll(strip, next)
}
