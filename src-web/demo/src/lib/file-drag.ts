/**
 * File drags from explorer rows (intra-app) and OS/Finder drops.
 * Composer: workspace-relative @file chips, images → attachments.
 * Terminal: shell-quoted paths written to the PTY.
 *
 * HTML5 DataTransfer custom types are unreliable in WebKit, so the live explorer
 * payload lives in module state. MIME data is a fallback for the drop event.
 */

export const FILE_DRAG_MIME = 'application/x-recur-workspace-file'

/** Wails native drop target marker (EnableFileDrop). */
export const FILE_DROP_TARGET_ATTR = 'data-file-drop-target'

/** Identifies which AgentChat instance should receive a Wails OS file drop. */
export const CHAT_DROP_ATTR = 'data-chat-drop'

const IMAGE_EXT_RE = /\.(png|jpe?g|gif|webp|bmp|svg|ico|avif|heic|heif)$/i

let activePaths: string[] = []
let dragCleanupArmed = false
let dragIdleTimer: ReturnType<typeof setTimeout> | undefined

/** Clear `activePaths` if no `dragover` has fired for this long (detached source, no drop). */
const FILE_DRAG_IDLE_MS = 1000

export function normalizeFileDragPaths(paths: readonly string[]): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  for (const raw of paths) {
    const p = raw.trim().replace(/\\/g, '/').replace(/^\/+/, '')
    if (!p || seen.has(p)) continue
    seen.add(p)
    out.push(p)
  }
  return out
}

function typesHas(dt: DataTransfer, type: string): boolean {
  try {
    return Array.from(dt.types as unknown as string[]).includes(type)
  } catch {
    return false
  }
}

export function startFileDrag(paths: readonly string[], dt?: DataTransfer | null): string[] {
  const next = normalizeFileDragPaths(paths)
  activePaths = next
  if (dt && next.length) {
    dt.effectAllowed = 'copyMove'
    try { dt.setData(FILE_DRAG_MIME, JSON.stringify(next)) } catch { /* WebKit may reject custom types */ }
    // Do not set text/plain — CodeMirror's built-in drop handler inserts it.
    // text/uri-list is a WebKit-safe fallback so the drag still initiates when
    // the custom MIME is rejected. Workspace-relative paths are not file: URIs,
    // so pathFromFileUrl will not treat them as OS drops.
    try { dt.setData('text/uri-list', next.join('\r\n')) } catch { /* ignore */ }
  }
  if (next.length) armFileDragCleanup()
  return next
}

function clearFileDragIdleTimer() {
  if (dragIdleTimer === undefined) return
  clearTimeout(dragIdleTimer)
  dragIdleTimer = undefined
}

function bumpFileDragIdle() {
  clearFileDragIdleTimer()
  dragIdleTimer = setTimeout(() => {
    disarmFileDragCleanup()
    endFileDrag()
  }, FILE_DRAG_IDLE_MS)
}

function onFileDragDone() {
  disarmFileDragCleanup()
  // Capture `drop` runs before the chat target handler; delay the clear so
  // `fileDragPaths()` still sees the payload. `stopPropagation` on the chat
  // drop does not skip this capture listener.
  setTimeout(() => { endFileDrag() }, 0)
}

function disarmFileDragCleanup() {
  if (!dragCleanupArmed) return
  dragCleanupArmed = false
  clearFileDragIdleTimer()
  if (typeof document === 'undefined') return
  document.removeEventListener('dragend', onFileDragDone, true)
  document.removeEventListener('drop', onFileDragDone, true)
  document.removeEventListener('dragover', bumpFileDragIdle, true)
}

function armFileDragCleanup() {
  if (dragCleanupArmed || typeof document === 'undefined') return
  dragCleanupArmed = true
  document.addEventListener('dragend', onFileDragDone, true)
  document.addEventListener('drop', onFileDragDone, true)
  document.addEventListener('dragover', bumpFileDragIdle, true)
  bumpFileDragIdle()
}

export function endFileDrag(): void {
  activePaths = []
}

