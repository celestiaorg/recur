/**
 * Per-file editor view state: caret, scroll, and Code/Preview/Diff.
 *
 * Open tabs keep their EditorState + scroll snapshot inside CodeEditor, so
 * switching between them never needs these coordinates. This module covers
 * cold restores — session reload, workspace switch — where only line/col
 * numbers survived. Restores go through EditorState selection +
 * EditorView.scrollIntoView, never by assigning scrollDOM.scrollTop, which
 * desyncs the height map and makes clicks land on the wrong line.
 */

import { EditorSelection, Text, Transaction, type StateEffect } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { scrollerNearBottom } from './caches/chat-transcript-ui'
import { PREVIEW_IFRAME_RESTORE } from './preview'

export type EditorViewMode = 'code' | 'preview' | 'diff'

export type FileViewState = {
  viewMode: EditorViewMode
  /** 1-based. */
  line: number
  /** 1-based. */
  col: number
  /** Present when the selection is non-empty. */
  anchorLine?: number
  anchorCol?: number
  /** 1-based line at the top of the viewport (preferred over pixel scroll). */
  topLine?: number
  scrollTop: number
  scrollLeft: number
  /** FilePreview scroller — independent of CodeMirror scrollTop. */
  previewScrollTop?: number
  previewScrollLeft?: number
  /** Mermaid preview camera scale (1 = 100%). */
  previewZoom?: number
}

const MAX_STORED_VIEWS = 64
/** Clean buffer bodies larger than this are not cached in RAM / snapshots. */
export const EDITOR_BODY_CAP = 1024 * 1024

export function posAtLineCol(doc: Text, line: number, col: number): number {
  const ln = doc.line(Math.max(1, Math.min(Math.floor(line) || 1, doc.lines)))
  const offset = Math.max(0, (Math.floor(col) || 1) - 1)
  return ln.from + Math.min(offset, ln.length)
}

export function selectionFromViewState(doc: Text, vs: FileViewState | null | undefined): EditorSelection {
  if (!vs) return EditorSelection.single(0)
  const head = posAtLineCol(doc, vs.line, vs.col)
  const anchor = vs.anchorLine != null
    ? posAtLineCol(doc, vs.anchorLine, vs.anchorCol ?? 1)
    : head
  return EditorSelection.single(anchor, head)
}

export function scrollEffectFromViewState(doc: Text, vs: FileViewState | null | undefined) {
  if (!vs) return undefined
  const top = posAtLineCol(doc, vs.topLine ?? vs.line, 1)
  return EditorView.scrollIntoView(top, { y: 'start', yMargin: 0 })
}

export function captureFileViewState(view: EditorView, viewMode: EditorViewMode): FileViewState | null {
  // display:none (terminal-first layout) zeroes geometry — skip so a previously
  // saved caret/scroll is not overwritten with line 1 / scrollTop 0.
  if (view.scrollDOM.clientHeight === 0) return null
  try {
    const sel = view.state.selection.main
    const headLine = view.state.doc.lineAt(sel.head)
    const box = view.scrollDOM.getBoundingClientRect()
    const topPos = view.posAtCoords({ x: box.left + 24, y: box.top + 8 }, false)
    const topLine = topPos == null ? 1 : view.state.doc.lineAt(topPos).number
    const state: FileViewState = {
      viewMode,
      line: headLine.number,
      col: sel.head - headLine.from + 1,
      topLine,
      scrollTop: view.scrollDOM.scrollTop,
      scrollLeft: view.scrollDOM.scrollLeft,
    }
    if (!sel.empty) {
      const anchorLine = view.state.doc.lineAt(sel.anchor)
      state.anchorLine = anchorLine.number
      state.anchorCol = sel.anchor - anchorLine.from + 1
    }
    return state
  } catch {
    // Height map / DOM tiles can disagree mid setState (CM: "No tile at position").
    return null
  }
}

/** Scroll snapshot, or undefined when the tile map is mid-rebuild. */
export function safeScrollSnapshot(view: EditorView): StateEffect<unknown> | undefined {
  try {
    return view.scrollSnapshot()
  } catch {
    return undefined
  }
}

/**
 * Focus without scrolling the caret into view. Safari 26 ignores
 * preventScroll, so a tab switch would yank a scrolled-but-unclicked file
 * back to the caret (usually line 1).
 *
 * Pass `scroll` when a restore is in flight: CodeMirror applies a dispatched
 * scroll target on the next measure frame, so capturing the *current* DOM
 * scroll here would snapshot the pre-restore position and, being the last
 * dispatch, win over the restore.
 *
 * `opts.focus === false` still re-dispatches `scroll` on this frame and the
 * next (the height map is still estimating after setState). Keep-alive
 * workspace restore uses that so the parked offset is not lost when we
 * delay the actual focus.
 */
