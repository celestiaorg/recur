/**
 * User-chosen TextMate language for an open file.
 *
 * VS Code's status-bar "Change Language Mode" is session-only unless you also
 * write `files.associations`. Recur restores editor tabs, so a forgotten pick
 * would be annoying — the path override is persisted on the workspace editor
 * session (same gzip file as tabs), not localStorage. An optional file-type
 * association (`*.ext` / basename) covers the "treat every .conf as YAML" case.
 *
 * This module is the in-memory resolver. Observatory hydrates it from the
 * session on restore and persists via `setSyntaxModePersist` after a pick.
 * Paths are workspace-relative, so the maps must not leak across worktrees.
 */
import { writable } from 'svelte/store'

/** Empty string = explicit Plain Text (no grammar). Missing key = auto-detect. */
export const SYNTAX_PLAIN = ''

let byPath: Record<string, string> = {}
let byFileType: Record<string, string> = {}
let persistHook: (() => void) | null = null

/** Bumps when overrides change so the status bar and editors re-resolve. */
export const syntaxModeEpoch = writable(0)

export type FileTypeAssociation = { key: string; label: string }

export type SyntaxModeMaps = {
  syntaxByPath: Record<string, string>
  syntaxByFileType: Record<string, string>
}

export function normalizeSyntaxPath(path: string): string {
  return path.replace(/\\/g, '/').replace(/^\.\//, '')
}

/**
 * Association key for "always open files like this as…".
 * Dotfiles and extensionless names use the basename; everyone else `*.ext`.
 */
export function fileTypeAssociationForPath(path: string): FileTypeAssociation | null {
  if (path.startsWith('scratch://')) return null
  const base = (path.split(/[/\\]/).pop() ?? path).trim()
  if (!base) return null
  const lower = base.toLowerCase()
  if (lower.startsWith('.')) return { key: lower, label: base }
  const dot = lower.lastIndexOf('.')
  if (dot > 0) {
    const ext = lower.slice(dot + 1)
    if (ext) return { key: `*.${ext}`, label: `*.${ext}` }
  }
  return { key: lower, label: base }
}

function hasKey(map: Record<string, string>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(map, key)
}

/**
 * Override for `path` if the user picked one.
 * `undefined` = auto-detect; `''` = Plain Text; otherwise a TextMate scope.
 */
export function syntaxOverrideForPath(path: string): string | undefined {
  const p = normalizeSyntaxPath(path)
  if (!p) return undefined
  if (hasKey(byPath, p)) return byPath[p]
  const assoc = fileTypeAssociationForPath(p)
  if (assoc && hasKey(byFileType, assoc.key)) return byFileType[assoc.key]
  return undefined
}

export function syntaxPathOverride(path: string): string | undefined {
  const p = normalizeSyntaxPath(path)
  return p && hasKey(byPath, p) ? byPath[p] : undefined
}

export function syntaxFileTypeOverride(key: string): string | undefined {
  return hasKey(byFileType, key) ? byFileType[key] : undefined
}

function bump() {
  syntaxModeEpoch.update((n) => n + 1)
  persistHook?.()
}

export function parseSyntaxMap(raw: unknown): Record<string, string> {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const out: Record<string, string> = {}
  for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
    if (k && typeof v === 'string') out[k] = v
  }
  return out
}

function mapsEqual(a: Record<string, string>, b: Record<string, string>): boolean {
  const keys = Object.keys(a)
  if (keys.length !== Object.keys(b).length) return false
  for (const k of keys) {
    if (a[k] !== b[k]) return false
  }
  return true
}

/** Copy of the current maps for session write / keep-alive snapshot. */
export function syntaxModeSnapshot(): SyntaxModeMaps {
  return pruneSyntaxMaps({ syntaxByPath: { ...byPath }, syntaxByFileType: { ...byFileType } })
}

export function syntaxMapsNonEmpty(maps: SyntaxModeMaps = syntaxModeSnapshot()): boolean {
  return Object.keys(maps.syntaxByPath).length > 0 || Object.keys(maps.syntaxByFileType).length > 0
}

/**
 * Replace in-memory maps from a workspace session. Does not persist — the
 * caller just loaded these from disk (or is clearing on a workspace switch).
 * `mergeLive` keeps any picks made after a cold empty hydrate so a file
 * opened before the session RPC lands does not lose a language choice.
 */
export function hydrateSyntaxMode(
  byPathRaw?: unknown,
  byFileTypeRaw?: unknown,
  mergeLive = false,
) {
  const nextPath = parseSyntaxMap(byPathRaw)
  const nextType = parseSyntaxMap(byFileTypeRaw)
  if (mergeLive) {
    Object.assign(nextPath, byPath)
    Object.assign(nextType, byFileType)
  }
  if (mapsEqual(byPath, nextPath) && mapsEqual(byFileType, nextType)) return
  byPath = nextPath
  byFileType = nextType
  syntaxModeEpoch.update((n) => n + 1)
}

/**
 * Observatory registers a session write here so a language pick ships with
 * the next editor-session patch. Hydrate never calls this.
 */
export function setSyntaxModePersist(fn: (() => void) | null) {
  persistHook = fn
}

/** Max persisted per-path language overrides (oldest dropped). */
export const MAX_SYNTAX_BY_PATH = 256
/** Max persisted `*.ext` / basename associations. */
export const MAX_SYNTAX_BY_FILE_TYPE = 64

function capSyntaxMap(map: Record<string, string>, max: number): Record<string, string> {
  const keys = Object.keys(map)
  if (keys.length <= max) return map
  const out: Record<string, string> = {}
  for (const k of keys.slice(keys.length - max)) out[k] = map[k]
  return out
}

/** Drop scratch:// keys and apply size caps before a session write. */
export function pruneSyntaxMaps(maps: SyntaxModeMaps): SyntaxModeMaps {
  const byPath: Record<string, string> = {}
  for (const [k, v] of Object.entries(maps.syntaxByPath)) {
    if (k.startsWith('scratch://')) continue
    byPath[k] = v
  }
  return {
    syntaxByPath: capSyntaxMap(byPath, MAX_SYNTAX_BY_PATH),
    syntaxByFileType: capSyntaxMap(maps.syntaxByFileType, MAX_SYNTAX_BY_FILE_TYPE),
  }
}

function putCapped(map: Record<string, string>, key: string, value: string, max: number) {
  if (hasKey(map, key)) delete map[key]
  map[key] = value
  const keys = Object.keys(map)
  if (keys.length > max) delete map[keys[0]]
}

/** Set or clear the per-file override (`null` = Auto Detect). */
export function setSyntaxForPath(path: string, scope: string | null) {
  const p = normalizeSyntaxPath(path)
  if (!p) return
  if (scope == null) {
    if (!hasKey(byPath, p)) return
    delete byPath[p]
  } else if (byPath[p] === scope) {
    return
  } else {
    putCapped(byPath, p, scope, MAX_SYNTAX_BY_PATH)
  }
  bump()
}

/** Persist a file-type association (`null` clears it). */
export function setSyntaxForFileType(key: string, scope: string | null) {
  const k = key.trim()
  if (!k) return
  if (scope == null) {
    if (!hasKey(byFileType, k)) return
    delete byFileType[k]
  } else if (byFileType[k] === scope) {
    return
  } else {
    putCapped(byFileType, k, scope, MAX_SYNTAX_BY_FILE_TYPE)
  }
  bump()
}

/** Reset in-memory overrides (tests). */
export function _resetSyntaxModeForTests() {
  byPath = {}
  byFileType = {}
  persistHook = null
  syntaxModeEpoch.set(0)
}
