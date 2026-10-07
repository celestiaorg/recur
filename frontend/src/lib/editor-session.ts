/**
 * Persist open editor tabs per workspace for session restore.
 *
 * The session itself (tabs, selection, scratch docs, unsaved buffers,
 * per-file caret/scroll/view-mode, TextMate language-mode maps, the
 * Observatory Editor vs Review column, and left/right pane kind plus
 * visibility) lives Go-side as one gzipped JSON
 * file under the workspace data dir — it shares the workspace's lifecycle
 * (deleted with it), survives webview-storage wipes, and has no localStorage
 * quota to trim against. This module is the RPC client:
 *
 *  - Writes are debounced and CHANGE-GATED. Each flush diffs the session
 *    against what was last sent and ships a patch (meta when it changed,
 *    upserted buffers/scratch docs, explicit drops); the backend merges. An
 *    unchanged session costs no bridge call at all.
 *  - Unsaved buffers travel as {draft, baseHash} — a sha256 of the disk text
 *    the draft diverged from — instead of a full saved copy. Restore compares
 *    the hash against the freshly read file to detect "disk moved underneath
 *    the stash".
 *  - Per-workspace RPCs run through an ordering queue: the backend merge
 *    assumes patches arrive in the order they were built.
 *
 * Explorer scope (Changed vs All files) stays in localStorage: it is tiny,
 * wants synchronous access, and is a choice the user can make with no session
 * at all. Legacy localStorage sessions are imported once, then removed.
 */

import { EditorSessionService } from '../../bindings/recur/internal/app'
import { editorTextEq, normalizeEditorNewlines } from './editor-dirty'
import {
  parseFileViews, pruneFileViews, type FileViewState,
} from './editor-view-state'
import { parseSyntaxMap, pruneSyntaxMaps } from './syntax-mode'
import type { CodePanelKey } from './stores'

/** Pre-split blob and per-workspace keys from the localStorage era. */
const LEGACY_V1_KEY = 'recur.editorSession.v1'
const LEGACY_V2_PREFIX = 'recur.editorSession.v2:'

/** Max UTF-16 length stored per scratch buffer (bounds bridge payloads). */
const MAX_SCRATCH_CHARS = 400_000
/** Cap how many scratch docs we persist with a session. */
const MAX_SCRATCH_DOCS = 12
/**
 * Max UTF-16 length per unsaved disk buffer. Oversize drafts are skipped
 * entirely — never truncated — so a restart cannot silently mangle source.
 * Storage is on disk now, so this only bounds what crosses the bridge per
 * flush; it comfortably covers anything the editor lets you edit.
 */
export const MAX_UNSAVED_CHARS = 4_000_000
/** Cap how many dirty disk buffers we persist with a session. */
const MAX_UNSAVED_BUFFERS = 100
/** Cap persisted tab-strip length. */
const MAX_OPEN_FILES = 64
/** Debounce keystroke-driven writes. */
const SAVE_DEBOUNCE_MS = 400
/**
 * Synchronous last-line-of-defence for quit / pagehide. The Go flush is
 * async (queue → SHA-256 → bridge IPC) and cannot complete once the webview
 * is tearing down; this key is written in the same turn as `flushEditorSessionNow`
 * so the next boot can import whatever the IPC dropped.
 */
const QUIT_JOURNAL_PREFIX = 'recur.editorSession.quit:'

export type ScratchDoc = {
  title: string
  content: string
  lang: string
  /** When set (e.g. image/*), observatory renders as binary instead of text. */
  mime?: string
}

/**
 * Unsaved editor text kept when leaving a tab, or across an app restart.
 * A live stash carries the disk text it diverged from (`saved`); a stash
 * restored from the backend carries only that text's hash (`baseHash`) —
 * enough to detect drift when the tab is next opened.
 */
export type StashedBuffer = { draft: string; saved?: string; baseHash?: string }