export function focusPreservingScroll(
  view: EditorView,
  scroll?: StateEffect<unknown>,
  still?: () => boolean,
  opts?: { focus?: boolean },
): void {
  const snap = scroll ?? safeScrollSnapshot(view)
  const doc = view.state.doc
  if (opts?.focus !== false) {
    try {
      view.focus()
    } catch {
      return
    }
  }
  if (snap) {
    try {
      view.dispatch({ effects: snap })
    } catch { /* tile map mid-rebuild */ }
  }
  requestAnimationFrame(() => {
    if (!view.dom.isConnected) return
    if (still ? !still() : view.state.doc !== doc) return
    if (!snap) return
    try {
      view.dispatch({ effects: snap })
    } catch { /* ignore */ }
  })
}

/** Full-doc replace. Apply scroll in a follow-up dispatch so CodeMirror
 *  clips it against the new doc — mapping through a full delete sends
 *  mid-file anchors to 0, and clipping in the same transaction uses the
 *  old length (`Invalid position N in document of length M`). */
export function replaceDocKeepingScroll(
  view: EditorView,
  nextDoc: string,
  scroll?: StateEffect<unknown>,
): void {
  const spec = {
    changes: { from: 0, to: view.state.doc.length, insert: nextDoc },
    selection: { anchor: Math.min(view.state.selection.main.head, nextDoc.length) },
    annotations: Transaction.addToHistory.of(false),
  }
  try {
    view.dispatch(spec)
  } catch {
    const cur = view.state
    view.setState(
      cur.doc.toString() === nextDoc
        ? cur
        : cur.update({
            changes: { from: 0, to: cur.doc.length, insert: nextDoc },
            selection: { anchor: Math.min(cur.selection.main.head, nextDoc.length) },
            annotations: Transaction.addToHistory.of(false),
          }).state,
    )
  }
  if (!scroll) return
  try {
    view.dispatch({ effects: scroll })
  } catch { /* effect sits past this doc */ }
}

function restoreScrollLeft(view: EditorView, scrollLeft: number): void {
  if (scrollLeft <= 0) return
  view.scrollDOM.scrollLeft = scrollLeft
  requestAnimationFrame(() => {
    if (!view.dom.isConnected) return
    view.scrollDOM.scrollLeft = scrollLeft
  })
}

export function applyFileViewState(view: EditorView, vs: FileViewState): void {
  const sel = selectionFromViewState(view.state.doc, vs)
  const scroll = scrollEffectFromViewState(view.state.doc, vs)
  try {
    view.dispatch({
      selection: sel,
      effects: scroll ? [scroll] : undefined,
      scrollIntoView: false,
      annotations: Transaction.addToHistory.of(false),
    })
  } catch {
    return
  }
  view.requestMeasure()
  restoreScrollLeft(view, vs.scrollLeft)
}

/**
 * Apply now (so the first paint is close) and again after layout, when the
 * scroller's scrollHeight is real. Setting scrollTop before measure gets
 * clamped to 0; clicking then maps through a stale height map.
 */
export function scheduleApplyFileViewState(
  view: EditorView,
  vs: FileViewState,
  still: () => boolean,
): void {
  applyFileViewState(view, vs)
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      if (!still()) return
      applyFileViewState(view, vs)
    })
  })
}

