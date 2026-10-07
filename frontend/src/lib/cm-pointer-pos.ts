/**
 * Pointer → document position using the painted DOM, not CodeMirror's
 * height map.
 *
 * After a scrollbar thumb jump, intervening lines are unmeasured. Default
 * mouse selection uses posAtCoords(…, false), which estimates and lands many
 * lines above the pointer. A second mapping on mouseup at the same client
 * coordinates then disagrees with mousedown — that becomes a phantom range,
 * and the browser scrolls the bogus caret into view.
 *
 * Wheel scrolling measures as it goes, so the same click works.
 */

import { EditorSelection, type EditorState, type Extension, type SelectionRange } from '@codemirror/state'
import { EditorView, ViewPlugin, type MouseSelectionStyle } from '@codemirror/view'

const POINTER_SLOP_PX = 4
const WHEEL_MEASURE_MS = 120

export function caretFromPoint(x: number, y: number): { node: Node; offset: number } | null {
  if (typeof document === 'undefined') return null
  const doc = document as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null
    caretRangeFromPoint?: (x: number, y: number) => Range | null
  }
  if (typeof doc.caretPositionFromPoint === 'function') {
    const pos = doc.caretPositionFromPoint(x, y)
    if (pos?.offsetNode) return { node: pos.offsetNode, offset: pos.offset }
  }
  if (typeof doc.caretRangeFromPoint === 'function') {
    const range = doc.caretRangeFromPoint(x, y)
    if (range?.startContainer) return { node: range.startContainer, offset: range.startOffset }
  }
  return null
}

export function pointerMoved(
  a: Pick<MouseEvent, 'clientX' | 'clientY'>,
  b: Pick<MouseEvent, 'clientX' | 'clientY'>,
  slop = POINTER_SLOP_PX,
): boolean {
  return Math.max(Math.abs(a.clientX - b.clientX), Math.abs(a.clientY - b.clientY)) > slop
}

export function pickClosestLine<T>(
  lines: { item: T; top: number; bottom: number }[],
  y: number,
): T | null {
  let best: T | null = null
  let bestDist = Infinity
  for (const line of lines) {
    const dist = y < line.top ? line.top - y : y > line.bottom ? y - line.bottom : 0
    if (dist < bestDist) {
      bestDist = dist
      best = line.item
    }
    if (dist === 0) break
  }
  return best
}

export function nearestPaintedLine(content: Element, y: number): Element | null {
  const found: { item: Element; top: number; bottom: number }[] = []
  content.querySelectorAll('.cm-line').forEach(el => {
    const r = el.getBoundingClientRect()
    if (r.height > 0) found.push({ item: el, top: r.top, bottom: r.bottom })
  })
  return pickClosestLine(found, y)
}

export function paintedLineAtPoint(content: Element, x: number, y: number): Element | null {
  const hit = caretFromPoint(x, y)
  if (hit && content.contains(hit.node)) {
    const el = hit.node.nodeType === Node.TEXT_NODE
      ? hit.node.parentElement
      : hit.node instanceof Element ? hit.node : null
    const line = el?.closest('.cm-line')
    if (line && content.contains(line)) return line
  }
  if (typeof document !== 'undefined') {
    const el = document.elementFromPoint(x, y)
    const line = el?.closest?.('.cm-line')
    if (line && content.contains(line)) return line
  }
  return nearestPaintedLine(content, y)
}

function syncScrollerGeometry(view: EditorView): void {
  const scroller = view.scrollDOM
  void scroller.scrollTop
  void scroller.scrollLeft
  void scroller.offsetHeight
  void view.contentDOM.getBoundingClientRect()
}

function assocAt(view: EditorView, pos: number, clientX: number): -1 | 1 {
  const coords = view.coordsAtPos(pos, 1)
  return coords && clientX < (coords.left + coords.right) / 2 ? 1 : -1
}

function posFromDOM(
  view: EditorView,
  node: Node,
  offset: number,
  clientX: number,
): { pos: number; assoc: -1 | 1 } | null {
  try {
    const pos = view.posAtDOM(node, offset)
    return { pos, assoc: assocAt(view, pos, clientX) }
  } catch {
    return null
  }
}

export function posAtPointer(view: EditorView, event: MouseEvent): { pos: number; assoc: -1 | 1 } {
  syncScrollerGeometry(view)
  const x = event.clientX
  const y = event.clientY
  const hit = caretFromPoint(x, y)
  if (hit && view.contentDOM.contains(hit.node)) {
    const el = hit.node.nodeType === Node.TEXT_NODE
      ? hit.node.parentElement
      : hit.node instanceof Element ? hit.node : null
    if (el?.closest('.cm-line')) {
      const fromHit = posFromDOM(view, hit.node, hit.offset, x)
      if (fromHit) return fromHit
    }
  }

  const line = paintedLineAtPoint(view.contentDOM, x, y)
  if (line) {
    const linePos = posFromDOM(view, line, 0, x)
    const precise = view.posAndSideAtCoords({ x, y })
    if (precise && linePos) {
      const painted = view.lineBlockAt(linePos.pos)
      const guessed = view.lineBlockAt(precise.pos)
      if (painted.from === guessed.from) return precise
    }
    if (linePos) return linePos
  }

  const precise = view.posAndSideAtCoords({ x, y })
  if (precise) return precise
  // Do not use posAtCoords(…, false) — that is the jump+select path.
  const head = view.state.selection.main.head
  return { pos: head, assoc: 1 }
}

