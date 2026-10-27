/** Overlay thumb above sticky rows — native scrollbars sit under z-index. */

export function overlayThumb(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  minThumb = 24,
): { h: number; top: number; overflow: boolean } {
  const maxScroll = scrollHeight - clientHeight
  const track = Math.max(clientHeight, 1)
  const ratio = Math.min(1, track / Math.max(scrollHeight, 1))
  const h = Math.min(track, Math.max(minThumb, track * ratio))
  const top = maxScroll > 0 ? (scrollTop / maxScroll) * (track - h) : 0
  return { h, top, overflow: maxScroll > 1 }
}

export function scrollFromTrack(
  thumbTop: number,
  scrollHeight: number,
  clientHeight: number,
  minThumb = 24,
): number {
  const { h } = overlayThumb(0, scrollHeight, clientHeight, minThumb)
  const travel = Math.max(clientHeight, 1) - h
  const maxScroll = scrollHeight - clientHeight
  if (travel <= 0 || maxScroll <= 0) return 0
  return (Math.max(0, Math.min(travel, thumbTop)) / travel) * maxScroll
}

const HIDE_MS = 700

/** `{@attach overlayScrollbar()}` — parent should be `.overlay-sb-shell`. */
export function overlayScrollbar() {
  return (el: HTMLElement) => {
    const shell = el.parentElement
    if (!shell) return
    el.classList.add('overlay-sb-scroller')
    const sb = document.createElement('div')
    sb.className = 'overlay-sb'
    sb.setAttribute('role', 'scrollbar')
    sb.setAttribute('aria-orientation', 'vertical')
    sb.setAttribute('aria-valuemin', '0')
    const thumb = document.createElement('div')
    thumb.className = 'overlay-sb-thumb'
    sb.append(thumb)
    shell.append(sb)

    let hideTimer = 0
    let grabbing = false
    let hover = false
    let grab = 0

    const paint = (show?: boolean) => {
      const { h, top, overflow } = overlayThumb(el.scrollTop, el.scrollHeight, el.clientHeight)
      thumb.style.height = `${h}px`
      thumb.style.transform = `translateY(${top}px)`
      sb.classList.toggle('live', overflow)
      sb.setAttribute('aria-valuenow', String(Math.round(el.scrollTop)))
      sb.setAttribute('aria-valuemax', String(Math.max(0, el.scrollHeight - el.clientHeight)))
      if (show == null) {
        if (!overflow) sb.classList.remove('on')
        return
      }
      sb.classList.toggle('on', overflow && (show || grabbing || hover))
    }
    const hideSoon = () => {
      window.clearTimeout(hideTimer)
      if (grabbing || hover) return
      hideTimer = window.setTimeout(() => paint(false), HIDE_MS)
    }
    const onScroll = () => paint()
    const yOnTrack = (clientY: number) => clientY - sb.getBoundingClientRect().top
    const onPointerDown = (e: PointerEvent) => {
      if (e.button !== 0) return
      e.preventDefault()
      const { h, top } = overlayThumb(el.scrollTop, el.scrollHeight, el.clientHeight)
      const y = yOnTrack(e.clientY)
      grab = y >= top && y <= top + h ? y - top : h / 2
      grabbing = true
      try { sb.setPointerCapture(e.pointerId) } catch { /* noop */ }
      el.scrollTop = scrollFromTrack(y - grab, el.scrollHeight, el.clientHeight)
      paint(true)
    }
    const onPointerMove = (e: PointerEvent) => {
      if (!grabbing) return
      el.scrollTop = scrollFromTrack(yOnTrack(e.clientY) - grab, el.scrollHeight, el.clientHeight)
    }
    const onPointerUp = () => {
      grabbing = false
      hideSoon()
    }
    const onEnter = () => {
      hover = true
      paint(true)
    }
    const onLeave = () => {
      hover = false
      hideSoon()
    }
    paint(false)
    el.addEventListener('scroll', onScroll, { passive: true })
    sb.addEventListener('pointerdown', onPointerDown)
    sb.addEventListener('pointermove', onPointerMove)
    sb.addEventListener('pointerup', onPointerUp)
    sb.addEventListener('pointercancel', onPointerUp)
    sb.addEventListener('pointerenter', onEnter)
    sb.addEventListener('pointerleave', onLeave)
    const ro = new ResizeObserver(() => paint())
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
      paint()
    })
    mo.observe(el, { childList: true })
    return () => {
      window.clearTimeout(hideTimer)
      el.removeEventListener('scroll', onScroll)
      sb.removeEventListener('pointerdown', onPointerDown)
      sb.removeEventListener('pointermove', onPointerMove)
      sb.removeEventListener('pointerup', onPointerUp)
      sb.removeEventListener('pointercancel', onPointerUp)
      sb.removeEventListener('pointerenter', onEnter)
      sb.removeEventListener('pointerleave', onLeave)
      ro.disconnect()
      mo.disconnect()
      sb.remove()
      el.classList.remove('overlay-sb-scroller')
    }
  }
}