export function parseFileViewState(raw: unknown): FileViewState | null {
  if (!raw || typeof raw !== 'object') return null
  const o = raw as Record<string, unknown>
  const viewMode: EditorViewMode =
    o.viewMode === 'preview' || o.viewMode === 'diff' ? o.viewMode : 'code'
  const line = Number(o.line)
  const col = Number(o.col)
  const scrollTop = o.scrollTop == null ? 0 : Number(o.scrollTop)
  if (!Number.isFinite(line) || line < 1) return null
  if (!Number.isFinite(col) || col < 1) return null
  if (!Number.isFinite(scrollTop) || scrollTop < 0) return null
  const scrollLeft = Number(o.scrollLeft)
  const state: FileViewState = {
    viewMode,
    line: Math.min(Math.floor(line), 1_000_000_000),
    col: Math.min(Math.floor(col), 1_000_000),
    scrollTop: Math.min(scrollTop, 1_000_000_000),
    scrollLeft: Number.isFinite(scrollLeft) && scrollLeft > 0
      ? Math.min(scrollLeft, 1_000_000_000)
      : 0,
  }
  const topLine = Number(o.topLine)
  if (Number.isFinite(topLine) && topLine >= 1) {
    state.topLine = Math.min(Math.floor(topLine), 1_000_000_000)
  }
  const anchorLine = Number(o.anchorLine)
  const anchorCol = Number(o.anchorCol)
  if (Number.isFinite(anchorLine) && anchorLine >= 1 && Number.isFinite(anchorCol) && anchorCol >= 1) {
    state.anchorLine = Math.floor(anchorLine)
    state.anchorCol = Math.floor(anchorCol)
  }
  const previewScrollTop = Number(o.previewScrollTop)
  if (Number.isFinite(previewScrollTop) && previewScrollTop !== 0) {
    state.previewScrollTop = Math.max(-1_000_000_000, Math.min(Math.floor(previewScrollTop), 1_000_000_000))
  }
  const previewScrollLeft = Number(o.previewScrollLeft)
  if (Number.isFinite(previewScrollLeft) && previewScrollLeft !== 0) {
    state.previewScrollLeft = Math.max(-1_000_000_000, Math.min(Math.floor(previewScrollLeft), 1_000_000_000))
  }
  const previewZoom = Number(o.previewZoom)
  if (Number.isFinite(previewZoom) && previewZoom > 0) {
    state.previewZoom = Math.min(previewZoom, 8)
  }
  return state
}

export function parseFileViews(raw: unknown): Record<string, FileViewState> {
  if (!raw || typeof raw !== 'object') return {}
  const out: Record<string, FileViewState> = {}
  for (const [path, v] of Object.entries(raw as Record<string, unknown>)) {
    if (!path) continue
    const parsed = parseFileViewState(v)
    if (parsed) out[path] = parsed
  }
  return out
}

const HOT_TAB_STRIP = '.obs-tb-center .tab-strip-scroll'
const HOT_CHAT_TAB_STRIP = '.wsv-chat-tabs .tab-strip-scroll'
const HOT_LIVE_SCROLLER = '.obs-editor-pane:not(.split-side) .obs-surface:not(.off) .cm-scroller'
const HOT_PREVIEW_SCROLLER = '.obs-editor-pane:not(.split-side) .obs-surface .file-preview'
const HOT_CHAT_SCROLL = '.chat-scroll'

type HotShellGeometry = {
  tabScroll: number
  chatTabScroll: number
  editorScrollTop: number
  previewScrollTop: number
  previewScrollLeft: number
  chatScrollTop: number
  chatPinned: boolean
}

const hotShellGeometry = new WeakMap<HTMLElement, HotShellGeometry>()

function previewOffsets(el: HTMLElement): { top: number; left: number } {
  const dataTop = el.dataset.previewTop
  const dataLeft = el.dataset.previewLeft
  const top = dataTop != null && dataTop !== '' ? Number(dataTop) : el.scrollTop
  const left = dataLeft != null && dataLeft !== '' ? Number(dataLeft) : el.scrollLeft
  return {
    top: Number.isFinite(top) ? top : 0,
    left: Number.isFinite(left) ? left : 0,
  }
}

function restorePreviewNode(el: HTMLElement, top: number, left: number): void {
  el.scrollTop = top
  el.scrollLeft = left
  el.dataset.previewTop = String(top)
  el.dataset.previewLeft = String(left)
  const iframe = el.querySelector(':scope > iframe')
  if (iframe instanceof HTMLIFrameElement) {
    iframe.contentWindow?.postMessage({ type: PREVIEW_IFRAME_RESTORE, top, left }, '*')
  }
}

/**
 * Capture overflow positions before a hot shell is moved into `#wsv-park`.
 * File-tab strip, agent-tab strip, editor, file preview, and agent-chat scroll
 * all reset on the node move. A chat stuck to the tail stores a pin, not a
 * pixel — restoring the old `scrollTop` after the transcript grew would land
 * mid-thread.
 */