export type Session = {
  openFiles: string[]
  selectedPath: string
  splitPath?: string
  scratchDocs?: Record<string, ScratchDoc>
  scratchSeq?: number
  stashedBuffers?: Record<string, StashedBuffer>
  /** Per-file caret / scroll / Code|Preview|Diff — restored on cold start. */
  fileViews?: Record<string, FileViewState>
  /**
   * Per-path TextMate scope (`''` = Plain Text). Independent of the tab strip
   * so closing a file does not forget "this Dockerfile is dockerfile".
   */
  syntaxByPath?: Record<string, string>
  /** `*.ext` / basename associations ("always open files like this as…"). */
  syntaxByFileType?: Record<string, string>
  /**
   * Observatory main column. Omitted / anything but `'review'` is Editor.
   * Survives cold remount via the workspace editor-session file.
   */
  mainSurface?: 'editor' | 'review'
  /** Left dock panel. Omitted / unknown is Maestro. */
  leftKind?: CodePanelKey
  /** Right dock panel. Omitted / unknown is Files. */
  rightKind?: CodePanelKey
  /**
   * Left/right dock visibility. Omitted is open (cold default). `false` is
   * stored as a JSON bool so omitempty cannot drop a hidden pane.
   */
  leftOpen?: boolean
  rightOpen?: boolean
  /**
   * When false/omitted, meta patches leave stored syntax maps alone — a
   * tab-strip write before restore must not ship the cold-empty maps.
   * When true, empty maps are sent explicitly so Auto Detect can clear.
   */
  syntaxMapsKnown?: boolean
}

/** True when left/right kind or visibility is not the Observatory cold default. */
export function sessionHasPaneChrome(sess: {
  leftKind?: string
  rightKind?: string
  leftOpen?: boolean
  rightOpen?: boolean
}): boolean {
  return (!!sess.leftKind && sess.leftKind !== 'maestro')
    || (!!sess.rightKind && sess.rightKind !== 'files')
    || sess.leftOpen === false
    || sess.rightOpen === false
}

/** First-wins unique. Empty paths dropped. The tab strip keys on path. */
export function uniqueOpenFiles(paths: readonly string[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const p of paths) {
    if (!p || seen.has(p)) continue
    seen.add(p)
    out.push(p)
  }
  return out
}

/**
 * Rewrite open tabs after a rename/move. If `to` is already a tab (deleted
 * original, then something else renamed onto that name), drop the stale `to`
 * and keep the remapped slot so the strip cannot key two chips the same.
 */
export function remapOpenFiles(paths: readonly string[], from: string, to: string): string[] {
  if (!from || !to || from === to) return uniqueOpenFiles(paths)
  const wasFrom = (p: string) => p === from || p.startsWith(from + '/')
  const rewrite = (p: string) => (p === from ? to : to + p.slice(from.length))
  const claimed = new Set(paths.filter(wasFrom).map(rewrite))
  const seen = new Set<string>()
  const out: string[] = []
  for (const p of paths) {
    if (!wasFrom(p) && claimed.has(p)) continue
    const np = wasFrom(p) ? rewrite(p) : p
    if (!np || seen.has(np)) continue
    seen.add(np)
    out.push(np)
  }
  return out
}

function isSessionPanelKey(v: unknown): v is CodePanelKey {
  return v === 'maestro' || v === 'plan' || v === 'findings' || v === 'files'
    || v === 'scm' || v === 'outline' || v === 'search' || v === 'refs' || v === 'artifacts'
}

function paneChromeOf(sess: Session): Pick<Session, 'leftKind' | 'rightKind' | 'leftOpen' | 'rightOpen'> {
  return {
    ...(isSessionPanelKey(sess.leftKind) ? { leftKind: sess.leftKind } : {}),
    ...(isSessionPanelKey(sess.rightKind) ? { rightKind: sess.rightKind } : {}),
    ...(typeof sess.leftOpen === 'boolean' ? { leftOpen: sess.leftOpen } : {}),
    ...(typeof sess.rightOpen === 'boolean' ? { rightOpen: sess.rightOpen } : {}),
  }
}

// ---- wire shapes (mirror internal/app EditorSession* structs) ----

type WireBuffer = { draft: string; baseHash: string }
type WireMeta = {
  openFiles: string[]
  selectedPath: string
  splitPath?: string
  scratchSeq?: number
  fileViews?: Record<string, FileViewState>
  syntaxByPath?: Record<string, string>
  syntaxByFileType?: Record<string, string>
  mainSurface?: 'review'
  leftKind?: string
  rightKind?: string
  leftOpen?: boolean
  rightOpen?: boolean
}
type WirePatch = {
  meta?: WireMeta
  buffers?: Record<string, WireBuffer>
  dropBuffers?: string[]
  scratchDocs?: Record<string, ScratchDoc>
  dropScratchDocs?: string[]
}

/**
 * sha256 hex of the text as CodeMirror stores it (LF newlines), so a CRLF
 * file read from disk hashes the same as the buffer it seeded.
 */
