/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test, vi } from 'vitest'
import {
  RESHOW_WINDOW_MS,
  SHOW_DELAY_MS,
  TIP_CURSOR_ANCHOR_PX,
  UI_TIP_ATTR,
  UI_TIP_CLASS,
  UI_TIP_ID,
  describeWithTip,
  applyConsider,
  applyHide,
  applyTimer,
  holdTitle,
  idleTip,
  placeTooltip,
  pointInRect,
  releaseTitle,
  showDelayMs,
  shouldReshowAfterDismiss,
  syncHeldTitle,
  titledFromElement,
  titledFromPoint,
  tooltipText,
  undescribeWithTip,
  scrollHidesTip,
} from './title-tooltip'

afterEach(() => {
  document.body.replaceChildren()
})

describe('tooltipText / titledFromElement', () => {
  test('reads title, then the held copy, and ignores whitespace-only', () => {
    const el = document.createElement('button')
    el.setAttribute('title', '  Settings  ')
    expect(tooltipText(el)).toBe('Settings')
    el.removeAttribute('title')
    el.setAttribute(UI_TIP_ATTR, '  Held  ')
    expect(tooltipText(el)).toBe('Held')
    el.setAttribute(UI_TIP_ATTR, '   ')
    expect(tooltipText(el)).toBe('')
  })

  test('walks to the nearest titled ancestor and skips the overlay itself', () => {
    const wrap = document.createElement('div')
    wrap.title = 'Parent'
    const child = document.createElement('span')
    wrap.appendChild(child)
    document.body.appendChild(wrap)
    expect(titledFromElement(child)).toBe(wrap)

    const tip = document.createElement('div')
    tip.className = UI_TIP_CLASS
    tip.title = 'Overlay'
    const inner = document.createElement('span')
    tip.appendChild(inner)
    document.body.appendChild(tip)
    expect(titledFromElement(inner)).toBeNull()
  })

  test('skips sidebar workspace rows and their badges', () => {
    const row = document.createElement('button')
    row.className = 'tloop'
    row.title = 'abc12 — ship the tooltip'
    const badge = document.createElement('span')
    badge.title = '+4 −1'
    row.appendChild(badge)
    document.body.appendChild(row)
    expect(titledFromElement(row)).toBeNull()
    expect(titledFromElement(badge)).toBeNull()
  })

  test('keeps tooltips on sidebar workspace hover actions', () => {
    const row = document.createElement('button')
    row.className = 'tloop'
    const acts = document.createElement('span')
    acts.className = 'tloop-acts'
    const rm = document.createElement('span')
    rm.title = 'Remove'
    acts.appendChild(rm)
    row.appendChild(acts)
    document.body.appendChild(row)
    expect(titledFromElement(rm)).toBe(rm)
  })

  test('skips pane resize bars', () => {
    const v = document.createElement('button')
    v.className = 'vsplit'
    v.title = 'Drag to resize'
    const h = document.createElement('button')
    h.className = 'hsplit findings-hsplit'
    h.title = 'Drag to resize'
    document.body.append(v, h)
    expect(titledFromElement(v)).toBeNull()
    expect(titledFromElement(h)).toBeNull()
  })

  test('skips iframes — title is the document accessible name, not a hover tip', () => {
    const frame = document.createElement('iframe')
    frame.title = 'docs/guide.html'
    document.body.appendChild(frame)
    expect(titledFromElement(frame)).toBeNull()
  })

  test('honors data-ui-tip=off on an ancestor', () => {
    const wrap = document.createElement('div')
    wrap.setAttribute('data-ui-tip', 'off')
    const btn = document.createElement('button')
    btn.title = 'Nope'
    wrap.appendChild(btn)
    document.body.appendChild(wrap)
    expect(titledFromElement(btn)).toBeNull()
  })

  test('skips empty titles', () => {
    const el = document.createElement('button')
    el.title = '   '
    expect(titledFromElement(el)).toBeNull()
  })
})

