/** Keep `.fade-top` / `.fade-bot` in sync with whether a scroller has left each edge. */

export const SCROLL_FADE_TOP_CLASS = 'fade-top'
export const SCROLL_FADE_BOT_CLASS = 'fade-bot'
/** Chat CSS: JS has measured. Without this, `.chat-scroll` keeps a default top fade. */
export const SCROLL_FADE_READY_CLASS = 'fade-ready'
export const SCROLL_FADE_SLACK = 1

export type ScrollEdgeFadeOpts = {
  topSlack?: number
  botSlack?: number
  /**
   * Stick-to-bottom chats restore scroll after first paint. Until then, leave
   * the CSS default top fade (do not add `.fade-ready`).
   */
  pinnedToBottom?: () => boolean
}

export function isScrolledFromTop(
  el: Pick<HTMLElement, 'scrollTop'>,
  slack = SCROLL_FADE_SLACK,
): boolean {
  return el.scrollTop > slack
}

/** True when more content sits below the viewport (not pinned to the tail). */
export function isAwayFromBottom(
  scrollTop: number,
  maxScroll: number,
  slack = SCROLL_FADE_SLACK,
): boolean {
  return maxScroll > slack && scrollTop < maxScroll - slack
}

export function scrollerIsParked(el: HTMLElement): boolean {
  return !!el.closest('#wsv-park')
}

export function syncScrollFade(el: HTMLElement, slack = SCROLL_FADE_SLACK): void {
  el.classList.toggle(SCROLL_FADE_TOP_CLASS, isScrolledFromTop(el, slack))
}

/** `{@attach}` — keeps `.fade-top` in sync with scroll and size changes. */
export function fadeScrollTop(el: HTMLElement) {
  const sync = () => syncScrollFade(el)
  sync()
  const raf = requestAnimationFrame(sync)
  el.addEventListener('scroll', sync, { passive: true })
  const ro = new ResizeObserver(sync)
  ro.observe(el)
  return () => {
    cancelAnimationFrame(raf)
    el.removeEventListener('scroll', sync)
    ro.disconnect()
    el.classList.remove(SCROLL_FADE_TOP_CLASS)
  }
}

/**
 * `{@attach fadeScrollEdges({ botSlack })}` — keeps both edge classes in sync.
 * Scroll only reads `scrollTop`; `scrollHeight` / `clientHeight` are cached
 * from ResizeObserver (post-layout). childList watches the scroller's direct
 * children (the transcript column), not rows inside it.
 */
export function fadeScrollEdges(opts: ScrollEdgeFadeOpts = {}) {
  const topSlack = opts.topSlack ?? SCROLL_FADE_SLACK
  const botSlack = opts.botSlack ?? SCROLL_FADE_SLACK
  const pinnedToBottom = opts.pinnedToBottom
  return (el: HTMLElement) => {
    let maxScroll = 0
    const apply = () => {
      // Parked shells reset overflow to 0. Syncing would drop `.fade-top`
      // and the live slot would paint an unmasked edge for a frame.
      if (scrollerIsParked(el)) return
      const top = el.scrollTop
      const fromTop = isScrolledFromTop({ scrollTop: top }, topSlack)
      // Stick-to-bottom restore happens after first paint. Leave the CSS
      // default top fade in place until scrollTop actually moves.
      if (!fromTop && pinnedToBottom?.() && maxScroll > botSlack) return
      el.classList.add(SCROLL_FADE_READY_CLASS)
      el.classList.toggle(SCROLL_FADE_TOP_CLASS, fromTop)
      el.classList.toggle(SCROLL_FADE_BOT_CLASS, isAwayFromBottom(top, maxScroll, botSlack))
    }
    const measure = () => {
      maxScroll = Math.max(0, el.scrollHeight - el.clientHeight)
      apply()
    }
    measure()
    const raf = requestAnimationFrame(measure)
    el.addEventListener('scroll', apply, { passive: true })
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    for (const child of el.children) ro.observe(child)
    const mo = new MutationObserver((records) => {
      for (const rec of records) {
        for (const node of rec.removedNodes) {
          if (node instanceof Element) ro.unobserve(node)
        }
        for (const node of rec.addedNodes) {
          if (node instanceof Element) ro.observe(node)
        }
      }
      measure()
    })
    mo.observe(el, { childList: true })
    return () => {
      cancelAnimationFrame(raf)
      el.removeEventListener('scroll', apply)
      ro.disconnect()
      mo.disconnect()
      // Drop fade-ready so CSS falls back to the default top fade — removing
      // `.fade-top` while leaving fade-ready would unmask for a frame.
      el.classList.remove(SCROLL_FADE_TOP_CLASS, SCROLL_FADE_BOT_CLASS, SCROLL_FADE_READY_CLASS)
    }
  }
}
