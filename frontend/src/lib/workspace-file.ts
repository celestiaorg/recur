/** Asset-server URL that streams a worktree file without the Wails JSON bridge. */
export function workspaceFileURL(
  projectId: string,
  workspaceId: string,
  path: string,
  mtime = 0,
  at?: 'base',
): string {
  const q = new URLSearchParams()
  q.set('project', projectId)
  if (workspaceId) q.set('workspace', workspaceId)
  q.set('path', path)
  if (mtime) q.set('t', String(mtime))
  if (at) q.set('at', at)
  return `/workspace-file?${q}`
}

/** Asset-server URL that streams an absolute-path file outside the worktree. */
export const EXTERNAL_FILE_PATH = '/external-file'

export function externalFileURL(absPath: string, mtime = 0): string {
  const q = new URLSearchParams({ path: absPath })
  if (mtime) q.set('t', String(mtime))
  return `${EXTERNAL_FILE_PATH}?${q}`
}

/** Raster or PDF — stream via the asset server; do not treat as editable text. */
export function isBinaryPreviewMime(mime?: string): boolean {
  return !!mime && (mime.startsWith('image/') || mime === 'application/pdf')
}

/** True when ReadFile returned metadata only — body must be fetched. */
export function fileNeedsHTTPBody(f: { content?: string; mime?: string; size: number }): boolean {
  if (f.mime) return false
  return !f.content && f.size > 0
}

export async function fetchWorkspaceFileText(
  projectId: string,
  workspaceId: string,
  path: string,
  signal?: AbortSignal,
): Promise<string> {
  const r = await fetch(workspaceFileURL(projectId, workspaceId, path), { signal })
  if (!r.ok) {
    const msg = (await r.text().catch(() => '')).trim() || r.statusText
    throw new Error(msg || `HTTP ${r.status}`)
  }
  return r.text()
}

export async function readExternalEditorFile(
  path: string,
  signal?: AbortSignal,
): Promise<{
  path: string
  language: string
  content: string
  size: number
  writable: boolean
  mime?: string
  mtime?: number
}> {
  const r = await fetch(externalFileURL(path), { signal })
  if (!r.ok) {
    const msg = (await r.text().catch(() => '')).trim() || r.statusText
    throw new Error(msg || `not found: ${path}`)
  }
  const rawType = (r.headers.get('content-type') || '').split(';')[0].trim()
  const sizeHdr = Number(r.headers.get('content-length') || 0)
  const parsed = Date.parse(r.headers.get('last-modified') || '')
  const mtime = Number.isFinite(parsed) ? parsed : 0
  if (isBinaryPreviewMime(rawType)) {
    try { await r.body?.cancel() } catch { /* already consumed */ }
    return {
      path,
      language: '',
      content: '',
      size: sizeHdr,
      writable: false,
      mime: rawType,
      mtime,
    }
  }
  const content = await r.text()
  const i = path.replace(/\\/g, '/').lastIndexOf('.')
  const ext = i >= 0 ? path.slice(i + 1) : ''
  return {
    path,
    language: ext || 'text',
    content,
    size: content.length,
    writable: false,
    mtime,
  }
}
