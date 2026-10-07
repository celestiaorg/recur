/**
 * Keep xterm Terminal instances across pane unmount.
 *
 * Workspace evict (and hiding the dock) used to dispose the widget and
 * DetachTerminal; the next mount paid Attach + replay + new Terminal().
 * Warm miss with 4–6 panes was ~650ms even with an empty chat. Park the
 * widget, keep Go attached, and reparent `term.element` on return.
 *
 * CloseTerminal / user close must discard — a parked screen for a dead PTY
 * would lie. Byte-capped like the other warm parks; each widget is a ~1MiB
 * estimate (screen + Terminal internals, WebGL already dropped).
 */

import { ByteLru } from './byte-lru'

/** Accounting fudge per parked Terminal — not V8 heap, a widget budget. */
export const TERM_PARK_ENTRY_BYTES = 1024 * 1024
/** ~48 parked PTYs on a modern machine; LRU drops the oldest over this. */
export const TERM_PARK_MAX_BYTES = 48 * 1024 * 1024

export type ParkedTerm = {
  term: { element?: HTMLElement; dispose: () => void }
  fit: { fit: () => void }
  writer: { dispose: () => void }
  disposers: (() => void)[]
  lastCols: number
  lastRows: number
  workspaceId?: string
}

const parked = new ByteLru<ParkedTerm>({
  maxBytes: TERM_PARK_MAX_BYTES,
  sizeOf: () => TERM_PARK_ENTRY_BYTES,
})
/** Close/kill: next pane onDestroy must dispose, not stash. */
const discardIds = new Set<string>()

let frag: DocumentFragment | undefined

function parkRoot(): DocumentFragment {
  if (!frag) frag = document.createDocumentFragment()
  return frag
}

export function stashParkedTerminal(id: string, payload: ParkedTerm): { id: string; payload: ParkedTerm }[] {
  if (!id) return [{ id: '', payload }]
  discardIds.delete(id)
  const screen = payload.term.element
  if (screen) parkRoot().appendChild(screen)
  return parked.set(id, payload).map(({ key, value }) => ({ id: key, payload: value }))
}

export function takeParkedTerminal(id: string): ParkedTerm | undefined {
  if (!id) return undefined
  return parked.take(id)
}

/** Mark this session for real dispose. Returns a stash if the pane already unmounted. */
export function discardParkedTerminal(id: string): ParkedTerm | undefined {
  if (!id) return undefined
  discardIds.add(id)
  return takeParkedTerminal(id)
}

export function shouldDisposeTerminal(id: string): boolean {
  return discardIds.delete(id)
}

export function parkedTerminalCount(): number {
  return parked.size
}

/** Drop parked widgets whose workspace is gone. Caller must release + detach. */
export function pruneParkedTerminals(keepWorkspaceIds: Iterable<string>): { id: string; payload: ParkedTerm }[] {
  const keep = new Set(keepWorkspaceIds)
  const dropped: { id: string; payload: ParkedTerm }[] = []
  for (const id of [...parked.keys()]) {
    const payload = parked.peek(id)
    if (!payload || !payload.workspaceId || keep.has(payload.workspaceId)) continue
    parked.take(id)
    dropped.push({ id, payload })
  }
  return dropped
}

export function releaseParkedTerminal(p: ParkedTerm): void {
  for (const fn of p.disposers) {
    try { fn() } catch { /* already gone */ }
  }
  try { p.writer.dispose() } catch { /* already gone */ }
  try { p.term.dispose() } catch { /* already gone */ }
}

type TermPaneHooks = {
  onTitle?: (title: string) => void
  onExit?: (exitCode: number) => void
}

const paneHooks = new Map<string, TermPaneHooks>()

/** Live pane callbacks — parked widgets look these up so a remount can replace them. */
export function bindTermPaneHooks(id: string, hooks: TermPaneHooks): () => void {
  paneHooks.set(id, hooks)
  return () => {
    if (paneHooks.get(id) === hooks) paneHooks.delete(id)
  }
}

export function termPaneHooks(id: string): TermPaneHooks | undefined {
  return paneHooks.get(id)
}

export function resetTermParkForTests(): void {
  parked.clear()
  parked.maxBytes = TERM_PARK_MAX_BYTES
  discardIds.clear()
  paneHooks.clear()
  frag = undefined
}

export function setTermParkMaxBytesForTests(n: number): void {
  parked.maxBytes = n
}
