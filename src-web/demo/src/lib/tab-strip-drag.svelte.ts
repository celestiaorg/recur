import { tick } from 'svelte'
import {
  CHAT_TAB_DRAG_THRESHOLD, insertIndexFromPointerX, reorderByInsert, restoreStripOrder,
} from './chat-tab-reorder'

const FLIP_MS = 180
const SETTLE_MS = 200
const PLACEHOLDER_CLASS = 'tab-chip-ph'
const EDGE = 28
const EDGE_STEP = 12

export const TAB_STRIP_DRAGGING_CLASS = 'dragging-tabs'

export type TabStripOwner<T> = {
  getItems: () => T[]
  setItems: (items: T[]) => void
}

export type TabStripDragOpts<T> = {
  getStrip: () => HTMLElement | undefined
  getFloat: () => HTMLElement | undefined
  getItems: () => T[]
  setItems: (items: T[]) => void
  keyOf: (item: T) => string
  canStart?: () => boolean
  onCommit?: (items: T[], previous: T[]) => void | Promise<void>
  /** Frozen get/set for the list being dragged. Captured at drag start so cancel/rollback never retarget. */
  bindOwner?: () => TabStripOwner<T>
}

/** Pointer-drag reorder for a TabStrip: placeholder + floating chip + FLIP. */
export class TabStripDrag<T> {
  active = $state(false)
  settling = $state(false)
  committing = $state(false)
  fromKey = $state<string | null>(null)
  x = $state(0)
  y = $state(0)
  w = $state(0)
  h = $state(0)
  suppressClick = false

  #opts: TabStripDragOpts<T>
  #owner: TabStripOwner<T> | null = null
  #offsetX = 0
  #offsetY = 0
  #before: T[] | null = null
  #pointerId: number | null = null
  #onMove: ((ev: PointerEvent) => void) | null = null
  #onUp: ((ev: PointerEvent) => void) | null = null
  #flipTimer = 0
  #scrollRaf = 0
  #scrollX = 0
  #gen = 0

  constructor(opts: TabStripDragOpts<T>) {
    this.#opts = opts
  }