export async function editorContentHash(text: string): Promise<string> {
  const data = new TextEncoder().encode(normalizeEditorNewlines(text))
  const digest = await crypto.subtle.digest('SHA-256', data)
  let out = ''
  for (const b of new Uint8Array(digest)) out += b.toString(16).padStart(2, '0')
  return out
}

// ---- per-workspace sent-state (the change gate) + RPC ordering queue ----

/**
 * What the backend holds after the last acknowledged RPC, compared by
 * reference/equality to decide what a flush must ship. `base` is the buffer's
 * saved text (live stash) or baseHash (restored stash) — either way a stable
 * string whose identity changes exactly when the stash base changes.
 */
type SentState = {
  metaJson: string
  buffers: Map<string, { draft: string; base: string }>
  scratch: Map<string, string>
}

const sentByWorkspace = new Map<string, SentState>()
const queueByWorkspace = new Map<string, Promise<unknown>>()

/** Serialize RPCs per workspace so the backend merge sees patches in order. */
function enqueue<T>(workspaceId: string, task: () => Promise<T>): Promise<T> {
  const prev = queueByWorkspace.get(workspaceId) ?? Promise.resolve()
  const next = prev.then(task, task)
  queueByWorkspace.set(workspaceId, next.catch(() => {}))
  return next
}

function emptySent(): SentState {
  return { metaJson: '', buffers: new Map(), scratch: new Map() }
}

function sentIsEmpty(sent: SentState): boolean {
  return !sent.metaJson && sent.buffers.size === 0 && sent.scratch.size === 0
}

// ---- legacy localStorage import (one-time) ----

let migrationPromise: Promise<Map<string, Session>> | null = null

function workspaceIdFromStorageKey(id: string): string {
  const sep = id.lastIndexOf('::')
  return sep >= 0 ? id.slice(sep + 2) : id
}

/**
 * Sweep every legacy localStorage session into the backend once, then remove
 * the keys. The parsed sessions are also returned so the first restore after
 * the migration doesn't depend on its own import round trip.
 */
function migrateLegacyStorage(): Promise<Map<string, Session>> {
  migrationPromise ??= (async () => {
    const byWorkspace = new Map<string, Session>()
    try {
      // v1 blob first so a newer v2 key for the same workspace wins below.
      const rawV1 = localStorage.getItem(LEGACY_V1_KEY)
      if (rawV1) {
        try {
          const parsed = JSON.parse(rawV1)
          if (parsed && typeof parsed === 'object') {
            for (const [id, sess] of Object.entries(parsed as Record<string, unknown>)) {
              const ws = workspaceIdFromStorageKey(id)
              if (ws && ws !== '_' && sess && typeof sess === 'object') {
                byWorkspace.set(ws, sess as Session)
              }
            }
          }
        } catch { /* corrupt blob — drop it */ }
      }
      const v2Keys: string[] = []
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i)
        if (k?.startsWith(LEGACY_V2_PREFIX)) v2Keys.push(k)
      }
      for (const k of v2Keys) {
        const ws = workspaceIdFromStorageKey(k.slice(LEGACY_V2_PREFIX.length))
        if (!ws || ws === '_') continue
        try {
          const sess = JSON.parse(localStorage.getItem(k) ?? '')
          if (sess && typeof sess === 'object') byWorkspace.set(ws, sess as Session)
        } catch { /* corrupt key — drop it */ }
      }
      for (const [ws, sess] of byWorkspace) {
        await importLegacySession(ws, sess)
      }
      try { localStorage.removeItem(LEGACY_V1_KEY) } catch { /* ignore */ }
      for (const k of v2Keys) {
        try { localStorage.removeItem(k) } catch { /* ignore */ }
      }
    } catch {
      // localStorage unavailable (disabled storage) — nothing to migrate.
    }
    return byWorkspace
  })()
  return migrationPromise
}

/**
 * Push one legacy session to the backend as a full patch (best-effort).
 * Deliberately NOT routed through the per-workspace queue: every queued task
 * awaits the migration before its own RPC, so going through the queue here
 * would deadlock — and migration-before-everything already guarantees order.
 */
async function importLegacySession(workspaceId: string, sess: Session): Promise<void> {
  try {
    const { patch, next } = await buildPatch(normalizeSession(sess), null)
    if (!patch) return
    await EditorSessionService.UpdateEditorSession(workspaceId, patch)
    sentByWorkspace.set(workspaceId, next)
  } catch {
    // Workspace no longer exists (or backend unavailable) — the key is
    // removed either way; a stale localStorage session is not worth keeping.
  }
}

