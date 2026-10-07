/** Shared Files-changed / Commits layout prefs (same keys as the PR detail view). */

export type DiffLayout = 'unified' | 'split'
export type ReviewTab = 'changes' | 'commits'
export type FileListMode = 'flat' | 'tree'

const DIFF_LAYOUT_KEY = 'recur.prDiffLayout'
const FILES_VIEW_KEY = 'recur.prFilesView'
const REVIEW_SIDEBAR_KEY = 'recur.reviewSidebar'
const REVIEW_TAB_KEY = 'recur.obsReviewTab'
const FILE_LIST_MODE_KEY = 'recur.reviewFileListMode'

export function readDiffLayout(): DiffLayout {
  try {
    const v = localStorage.getItem(DIFF_LAYOUT_KEY)
    if (v === 'split' || v === 'unified') return v
  } catch { /* ignore */ }
  return 'unified'
}

export function writeDiffLayout(next: DiffLayout) {
  try { localStorage.setItem(DIFF_LAYOUT_KEY, next) } catch { /* ignore */ }
}

export function readReviewSidebar(): boolean {
  try {
    const v = localStorage.getItem(REVIEW_SIDEBAR_KEY)
    if (v === 'on' || v === 'off') return v === 'on'
    // migrate: old list/stack pref — list had a sidebar, stack didn't
    const old = localStorage.getItem(FILES_VIEW_KEY)
    if (old === 'list' || old === 'stack') return old === 'list'
  } catch { /* ignore */ }
  return true
}

export function writeReviewSidebar(open: boolean) {
  try { localStorage.setItem(REVIEW_SIDEBAR_KEY, open ? 'on' : 'off') } catch { /* ignore */ }
}

export function readReviewTab(): ReviewTab {
  try {
    const v = localStorage.getItem(REVIEW_TAB_KEY)
    if (v === 'changes' || v === 'commits') return v
  } catch { /* ignore */ }
  return 'changes'
}

export function writeReviewTab(next: ReviewTab) {
  try { localStorage.setItem(REVIEW_TAB_KEY, next) } catch { /* ignore */ }
}

export function readFileListMode(): FileListMode {
  try {
    const v = localStorage.getItem(FILE_LIST_MODE_KEY)
    if (v === 'flat' || v === 'tree') return v
  } catch { /* ignore */ }
  return 'tree'
}

export function writeFileListMode(next: FileListMode) {
  try { localStorage.setItem(FILE_LIST_MODE_KEY, next) } catch { /* ignore */ }
}

type DiffBody = {
  path: string
  oldPath?: string
  status?: string
  diff?: { add?: number; del?: number } | null
  lines?: readonly { type?: string; old?: string; new?: string; codeHtml?: string }[] | null
}

function dirOf(p: string): string {
  const i = p.lastIndexOf('/')
  return i < 0 ? '' : p.slice(0, i)
}

function baseOf(p: string): string {
  return p.slice(p.lastIndexOf('/') + 1)
}

type PathLabel = { path?: string; oldPath?: string }

/** Header/list label: `dir/old.ts → new.ts` when the parent directory is unchanged. */
export function fileDiffPathLabel(f: PathLabel): string {
  const path = f.path ?? ''
  const oldPath = f.oldPath ?? ''
  if (!oldPath || oldPath === path) return path
  if (dirOf(oldPath) === dirOf(path)) return `${oldPath} → ${baseOf(path)}`
  return `${oldPath} → ${path}`
}

/** Tree-row label: basename, or `old → new` (full old path when the directory changed). */
export function fileDiffTreeLabel(f: PathLabel): string {
  const path = f.path ?? ''
  const oldPath = f.oldPath ?? ''
  const name = baseOf(path)
  if (!oldPath || oldPath === path) return name
  if (dirOf(oldPath) === dirOf(path)) return `${baseOf(oldPath)} → ${name}`
  return `${oldPath} → ${name}`
}

function fileDiffBodyEqual(a: DiffBody, b: DiffBody): boolean {
  if (a.path !== b.path || (a.oldPath ?? '') !== (b.oldPath ?? '') || (a.status ?? '') !== (b.status ?? '')) return false
  if ((a.diff?.add ?? 0) !== (b.diff?.add ?? 0) || (a.diff?.del ?? 0) !== (b.diff?.del ?? 0)) return false
  const al = a.lines ?? []
  const bl = b.lines ?? []
  if (al.length !== bl.length) return false
  for (let i = 0; i < al.length; i++) {
    const x = al[i], y = bl[i]
    if (x.type !== y.type || x.old !== y.old || x.new !== y.new || x.codeHtml !== y.codeHtml) return false
  }
  return true
}

/** Keep previous FileDiff object identities when a live refresh is a no-op. */
export function reuseFileDiffs<T extends DiffBody>(prev: readonly T[], next: readonly T[]): T[] {
  if (!next.length) return (prev.length ? next : prev) as T[]
  if (!prev.length) return next as T[]
  const byPath = new Map(prev.map((f) => [f.path, f]))
  let changed = prev.length !== next.length
  const out = next.map((f, i) => {
    const old = byPath.get(f.path)
    if (old && fileDiffBodyEqual(old, f)) {
      if (old !== prev[i]) changed = true
      return old
    }
    changed = true
    return f
  })
  return changed ? out : prev as T[]
}