  onPointerDown = (e: PointerEvent, key: string) => {
    this.suppressClick = false
    if (e.button !== 0 || this.settling || this.active || this.committing) return
    if (this.#opts.canStart && !this.#opts.canStart()) return
    this.#unbindPointer()
    const chip = e.currentTarget as HTMLElement
    const startX = e.clientX
    const startY = e.clientY
    const rect = chip.getBoundingClientRect()
    this.#offsetX = e.clientX - rect.left
    this.#offsetY = e.clientY - rect.top
    this.w = rect.width
    this.h = rect.height
    this.#pointerId = e.pointerId

    const onMove = (ev: PointerEvent) => {
      if (ev.pointerId !== this.#pointerId || this.settling) return
      if (!this.active) {
        if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < CHAT_TAB_DRAG_THRESHOLD) return
        this.#bindOwner()
        this.active = true
        this.fromKey = key
        this.#before = [...this.#list()]
        this.suppressClick = true
        document.body.classList.add(TAB_STRIP_DRAGGING_CLASS)
      }
      ev.preventDefault()
      this.x = ev.clientX - this.#offsetX
      this.y = ev.clientY - this.#offsetY
      this.#reorderByPointerX(ev.clientX)
      this.#autoScroll(ev.clientX)
    }

    const onUp = (ev: PointerEvent) => {
      if (ev.pointerId !== this.#pointerId) return
      this.#unbindPointer()
      void this.#finish()
    }

    this.#onMove = onMove
    this.#onUp = onUp
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
  }

  /** ⌥← / ⌥→ on a focused chip — one slot, then persist. */
  onKeydown = (e: KeyboardEvent, key: string) => {
    if (this.active || this.settling || this.committing) return
    if (!e.altKey || e.metaKey || e.ctrlKey || e.shiftKey) return
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
    if (this.#opts.canStart && !this.#opts.canStart()) return
    e.preventDefault()
    e.stopPropagation()
    this.moveBy(key, e.key === 'ArrowLeft' ? -1 : 1)
  }

  moveBy(key: string, dir: -1 | 1) {
    if (this.active || this.settling || this.committing) return
    this.#bindOwner()
    const list = this.#list()
    const keyOf = this.#opts.keyOf
    const fromIndex = list.findIndex((t) => keyOf(t) === key)
    if (fromIndex < 0) {
      this.#owner = null
      return
    }
    const next = reorderByInsert(list, fromIndex, fromIndex + dir)
    if (next.every((t, i) => keyOf(t) === keyOf(list[i]!))) {
      this.#owner = null
      return
    }
    this.#write(next)
    void this.#commit(next, list)
  }

  /** Drop an in-flight pointer drag without persisting. Workspace switch / unmount. */
  cancel() {
    this.#gen += 1
    this.#unbindPointer()
    this.#stopScroll()
    if (this.#flipTimer) {
      clearTimeout(this.#flipTimer)
      this.#flipTimer = 0
    }
    this.#pointerId = null
    const snapshot = this.#before
    this.#before = null
    this.active = false
    this.fromKey = null
    this.settling = false
    this.suppressClick = false
    document.body.classList.remove(TAB_STRIP_DRAGGING_CLASS)
    // In-flight persist keeps the optimistic order; rollback still has #owner.
    if (this.committing) return
    if (snapshot) this.#restore(snapshot)
    this.#owner = null
  }

  async #commit(items: T[], previous: T[]) {
    if (!this.#owner) this.#bindOwner()
    this.committing = true
    try {
      await this.#opts.onCommit?.(items, previous)
    } finally {
      this.committing = false
      this.#owner = null
    }
  }

  rollback(snapshot: T[]) {
    if (!this.#owner) return
    this.#restore(snapshot)
  }

  #bindOwner() {
    this.#owner = this.#opts.bindOwner?.() ?? {
      getItems: this.#opts.getItems,
      setItems: this.#opts.setItems,
    }
  }

  #list(): T[] {
    return (this.#owner ?? this.#opts).getItems()
  }

  #write(items: T[]) {
    (this.#owner ?? this.#opts).setItems(items)
  }

  #restore(snapshot: T[]) {
    const owner = this.#owner ?? this.#opts
    owner.setItems(restoreStripOrder(owner.getItems(), snapshot, this.#opts.keyOf))
  }

  #unbindPointer() {
    if (this.#onMove) {
      window.removeEventListener('pointermove', this.#onMove)
      this.#onMove = null
    }
    if (this.#onUp) {
      window.removeEventListener('pointerup', this.#onUp)
      window.removeEventListener('pointercancel', this.#onUp)
      this.#onUp = null
    }
  }

  #prefersReducedMotion() {
    return typeof window !== 'undefined' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches
  }

  #measureFlipLefts(): Map<string, number> {
    const map = new Map<string, number>()
    const strip = this.#opts.getStrip()
    if (!strip) return map
    for (const el of strip.querySelectorAll<HTMLElement>('.tab-chip[data-tab-key]:not(.is-float)')) {
      const k = el.dataset.tabKey
      if (k) map.set(k, el.getBoundingClientRect().left)
    }
    const ph = strip.querySelector(`.${PLACEHOLDER_CLASS}`) as HTMLElement | null
    if (ph) map.set('__ph__', ph.getBoundingClientRect().left)
    return map
  }

  async #flip(from: Map<string, number>) {
    if (this.#flipTimer) {
      clearTimeout(this.#flipTimer)
      this.#flipTimer = 0
    }
    await tick()
    const strip = this.#opts.getStrip()
    if (!strip || this.#prefersReducedMotion() || from.size === 0) return
    const movers: { el: HTMLElement; dx: number }[] = []
    for (const el of strip.querySelectorAll<HTMLElement>('.tab-chip[data-tab-key]:not(.is-float)')) {
      const k = el.dataset.tabKey
      if (!k || !from.has(k)) continue
      const dx = from.get(k)! - el.getBoundingClientRect().left
      if (Math.abs(dx) < 0.5) continue
      movers.push({ el, dx })
    }
    const ph = strip.querySelector(`.${PLACEHOLDER_CLASS}`) as HTMLElement | null
    if (ph && from.has('__ph__')) {
      const dx = from.get('__ph__')! - ph.getBoundingClientRect().left
      if (Math.abs(dx) >= 0.5) movers.push({ el: ph, dx })
    }
    if (!movers.length) return
    for (const { el, dx } of movers) {
      el.style.transition = 'none'
      el.style.transform = `translateX(${dx}px)`
    }
    void strip.offsetWidth
    for (const { el } of movers) {
      el.style.transition = `transform ${FLIP_MS}ms cubic-bezier(.2,.8,.2,1)`
      el.style.transform = ''
    }
    this.#flipTimer = window.setTimeout(() => {
      this.#flipTimer = 0
      for (const { el } of movers) {
        if (!el.isConnected) continue
        el.style.transition = ''
        el.style.transform = ''
      }
    }, FLIP_MS + 40)
  }

  #reorderByPointerX(clientX: number) {
    if (!this.fromKey || this.settling) return
    const strip = this.#opts.getStrip()
    if (!strip) return
    const list = this.#list()
    const keyOf = this.#opts.keyOf
    const fromIndex = list.findIndex((t) => keyOf(t) === this.fromKey)
    if (fromIndex < 0) return
    const others = list.filter((t) => keyOf(t) !== this.fromKey)
    const mids: number[] = []
    for (const t of others) {
      const el = strip.querySelector(
        `.tab-chip[data-tab-key="${CSS.escape(keyOf(t))}"]:not(.is-float)`,
      ) as HTMLElement | null
      if (!el) return
      const rect = el.getBoundingClientRect()
      mids.push(rect.left + rect.width / 2)
    }
    const insertAt = insertIndexFromPointerX(mids, clientX)
    const next = reorderByInsert(list, fromIndex, insertAt)
    if (next.every((t, i) => keyOf(t) === keyOf(list[i]!))) return
    const from = this.#measureFlipLefts()
    this.#write(next)
    void this.#flip(from)
  }

  #autoScroll(clientX: number) {
    this.#scrollX = clientX
    const strip = this.#opts.getStrip()
    if (!strip) return
    const rect = strip.getBoundingClientRect()
    const inBand = clientX < rect.left + EDGE || clientX > rect.right - EDGE
    if (inBand) {
      if (!this.#scrollRaf) this.#scrollRaf = requestAnimationFrame(this.#tickScroll)
      return
    }
    this.#stopScroll()
  }

  #tickScroll = () => {
    this.#scrollRaf = 0
    if (!this.active || this.settling) return
    const strip = this.#opts.getStrip()
    if (!strip) return
    const rect = strip.getBoundingClientRect()
    const x = this.#scrollX
    if (x < rect.left + EDGE) strip.scrollLeft -= EDGE_STEP
    else if (x > rect.right - EDGE) strip.scrollLeft += EDGE_STEP
    else return
    this.#reorderByPointerX(x)
    this.#scrollRaf = requestAnimationFrame(this.#tickScroll)
  }

  #stopScroll() {
    if (!this.#scrollRaf) return
    cancelAnimationFrame(this.#scrollRaf)
    this.#scrollRaf = 0
  }

  async #settleFloat() {
    await tick()
    const strip = this.#opts.getStrip()
    const ph = strip?.querySelector(`.${PLACEHOLDER_CLASS}`) as HTMLElement | null
    const floatEl = this.#opts.getFloat()
    if (!ph || !floatEl) return
    const target = ph.getBoundingClientRect()
    if (this.#prefersReducedMotion()) {
      this.x = target.left
      this.y = target.top
      return
    }
    this.settling = true
    try {
      await floatEl.animate(
        [
          { left: `${this.x}px`, top: `${this.y}px`, boxShadow: '0 8px 24px rgba(0,0,0,.35)' },
          { left: `${target.left}px`, top: `${target.top}px`, boxShadow: '0 0 0 rgba(0,0,0,0)' },
        ],
        { duration: SETTLE_MS, easing: 'cubic-bezier(.2,.8,.2,1)', fill: 'forwards' },
      ).finished
    } catch {
      /* animation cancelled */
    }
    this.x = target.left
    this.y = target.top
    for (const a of floatEl.getAnimations()) a.cancel()
  }

  async #finish() {
    const wasActive = this.active
    const snapshot = this.#before
    const gen = this.#gen
    this.#pointerId = null
    this.#stopScroll()
    if (this.#flipTimer) {
      clearTimeout(this.#flipTimer)
      this.#flipTimer = 0
    }
    document.body.classList.remove(TAB_STRIP_DRAGGING_CLASS)

    if (!wasActive) {
      this.active = false
      this.fromKey = null
      this.#before = null
      this.settling = false
      return
    }

    await this.#settleFloat()
    if (gen !== this.#gen) return

    this.active = false
    this.fromKey = null
    this.#before = null
    this.settling = false

    if (!snapshot) return
    const current = this.#list()
    const keyOf = this.#opts.keyOf
    const same =
      current.length === snapshot.length &&
      current.every((t, i) => keyOf(t) === keyOf(snapshot[i]!))
    if (same) return
    if (gen !== this.#gen) return
    await this.#commit(current, snapshot)
  }
}