export function selectionForClick(
  state: EditorState,
  pos: number,
  assoc: -1 | 1,
  type: number,
): SelectionRange {
  if (type <= 1) return EditorSelection.cursor(pos, assoc)
  if (type === 2) {
    const w = state.wordAt(pos)
    return w ? EditorSelection.range(w.from, w.to) : EditorSelection.cursor(pos, assoc)
  }
  const line = state.doc.lineAt(pos)
  let to = line.to
  if (to < state.doc.length) to++
  return EditorSelection.range(line.from, to)
}

/** Safari 26 ignores preventScroll; scrollbar clicks often blur contentDOM. */
const HOLD_SCROLL_MS = 120

function holdScrollOnPointerFocus(): Extension {
  return EditorView.domEventObservers({
    mousedown(event, view) {
      if (event.button !== 0) return
      // Thumb drag targets the scroller itself — don't freeze that.
      if (event.target === view.scrollDOM) return
      if (view.hasFocus) return
      const scroller = view.scrollDOM
      const top = scroller.scrollTop
      const left = scroller.scrollLeft
      let stopped = false
      const restore = () => {
        if (stopped) return
        if (scroller.scrollTop !== top) scroller.scrollTop = top
        if (scroller.scrollLeft !== left) scroller.scrollLeft = left
      }
      const onScroll = () => restore()
      const stop = () => {
        if (stopped) return
        stopped = true
        scroller.removeEventListener('scroll', onScroll, true)
        window.removeEventListener('pointercancel', stop, true)
        window.removeEventListener('blur', stop)
        clearTimeout(timer)
      }
      scroller.addEventListener('scroll', onScroll, true)
      // mouseup is not the release valve: a missed one (release outside the
      // window, native menu, blur) used to leave this listener on forever.
      // Freeze only until focus-induced scroll settles (~2 frames), so a
      // click-drag can still autoscroll. pointercancel / blur / timeout
      // cover the case where rAF never runs.
      window.addEventListener('pointercancel', stop, true)
      window.addEventListener('blur', stop)
      const timer = window.setTimeout(stop, HOLD_SCROLL_MS)
      queueMicrotask(restore)
      requestAnimationFrame(() => {
        restore()
        requestAnimationFrame(() => {
          restore()
          stop()
        })
      })
    },
  })
}

/** Thumb jumps skip measuring; force a measure once the scrollbar settles. */
function measureAfterThumbScroll(): Extension {
  return ViewPlugin.fromClass(class {
    lastWheel = 0
    constructor(readonly view: EditorView) {
      this.onWheel = this.onWheel.bind(this)
      this.onScrollEnd = this.onScrollEnd.bind(this)
      view.scrollDOM.addEventListener('wheel', this.onWheel, { passive: true })
      view.scrollDOM.addEventListener('scrollend', this.onScrollEnd)
    }
    onWheel() {
      this.lastWheel = Date.now()
    }
    onScrollEnd() {
      if (Date.now() - this.lastWheel < WHEEL_MEASURE_MS) return
      this.view.requestMeasure()
    }
    destroy() {
      this.view.scrollDOM.removeEventListener('wheel', this.onWheel)
      this.view.scrollDOM.removeEventListener('scrollend', this.onScrollEnd)
    }
  })
}

/** Replace CM's default mouse selection so clicks follow the painted line. */
export function pointerMouseSelection(): Extension {
  return [
    holdScrollOnPointerFocus(),
    measureAfterThumbScroll(),
    EditorView.mouseSelectionStyle.of((view, event): MouseSelectionStyle | null => {
      if (event.button !== 0 || event.altKey) return null
      const type = event.detail || 1
      const startEvent = event
      let start = posAtPointer(view, event)
      let startSel = view.state.selection
      return {
        update(update) {
          if (!update.docChanged) return false
          start = { pos: update.changes.mapPos(start.pos), assoc: start.assoc }
          startSel = startSel.map(update.changes)
          return true
        },
        get(cur, extend, multiple) {
          const curPos = pointerMoved(startEvent, cur) ? posAtPointer(view, cur) : start
          let range = selectionForClick(view.state, curPos.pos, curPos.assoc, type)
          if (start.pos !== curPos.pos && !extend) {
            const startRange = selectionForClick(view.state, start.pos, start.assoc, type)
            const from = Math.min(startRange.from, range.from)
            const to = Math.max(startRange.to, range.to)
            range = from < range.from
              ? EditorSelection.range(from, to, range.assoc)
              : EditorSelection.range(to, from, range.assoc)
          }
          if (extend) {
            return startSel.replaceRange(startSel.main.extend(range.from, range.to, range.assoc))
          }
          if (multiple) return startSel.addRange(range)
          return EditorSelection.create([range])
        },
      }
    }),
  ]
}
