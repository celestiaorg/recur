/**
 * Hover / caret Revert for unified-merge hunks (D52).
 *
 * CM's mergeControls sit inside the deleted-chunk widget (top of the hunk)
 * and always emit accept+reject. We paint one button at the hunk's
 * bottom-right, just outside, while the pointer or caret is in that hunk.
 * Click / ⌥⌘Z runs rejectChunk on that same hunk — buffer edit, unsaved.
 */
import { getChunks, rejectChunk, type Chunk } from '@codemirror/merge'
import { type EditorState, type Extension } from '@codemirror/state'
import { EditorView, ViewPlugin, type ViewUpdate } from '@codemirror/view'
import { formatKeyEquivalentDisplay } from './keyequiv'

const HIDE_MS = 140
const OUTSIDE_GAP = 4
const RIGHT_INSET = 8
const BUTTON_FALLBACK_W = 88

/** Canonical chord for `revertHunkCommand` (`Mod-Alt-z`). */
export const REVERT_HUNK_CHORD = 'opt+cmd+z'
export const REVERT_HUNK_KEY_DISPLAY = formatKeyEquivalentDisplay(REVERT_HUNK_CHORD)

export function chunkAtPos(state: EditorState, pos: number): Chunk | null {
  const info = getChunks(state)
  if (!info) return null
  return info.chunks.find((ch) => ch.fromB <= pos && pos <= ch.endB) ?? null
}

export function caretChunk(state: EditorState): Chunk | null {
  return chunkAtPos(state, state.selection.main.head)
}

/**
 * Chip target: last hover if the pointer is still aimed at a hunk, else caret.
 * Keyboard selection clears hover first so a parked pointer cannot pin the chip.
 */
export function revertAimChunk(state: EditorState, hoverFromB: number): Chunk | null {
  if (hoverFromB >= 0) {
    const hover = chunkAtPos(state, hoverFromB)
    if (hover) return hover
  }
  return caretChunk(state)
}

/** Revert the hunk the chip is advertising (keyboard path). */
export function revertHunkCommand(view: EditorView): boolean {
  if (view.state.readOnly) return false
  const p = view.plugin(mergeRevertHoverPlugin)
  return rejectChunk(view, p && p.fromB >= 0 ? p.fromB : undefined)
}

export function mergeRevertButton(onClick: (e: MouseEvent) => void): HTMLButtonElement {
  const btn = document.createElement('button')
  btn.type = 'button'
  btn.name = 'reject'
  btn.className = 'cm-revertHunk'
  const label = document.createElement('span')
  label.className = 'cm-revertHunk-label'
  label.textContent = 'Revert'
  const key = document.createElement('kbd')
  key.className = 'cm-revertHunk-key'
  key.textContent = REVERT_HUNK_KEY_DISPLAY
  btn.append(label, key)
  btn.title = `Revert this hunk to the workspace base (${REVERT_HUNK_KEY_DISPLAY})`
  btn.setAttribute('aria-label', `Revert this hunk to the workspace base (${REVERT_HUNK_KEY_DISPLAY})`)
  btn.hidden = true
  // preventDefault so the press does not place a caret under the chip;
  // activate on click so a drag-off cancels.
  btn.onmousedown = (e) => e.preventDefault()
  btn.onclick = onClick
  return btn
}

/**
 * Content-x of the chip: the nearer of the content's right edge and the
 * visible scroller edge (unwrapped long lines make content much wider).
 */
export function hunkButtonLeft(
  contentRight: number,
  scrollLeft: number,
  clientWidth: number,
  width: number,
  inset = RIGHT_INSET,
): number {
  return Math.min(contentRight, scrollLeft + clientWidth) - width - inset
}

function deletedChunkEl(view: EditorView, chunk: Chunk): HTMLElement | null {
  for (const el of view.contentDOM.querySelectorAll('.cm-deletedChunk')) {
    try {
      if (view.posAtDOM(el) === chunk.fromB) return el as HTMLElement
    } catch { /* widget unmounted mid-query */ }
  }
  return null
}

/** Content-relative box of the hunk (deleted widget + B-side lines). */
export function hunkContentBox(view: EditorView, chunk: Chunk): { top: number; bottom: number } | null {
  let top = Infinity
  let bottom = -Infinity
  if (chunk.fromB < chunk.toB && view.state.doc.length) {
    const from = Math.min(chunk.fromB, view.state.doc.length)
    const last = Math.min(Math.max(chunk.toB - 1, from), view.state.doc.length)
    top = Math.min(top, view.lineBlockAt(from).top)
    bottom = Math.max(bottom, view.lineBlockAt(last).bottom)
  }
  const del = deletedChunkEl(view, chunk)
  if (del) {
    const scroll = view.scrollDOM
    const sr = scroll.getBoundingClientRect()
    const r = del.getBoundingClientRect()
    top = Math.min(top, r.top - sr.top + scroll.scrollTop)
    bottom = Math.max(bottom, r.bottom - sr.top + scroll.scrollTop)
  }
  if (!Number.isFinite(bottom) || bottom < top) return null
  return { top, bottom }
}

/** Document pos under the pointer, using painted DOM first so a stale height map cannot throw. */
function eventPos(view: EditorView, e: PointerEvent): number | null {
  const t = e.target
  if (t instanceof Node) {
    const el = t instanceof Element ? t : t.parentElement
    const hit = el?.closest('.cm-deletedChunk, .cm-line')
    if (hit && view.contentDOM.contains(hit)) {
      try {
        return view.posAtDOM(hit)
      } catch { /* widget unmounted mid-query */ }
    }
  }
  try {
    return view.posAtCoords({ x: e.clientX, y: e.clientY })
  } catch {
    // Height map / DOM tiles can disagree mid-draw (CM: "No tile at position").
    return null
  }
}

