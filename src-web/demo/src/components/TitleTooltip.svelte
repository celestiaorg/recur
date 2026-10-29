<script lang="ts">
  import { onDestroy, tick } from 'svelte'
  import {
    UI_TIP_ATTR,
    UI_TIP_ID,
    applyConsider,
    applyHide,
    applyTimer,
    describeWithTip,
    holdTitle,
    placeTooltip,
    pointInRect,
    releaseTitle,
    shouldReshowAfterDismiss,
    syncHeldTitle,
    titledFromElement,
    titledFromPoint,
    tooltipText,
    scrollHidesTip,
    undescribeWithTip,
    type TipCore,
    type TipHideReason,
    type TipPlacement,
  } from '../lib/title-tooltip'

  let visible = $state(false)
  let placed = $state(false)
  let text = $state('')
  let x = $state(0)
  let y = $state(0)
  let placement = $state<TipPlacement>('below')
  let tipEl = $state<HTMLDivElement | null>(null)

  let target: HTMLElement | null = null
  /** Stay hidden on this node until the pointer actually leaves it (Escape / click). */
  let dismissed: HTMLElement | null = null
  let pending: HTMLElement | null = null
  let showTimer = 0
  let moveRaf = 0
  let goneRaf = 0
  let lastHideAt = 0
  let havePointer = false
  let lastX = 0
  let lastY = 0
  let observer: MutationObserver | null = null
  let showGen = 0

  function clearTimer() {
    if (showTimer) {
      clearTimeout(showTimer)
      showTimer = 0
    }
    pending = null
  }

  function cancelMove() {
    if (moveRaf) {
      cancelAnimationFrame(moveRaf)
      moveRaf = 0
    }
  }

  function unwatch() {
    observer?.disconnect()
    observer = null
  }

  function stopGoneCheck() {
    if (goneRaf) {
      cancelAnimationFrame(goneRaf)
      goneRaf = 0
    }
  }

  function armGoneCheck() {
    if (goneRaf) return
    const step = () => {
      goneRaf = 0
      if (!target) return
      if (!target.isConnected) {
        hide()
        return
      }
      goneRaf = requestAnimationFrame(step)
    }
    goneRaf = requestAnimationFrame(step)
  }

  function observeTitle(el: HTMLElement, onMutate: () => void) {
    unwatch()
    observer = new MutationObserver(onMutate)
    observer.observe(el, { attributes: true, attributeFilter: ['title', UI_TIP_ATTR] })
    if (el.parentNode) observer.observe(el.parentNode, { childList: true })
  }

  function watch(el: HTMLElement) {
    observeTitle(el, () => {
      if (target !== el) return
      if (!el.isConnected) {
        hide()
        return
      }
      const next = syncHeldTitle(el)
      if (!next) {
        hide()
        return
      }
      if (next === text) return
      text = next
      void layout()
    })
  }

  /** After click/Escape, keep watching so a new title can replace the dismissed copy. */
  function watchDismissed(el: HTMLElement, dismissedText: string) {
    observeTitle(el, () => {
      if (dismissed !== el) return
      if (!el.isConnected) {
        hide()
        return
      }
      const next = tooltipText(el)
      if (!shouldReshowAfterDismiss(dismissedText, next)) return
      dismissed = null
      unwatch()
      if (!havePointer) return
      if (titledFromPoint(lastX, lastY) !== el) return
      void reveal(el)
    })
  }

  function core(): TipCore<HTMLElement> {
    return { target, pending, dismissed, visible, lastHideAt }
  }

  function dropTarget() {
    if (!target) return
    unwatch()
    undescribeWithTip(target)
    releaseTitle(target)
    target = null
  }

  function hide(reason: TipHideReason = 'interrupt') {
    const next = applyHide(core(), reason, Date.now())
    const stay = next.dismissed
    const shownText = text
    dismissed = next.dismissed
    lastHideAt = next.lastHideAt
    clearTimer()
    cancelMove()
    stopGoneCheck()
    dropTarget()
    unwatch()
    visible = false
    placed = false
    text = ''
    x = 0
    y = 0
    showGen++
    if (reason === 'dismiss' && stay) {
      watchDismissed(stay, shownText || tooltipText(stay) || '')
    }
  }

  async function reveal(el: HTMLElement) {
    if (target && target !== el) dropTarget()
    const next = holdTitle(el)
    if (!next) {
      hide()
      return
    }
    target = el
    describeWithTip(el)
    watch(el)
    armGoneCheck()
    // Hide until laid out so a retarget doesn't paint new text at the old x/y.
    placed = false
    text = next
    visible = true
    await layout()
  }

  async function layout() {
    const el = target
    const gen = showGen
    await tick()
    if (gen !== showGen || !el || !el.isConnected || !tipEl || target !== el) return
    const p = placeTooltip(
      el.getBoundingClientRect(),
      tipEl.getBoundingClientRect(),
      { width: window.innerWidth, height: window.innerHeight },
      havePointer ? lastX : undefined,
    )
    x = p.x
    y = p.y
    placement = p.placement
    placed = true
  }

  function consider(el: HTMLElement | null) {
    if (target && !target.isConnected) hide()
    if (el && !el.isConnected) el = null
    const { next, action } = applyConsider(core(), el, Date.now())
    dismissed = next.dismissed
    lastHideAt = next.lastHideAt
    if (action.kind === 'noop') return
    if (action.kind === 'hide') {
      hide('leave')
      return
    }
    clearTimer()
    pending = next.pending
    if (action.kind === 'reveal') {
      void reveal(action.el)
      return
    }
    showTimer = window.setTimeout(() => {
      showTimer = 0
      const fired = applyTimer(core())
      pending = fired.next.pending
      if (fired.action.kind === 'reveal') void reveal(fired.action.el)
    }, action.delay)
  }

  function applyPointer(x0: number, y0: number) {
    lastX = x0
    lastY = y0
    if (target && target.isConnected && pointInRect(target.getBoundingClientRect(), x0, y0)) {
      const top = document.elementFromPoint(x0, y0)
      if (top instanceof Element && (top === target || target.contains(top))) return
    }
    consider(titledFromPoint(x0, y0))
  }

  function onPointer(e: PointerEvent) {
    if (e.pointerType === 'touch') return
    havePointer = true
    lastX = e.clientX
    lastY = e.clientY
    if (e.buttons) {
      hide('dismiss')
      return
    }
    if (moveRaf) return
    moveRaf = requestAnimationFrame(() => {
      moveRaf = 0
      applyPointer(lastX, lastY)
    })
  }

  function onFocusIn(e: FocusEvent) {
    const el = e.target instanceof Element ? titledFromElement(e.target) : null
    if (el) consider(el)
  }

  function onFocusOut(e: FocusEvent) {
    const next = e.relatedTarget instanceof Element ? titledFromElement(e.relatedTarget) : null
    if (next) return
    if (havePointer) consider(titledFromPoint(lastX, lastY))
    else consider(null)
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && (visible || showTimer || target || pending)) hide('dismiss')
  }

  function onPointerOut(e: PointerEvent) {
    if (!e.relatedTarget) hide()
  }

  function onScroll(e: Event) {
    if (scrollHidesTip(e.target, target ?? pending)) hide()
  }

  onDestroy(() => {
    clearTimer()
    cancelMove()
    stopGoneCheck()
    dropTarget()
    unwatch()
  })
</script>

<svelte:window onblur={() => hide()} onresize={() => hide()} />
<svelte:document
  onpointerover={onPointer}
  onpointermove={onPointer}
  onpointerdown={() => hide('dismiss')}
  onpointerup={onPointer}
  onpointercancel={() => hide('dismiss')}
  onpointerout={onPointerOut}
  onfocusin={onFocusIn}
  onfocusout={onFocusOut}
  onkeydown={onKeydown}
  onscrollcapture={onScroll}
/>

<div
  bind:this={tipEl}
  id={UI_TIP_ID}
  class="ui-tip"
  class:above={placement === 'above'}
  class:ready={visible && placed}
  role="tooltip"
  aria-hidden={visible && placed ? undefined : 'true'}
  style:left="{x}px"
  style:top="{y}px"
><div class="ui-tip-text">{text}</div></div>