export function stashHotShellGeometry(node: HTMLElement): void {
  const strip = node.querySelector<HTMLElement>(HOT_TAB_STRIP)
  const chatStrip = node.querySelector<HTMLElement>(HOT_CHAT_TAB_STRIP)
  const scroller = node.querySelector<HTMLElement>(HOT_LIVE_SCROLLER)
  const preview = node.querySelector<HTMLElement>(HOT_PREVIEW_SCROLLER)
  const chat = node.querySelector<HTMLElement>(HOT_CHAT_SCROLL)
  const prev = hotShellGeometry.get(node)
  const liveStrip = strip && strip.clientWidth >= 1
  const liveChatStrip = chatStrip && chatStrip.clientWidth >= 1
  const liveEditor = scroller && scroller.clientHeight >= 1
  const livePreview = preview && preview.clientHeight >= 1
  const liveChat = chat && chat.clientHeight >= 1
  const previewPos = livePreview ? previewOffsets(preview) : null
  hotShellGeometry.set(node, {
    tabScroll: liveStrip ? strip.scrollLeft : prev?.tabScroll ?? 0,
    chatTabScroll: liveChatStrip ? chatStrip.scrollLeft : prev?.chatTabScroll ?? 0,
    editorScrollTop: liveEditor ? scroller.scrollTop : prev?.editorScrollTop ?? 0,
    previewScrollTop: previewPos ? previewPos.top : prev?.previewScrollTop ?? 0,
    previewScrollLeft: previewPos ? previewPos.left : prev?.previewScrollLeft ?? 0,
    chatScrollTop: liveChat ? chat.scrollTop : prev?.chatScrollTop ?? 0,
    chatPinned: liveChat ? scrollerNearBottom(chat) : prev?.chatPinned ?? false,
  })
}

/**
 * `appendChild` / reparenting zeros overflow. Stash, place, restore: used
 * when the agent dock moves into `#prd-agent` (and back).
 */
export function relocatePreservingOverflow(node: HTMLElement, place: (n: HTMLElement) => void): void {
  stashHotShellGeometry(node)
  place(node)
  void node.offsetWidth
  restoreHotShellGeometry(node, { defer: false })
}

/** Assign overflow without an overlay-scrollbar flash (macOS shows the bar on any visible scrollLeft write). */
function assignOverflow(el: HTMLElement, prop: 'scrollLeft' | 'scrollTop', value: number): void {
  if (el[prop] === value) return
  const prev = el.style.overflow
  el.style.overflow = 'hidden'
  el[prop] = value
  el.style.overflow = prev
}

export function restoreHotShellGeometry(node: HTMLElement, opts?: { skipEditor?: boolean; defer?: boolean }): void {
  const snap = hotShellGeometry.get(node)
  if (!snap) return
  const apply = () => {
    const strip = node.querySelector<HTMLElement>(HOT_TAB_STRIP)
    const chatStrip = node.querySelector<HTMLElement>(HOT_CHAT_TAB_STRIP)
    const scroller = node.querySelector<HTMLElement>(HOT_LIVE_SCROLLER)
    const preview = node.querySelector<HTMLElement>(HOT_PREVIEW_SCROLLER)
    const chat = node.querySelector<HTMLElement>(HOT_CHAT_SCROLL)
    if (strip) strip.scrollLeft = snap.tabScroll
    if (chatStrip) assignOverflow(chatStrip, 'scrollLeft', snap.chatTabScroll)
    if (scroller && !opts?.skipEditor) scroller.scrollTop = snap.editorScrollTop
    if (preview) restorePreviewNode(preview, snap.previewScrollTop, snap.previewScrollLeft)
    if (chat) {
      chat.scrollTop = snap.chatPinned ? chat.scrollHeight : snap.chatScrollTop
    }
  }
  apply()
  if (opts?.defer === false) return
  requestAnimationFrame(() => {
    apply()
    requestAnimationFrame(apply)
  })
}

export function peekHotShellEditorScrollTop(node: HTMLElement): number | undefined {
  const snap = hotShellGeometry.get(node)
  if (!snap || snap.editorScrollTop <= 0) return undefined
  return snap.editorScrollTop
}

export function pruneFileViews(
  views: Record<string, FileViewState>,
  keepPaths: Iterable<string>,
  max = MAX_STORED_VIEWS,
): Record<string, FileViewState> {
  const keep = new Set(keepPaths)
  const out: Record<string, FileViewState> = {}
  for (const path of keep) {
    const vs = views[path]
    if (vs) out[path] = vs
    if (Object.keys(out).length >= max) return out
  }
  return out
}

export function resolveRestoredViewMode(
  saved: EditorViewMode | undefined,
  opts: {
    wantDiff?: boolean
    deleted?: boolean
    canDiff?: boolean
    canPreview?: boolean
    /** Binary raster — Preview is the on-disk image; Diff is the visual compare. */
    isImage?: boolean
  },
): EditorViewMode {
  if (opts.isImage) {
    if (opts.deleted) return 'diff'
    if (opts.wantDiff) return opts.canDiff ? 'diff' : 'preview'
    if (saved === 'diff') return opts.canDiff ? 'diff' : 'preview'
    return 'preview'
  }
  if (opts.deleted) return 'diff'
  if (opts.wantDiff) return opts.canDiff ? 'diff' : 'code'
  if (saved === 'diff') return opts.canDiff ? 'diff' : 'code'
  if (saved === 'preview') return opts.canPreview ? 'preview' : 'code'
  return 'code'
}
