/**
 * FileAtBase text for the SCM gutter.
 *
 * `null` = not loaded / non-git / scratch — no marks. Empty string means the
 * path did not exist at the spin-off revision and is painted as a full-file
 * add, but only for real SCM additions (`u`/`a`). Gitignored files also miss
 * that revision (porcelain skips `!!`) and must not get a green wall.
 */
export function scmGutterBase(cached: string | null, gitStatus: string): string | null {
  if (cached == null) return null
  if (cached === '' && gitStatus !== 'u' && gitStatus !== 'a') return null
  return cached
}

/** Observatory editor view chip: Code / Diff / Preview. */

export type EditorViewMode = 'code' | 'diff' | 'preview'

/**
 * Raster image tabs in the observatory — matches Go `imageMIME`.
 * SVG is omitted: it stays editable text with Code + Preview.
 */
const BINARY_IMAGE_EXT_RE = /\.(png|jpe?g|gif|webp|ico|bmp|avif|tiff?)$/i

export function isBinaryImagePath(path: string): boolean {
  const base = path.trim().split(/[\\/]/).pop() ?? ''
  const q = base.lastIndexOf('?')
  const name = q >= 0 ? base.slice(0, q) : base
  return BINARY_IMAGE_EXT_RE.test(name)
}

export function isPdfPath(path: string): boolean {
  const base = path.trim().split(/[\\/]/).pop() ?? ''
  const q = base.lastIndexOf('?')
  const name = q >= 0 ? base.slice(0, q) : base
  return name.toLowerCase().endsWith('.pdf')
}

/** Played with <video> — matches Go `videoMIME`. */
export function isVideoPath(path: string): boolean {
  const base = path.trim().split(/[\\/]/).pop() ?? ''
  const q = base.lastIndexOf('?')
  const name = q >= 0 ? base.slice(0, q) : base
  return /\.(mp4|m4v|mov|webm|ogv)$/i.test(name)
}

/** PDF or video: Preview only, no Code or Diff. */
export function isMediaPath(path: string): boolean {
  return isPdfPath(path) || isVideoPath(path)
}

/** Enabled chip options in crumb-bar order (Code, Diff, Preview). */
export function editorViewModes(opts: {
  hasFile: boolean
  deleted: boolean
  canShowDiff: boolean
  hasPreview: boolean
  /** Binary raster (png/jpeg/…) — Preview (on-disk) and Diff (image compare); never Code. */
  isImage?: boolean
}): EditorViewMode[] {
  if (opts.isImage) {
    const modes: EditorViewMode[] = []
    if (opts.hasFile && !opts.deleted) modes.push('preview')
    if (opts.canShowDiff) modes.push('diff')
    return modes
  }
  const modes: EditorViewMode[] = []
  if (opts.hasFile && !opts.deleted) modes.push('code')
  if (opts.canShowDiff) modes.push('diff')
  if (opts.hasPreview && opts.hasFile && !opts.deleted) modes.push('preview')
  return modes
}

/** Next chip in cycle order, or null when there is nothing to toggle. */
export function nextEditorViewMode(
  current: EditorViewMode,
  modes: readonly EditorViewMode[],
): EditorViewMode | null {
  if (modes.length < 2) return null
  const i = modes.indexOf(current)
  return modes[i < 0 ? 0 : (i + 1) % modes.length]!
}

/**
 * Diff can mount only when both sides of the comparison are this path.
 *
 * `collapseUnchanged` is initialized once on EditorState.create. If the merge
 * pane mounts with an empty/stale current doc (base RPC won the race) and then
 * swaps in the real buffer, those collapsed ranges are never recomputed — the
 * file paints fully expanded until Code → Diff remounts.
 */
export function mergeViewReady(opts: {
  selectedPath: string
  baseDocPath: string
  canShowDiff: boolean
  /** Deleted files and submodule directories have no current buffer. */
  missingCurrent?: boolean
  /** Hydrated editor path; empty until ReadFile lands. */
  editorPath: string
}): boolean {
  if (!opts.selectedPath || opts.baseDocPath !== opts.selectedPath || !opts.canShowDiff) {
    return false
  }
  return !!opts.missingCurrent || opts.editorPath === opts.selectedPath
}

/** Current-side merge text. Empty until this path is hydrated (or when missing). */
export function mergeViewCurrent(opts: {
  missingCurrent?: boolean
  editorPath: string
  selectedPath: string
  draft: string
}): string {
  if (opts.missingCurrent) return ''
  return opts.editorPath === opts.selectedPath ? opts.draft : ''
}

/**
 * A deleted-on-disk tab whose git status flipped back to a real file needs a
 * ReadFile — editorPath was intentionally left empty and the live refresh
 * skips that mismatch.
 */
export function shouldHydrateRestoredCurrent(opts: {
  selectedPath: string
  editorPath: string
  selectedIsDir?: boolean
  fileLoading?: boolean
  hydrateFailedPath?: string
  prevStatus: string
  nextStatus: string
}): boolean {
  if (!opts.selectedPath) return false
  if (opts.editorPath === opts.selectedPath || opts.selectedIsDir || opts.fileLoading) return false
  if (opts.hydrateFailedPath === opts.selectedPath) return false
  return opts.prevStatus === 'd' && !!opts.nextStatus && opts.nextStatus !== 'd'
}