// ---- load / restore ----

/**
 * Resolves null when there is genuinely no session to restore; REJECTS when
 * the backend read failed — the caller must be able to tell the difference,
 * because "no session" makes it safe to later clear the store and a failed
 * read does not.
 */
export function loadEditorSession(
  projectId: string,
  workspaceId: string,
): Promise<Session | null> {
  void projectId
  if (!workspaceId) return Promise.resolve(null)
  // Enqueue SYNCHRONOUSLY (awaits happen inside the task) so a load issued
  // right after a flush — the A→B→A workspace switch — cannot overtake the
  // still-settling patch and read stale state.
  return enqueue(workspaceId, async () => {
    const migrated = await migrateLegacyStorage()
    let sess = migrated.get(workspaceId) ?? null
    if (sess) {
      migrated.delete(workspaceId) // one restore per import
    }
    const journal = readQuitJournal(workspaceId)
    if (journal) {
      // Quit journal is newer than both the legacy import and the last
      // acknowledged backend snapshot — it was written in the pagehide turn
      // the IPC may not have survived. It is a delta of what the frontend
      // knew: if it did not claim the syntax maps, fill them from disk so
      // an unsettled flush cannot hydrate empty and wipe stored picks.
      let recovered = journal
      if (journal.syntaxMapsKnown !== true) {
        try {
          const wire = await EditorSessionService.GetEditorSession(workspaceId)
          const stored = sessionFromWire(wire)
          recovered = {
            ...journal,
            syntaxByPath: optionalSyntaxMap({
              ...parseSyntaxMap(stored?.syntaxByPath),
              ...parseSyntaxMap(journal.syntaxByPath),
            }),
            syntaxByFileType: optionalSyntaxMap({
              ...parseSyntaxMap(stored?.syntaxByFileType),
              ...parseSyntaxMap(journal.syntaxByFileType),
            }),
            syntaxMapsKnown: true,
          }
        } catch {
          // Journal exists because IPC was unreliable; a failed read must not
          // also drop unsaved buffers. Leave maps unknown so persist omits them.
          recovered = { ...journal, syntaxMapsKnown: false }
        }
      }
      await importLegacySession(workspaceId, recovered)
      dropQuitJournal(workspaceId)
      sess = recovered
    } else if (!sess) {
      const wire = await EditorSessionService.GetEditorSession(workspaceId)
      sess = sessionFromWire(wire)
      // Seed the change gate only when we have no acknowledged state yet:
      // this module is the sole writer, so existing sent-state is already
      // accurate — and richer (saved text beats its hash for comparisons).
      if (!sentByWorkspace.has(workspaceId)) {
        sentByWorkspace.set(workspaceId, sess ? sentStateFor(sess) : emptySent())
      }
    }
    if (!sess) return null
    sess = { ...sess, openFiles: uniqueOpenFiles(sess.openFiles) }
    const hasSyntax = !!(
      (sess.syntaxByPath && Object.keys(sess.syntaxByPath).length)
      || (sess.syntaxByFileType && Object.keys(sess.syntaxByFileType).length)
    )
    if (!sess.openFiles.length) {
      // Language maps, the Review column, and pane chrome outlive the tab strip.
      const review = sess.mainSurface === 'review'
      const panes = sessionHasPaneChrome(sess)
      if (!hasSyntax && !review && !panes) return null
      return {
        openFiles: [],
        selectedPath: '',
        syntaxByPath: sess.syntaxByPath,
        syntaxByFileType: sess.syntaxByFileType,
        syntaxMapsKnown: sess.syntaxMapsKnown,
        ...(review ? { mainSurface: 'review' as const } : {}),
        ...paneChromeOf(sess),
      }
    }
    const keep = [...sess.openFiles]
    if (sess.selectedPath) keep.unshift(sess.selectedPath)
    if (sess.splitPath) keep.push(sess.splitPath)
    sess.stashedBuffers = pruneStashedBuffers(sess.stashedBuffers, keep)
    sess.fileViews = pruneFileViews(parseFileViews(sess.fileViews), keep)
    if (!Object.keys(sess.fileViews).length) delete sess.fileViews
    return sess
  })
}

function optionalSyntaxMap(raw: unknown): Record<string, string> | undefined {
  const m = parseSyntaxMap(raw)
  return Object.keys(m).length ? m : undefined
}

