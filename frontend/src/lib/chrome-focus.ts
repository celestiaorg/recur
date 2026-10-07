/** WKWebView focuses the first tabbable chrome control when the window
 *  becomes key and treats that as :focus-visible — the sidebar Add project
 *  icon (or whichever button is first) paints an amber ring. Swallow that
 *  activation focus for a short window after the app is keyed. */

const CHROME = '.rail, .titlebar'
const TYPING = 'input, textarea, select, [contenteditable="true"]'

/** How long after the window is keyed we still treat chrome focus as activation. */
export const LAUNCH_CHROME_FOCUS_MS = 500

function isLaunchChromeTarget(el: EventTarget | null): el is HTMLElement {
  if (!(el instanceof HTMLElement)) return false
  if (el.matches(TYPING)) return false
  return !!el.closest(CHROME)
}

function blurLaunchChrome(el: EventTarget | null) {
  if (isLaunchChromeTarget(el)) el.blur()
}

/** Install once at boot. Returns a disposer for tests. */
export function ignoreLaunchChromeFocus(opts?: { armMs?: number }): () => void {
  const armMs = opts?.armMs ?? LAUNCH_CHROME_FOCUS_MS
  let armed = true
  let disposed = false
  let raf = 0
  let timer = 0
  // Chrome control focused when the window is backgrounded. Restored on the
  // next keying; only a *different* chrome node is treated as activation.
  let parked: HTMLElement | null = null

  const tryBlur = (el: EventTarget | null) => {
    if (!armed || el === parked) return
    blurLaunchChrome(el)
  }
  const disarm = () => {
    armed = false
    if (timer) {
      clearTimeout(timer)
      timer = 0
    }
  }
  const scheduleDisarm = () => {
    if (!armed) return
    if (timer) clearTimeout(timer)
    timer = window.setTimeout(disarm, armMs)
  }
  const onKey = () => disarm()
  const onPointer = () => disarm()
  const onFocusIn = (e: FocusEvent) => tryBlur(e.target)
  // Stay armed while the window is in the background so a later Cmd-Tab
  // cannot expire the timer before WKWebView re-applies activation focus.
  const onWindowBlur = () => {
    parked = isLaunchChromeTarget(document.activeElement) ? document.activeElement : null
    armed = true
    if (timer) {
      clearTimeout(timer)
      timer = 0
    }
  }
  // Arm here too: the first keying can arrive after the boot timer with no
  // preceding blur (window shown after load). Same short window as Cmd-Tab.
  const onWindowFocus = () => {
    armed = true
    scheduleDisarm()
  }

  window.addEventListener('keydown', onKey, true)
  window.addEventListener('pointerdown', onPointer, true)
  window.addEventListener('focusin', onFocusIn, true)
  window.addEventListener('blur', onWindowBlur)
  window.addEventListener('focus', onWindowFocus)
  queueMicrotask(() => tryBlur(document.activeElement))
  raf = requestAnimationFrame(() => tryBlur(document.activeElement))
  scheduleDisarm()

  return () => {
    if (disposed) return
    disposed = true
    disarm()
    cancelAnimationFrame(raf)
    window.removeEventListener('keydown', onKey, true)
    window.removeEventListener('pointerdown', onPointer, true)
    window.removeEventListener('focusin', onFocusIn, true)
    window.removeEventListener('blur', onWindowBlur)
    window.removeEventListener('focus', onWindowFocus)
  }
}