/** True while an explorer file drag is in flight (dragover-safe: no getData). */
export function isFileDrag(dt?: DataTransfer | null): boolean {
  if (activePaths.length) return true
  return !!dt && typesHas(dt, FILE_DRAG_MIME)
}

/** OS / Finder file drag (`Files` in dataTransfer.types). */
export function isExternalFileDrag(dt?: DataTransfer | null): boolean {
  return !!dt && typesHas(dt, 'Files')
}

/** Explorer path drag or an OS file drag — composer should accept the drop. */
export function isComposerFileDrop(dt?: DataTransfer | null): boolean {
  return isFileDrag(dt) || isExternalFileDrag(dt)
}

export function fileDragPaths(dt?: DataTransfer | null): string[] {
  if (activePaths.length) return activePaths
  if (!dt) return []
  let raw = ''
  try { raw = dt.getData(FILE_DRAG_MIME) } catch { /* ignore */ }
  if (!raw) return []
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return normalizeFileDragPaths(parsed.filter((x): x is string => typeof x === 'string'))
  } catch {
    return []
  }
}

export function isImagePath(path: string): boolean {
  const base = path.trim().split(/[\\/]/).pop() ?? ''
  return IMAGE_EXT_RE.test(base)
}

/** png/jpeg/gif/webp — the MIME set chat paste / /chat-image accepts. */
const CHAT_IMAGE_EXT_RE = /\.(png|jpe?g|gif|webp)$/i

export function isChatImagePath(path: string): boolean {
  const base = path.trim().split(/[\\/]/).pop() ?? ''
  return CHAT_IMAGE_EXT_RE.test(base)
}

/** True when `@path` would survive FILE_AT_RE / Go fileAtRe (no spaces or `@`). */
export function isChipableFilePath(path: string): boolean {
  const p = path.trim()
  return !!p && !/[\s@]/.test(p)
}