function sessionFromWire(wire: unknown): Session | null {
  const w = wire as {
    openFiles?: unknown
    selectedPath?: unknown
    splitPath?: unknown
    scratchSeq?: unknown
    scratchDocs?: Record<string, ScratchDoc> | null
    buffers?: Record<string, WireBuffer> | null
    fileViews?: unknown
    syntaxByPath?: unknown
    syntaxByFileType?: unknown
    mainSurface?: unknown
    leftKind?: unknown
    rightKind?: unknown
    leftOpen?: unknown
    rightOpen?: unknown
  } | null
  if (!w || !Array.isArray(w.openFiles)) return null
  const stashedBuffers: Record<string, StashedBuffer> = {}
  for (const [path, buf] of Object.entries(w.buffers ?? {})) {
    if (buf && typeof buf.draft === 'string') {
      stashedBuffers[path] = { draft: buf.draft, baseHash: buf.baseHash || '' }
    }
  }
  const scratchDocs: Record<string, ScratchDoc> = {}
  for (const [path, doc] of Object.entries(w.scratchDocs ?? {})) {
    if (doc && typeof doc.content === 'string') scratchDocs[path] = doc
  }
  const fileViews = parseFileViews(w.fileViews)
  return {
    openFiles: uniqueOpenFiles(w.openFiles.filter((p): p is string => typeof p === 'string')),
    selectedPath: typeof w.selectedPath === 'string' ? w.selectedPath : '',
    splitPath: typeof w.splitPath === 'string' && w.splitPath ? w.splitPath : undefined,
    scratchSeq: typeof w.scratchSeq === 'number' ? w.scratchSeq : undefined,
    scratchDocs: Object.keys(scratchDocs).length ? scratchDocs : undefined,
    stashedBuffers: Object.keys(stashedBuffers).length ? stashedBuffers : undefined,
    fileViews: Object.keys(fileViews).length ? fileViews : undefined,
    syntaxByPath: optionalSyntaxMap(w.syntaxByPath),
    syntaxByFileType: optionalSyntaxMap(w.syntaxByFileType),
    mainSurface: w.mainSurface === 'review' ? 'review' : undefined,
    leftKind: isSessionPanelKey(w.leftKind) ? w.leftKind : undefined,
    rightKind: isSessionPanelKey(w.rightKind) ? w.rightKind : undefined,
    leftOpen: typeof w.leftOpen === 'boolean' ? w.leftOpen : undefined,
    rightOpen: typeof w.rightOpen === 'boolean' ? w.rightOpen : undefined,
  }
}

/** Sent-state matching a session that just came FROM the backend. */
function sentStateFor(sess: Session): SentState {
  const sent = emptySent()
  sent.metaJson = JSON.stringify(metaFor(sess))
  for (const [path, buf] of Object.entries(sess.stashedBuffers ?? {})) {
    sent.buffers.set(path, { draft: buf.draft, base: buf.saved ?? buf.baseHash ?? '' })
  }
  for (const [path, doc] of Object.entries(sess.scratchDocs ?? {})) {
    sent.scratch.set(path, doc.content)
  }
  return sent
}

// ---- save / flush ----

type PendingSave = { projectId: string; workspaceId: string; session: Session }
let pendingSave: PendingSave | null = null
let saveTimer: ReturnType<typeof setTimeout> | null = null

export function saveEditorSession(
  projectId: string,
  workspaceId: string,
  session: Session,
): void {
  if (!workspaceId) return
  pendingSave = { projectId, workspaceId, session }
  if (saveTimer != null) return
  saveTimer = setTimeout(() => {
    saveTimer = null
    const p = pendingSave
    pendingSave = null
    if (p) void flushSession(p)
  }, SAVE_DEBOUNCE_MS)
}

/** Write any pending session now — call on quit / workspace switch / unmount.
 *  The bridge write is still async; a synchronous localStorage journal covers
 *  the pagehide/quit path where that IPC cannot finish. */
export function flushEditorSessionNow(): void {
  if (saveTimer != null) {
    clearTimeout(saveTimer)
    saveTimer = null
  }
  const p = pendingSave
  pendingSave = null
  if (!p) return
  writeQuitJournal(p.workspaceId, p.session)
  void flushSession(p)
}

function cancelPendingSave(workspaceId: string): void {
  if (pendingSave?.workspaceId !== workspaceId) return
  pendingSave = null
  if (saveTimer != null) {
    clearTimeout(saveTimer)
    saveTimer = null
  }
}

