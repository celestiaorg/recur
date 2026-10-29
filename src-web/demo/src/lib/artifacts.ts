import { formatRelativeAge } from './format'
import type { IconName } from './icons'

/** Fields the Observatory needs to open/preview an artifact. */
export type ArtifactRef = {
  id: string
  title: string
  kind: string
  path: string
}

function strField(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

/**
 * Normalize an artifact.created / artifact.presented payload.
 * Event.Emit JSON-encodes `domain.Artifact`; json tags are lowerCamel
 * (`id`/`path`/`title`/`kind`) whether the struct is top-level or nested
 * in map[string]any.
 */
export function artifactFromEvent(raw: unknown): ArtifactRef | null {
  if (!raw || typeof raw !== 'object') return null
  const o = raw as Record<string, unknown>
  const id = strField(o.id)
  const path = strField(o.path)
  if (!id || !path) return null
  return {
    id,
    title: strField(o.title) || id,
    kind: strField(o.kind).toLowerCase(),
    path,
  }
}

/** Per-workspace unseen artifact ids (badge = set size). */
export type UnseenArtifactIds = Record<string, readonly string[]>

/** Record an unseen artifact once — create+present of the same id stays one. */
export function addUnseenArtifactId(
  byWorkspace: UnseenArtifactIds,
  workspaceId: string,
  artifactId: string,
): UnseenArtifactIds {
  const wid = workspaceId.trim()
  const id = artifactId.trim()
  if (!wid || !id) return byWorkspace
  const prev = byWorkspace[wid] ?? []
  if (prev.includes(id)) return byWorkspace
  return { ...byWorkspace, [wid]: [...prev, id] }
}

export function clearUnseenArtifactIds(
  byWorkspace: UnseenArtifactIds,
  workspaceId: string,
): UnseenArtifactIds {
  const wid = workspaceId.trim()
  if (!wid || !(byWorkspace[wid]?.length)) return byWorkspace
  return { ...byWorkspace, [wid]: [] }
}

/** Icon for a domain ArtifactKind (html/svg/mermaid/…). */
export function artifactIcon(kind: string): IconName {
  switch (String(kind || '').toLowerCase()) {
    case 'mermaid': return 'sitemap'
    case 'html': return 'file-type-html'
    case 'svg': return 'file-type-svg'
    case 'png':
    case 'jpeg':
    case 'jpg':
    case 'gif':
    case 'webp': return 'photo'
    case 'csv': return 'file-type-csv'
    case 'json': return 'braces'
    case 'md':
    case 'markdown': return 'markdown'
    default: return 'file'
  }
}

/** One artifact as list_artifacts JSON (and the chat list). */
export type ListedArtifact = {
  id: string
  title: string
  kind: string
  path: string
  createdAt: string
  createdBy: string
}

export type ListedArtifacts = { count: number; artifacts: ListedArtifact[] }

export type ArtifactKindTally = { kind: string; count: number }

const KIND_TALLY_ORDER = ['html', 'svg', 'mermaid', 'png', 'jpeg', 'jpg', 'csv', 'json', 'md', 'markdown'] as const

function asRecord(v: unknown): Record<string, unknown> | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? v as Record<string, unknown> : null
}

function parseJSONObject(text: string): Record<string, unknown> | null {
  const t = text.trim()
  if (!t) return null
  const tryParse = (s: string): Record<string, unknown> | null => {
    try {
      return asRecord(JSON.parse(s))
    } catch {
      return null
    }
  }
  const direct = tryParse(t)
  if (direct) return direct
  const i = t.indexOf('{')
  if (i > 0) return tryParse(t.slice(i))
  return null
}

export function parseListedArtifact(a: unknown): ListedArtifact {
  const o = asRecord(a) ?? {}
  return {
    id: typeof o.id === 'string' ? o.id : String(o.id ?? ''),
    title: typeof o.title === 'string' ? o.title : '',
    kind: typeof o.kind === 'string' ? o.kind.trim().toLowerCase() : '',
    path: typeof o.path === 'string' ? o.path : '',
    createdAt: typeof o.createdAt === 'string' ? o.createdAt : '',
    createdBy: typeof o.createdBy === 'string' ? o.createdBy : '',
  }
}

/**
 * Parse the list_artifacts tool result. Accepts a leading prose prefix so a
 * wrapped MCP payload still yields rows instead of a minified JSON dump.
 * A Go nil slice marshals as `"artifacts":null` — treat that as [].
 */
export function parseListArtifactsResult(text: string | undefined | null): ListedArtifacts | null {
  if (!text) return null
  const raw = parseJSONObject(text)
  if (!raw) return null
  if (Array.isArray(raw.artifacts)) {
    return {
      count: typeof raw.count === 'number' ? raw.count : raw.artifacts.length,
      artifacts: raw.artifacts.map(parseListedArtifact),
    }
  }
  if (raw.artifacts == null && ('artifacts' in raw || typeof raw.count === 'number')) {
    return { count: typeof raw.count === 'number' ? raw.count : 0, artifacts: [] }
  }
  return null
}

/**
 * Scratch editor path for an artifact tab (`scratch://artifact-<id>.<lang>`).
 * Lang is the last dotted segment so ids may themselves contain dots.
 */
export function artifactIdFromScratchPath(path: string): string {
  const m = /^scratch:\/\/artifact-(.+)\.([^.]+)$/.exec(path)
  return m?.[1] ?? ''
}

/** True when Observatory.openArtifact can load this row (id + workspaces/ path). */
export function artifactCanOpen(a: { id?: string; path?: string }): boolean {
  const id = (a.id || '').trim()
  const rel = (a.path || '').replace(/^\//, '')
  return !!id && rel.startsWith('workspaces/')
}

/** Structured list for the chat card. Unparseable / error results return null. */
export function resolveListArtifacts(
  result: string | undefined,
  resultError: boolean | undefined,
): ListedArtifacts | null {
  if (resultError) return null
  return parseListArtifactsResult(result)
}

export function artifactKindTally(artifacts: { kind?: string }[]): ArtifactKindTally[] {
  const counts = new Map<string, number>()
  for (const a of artifacts) {
    const k = (a.kind || '').toLowerCase()
    if (!k) continue
    counts.set(k, (counts.get(k) || 0) + 1)
  }
  const out: ArtifactKindTally[] = []
  for (const k of KIND_TALLY_ORDER) {
    const n = counts.get(k)
    if (n) out.push({ kind: k, count: n })
  }
  for (const [k, n] of counts) {
    if (!(KIND_TALLY_ORDER as readonly string[]).includes(k)) out.push({ kind: k, count: n })
  }
  return out
}

/** Compact age for a list row (`12m`, `3h`) plus a full tooltip stamp. */
export function artifactWhen(iso: string, now = Date.now()): { rel: string; full: string } | null {
  if (!iso.trim()) return null
  return formatRelativeAge(Date.parse(iso), now)
}
