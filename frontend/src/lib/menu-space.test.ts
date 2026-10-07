/** @vitest-environment happy-dom */
import { describe, expect, test, vi } from 'vitest'
import {
  HINT_MENU_MIN_WIDTH,
  MENU_EDGE_PX,
  MENU_GAP_PX,
  MORE_MENU_GAP_PX,
  clampMenuLeft,
  menuFixedBox,
  menuPreferUp,
  menuViewportSpace,
  moreMenuFixedBox,
  portalMenuMods,
  rectsOverlap,
  triggerVisibleInClip,
  scrollIsInsideMenu,
  watchMenuViewport,
} from './menu-space'

describe('menuViewportSpace', () => {
  test('upward menu is the space above the trigger minus gap and edge', () => {
    expect(menuViewportSpace({ top: 220, bottom: 244 }, { up: true, viewportHeight: 800 }))
      .toBe(220 - MENU_GAP_PX - MENU_EDGE_PX)
  })

  test('downward menu is the space below the trigger minus gap and edge', () => {
    expect(menuViewportSpace({ top: 600, bottom: 630 }, { up: false, viewportHeight: 800 }))
      .toBe(800 - 630 - MENU_GAP_PX - MENU_EDGE_PX)
  })

  test('never exceeds the real gap when the trigger sits against the edge', () => {
    expect(menuViewportSpace({ top: 4, bottom: 28 }, { up: true, viewportHeight: 800 }))
      .toBe(0)
    expect(menuViewportSpace({ top: 780, bottom: 798 }, { up: false, viewportHeight: 800 }))
      .toBe(0)
    expect(menuViewportSpace({ top: 20, bottom: 44 }, { up: true, viewportHeight: 800 }))
      .toBe(20 - MENU_GAP_PX - MENU_EDGE_PX)
  })

  test('create-workspace modal: a 560px cap would overflow a 22vh-padded trigger', () => {
    // Overlay padding is 22vh; footer trigger sits near the bottom of a ~280px modal.
    const vh = 720
    const triggerTop = Math.round(vh * 0.22) + 260
    const space = menuViewportSpace({ top: triggerTop, bottom: triggerTop + 24 }, {
      up: true,
      viewportHeight: vh,
    })
    expect(space).toBeLessThan(560)
    expect(space).toBe(triggerTop - MENU_GAP_PX - MENU_EDGE_PX)
  })
})

describe('menuPreferUp', () => {
  test('opens down when more room sits below the trigger', () => {
    expect(menuPreferUp({ top: 80, bottom: 110 }, { viewportHeight: 800 })).toBe(false)
  })

  test('opens up when the trigger sits near the bottom', () => {
    expect(menuPreferUp({ top: 640, bottom: 670 }, { viewportHeight: 800 })).toBe(true)
  })

  test('stays down when both sides are equal', () => {
    expect(menuPreferUp({ top: 388, bottom: 412 }, { viewportHeight: 800 })).toBe(false)
  })
})

describe('menuFixedBox', () => {
  test('pins a downward menu to the trigger bottom and width', () => {
    const box = menuFixedBox(
      { top: 80, bottom: 118, left: 40, width: 200 },
      { viewportWidth: 800, viewportHeight: 600 },
    )
    expect(box.top).toBe(118 + MENU_GAP_PX)
    expect(box.bottom).toBeUndefined()
    expect(box.left).toBe(40)
    expect(box.width).toBe(200)
    expect(box.spacePx).toBe(600 - 118 - MENU_GAP_PX - MENU_EDGE_PX)
  })

  test('pins an upward menu from the viewport bottom', () => {
    const box = menuFixedBox(
      { top: 400, bottom: 424, left: 20, width: 160 },
      { up: true, viewportWidth: 800, viewportHeight: 720 },
    )
    expect(box.top).toBeUndefined()
    expect(box.bottom).toBe(720 - 400 + MENU_GAP_PX)
    expect(box.left).toBe(20)
  })

  test('shifts left so a wide menu stays on-screen', () => {
    const box = menuFixedBox(
      { top: 40, bottom: 70, left: 700, width: 180 },
      { minWidth: 240, viewportWidth: 800, viewportHeight: 600 },
    )
    expect(box.width).toBe(240)
    expect(box.left + box.width).toBeLessThanOrEqual(800 - MENU_EDGE_PX)
  })
})

describe('clampMenuLeft', () => {
  test('does not move a menu that already fits', () => {
    expect(clampMenuLeft(40, 200, { viewportWidth: 800 })).toBe(40)
  })

  test('never goes past the left edge', () => {
    expect(clampMenuLeft(-12, 200, { viewportWidth: 800 })).toBe(MENU_EDGE_PX)
  })
})

