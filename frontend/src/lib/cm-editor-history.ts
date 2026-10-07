/**
 * Per-file CodeMirror undo/redo that survives the live editor.
 *
 * CodeEditor's in-component tab cache holds full EditorStates. Workspace
 * switches park that cache (cm-session-tabs) so returning to a root does not
 * re-parse; the cache still dies on a miss, tab-close-to-empty unmount, and
 * MergeEditor's keyed remounts. This module is the second tier — `historyField`
 * (+ selection) serialized under a scope\0path key and rebuilt into a fresh
 * state with live extensions, the way VS Code / Sublime keep one undo stack
 * per file.
 *
 * Retention is a byte budget (not a file count), matching VS Code's closed-file
 * undo heap. The newest snapshot is always kept even if it alone exceeds the
 * budget; older files are dropped until we fit.
 */

import { history, historyField, redoDepth, undoDepth } from '@codemirror/commands'
import { EditorState, type Extension } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'

/** Same 20MB ceiling VS Code uses for closed-file undo stacks. */
export const EDITOR_HISTORY_MAX_BYTES = 20 * 1024 * 1024
/**
 * Skip toJSON when the doc alone would dominate the budget. A quarter of
 * 20 MiB is 5 MiB of source — JSON escaping can roughly double that and
 * still leave room for other files. EDITOR_BODY_CAP (1 MiB) is a RAM cap
 * for parked EditorStates, not an undo-retention cap.
 */
export const EDITOR_HISTORY_DOC_CAP = EDITOR_HISTORY_MAX_BYTES / 4
/** Undo events kept per file. CodeMirror default is 100; 250 is a deeper stack. */
export const EDITOR_HISTORY_MIN_DEPTH = 250

const utf8 = new TextEncoder()

export function editorHistoryExtension(): Extension {
  return history({ minDepth: EDITOR_HISTORY_MIN_DEPTH })
}

export type EditorHistoryJSON = {
  doc: string
  selection?: unknown
  history?: unknown
}

type Snap = {
  json: EditorHistoryJSON
  scrollTop: number
  scrollLeft: number
  bytes: number
  /** undoDepth + redoDepth at capture time — used to refuse shallower overwrites. */
  depth: number
}

const cache = new Map<string, Snap>()
let totalBytes = 0
let maxBytes = EDITOR_HISTORY_MAX_BYTES

export function editorHistoryKey(scope: string, path: string): string {
  return `${scope}\0${path}`
}

function payloadBytes(json: EditorHistoryJSON): number {
  return utf8.encode(JSON.stringify(json)).byteLength
}

function drop(key: string): Snap | undefined {
  const snap = cache.get(key)
  if (!snap) return undefined
  cache.delete(key)
  totalBytes -= snap.bytes
  if (totalBytes < 0) totalBytes = 0
  return snap
}

function evict(): void {
  while (totalBytes > maxBytes && cache.size > 1) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    drop(oldest)
  }
}

function touch(key: string, snap: Snap): void {
  drop(key)
  cache.set(key, snap)
  totalBytes += snap.bytes
  evict()
}

/** Snapshot a state's history + selection for `key` (no live view needed —
 *  CodeEditor serializes its stashed background tabs through this). */
export function captureEditorHistoryFromState(
  key: string,
  state: EditorState,
  scrollTop = 0,
  scrollLeft = 0,
): void {
  if (!key) return
  try {
    const depth = undoDepth(state) + redoDepth(state)
    // Unedited / read-only glances store the whole doc and would evict real
    // undo stacks (a 15MB log is most of the 20MB budget). Nothing to restore.
    if (depth === 0) return
    // Edited docs past DOC_CAP: toJSON + JSON.stringify copies the full
    // buffer on the caller’s thread, and one 12–16 MiB snapshot can evict
    // every other file. Skip before serializing. Dirty buffers under
    // EDITOR_BODY_CAP stay in the in-workspace tab map; 1–5 MiB dirty
    // buffers still land here on workspace switch / unmount / Diff remount
    // (the tab map does not survive those). Above DOC_CAP, undo is live-only.
    if (state.doc.length > EDITOR_HISTORY_DOC_CAP) return
    const json = state.toJSON({ history: historyField }) as EditorHistoryJSON
    if (typeof json.doc !== 'string') return
    const existing = cache.get(key)
    // Same buffer, weaker stack: the other split pane synced edits with
    // addToHistory(false) and must not clobber the pane the user typed in.
    if (existing && existing.json.doc === json.doc && existing.depth > depth) return
    touch(key, {
      json,
      scrollTop,
      scrollLeft,
      bytes: payloadBytes(json),
      depth,
    })
  } catch {
    // historyField absent (tests / odd extension sets) — skip.
  }
}

/** Snapshot the current view's history, selection, and scroll for `key`. */
export function captureEditorHistory(key: string, view: EditorView): void {
  captureEditorHistoryFromState(key, view.state, view.scrollDOM.scrollTop, view.scrollDOM.scrollLeft)
}

export type RestoredEditor = {
  state: EditorState
  scrollTop: number
  scrollLeft: number
}

/**
 * Rebuild an EditorState from a cached snapshot. Returns null when there is
 * no entry, or when the document no longer matches (disk / discard) — stale
 * history must not replay onto a different buffer.
 */
export function restoreEditorState(
  key: string,
  doc: string,
  extensions: Extension[],
): RestoredEditor | null {
  const snap = cache.get(key)
  if (!snap) return null
  if (snap.json.doc !== doc) {
    drop(key)
    return null
  }
  touch(key, snap)
  try {
    const state = EditorState.fromJSON(snap.json, { extensions }, { history: historyField })
    return { state, scrollTop: snap.scrollTop, scrollLeft: snap.scrollLeft }
  } catch {
    drop(key)
    return null
  }
}

export function forgetEditorHistory(key: string): void {
  drop(key)
}

/** Drop a file and any children (tab close, delete, directory drop). */
export function forgetEditorHistoryTree(scope: string, path: string): void {
  if (!path) return
  const prefix = editorHistoryKey(scope, path)
  for (const k of [...cache.keys()]) {
    if (k === prefix || k.startsWith(prefix + '/')) drop(k)
  }
}

/** Follow a rename / move so undo survives the path change. */
export function remapEditorHistory(scope: string, from: string, to: string): void {
  if (!from || !to || from === to) return
  const fromKey = editorHistoryKey(scope, from)
  const moves: [string, string][] = []
  for (const k of cache.keys()) {
    if (k === fromKey) {
      moves.push([k, editorHistoryKey(scope, to)])
    } else if (k.startsWith(fromKey + '/')) {
      moves.push([k, editorHistoryKey(scope, to + k.slice(fromKey.length))])
    }
  }
  for (const [oldKey, newKey] of moves) {
    const snap = drop(oldKey)
    if (!snap) continue
    touch(newKey, snap)
  }
}

/** Test helper — not used in production. */
export function resetEditorHistoryForTests(): void {
  cache.clear()
  totalBytes = 0
  maxBytes = EDITOR_HISTORY_MAX_BYTES
}

/** Test helper — shrink the byte budget so eviction is cheap to assert. */
export function setEditorHistoryMaxBytesForTests(n: number): void {
  maxBytes = n
  evict()
}

/** Test helper. */
export function editorHistoryCacheStatsForTests(): { files: number; bytes: number } {
  return { files: cache.size, bytes: totalBytes }
}
