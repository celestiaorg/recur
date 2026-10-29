/**
 * Fit a Select menu to the window.
 *
 * Menus portal to `document.body` with `position: fixed` so overflow:hidden
 * cards (SettingsGroup) and overflow:auto panes cannot clip them or grow
 * the page. html/body overflow is also hidden, so a menu taller than the
 * space above (or below) the trigger is clipped by the window — the
 * create-workspace model list opening upward is the usual case. Callers
 * set `--sel-space` to this value so CSS can min() it against each
 * variant's designed cap.
 */

/** Gap between trigger and menu — keep in sync with `.sel-menu` CSS. */
export const MENU_GAP_PX = 6
/** Gap under a comment ⋯ trigger — keep in sync with `.cmt-more-menu` placement. */
export const MORE_MENU_GAP_PX = 4
/** Inset from the window edge so the menu isn't flush against the clip. */
export const MENU_EDGE_PX = 8

export type MenuSpaceRect = { top: number; bottom: number; left?: number; width?: number }

export type MenuFixedBox = {
  top: number | undefined
  bottom: number | undefined
  left: number
  width: number
  spacePx: number
}

/** Pixels the menu may occupy without crossing the viewport edge.
 *  Never larger than the real gap — a floor would overflow a trigger that
 *  sits closer to the edge than the floor. */
export function menuViewportSpace(
  trigger: MenuSpaceRect,
  opts: { up?: boolean; viewportHeight?: number; gap?: number; edge?: number } = {},
): number {
  const gap = opts.gap ?? MENU_GAP_PX
  const edge = opts.edge ?? MENU_EDGE_PX
  const vh = opts.viewportHeight ??
    (typeof globalThis.window !== 'undefined' ? globalThis.window.innerHeight : 0)
  const space = opts.up
    ? trigger.top - gap - edge
    : vh - trigger.bottom - gap - edge
  return Math.max(0, Math.floor(space))
}

/** True when the menu has more room above the trigger than below. */
export function menuPreferUp(
  trigger: MenuSpaceRect,
  opts: { viewportHeight?: number; gap?: number; edge?: number } = {},
): boolean {
  const down = menuViewportSpace(trigger, { ...opts, up: false })
  const above = menuViewportSpace(trigger, { ...opts, up: true })
  return above > down
}

/** Viewport-fixed box for a portaled menu, aligned to the trigger. */
export function menuFixedBox(
  trigger: { top: number; bottom: number; left: number; width: number },
  opts: {
    up?: boolean
    minWidth?: number
    viewportWidth?: number
    viewportHeight?: number
    gap?: number
    edge?: number
  } = {},
): MenuFixedBox {
  const gap = opts.gap ?? MENU_GAP_PX
  const edge = opts.edge ?? MENU_EDGE_PX
  const vw = opts.viewportWidth ??
    (typeof globalThis.window !== 'undefined' ? globalThis.window.innerWidth : 0)
  const vh = opts.viewportHeight ??
    (typeof globalThis.window !== 'undefined' ? globalThis.window.innerHeight : 0)
  const width = Math.max(trigger.width, opts.minWidth ?? 0)
  const left = clampMenuLeft(trigger.left, width, { viewportWidth: vw, edge })
  const spacePx = menuViewportSpace(trigger, { up: opts.up, viewportHeight: vh, gap, edge })
  if (opts.up) {
    return { top: undefined, bottom: vh - trigger.top + gap, left, width, spacePx }
  }
  return { top: trigger.bottom + gap, bottom: undefined, left, width, spacePx }
}

/** Keep a fixed menu from crossing the right (or left) viewport edge. */
export function clampMenuLeft(
  left: number,
  width: number,
  opts: { viewportWidth?: number; edge?: number } = {},
): number {
  const edge = opts.edge ?? MENU_EDGE_PX
  const vw = opts.viewportWidth ??
    (typeof globalThis.window !== 'undefined' ? globalThis.window.innerWidth : 0)
  if (left + width > vw - edge) return Math.max(edge, vw - edge - width)
  return Math.max(edge, left)
}

