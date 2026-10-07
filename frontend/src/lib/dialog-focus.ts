/** Tab-cycle helpers for modal dialogs. Skip disabled controls, negative
 *  tabindex, and nested menus (those are arrow-key, not Tab, surfaces). */

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'textarea:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',')

const NESTED_MENU = '.sel-menu, .flags-menu, .ico-menu, .chat-att-lb, .att-ctx, .emoji-suggest, .user-mention, .chat-mention'

/** True when Tab belongs to the slash/@ menu or prompt-argument picker, not the modal trap. */
export function composerOwnsDialogTab(root: ParentNode | null, target: EventTarget | null): boolean {
  if (root?.querySelector('.chat-mention')) return true
  return target instanceof HTMLElement && target.classList.contains('chat-composer-prompt-arg-input')
}

export const KB_FOCUS = 'kb-focus'

export function dialogFocusables(root: HTMLElement): HTMLElement[] {
  const seen = new Set<HTMLElement>()
  const out: HTMLElement[] = []
  for (const el of root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)) {
    if (seen.has(el)) continue
    seen.add(el)
    if (el.tabIndex < 0) continue
    if (el.closest(NESTED_MENU)) continue
    if (el.closest('[hidden], [inert]')) continue
    if (el.getAttribute('aria-hidden') === 'true') continue
    if (!el.getClientRects().length) continue
    out.push(el)
  }
  return out
}

function triggerFor(el: HTMLElement): HTMLElement | null {
  const inSel = el.closest('.sel')
  if (inSel) return inSel.querySelector<HTMLElement>(':scope > .sel-btn')
  const id = el.closest('.sel-menu, .flags-menu, .ico-menu')?.getAttribute('data-sel-id')
  if (!id) return null
  return document.querySelector<HTMLElement>(`.sel[data-sel-id="${id}"] > .sel-btn`)
}

export function clearKeyboardFocus(root: ParentNode = document): void {
  root.querySelectorAll('.' + KB_FOCUS).forEach((n) => n.classList.remove(KB_FOCUS))
}

function wantsTabRing(el: HTMLElement): boolean {
  if (el.tagName === 'TEXTAREA' || el.classList.contains('nt-in')) return false
  if (el.classList.contains('nt-help')) return false
  return el.classList.contains('sel-btn') || el.classList.contains('nt-go') || el.classList.contains('wsv-simplify')
}

/** Focus a control after Tab. Ring class is only for pickers, Simplify, and submit. */
export function focusKeyboard(el: HTMLElement): void {
  clearKeyboardFocus()
  if (wantsTabRing(el)) el.classList.add(KB_FOCUS)
  el.focus()
}

/** Move focus to the next/previous tabbable control inside `root`, wrapping. */
export function cycleDialogTab(root: HTMLElement, shift: boolean): void {
  const items = dialogFocusables(root)
  if (!items.length) return
  const active = document.activeElement instanceof HTMLElement ? document.activeElement : null
  let idx = active ? items.indexOf(active) : -1
  if (idx < 0 && active) {
    const trigger = triggerFor(active)
    if (trigger) idx = items.indexOf(trigger)
  }
  let next: number
  if (idx < 0) {
    next = shift ? items.length - 1 : 0
  } else if (shift) {
    next = idx === 0 ? items.length - 1 : idx - 1
  } else {
    next = idx === items.length - 1 ? 0 : idx + 1
  }
  const el = items[next]
  if (el) focusKeyboard(el)
}
