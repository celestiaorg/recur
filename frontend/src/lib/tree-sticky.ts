/** Stuck folder headers: only the innermost one fades content beneath it. */

import { scrollDeltaToReveal } from './workspace-nav'

export const TREE_STICKY_SLACK = 1.5
export const TREE_IS_STUCK = 'is-stuck'
export const TREE_FADE_UNDER = 'fade-under'

function cssNum(el: HTMLElement, prop: string, fallback = 0): number {
  const n = parseFloat(getComputedStyle(el).getPropertyValue(prop) || el.style.getPropertyValue(prop))
  return Number.isFinite(n) ? n : fallback
}

function headerStuck(header: HTMLElement, treeTop: number, rowH: number, scrolled: boolean): boolean {
  const depth = cssNum(header, '--d')
  return scrolled && Math.abs(header.getBoundingClientRect().top - (treeTop + depth * rowH)) <= TREE_STICKY_SLACK
}

export function syncTreeStickyFades(
  scroller: HTMLElement,
  headerSelector: string,
  rowH = 24,
): void {
  const scrolled = scroller.scrollTop > 1
  const treeTop = scroller.getBoundingClientRect().top
  const headers = [...scroller.querySelectorAll<HTMLElement>(headerSelector)]
  let inner: HTMLElement | null = null
  let innerDepth = -1
  const stuckAt = headers.map((header) => {
    const stuck = headerStuck(header, treeTop, rowH, scrolled)
    const depth = cssNum(header, '--d')
    if (stuck && depth >= innerDepth) {
      inner = header
      innerDepth = depth
    }
    return stuck
  })
  const fadeH = cssNum(scroller, '--tree-fade', 20)
  headers.forEach((header, i) => {
    header.classList.toggle(TREE_IS_STUCK, stuckAt[i])
    header.classList.toggle(TREE_FADE_UNDER, header === inner && !fadeHitsLiveRow(scroller, header, fadeH))
  })
}

function fadeHitsLiveRow(scroller: HTMLElement, header: HTMLElement, fadeH: number): boolean {
  const hr = header.getBoundingClientRect()
  const fadeTop = hr.bottom
  const fadeBot = fadeTop + fadeH
  const overlaps = (el: HTMLElement) => {
    if (el === header) return false
    const r = el.getBoundingClientRect()
    return r.bottom > fadeTop && r.top < fadeBot
  }
  for (const row of scroller.querySelectorAll<HTMLElement>('.on')) {
    if (overlaps(row)) return true
  }
  for (const row of scroller.querySelectorAll<HTMLElement>('.obs-node, .obs-scm-row, .fl-tree, .fl-row')) {
    if (overlaps(row) && row.matches(':hover')) return true
  }
  return false
}

function stickyCoverInset(
  scroller: HTMLElement,
  headerSelector: string,
  focus: HTMLElement,
  rowH = 24,
): number {
  const scrolled = scroller.scrollTop > 1
  const treeTop = scroller.getBoundingClientRect().top
  let n = 0
  for (const header of scroller.querySelectorAll<HTMLElement>(headerSelector)) {
    if (header === focus || header.contains(focus)) continue
    if (headerStuck(header, treeTop, rowH, scrolled)) n++
  }
  return n * rowH
}

export function revealUnderSticky(
  scroller: HTMLElement,
  headerSelector: string,
  el: HTMLElement,
  rowH = 24,
): void {
  if (el === scroller || !scroller.contains(el)) return
  const view = scroller.getBoundingClientRect()
  const r = el.getBoundingClientRect()
  const delta = scrollDeltaToReveal(
    r.top, r.bottom, view.top, view.bottom,
    stickyCoverInset(scroller, headerSelector, el, rowH), 0,
  )
  if (delta === 0) return
  const max = Math.max(0, scroller.scrollHeight - scroller.clientHeight)
  scroller.scrollTop = Math.min(max, Math.max(0, scroller.scrollTop + delta))
}

/** `{@attach stickyFolderFades('.obs-node.dir')}` */
export function stickyFolderFades(headerSelector: string) {
  return (el: HTMLElement) => {
    let scheduled = 0
    const rowH = () => cssNum(el, '--tree-row-h', 24)
    const run = () => {
      scheduled = 0
      syncTreeStickyFades(el, headerSelector, rowH())
    }
    const schedule = () => {
      if (scheduled) return
      scheduled = requestAnimationFrame(run)
    }
    const onFocusIn = (e: FocusEvent) => {
      const t = e.target
      if (!(t instanceof HTMLElement)) return
      revealUnderSticky(el, headerSelector, t, rowH())
    }
    run()
    el.addEventListener('scroll', schedule, { passive: true })
    el.addEventListener('focusin', onFocusIn)
    el.addEventListener('pointerover', schedule)
    el.addEventListener('pointerout', schedule)
    const ro = new ResizeObserver(schedule)
    ro.observe(el)
    const mo = new MutationObserver(schedule)
    mo.observe(el, { childList: true, subtree: true, attributes: true, attributeFilter: ['class'] })
    return () => {
      cancelAnimationFrame(scheduled)
      el.removeEventListener('scroll', schedule)
      el.removeEventListener('focusin', onFocusIn)
      el.removeEventListener('pointerover', schedule)
      el.removeEventListener('pointerout', schedule)
      ro.disconnect()
      mo.disconnect()
      el.querySelectorAll(headerSelector).forEach((h) => {
        h.classList.remove(TREE_IS_STUCK, TREE_FADE_UNDER)
      })
    }
  }
}