function quitJournalKey(workspaceId: string): string {
  return QUIT_JOURNAL_PREFIX + workspaceId
}

/** Per-workspace generation of the quit journal; a flush may drop the journal
 *  only if no newer journal was written after that flush was enqueued. */
const journalGen = new Map<string, number>()

function writeQuitJournal(workspaceId: string, session: Session): void {
  journalGen.set(workspaceId, (journalGen.get(workspaceId) ?? 0) + 1)
  try {
    localStorage.setItem(quitJournalKey(workspaceId), JSON.stringify(session))
  } catch {
    // quota / private mode — the async flush is still in flight
  }
}

function readQuitJournal(workspaceId: string): Session | null {
  try {
    const raw = localStorage.getItem(quitJournalKey(workspaceId))
    if (!raw) return null
    const sess = JSON.parse(raw)
    if (!sess || typeof sess !== 'object') return null
    return sess as Session
  } catch {
    return null
  }
}

function dropQuitJournal(workspaceId: string): void {
  try {
    localStorage.removeItem(quitJournalKey(workspaceId))
  } catch { /* ignore */ }
}

function flushSession(p: PendingSave): Promise<void> {
  const workspaceId = p.workspaceId
  const genAtEnqueue = journalGen.get(workspaceId) ?? 0
  // Enqueue synchronously — see loadEditorSession for why order matters.
  return enqueue(workspaceId, async () => {
    await migrateLegacyStorage().catch(() => {})
    const session = normalizeSession(p.session)
    const prev = sentByWorkspace.get(workspaceId) ?? null
    const { patch, next } = await buildPatch(session, prev)
    const ackJournal = () => {
      // An in-flight older flush must not drop a newer pagehide journal.
      if ((journalGen.get(workspaceId) ?? 0) === genAtEnqueue) dropQuitJournal(workspaceId)
    }
    if (!patch) {
      ackJournal()
      return
    }
    try {
      await EditorSessionService.UpdateEditorSession(workspaceId, patch)
      sentByWorkspace.set(workspaceId, next)
      ackJournal()
    } catch {
      // Workspace deleted mid-flight or backend hiccup. Sent-state stays as
      // it was, so the next change recomputes (and re-ships) the same diff.
      // Quit journal is left in place so a subsequent load can recover.
    }
  })
}

/** Apply the persistence caps + keep-set pruning before diffing. */
function normalizeSession(session: Session): Session {
  const openFiles = uniqueOpenFiles(session.openFiles).slice(0, MAX_OPEN_FILES)
  const keep = new Set<string>()
  if (session.selectedPath) keep.add(session.selectedPath)
  for (const f of openFiles) keep.add(f)
  if (session.splitPath) keep.add(session.splitPath)
  return {
    openFiles,
    selectedPath: session.selectedPath,
    splitPath: session.splitPath || undefined,
    scratchSeq: session.scratchSeq && session.scratchSeq > 0 ? session.scratchSeq : undefined,
    scratchDocs: session.scratchDocs ? pruneScratchDocs(session.scratchDocs, keep) : undefined,
    stashedBuffers: pruneStashedBuffers(session.stashedBuffers, keep),
    fileViews: pruneSessionFileViews(session.fileViews, keep),
    ...pruneSyntaxMaps({
      syntaxByPath: parseSyntaxMap(session.syntaxByPath),
      syntaxByFileType: parseSyntaxMap(session.syntaxByFileType),
    }),
    syntaxMapsKnown: session.syntaxMapsKnown,
    ...(session.mainSurface === 'review' ? { mainSurface: 'review' as const } : {}),
    ...paneChromeOf(session),
  }
}

function metaFor(session: Session): WireMeta {
  const meta: WireMeta = {
    openFiles: session.openFiles.slice(0, MAX_OPEN_FILES),
    selectedPath: session.selectedPath,
  }
  if (session.splitPath) meta.splitPath = session.splitPath
  if (session.scratchSeq && session.scratchSeq > 0) meta.scratchSeq = session.scratchSeq
  if (session.fileViews && Object.keys(session.fileViews).length) {
    meta.fileViews = session.fileViews
  }
  // Omitted = unchanged on the backend. Empty objects clear stored maps.
  if (session.syntaxMapsKnown) {
    meta.syntaxByPath = session.syntaxByPath ?? {}
    meta.syntaxByFileType = session.syntaxByFileType ?? {}
  }
  if (session.mainSurface === 'review') meta.mainSurface = 'review'
  // Always ship pane chrome so a wholesale meta replace can clear SCM / unhide.
  meta.leftKind = isSessionPanelKey(session.leftKind) ? session.leftKind : 'maestro'
  meta.rightKind = isSessionPanelKey(session.rightKind) ? session.rightKind : 'files'
  meta.leftOpen = session.leftOpen !== false
  meta.rightOpen = session.rightOpen !== false
  return meta
}