describe('holdTitle / releaseTitle / syncHeldTitle', () => {
  test('strips title so the OS tooltip cannot appear, then restores it', () => {
    const el = document.createElement('button')
    el.title = 'New chat (⌘T)'
    expect(holdTitle(el)).toBe('New chat (⌘T)')
    expect(el.hasAttribute('title')).toBe(false)
    expect(el.getAttribute(UI_TIP_ATTR)).toBe('New chat (⌘T)')
    releaseTitle(el)
    expect(el.title).toBe('New chat (⌘T)')
    expect(el.hasAttribute(UI_TIP_ATTR)).toBe(false)
  })

  test('keeps a title written while held (Svelte update) instead of restoring stale text', () => {
    const el = document.createElement('button')
    el.title = 'Old'
    holdTitle(el)
    el.setAttribute('title', 'New')
    releaseTitle(el)
    expect(el.title).toBe('New')
    expect(el.hasAttribute(UI_TIP_ATTR)).toBe(false)
  })

  test('syncHeldTitle restrips a title that came back during hover', () => {
    const el = document.createElement('button')
    el.title = 'Old'
    holdTitle(el)
    el.setAttribute('title', 'Updated')
    expect(syncHeldTitle(el)).toBe('Updated')
    expect(el.hasAttribute('title')).toBe(false)
    expect(el.getAttribute(UI_TIP_ATTR)).toBe('Updated')
  })

  test('whitespace-only title is not held', () => {
    const el = document.createElement('button')
    el.title = '  '
    expect(holdTitle(el)).toBeNull()
    expect(el.hasAttribute('title')).toBe(false)
    expect(el.hasAttribute(UI_TIP_ATTR)).toBe(false)
  })

  test('holdTitle reuses the held copy when title is already stripped', () => {
    const el = document.createElement('button')
    el.title = 'Once'
    holdTitle(el)
    expect(holdTitle(el)).toBe('Once')
  })

  test('returns trimmed text for display but restores the raw attribute', () => {
    const el = document.createElement('button')
    el.title = '  Hello\n'
    expect(holdTitle(el)).toBe('Hello')
    expect(el.getAttribute(UI_TIP_ATTR)).toBe('  Hello\n')
    releaseTitle(el)
    expect(el.getAttribute('title')).toBe('  Hello\n')
  })
})

describe('titledFromPoint', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  test('does not see through an overlay to a titled control underneath', () => {
    const btn = document.createElement('button')
    btn.title = 'Settings'
    const overlay = document.createElement('div')
    document.body.append(btn, overlay)
    vi.spyOn(document, 'elementFromPoint').mockReturnValue(overlay)
    expect(titledFromPoint(10, 10)).toBeNull()
  })

  test('resolves a titled ancestor of the topmost hit', () => {
    const btn = document.createElement('button')
    btn.title = 'Settings'
    const icon = document.createElement('span')
    btn.appendChild(icon)
    document.body.append(btn)
    vi.spyOn(document, 'elementFromPoint').mockReturnValue(icon)
    expect(titledFromPoint(10, 10)).toBe(btn)
  })
})

describe('pointInRect', () => {
  test('includes the edges of the box', () => {
    const r = { left: 10, top: 20, right: 40, bottom: 50 }
    expect(pointInRect(r, 10, 20)).toBe(true)
    expect(pointInRect(r, 40, 50)).toBe(true)
    expect(pointInRect(r, 9, 30)).toBe(false)
    expect(pointInRect(r, 25, 51)).toBe(false)
  })
})

describe('scrollHidesTip', () => {
  test('chat auto-follow does not hide a sidebar control', () => {
    const chat = document.createElement('div')
    const btn = document.createElement('button')
    document.body.append(chat, btn)
    expect(scrollHidesTip(chat, btn)).toBe(false)
  })

  test("scrolling the control's ancestor hides the tip", () => {
    const side = document.createElement('div')
    const btn = document.createElement('button')
    side.appendChild(btn)
    document.body.append(side)
    expect(scrollHidesTip(side, btn)).toBe(true)
    expect(scrollHidesTip(document, btn)).toBe(true)
  })

  test('nothing watched is a no-op; unknown scroller still hides', () => {
    const btn = document.createElement('button')
    document.body.append(btn)
    expect(scrollHidesTip(document, null)).toBe(false)
    expect(scrollHidesTip(null, btn)).toBe(true)
  })
})