/** POSIX single-quote so zsh/bash see one argv. */
export function quoteForShell(path: string): string {
  if (!path) return ''
  if (/^[A-Za-z0-9_./:@%+=,-]+$/.test(path)) return path
  return `'${path.replace(/'/g, `'\\''`)}'`
}

export function isImageFile(file: File): boolean {
  if (file.type.startsWith('image/')) return true
  return isImagePath(file.name)
}

/** Decode `file:///abs/path` (and absolute filesystem paths) to a POSIX-ish path. */
export function pathFromFileUrl(raw: string): string {
  const s = raw.trim()
  if (!s) return ''
  if (/^file:/i.test(s)) {
    try {
      const u = new URL(s)
      let p = decodeURIComponent(u.pathname)
      if (/^\/[A-Za-z]:\//.test(p)) p = p.slice(1)
      return p
    } catch {
      return ''
    }
  }
  const posix = s.replace(/\\/g, '/')
  if (posix.startsWith('/') || /^[A-Za-z]:\//.test(posix)) return posix
  return ''
}

/** Absolute filesystem path (Finder drop, @/Users/… chip), not worktree-relative. */
export function isAbsFilesystemPath(path: string): boolean {
  const p = path.trim().replace(/\\/g, '/')
  if (!p || p.startsWith('scratch://')) return false
  return p.startsWith('/') || /^[A-Za-z]:\//.test(p)
}

/** Join a workspace-relative explorer path onto `root`. Already-absolute paths pass through. */
export function resolveExplorerDropPath(rel: string, root: string): string {
  const p = rel.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  if (!p) return ''
  if (isAbsFilesystemPath(p)) return p
  const r = root.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  if (!r) return p
  return `${r}/${p.replace(/^\/+/, '')}`
}

export function terminalDropText(paths: readonly string[], root = ''): string {
  return paths
    .map((p) => quoteForShell(root ? resolveExplorerDropPath(p, root) : p.replace(/\/+$/, '') || p))
    .filter(Boolean)
    .join(' ')
}

/**
 * Workspace-relative path when `path` is inside `root`, otherwise null.
 * Already-relative paths (no leading slash) are returned as-is.
 */
export function relativeToWorkspace(path: string, root: string): string | null {
  const p = path.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  if (!p) return null
  if (!p.startsWith('/')) return p
  const r = root.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  if (!r) return null
  const pl = p.toLowerCase()
  const rl = r.toLowerCase()
  if (pl === rl) return null
  if (pl.startsWith(rl + '/')) return p.slice(r.length + 1)
  return null
}

function fileOsPath(file: File): string {
  const p = (file as File & { path?: string }).path
  return typeof p === 'string' ? p.trim() : ''
}

function dedupeKeepSlash(paths: readonly string[]): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  for (const raw of paths) {
    const p = raw.trim().replace(/\\/g, '/')
    if (!p || seen.has(p)) continue
    seen.add(p)
    out.push(p)
  }
  return out
}

/** Absolute (or file://) paths from an OS drop. Empty for in-browser File blobs. */
export function pathsFromOsDataTransfer(dt?: DataTransfer | null): string[] {
  if (!dt) return []
  const found: string[] = []
  try {
    const uriList = dt.getData('text/uri-list')
    if (uriList) {
      for (const line of uriList.split(/\r?\n/)) {
        const t = line.trim()
        if (!t || t.startsWith('#')) continue
        const p = pathFromFileUrl(t)
        if (p) found.push(p)
      }
    }
  } catch { /* ignore */ }
  try {
    for (const f of Array.from(dt.files ?? [])) {
      const p = fileOsPath(f)
      if (p) found.push(pathFromFileUrl(p))
    }
  } catch { /* ignore */ }
  return dedupeKeepSlash(found)
}

export function imageFilesFromDrop(dt?: DataTransfer | null): File[] {
  if (!dt?.files?.length) return []
  return Array.from(dt.files).filter(isImageFile)
}

export function classifyOsDrop(opts: {
  paths: readonly string[]
  images: readonly File[]
  rootPath: string
}): { mentions: string[]; images: File[]; imagePaths: string[]; skipped: string[] } {
  const mentions: string[] = []
  const imagePaths: string[] = []
  const skipped: string[] = []
  const mentionedNames = new Set<string>()
  const blobNames = new Set(opts.images.map((f) => f.name))
  for (const raw of opts.paths) {
    const rel = relativeToWorkspace(raw, opts.rootPath)
    if (rel) {
      // Worktree files (including images) stay @file chips — the agent Reads them.
      if (isChipableFilePath(rel)) mentions.push(rel)
      else skipped.push(rel)
      const base = rel.split('/').pop()
      if (base) mentionedNames.add(base)
      continue
    }
    if (isChatImagePath(raw)) {
      const base = raw.split(/[\\/]/).pop() ?? ''
      // HTML5 File blobs attach via `images`; skip the path so we don't double-attach.
      if (!blobNames.has(base)) imagePaths.push(raw)
      continue
    }
    if (raw && isChipableFilePath(raw)) mentions.push(raw)
    else if (raw) skipped.push(raw)
  }
  const images = opts.images.filter((f) => !mentionedNames.has(f.name))
  return { mentions, images, imagePaths, skipped }
}

/**
 * Chat-tab keys are `workspaceId\\tcode`. Tabs are illegal/unstable in HTML
 * attributes, so the Wails drop target id uses a colon instead.
 */
export function encodeChatDropId(resetKey: string): string {
  return (resetKey || '').replaceAll('\t', ':')
}

/** Parse the `files.dropped` payload Go emits after a native OS drop. */
export function filesFromWailsDrop(data: unknown): { files: string[]; dropId: string } {
  if (!data || typeof data !== 'object') return { files: [], dropId: '' }
  const rec = data as Record<string, unknown>
  const raw = rec.files ?? rec.Filenames
  const files = Array.isArray(raw)
    ? raw.filter((x): x is string => typeof x === 'string' && !!x.trim())
    : []
  let dropId = ''
  const attrs = rec.attributes
  if (attrs && typeof attrs === 'object') {
    const v = (attrs as Record<string, unknown>)[CHAT_DROP_ATTR]
    if (typeof v === 'string') dropId = v
  }
  return { files, dropId }
}
