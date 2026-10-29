/**
 * In-memory keep-alive for Observatory UI state across workspace switches.
 * Returning to a recently viewed root restores tabs/tree/editor synchronously;
 * Observatory still runs a background refresh (stale-while-revalidate).
 *
 * Discrete from the other warm caches: the key is project+workspace, puts are
 * full snapshots, and eviction is just a drop (no widget dispose).
 */

import type {
  TreeEntry, FileContent, OutlineSymbol,
} from '../../../bindings/recur/internal/domain/models'
import { ByteLru, WARM_CACHE_MAX_BYTES, estimateBytes } from './byte-lru'
import type { ScratchDoc, StashedBuffer } from '../editor-session'
import type { FileViewState } from '../editor-view-state'
import type { ReviewTab } from '../review-diff'
import type { CodePanelKey } from '../stores'

export type { StashedBuffer }

/** Last workbench chrome that had keyboard focus in this observatory root. */
export type ObservatoryFocusZone = 'editor' | 'terminal' | 'agent' | 'other'

export type ObservatorySnapshot = {
  tree: TreeEntry[]
  expanded: string[]
  loadedDirs: string[]
  changedCollapsed: string[]
  changedPaths: string[]
  gitStatus: Record<string, string>
  /**
   * Whether GitStatus had settled when this snapshot was taken. Empty
   * `gitStatus` is not enough — that is also the shape of a clean tree.
   */
  gitStatusReady?: boolean
  workingStatus: Record<string, string>
  stagedFiles: string[]
  openFiles: string[]
  closedTabs: string[]
  selectedPath: string
  /** Explorer highlight; may be a folder while selectedPath stays the open file. */
  treePath?: string
  file: FileContent | null
  draft: string
  savedContent: string
  outline: OutlineSymbol[]
  viewMode: 'code' | 'preview' | 'diff'
  baseDoc: string
  baseDocPath: string
  browseAll: boolean
  scopeChosen: boolean
  leftKind: CodePanelKey
  rightKind: CodePanelKey
  /** Left dock visibility. Omitted on older snapshots → leave the parent default. */
  leftOpen?: boolean
  /** Right dock visibility. Omitted on older snapshots → stay at `$state(true)`. */
  showSide?: boolean
  splitOpen: boolean
  splitPath: string
  splitDraft: string
  splitSaved: string
  splitWritable: boolean
  focusedPane: 'main' | 'split'
  /** Restored on A→B→A so the terminal remount cannot steal the last chrome. */
  focusZone?: ObservatoryFocusZone
  mainEditorDirty: boolean
  splitEditorDirty: boolean
  sessionRestored: boolean
  /**
   * Whether `loadEditorSession` for this root actually settled (resolved, not
   * rejected, and not aborted by a root switch). An empty tab strip is only
   * safe to persist-as-clear when this is true — a keep-alive snapshot taken
   * before restore landed is not evidence the stored session is empty.
   */
  sessionLoadSettled?: boolean
  /**
   * Whether restore has authoritative syntax maps. False for an unsettled
   * quit journal whose maps were not filled from disk.
   */
  syntaxMapsReady?: boolean
  scratchDocs: Record<string, ScratchDoc>
  scratchSeq: number
  /** Per-file caret / scroll / Code|Preview|Diff — restored with the snapshot. */
  fileViews?: Record<string, FileViewState>
  /** Review surface tab when the snapshot was taken. */
  reviewTab?: ReviewTab
  reviewSelectedPath?: string
  reviewScrollTop?: number
  /** Editor vs Review occupying the main column. */
  mainSurface?: 'editor' | 'review'
  /** Per-path TextMate language override (`''` = Plain Text). */
  syntaxByPath?: Record<string, string>
  /** `*.ext` / basename language associations. */
  syntaxByFileType?: Record<string, string>
  /** Per-open-tab drafts so switching back does not refetch / remount empty. */
  tabBuffers?: Record<string, { draft: string; saved: string; file: FileContent }>
  /** Dirty buffers for background tabs; selected/split panes use draft fields. */
  stashedBuffers?: Record<string, StashedBuffer>
  /**
   * File-tab strip scrollLeft when the strip had live layout. Omitted when
   * the node is missing or detached (a fabricated 0 would pin restore and
   * suppress the reveal chase). Restored as-is on A→B→A so returning to a
   * workspace does not chase the active chip.
   */
  fileTabScrollLeft?: number
}

/** Typical snapshot is the active buffer + tree (~0.2–1MB). Dirty tab drafts
 *  are included; clean tabs are omitted (re-read from disk). */
export const OBSERVATORY_CACHE_MAX_BYTES = WARM_CACHE_MAX_BYTES

const cache = new ByteLru<ObservatorySnapshot>({
  maxBytes: OBSERVATORY_CACHE_MAX_BYTES,
  sizeOf: estimateBytes,
})

export function observatoryCacheKey(projectId: string, workspaceId: string): string {
  return `${projectId}\0${workspaceId}`
}

export function getObservatorySnapshot(key: string): ObservatorySnapshot | null {
  return cache.get(key) ?? null
}

export function putObservatorySnapshot(key: string, snap: ObservatorySnapshot): void {
  if (!key || key === '\0') return
  cache.set(key, snap)
}

/** Drop snapshots whose workspace id is no longer known. */
export function pruneObservatorySnapshots(liveWorkspaceIds: Iterable<string>): void {
  const live = new Set(liveWorkspaceIds)
  cache.prune((key) => {
    const ws = key.split('\0')[1] ?? ''
    return !ws || live.has(ws)
  })
}

export function dropObservatorySnapshot(key: string): void {
  cache.delete(key)
}

export function resetObservatoryCacheForTests(): void {
  cache.clear()
  cache.maxBytes = OBSERVATORY_CACHE_MAX_BYTES
}

export function setObservatoryCacheMaxBytesForTests(n: number): void {
  cache.maxBytes = n
}