function eventChunk(view: EditorView, e: PointerEvent, heldFromB: number): Chunk | null {
  const t = e.target
  if (t instanceof Node) {
    const btn = view.scrollDOM.querySelector('.cm-revertHunk')
    if (btn?.contains(t)) return chunkAtPos(view.state, heldFromB)
  }
  const pos = eventPos(view, e)
  if (pos == null) return null
  return chunkAtPos(view.state, pos)
}

export const mergeRevertHoverPlugin = ViewPlugin.fromClass(class {
  button: HTMLButtonElement
  hideTimer = 0
  fromB = -1
  hoverFromB = -1
  lastPointerX = Number.NaN
  lastPointerY = Number.NaN
  btnWidth = 0
  readonly onMove: (e: PointerEvent) => void
  readonly onLeave: () => void

  constructor(readonly view: EditorView) {
    this.button = mergeRevertButton((e) => {
      e.preventDefault()
      if (this.view.state.readOnly) return
      const at = this.fromB >= 0 ? this.fromB : undefined
      rejectChunk(this.view, at)
      this.hoverFromB = -1
      this.forceHide()
    })
    view.scrollDOM.appendChild(this.button)
    this.onMove = (e) => this.pointer(e)
    this.onLeave = () => {
      this.hoverFromB = -1
      this.scheduleHide()
    }
    view.scrollDOM.addEventListener('pointermove', this.onMove)
    view.scrollDOM.addEventListener('pointerleave', this.onLeave)
    this.button.addEventListener('pointerenter', () => this.clearHide())
    this.button.addEventListener('pointerleave', this.onLeave)
    if (!view.state.readOnly) this.sync(true)
  }

  update(u: ViewUpdate) {
    if (u.state.readOnly) {
      this.forceHide()
      return
    }
    if (u.docChanged) {
      if (this.fromB >= 0) this.fromB = u.changes.mapPos(this.fromB)
      if (this.hoverFromB >= 0) this.hoverFromB = u.changes.mapPos(this.hoverFromB)
    }
    // Caret / keyboard nav beats a pointer still sitting on another hunk.
    if (u.selectionSet) this.hoverFromB = -1
    if (u.docChanged || u.selectionSet || u.viewportChanged || u.geometryChanged) {
      this.sync(u.viewportChanged || u.geometryChanged || u.docChanged)
    }
  }

  targetChunk(): Chunk | null {
    return revertAimChunk(this.view.state, this.hoverFromB)
  }

  sync(forcePlace: boolean) {
    const ch = this.targetChunk()
    if (!ch) {
      this.forceHide()
      return
    }
    const same = ch.fromB === this.fromB && !this.button.hidden
    this.fromB = ch.fromB
    if (!same || forcePlace) this.place(ch)
  }

  pointer(e: PointerEvent) {
    if (this.view.state.readOnly) return
    // Ignore scroll/layout moves: the cursor did not actually travel.
    if (e.clientX === this.lastPointerX && e.clientY === this.lastPointerY) return
    this.lastPointerX = e.clientX
    this.lastPointerY = e.clientY
    const ch = eventChunk(this.view, e, this.fromB)
    if (!ch) {
      this.hoverFromB = -1
      this.scheduleHide()
      return
    }
    this.clearHide()
    this.hoverFromB = ch.fromB
    if (ch.fromB === this.fromB && !this.button.hidden) return
    this.fromB = ch.fromB
    this.place(ch)
  }

  place(chunk: Chunk) {
    const box = hunkContentBox(this.view, chunk)
    if (!box) {
      this.forceHide()
      return
    }
    this.button.hidden = false
    if (!this.btnWidth) this.btnWidth = this.button.offsetWidth || BUTTON_FALLBACK_W
    const scroll = this.view.scrollDOM
    const sr = scroll.getBoundingClientRect()
    const cr = this.view.contentDOM.getBoundingClientRect()
    const contentRight = cr.right - sr.left + scroll.scrollLeft
    const left = hunkButtonLeft(contentRight, scroll.scrollLeft, scroll.clientWidth, this.btnWidth)
    this.button.style.top = `${Math.round(box.bottom + OUTSIDE_GAP)}px`
    this.button.style.left = `${Math.round(left)}px`
  }

  scheduleHide() {
    this.clearHide()
    this.hideTimer = window.setTimeout(() => this.sync(false), HIDE_MS)
  }

  clearHide() {
    window.clearTimeout(this.hideTimer)
    this.hideTimer = 0
  }

  forceHide() {
    this.clearHide()
    this.button.hidden = true
    this.fromB = -1
  }

  destroy() {
    this.clearHide()
    this.view.scrollDOM.removeEventListener('pointermove', this.onMove)
    this.view.scrollDOM.removeEventListener('pointerleave', this.onLeave)
    this.button.remove()
  }
})

export function mergeRevertHover(): Extension {
  return [
    mergeRevertHoverPlugin,
    EditorView.theme({
      '.cm-revertHunk:not([hidden])': {
        position: 'absolute',
        zIndex: '6',
        display: 'flex',
        alignItems: 'center',
        boxSizing: 'border-box',
        margin: '0',
        padding: '1px 8px',
        cursor: 'pointer',
        userSelect: 'none',
      },
      '.cm-revertHunk-key': {
        marginLeft: '6px',
        opacity: '0.62',
        fontFamily: 'inherit',
        fontSize: '10px',
        fontWeight: '500',
        letterSpacing: '0.02em',
      },
    }),
  ]
}