describe('moreMenuFixedBox', () => {
  test('sits under the trigger and aligns to its right edge', () => {
    expect(moreMenuFixedBox({ bottom: 120, right: 700 }, { viewportWidth: 800 }))
      .toEqual({ top: 120 + MORE_MENU_GAP_PX, right: 100 })
  })

  test('never sits flush against the viewport edge', () => {
    expect(moreMenuFixedBox({ bottom: 40, right: 796 }, { viewportWidth: 800 }).right)
      .toBe(MENU_EDGE_PX)
  })
})

describe('triggerVisibleInClip', () => {
  test('rectsOverlap is the shared hit test', () => {
    expect(rectsOverlap({ top: 80, right: 200, bottom: 100, left: 180 }, { top: 40, right: 800, bottom: 600, left: 0 })).toBe(true)
    expect(rectsOverlap({ top: 10, right: 200, bottom: 30, left: 180 }, { top: 40, right: 800, bottom: 600, left: 0 })).toBe(false)
  })

  test('false once the trigger leaves the pane, so the menu cannot cover PR chrome', () => {
    const trigger = document.createElement('button')
    document.body.append(trigger)
    vi.spyOn(trigger, 'getBoundingClientRect').mockReturnValue(new DOMRect(180, 20, 22, 22))
    expect(triggerVisibleInClip(trigger, { clip: { top: 80, right: 800, bottom: 600, left: 0 } })).toBe(false)
    expect(triggerVisibleInClip(trigger, { clip: { top: 10, right: 800, bottom: 600, left: 0 } })).toBe(true)
    trigger.remove()
  })
})

describe('scrollIsInsideMenu', () => {
  test('treats the option list as inside and an ancestor scroller as outside', () => {
    const root = document.createElement('div')
    const list = document.createElement('div')
    const outside = document.createElement('div')
    root.append(list)
    expect(scrollIsInsideMenu(root, list)).toBe(true)
    expect(scrollIsInsideMenu(root, root)).toBe(true)
    expect(scrollIsInsideMenu(root, outside)).toBe(false)
    expect(scrollIsInsideMenu(null, list)).toBe(false)
  })
})

describe('watchMenuViewport', () => {
  test('ignores the menu list and coalesces bursts to one frame', async () => {
    const root = document.createElement('div')
    const list = document.createElement('div')
    const scroller = document.createElement('div')
    root.append(list)
    document.body.append(root, scroller)

    const onChange = vi.fn()
    const stop = watchMenuViewport(onChange, root)
    list.dispatchEvent(new Event('scroll', { bubbles: false }))
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(onChange).not.toHaveBeenCalled()

    scroller.dispatchEvent(new Event('scroll', { bubbles: false }))
    scroller.dispatchEvent(new Event('scroll', { bubbles: false }))
    window.dispatchEvent(new Event('resize'))
    expect(onChange).not.toHaveBeenCalled()
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    expect(onChange).toHaveBeenCalledTimes(1)

    stop()
    root.remove()
    scroller.remove()
  })
})

describe('portalMenuMods', () => {
  test('sm / ghost / crumb / autonomy / model pick their floors', () => {
    const sm = document.createElement('div')
    sm.className = 'sel sm'
    expect(portalMenuMods(sm)).toEqual({ className: 'sel-menu-sm', minWidth: 220 })

    const ghost = document.createElement('div')
    ghost.className = 'sel ghost'
    expect(portalMenuMods(ghost)).toEqual({ className: 'sel-menu-ghost', minWidth: 220 })

    const crumbHost = document.createElement('div')
    crumbHost.className = 'crumb-base'
    const crumb = document.createElement('div')
    crumb.className = 'sel'
    crumbHost.append(crumb)
    expect(portalMenuMods(crumb)).toEqual({ className: 'sel-menu-wide', minWidth: 220 })

    const autoHost = document.createElement('div')
    autoHost.className = 'nt-autonomy'
    const auto = document.createElement('div')
    auto.className = 'sel'
    autoHost.append(auto)
    expect(portalMenuMods(auto)).toEqual({ className: 'sel-menu-compact', minWidth: 140 })

    const modelHost = document.createElement('div')
    modelHost.className = 'model-pickers'
    const model = document.createElement('div')
    model.className = 'sel'
    modelHost.append(model)
    expect(portalMenuMods(model)).toEqual({ className: 'sel-menu-model', minWidth: 240 })
  })

  test('hinted rows (chat modes) take a wider floor so the label is not clipped', () => {
    const ghost = document.createElement('div')
    ghost.className = 'sel ghost'
    expect(portalMenuMods(ghost, { hinted: true })).toEqual({
      className: 'sel-menu-ghost sel-menu-hinted',
      minWidth: HINT_MENU_MIN_WIDTH,
    })
    expect(HINT_MENU_MIN_WIDTH).toBeGreaterThan(220)
  })
})