/**
 * Diff a normalized session against the last acknowledged state. Returns a
 * null patch when nothing changed. `prev == null` means the backend state is
 * unknown (never loaded) — ship everything, drop nothing.
 */
async function buildPatch(
  session: Session,
  prev: SentState | null,
): Promise<{ patch: WirePatch | null; next: SentState }> {
  const meta = metaFor(session)
  const metaJson = JSON.stringify(meta)
  const next = emptySent()
  next.metaJson = metaJson

  const upsertBuffers: Record<string, WireBuffer> = {}
  for (const [path, buf] of Object.entries(session.stashedBuffers ?? {})) {
    const base = buf.saved ?? buf.baseHash ?? ''
    next.buffers.set(path, { draft: buf.draft, base })
    const last = prev?.buffers.get(path)
    if (last && last.draft === buf.draft && last.base === base) continue
    upsertBuffers[path] = {
      draft: buf.draft,
      baseHash: buf.baseHash ?? (typeof buf.saved === 'string' ? await editorContentHash(buf.saved) : ''),
    }
  }
  const dropBuffers: string[] = []
  for (const path of prev?.buffers.keys() ?? []) {
    if (!next.buffers.has(path)) dropBuffers.push(path)
  }

  const upsertScratch: Record<string, ScratchDoc> = {}
  for (const [path, doc] of Object.entries(session.scratchDocs ?? {})) {
    next.scratch.set(path, doc.content)
    if (prev?.scratch.get(path) === doc.content) continue
    upsertScratch[path] = doc
  }
  const dropScratch: string[] = []
  for (const path of prev?.scratch.keys() ?? []) {
    if (!next.scratch.has(path)) dropScratch.push(path)
  }

  const metaChanged = !prev || prev.metaJson !== metaJson
  if (
    !metaChanged
    && !Object.keys(upsertBuffers).length && !dropBuffers.length
    && !Object.keys(upsertScratch).length && !dropScratch.length
  ) {
    return { patch: null, next }
  }
  const patch: WirePatch = {}
  if (metaChanged) patch.meta = meta
  if (Object.keys(upsertBuffers).length) patch.buffers = upsertBuffers
  if (dropBuffers.length) patch.dropBuffers = dropBuffers
  if (Object.keys(upsertScratch).length) patch.scratchDocs = upsertScratch
  if (dropScratch.length) patch.dropScratchDocs = dropScratch
  return { patch, next }
}

// ---- clear / drop ----

/** Remove this workspace's stored session (tabs closed / discarded). */
export function clearEditorSession(projectId: string, workspaceId: string): void {
  void projectId
  if (!workspaceId) return
  cancelPendingSave(workspaceId)
  dropQuitJournal(workspaceId)
  const sent = sentByWorkspace.get(workspaceId)
  // Already acknowledged-empty (or a clear is in flight): a persist $effect
  // dir-click must not enqueue another ClearEditorSession RPC.
  if (sent && sentIsEmpty(sent)) return
  sentByWorkspace.set(workspaceId, emptySent())
  void enqueue(workspaceId, async () => {
    try {
      await EditorSessionService.ClearEditorSession(workspaceId)
      sentByWorkspace.set(workspaceId, emptySent())
    } catch {
      if (sent) sentByWorkspace.set(workspaceId, sent)
      else sentByWorkspace.delete(workspaceId)
    }
  })
}

/**
 * Forget a deleted workspace's local state immediately. The stored session
 * file needs no call here: DeleteWorkspace sweeps workspaces/<id> wholesale.
 */
export function dropEditorSession(workspaceId: string): void {
  if (!workspaceId) return
  cancelPendingSave(workspaceId)
  dropQuitJournal(workspaceId)
  sentByWorkspace.delete(workspaceId)
  queueByWorkspace.delete(workspaceId)
  dropExplorerScope(workspaceId)
}

// ---- explorer scope (stays in localStorage: tiny, wants sync access) ----