/** Move a menu node to `document.body` so ancestor overflow cannot clip it. */
export function portalToBody(node: HTMLElement): () => void {
  document.body.append(node)
  return () => node.remove()
}

/** Floor for menus whose rows show an inline hint (chat modes). The 220px
 *  ghost floor was sized to the trigger; check + icon + "Agent" + hint
 *  overflows it and the label ellipsizes first. Keep in sync with
 *  `.sel-menu-hinted` in styles/_terminal.scss. */
export const HINT_MENU_MIN_WIDTH = 280

/** Classes + min-width for a portaled menu — descendant CSS no longer applies. */
export function portalMenuMods(
  root: HTMLElement | undefined,
  opts: { hinted?: boolean } = {},
): { className: string; minWidth: number } {
  const cls: string[] = []
  if (root?.classList.contains('sm')) cls.push('sel-menu-sm')
  if (root?.classList.contains('ghost')) cls.push('sel-menu-ghost')
  if (root?.closest('.crumb-base')) cls.push('sel-menu-wide')
  if (root?.closest('.nt-pickers')) cls.push('sel-menu-wide')
  if (root?.closest('.nt-autonomy')) cls.push('sel-menu-compact')
  if (root?.closest('.font-sel') || root?.classList.contains('font-sel')) cls.push('sel-menu-font')
  if (root?.closest('.model-pickers')) cls.push('sel-menu-model')
  if (root?.closest('.issue-pick')) cls.push('sel-menu-issue')
  if (opts.hinted) cls.push('sel-menu-hinted')
  let minWidth = 0
  if (cls.includes('sel-menu-hinted')) minWidth = HINT_MENU_MIN_WIDTH
  else if (cls.includes('sel-menu-model')) minWidth = 240
  else if (cls.includes('sel-menu-issue')) minWidth = 360
  else if (cls.includes('sel-menu-compact')) minWidth = 140
  else if (cls.includes('sel-menu-sm') || cls.includes('sel-menu-ghost') || cls.includes('sel-menu-wide')) {
    minWidth = 220
  }
  return { className: cls.join(' '), minWidth }
}

/** Top for a portaled popover under its trigger, flipped above when
 *  `menuHeight` would cross the bottom window edge. */
export function popoverTop(
  trigger: { top?: number; bottom: number },
  opts: { menuHeight?: number; viewportHeight?: number; gap?: number; edge?: number } = {},
): number {
  const gap = opts.gap ?? MORE_MENU_GAP_PX
  const edge = opts.edge ?? MENU_EDGE_PX
  const vh = opts.viewportHeight ??
    (typeof globalThis.window !== 'undefined' ? globalThis.window.innerHeight : 0)
  const below = trigger.bottom + gap
  const h = opts.menuHeight ?? 0
  if (!h || trigger.top === undefined || below + h <= vh - edge) return below
  return Math.max(edge, trigger.top - gap - h)
}

/** Re-run `onChange` when the trigger may have moved: window resize, or
 *  capture-phase scroll (ancestor overflow, not just the viewport).
 *  Scrolls inside `root` (the menu's own list) are ignored — they cannot
 *  move the trigger. Bursts collapse to one measurement per frame. */
export function watchMenuViewport(onChange: () => void, root?: Node | null): () => void {
  let raf = 0
  const schedule = () => {
    if (raf) return
    raf = requestAnimationFrame(() => {
      raf = 0
      onChange()
    })
  }
  const onScroll = (e: Event) => {
    if (scrollIsInsideMenu(root, e.target)) return
    schedule()
  }
  window.addEventListener('resize', schedule)
  window.addEventListener('scroll', onScroll, true)
  return () => {
    window.removeEventListener('resize', schedule)
    window.removeEventListener('scroll', onScroll, true)
    if (raf) cancelAnimationFrame(raf)
  }
}

/** True when the scroll happened inside the open menu (its option list),
 *  so the trigger cannot have moved. */
export function scrollIsInsideMenu(root: Node | null | undefined, target: EventTarget | null): boolean {
  return !!root && target instanceof Node && root.contains(target)
}
