/**
 * CSS `-webkit-app-region: no-drag` inside `.titlebar` (styles/_base.scss) plus
 * the traffic-light spacer (drag-to-move, but not zoom) and popups that hang
 * below the 52px bar (`.sel` / `.sel-menu`). `.tb-menu-wrap` / `.tb-winctl` are
 * the Linux custom-chrome menu and window controls.
 */
const TITLEBAR_NO_DRAG =
  'button, a, input, textarea, select, .tb-rail, .tb-icon-btn, .crumb-arrow, .crumb-base, .tb-traffic-clear, .tb-instance, .sel, .sel-menu, [role="menu"], .tb-menu-wrap, .tb-winctl'

/** True when a titlebar pointer event landed on zoomable chrome, not a control. */
export function isTitlebarZoomTarget(target: EventTarget | null, clientY?: number): boolean {
  if (!(target instanceof Element)) return false
  const bar = target.closest('.titlebar')
  if (!(bar instanceof Element)) return false

  const band = bar.getBoundingClientRect()
  if (typeof clientY === 'number' && band.height > 0 && clientY >= band.bottom) return false

  if (target.closest(TITLEBAR_NO_DRAG)) return false
  for (let n: Element | null = target; n && n !== bar; n = n.parentElement) {
    if (appRegion(n) === 'no-drag') return false
  }
  return true
}

function appRegion(el: Element): string {
  const style = getComputedStyle(el) as CSSStyleDeclaration & { webkitAppRegion?: string }
  // Prefer the dashed form: happy-dom (and some WebKit builds) expose
  // `-webkit-app-region` via getPropertyValue but leave `webkitAppRegion` empty.
  return (style.getPropertyValue('-webkit-app-region') || style.webkitAppRegion || '').trim()
}