/**
 * Explorer scope (Changed vs All files) lives under its own key rather than in
 * Session: the session is only written once a tab is open, and the scope is a
 * choice the user can make with nothing open at all.
 */
const SCOPE_KEY = 'recur.editorScope.v1'

function scopeKey(projectId: string, workspaceId: string): string {
  return `${projectId || '_'}::${workspaceId || '_'}`
}

/** null = never chosen for this root; fall back to the default for the view. */
export function loadExplorerScope(projectId: string, workspaceId: string): boolean | null {
  try {
    const raw = localStorage.getItem(SCOPE_KEY)
    if (!raw) return null
    const all = JSON.parse(raw)
    const v = all?.[scopeKey(projectId, workspaceId)]
    return typeof v === 'boolean' ? v : null
  } catch {
    return null
  }
}

export function saveExplorerScope(projectId: string, workspaceId: string, browseAll: boolean): void {
  try {
    const raw = localStorage.getItem(SCOPE_KEY)
    const all = raw ? (JSON.parse(raw) ?? {}) : {}
    all[scopeKey(projectId, workspaceId)] = browseAll
    localStorage.setItem(SCOPE_KEY, JSON.stringify(all))
  } catch {
    // quota / private mode — ignore
  }
}

function dropExplorerScope(workspaceId: string): void {
  try {
    const raw = localStorage.getItem(SCOPE_KEY)
    if (!raw) return
    const all = JSON.parse(raw)
    if (!all || typeof all !== 'object') return
    let changed = false
    for (const k of Object.keys(all)) {
      if (workspaceIdFromStorageKey(k) === workspaceId) {
        delete all[k]
        changed = true
      }
    }
    if (!changed) return
    if (Object.keys(all).length) localStorage.setItem(SCOPE_KEY, JSON.stringify(all))
    else localStorage.removeItem(SCOPE_KEY)
  } catch { /* ignore */ }
}

// ---- pruning helpers ----

/** Keep only scratch docs still referenced by open tabs; truncate huge buffers. */
export function pruneScratchDocs(
  docs: Record<string, ScratchDoc>,
  keepPaths: Iterable<string>,
): Record<string, ScratchDoc> {
  const keep = new Set(keepPaths)
  const out: Record<string, ScratchDoc> = {}
  let n = 0
  for (const path of keep) {
    if (!path.startsWith('scratch://')) continue
    const doc = docs[path]
    if (!doc) continue
    out[path] = {
      title: doc.title || 'Scratch',
      lang: doc.lang || 'txt',
      content: doc.content.length > MAX_SCRATCH_CHARS
        ? doc.content.slice(0, MAX_SCRATCH_CHARS)
        : doc.content,
    }
    if (++n >= MAX_SCRATCH_DOCS) break
  }
  return out
}

/** Highest `scratch://N-…` sequence seen in paths (for collision-free new tabs). */
export function maxScratchSeq(paths: Iterable<string>): number {
  let max = 0
  for (const p of paths) {
    const m = /^scratch:\/\/(\d+)-/.exec(p)
    if (m) max = Math.max(max, Number(m[1]) || 0)
  }
  return max
}

/** Keep dirty disk buffers for still-open tabs; skip scratch, clean, and oversized. */
export function pruneStashedBuffers(
  buffers: Record<string, StashedBuffer> | undefined,
  keepPaths: Iterable<string>,
): Record<string, StashedBuffer> | undefined {
  if (!buffers) return undefined
  const keep = new Set(keepPaths)
  const out: Record<string, StashedBuffer> = {}
  let n = 0
  for (const path of keep) {
    if (!path || path.startsWith('scratch://')) continue
    const buf = buffers[path]
    if (!buf || typeof buf.draft !== 'string') continue
    if (buf.draft.length > MAX_UNSAVED_CHARS) continue
    if (typeof buf.saved === 'string') {
      if (buf.saved.length > MAX_UNSAVED_CHARS) continue
      if (editorTextEq(buf.draft, buf.saved)) continue
    } else if (typeof buf.baseHash !== 'string' || !buf.baseHash) {
      continue
    }
    out[path] = buf
    if (++n >= MAX_UNSAVED_BUFFERS) break
  }
  return Object.keys(out).length ? out : undefined
}

function pruneSessionFileViews(
  views: Record<string, FileViewState> | undefined,
  keepPaths: Iterable<string>,
): Record<string, FileViewState> | undefined {
  const out = pruneFileViews(parseFileViews(views), keepPaths)
  return Object.keys(out).length ? out : undefined
}