describe('showDelayMs', () => {
  test('uses the full delay on a cold hover', () => {
    expect(showDelayMs(1000, 0, false)).toBe(SHOW_DELAY_MS)
  })

  test('is immediate while a tip is already up, or shortly after hide', () => {
    expect(showDelayMs(1000, 0, true)).toBe(0)
    expect(showDelayMs(1000, 1000 - RESHOW_WINDOW_MS + 1, false)).toBe(0)
    expect(showDelayMs(1000, 1000 - RESHOW_WINDOW_MS, false)).toBe(SHOW_DELAY_MS)
    expect(showDelayMs(1000, 1000 - RESHOW_WINDOW_MS - 1, false)).toBe(SHOW_DELAY_MS)
  })
})

describe('placeTooltip', () => {
  test('centers below the target and clamps to the viewport', () => {
    const p = placeTooltip(
      { left: 10, top: 10, right: 50, bottom: 30, width: 40 },
      { width: 80, height: 24 },
      { width: 200, height: 200 },
    )
    expect(p.placement).toBe('below')
    expect(p.x).toBe(8)
    expect(p.y).toBe(30 + 6)
  })

  test('flips above when there is no room below', () => {
    const p = placeTooltip(
      { left: 80, top: 160, right: 120, bottom: 190, width: 40 },
      { width: 60, height: 24 },
      { width: 200, height: 200 },
    )
    expect(p.placement).toBe('above')
    expect(p.y).toBe(160 - 6 - 24)
  })

  test('stays below when neither side fits, so the label stays near the control', () => {
    const p = placeTooltip(
      { left: 0, top: 0, right: 40, bottom: 20, width: 40 },
      { width: 80, height: 400 },
      { width: 200, height: 200 },
    )
    expect(p.placement).toBe('below')
    expect(p.y).toBe(8)
  })

  test('anchors wide targets to the cursor X when it sits over the target', () => {
    const target = { left: 0, top: 10, right: 400, bottom: 30, width: 400 }
    expect(target.width).toBeGreaterThanOrEqual(TIP_CURSOR_ANCHOR_PX)
    const p = placeTooltip(target, { width: 80, height: 20 }, { width: 400, height: 200 }, 200)
    expect(p.x).toBe(200 - 40)
  })

  test('ignores cursor X on narrow targets so icon-row tips stay centered', () => {
    const p = placeTooltip(
      { left: 100, top: 10, right: 124, bottom: 34, width: 24 },
      { width: 80, height: 20 },
      { width: 400, height: 200 },
      200,
    )
    expect(p.x).toBe(112 - 40)
  })
})

describe('tip machine', () => {
  const A = 'open-in'
  const B = 'copy-path'

  test('enter arms a delay; re-enter of the same pending target is a no-op', () => {
    let s = idleTip<string>()
    const first = applyConsider(s, A, 1000)
    expect(first.action).toEqual({ kind: 'schedule', el: A, delay: SHOW_DELAY_MS })
    expect(first.next.pending).toBe(A)
    const again = applyConsider(first.next, A, 1016)
    expect(again.action).toEqual({ kind: 'noop' })
    expect(again.next.pending).toBe(A)
    s = first.next
  })

  test('leave during the delay cancels pending and does not reveal', () => {
    const armed = applyConsider(idleTip<string>(), A, 1000).next
    const left = applyConsider(armed, null, 1080)
    expect(left.action).toEqual({ kind: 'hide', reason: 'leave' })
    expect(left.next.pending).toBeNull()
    expect(left.next.lastHideAt).toBe(1080)
    const tick = applyTimer(left.next)
    expect(tick.action).toEqual({ kind: 'noop' })
  })

  test('timer after a stay-put hover reveals the pending element', () => {
    const armed = applyConsider(idleTip<string>(), A, 1000).next
    const fired = applyTimer(armed)
    expect(fired.action).toEqual({ kind: 'reveal', el: A })
    expect(fired.next.pending).toBeNull()
  })

  test('click or Escape during the delay records dismissed so a later jiggle stays hidden', () => {
    const armed = applyConsider(idleTip<string>(), A, 1000).next
    const down = applyHide(armed, 'dismiss', 1100)
    expect(down.dismissed).toBe(A)
    expect(down.pending).toBeNull()
    const drift = applyHide(down, 'dismiss', 1110)
    expect(drift.dismissed).toBe(A)
    const up = applyConsider(drift, A, 1120)
    expect(up.action).toEqual({ kind: 'noop' })
  })

  test('dismiss while visible, then a second dismiss during the press, keeps the suppression', () => {
    let s = idleTip<string>()
    s = { ...applyConsider(s, A, 1000).next, target: A, visible: true, pending: null }
    const down = applyHide(s, 'dismiss', 2000)
    expect(down.dismissed).toBe(A)
    const move = applyHide(down, 'dismiss', 2010)
    expect(move.dismissed).toBe(A)
    expect(applyConsider(move, A, 2020).action).toEqual({ kind: 'noop' })
  })

  test('scroll interrupt does not grant a zero-delay reshow', () => {
    const shown = { ...idleTip<string>(), target: A, visible: true }
    const scrolled = applyHide(shown, 'interrupt', 3000)
    expect(scrolled.lastHideAt).toBe(0)
    const again = applyConsider(scrolled, A, 3010)
    expect(again.action).toEqual({ kind: 'schedule', el: A, delay: SHOW_DELAY_MS })
  })

  test('leave then enter another control inside the reshow window reveals immediately', () => {
    const shown = { ...idleTip<string>(), target: A, visible: true }
    const left = applyHide(shown, 'leave', 4000)
    expect(left.lastHideAt).toBe(4000)
    const next = applyConsider(left, B, 4100)
    expect(next.action).toEqual({ kind: 'reveal', el: B })
  })

  test('Escape then leave clears dismissed so a later re-enter can show again', () => {
    const shown = { ...idleTip<string>(), target: A, visible: true }
    const esc = applyHide(shown, 'dismiss', 5000)
    const left = applyConsider(esc, null, 5100)
    expect(left.next.dismissed).toBeNull()
    const back = applyConsider(left.next, A, 5600)
    expect(back.action.kind).toBe('schedule')
  })
})

describe('shouldReshowAfterDismiss', () => {
  test('a new title on the same node is a new tip', () => {
    expect(shouldReshowAfterDismiss('Remove', 'Confirm remove')).toBe(true)
    expect(shouldReshowAfterDismiss('Confirm remove', 'Confirm remove — uncommitted files')).toBe(true)
  })

  test('the same copy stays suppressed until the pointer leaves', () => {
    expect(shouldReshowAfterDismiss('Remove', 'Remove')).toBe(false)
    expect(shouldReshowAfterDismiss('Remove', null)).toBe(false)
    expect(shouldReshowAfterDismiss('Remove', '')).toBe(false)
  })
})

describe('aria-describedby bridging', () => {
  test('sets and clears the tooltip id, preserving a prior describedby', () => {
    const el = document.createElement('button')
    describeWithTip(el)
    expect(el.getAttribute('aria-describedby')).toBe(UI_TIP_ID)
    undescribeWithTip(el)
    expect(el.hasAttribute('aria-describedby')).toBe(false)

    el.setAttribute('aria-describedby', 'other')
    describeWithTip(el)
    expect(el.getAttribute('aria-describedby')).toBe(`other ${UI_TIP_ID}`)
    describeWithTip(el)
    expect(el.getAttribute('aria-describedby')).toBe(`other ${UI_TIP_ID}`)
    undescribeWithTip(el)
    expect(el.getAttribute('aria-describedby')).toBe('other')
  })
})
