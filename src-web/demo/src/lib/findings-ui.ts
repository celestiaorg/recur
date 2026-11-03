
import type { IconName } from './icons'

/** Finding identity fields used when matching UUID vs public short id. */
export type FindingRef = {
  id?: string
  shortId?: string
}

/** Severity → Tabler icon used in dense finding rows. */
export function findingSevIcon(severity: string | undefined): IconName {
  switch ((severity || '').toLowerCase()) {
    case 'critical': return 'sev-critical'
    case 'high': return 'sev-high'
    case 'medium': return 'sev-medium'
    case 'info': return 'sev-info'
    default: return 'sev-low'
  }
}

/** First sentence / line for the dense list title. */
export function findingTitle(msg: string): string {
  const t = (msg || '').replace(/\s+/g, ' ').trim()
  if (!t) return 'Finding'
  const cut = t.search(/[.!?](?:\s|$)/)
  const head = cut > 0 && cut <= 110 ? t.slice(0, cut + 1) : t
  return head.length > 120 ? `${head.slice(0, 117)}…` : head
}

export type FindingFlag = { count: number; severity: string }

const EMPTY_FINDING_FLAG: FindingFlag = { count: 0, severity: 'info' }

export function severityRank(s: string): number {
  return s === 'critical' ? 4 : s === 'high' ? 3 : s === 'medium' ? 2 : s === 'low' ? 1 : 0
}

/**
 * Explorer badge for a tree path. Always returns an object: Svelte compiles
 * `{#if flag}{flag.severity}` to `get(flag).severity`, which throws when the
 * flag flips to null mid-update (workspace switch clears findings while the
 * tree is rewritten).
 */
export function findingFlagForPath(
  path: string,
  isDir: boolean,
  byPath: ReadonlyMap<string, FindingFlag>,
): FindingFlag {
  if (!isDir) return byPath.get(path) ?? EMPTY_FINDING_FLAG
  const prefix = path + '/'
  let count = 0
  let severity = 'info'
  for (const [p, f] of byPath) {
    if (!p.startsWith(prefix)) continue
    count += f.count
    if (severityRank(f.severity) > severityRank(severity)) severity = f.severity
  }
  return count ? { count, severity } : EMPTY_FINDING_FLAG
}

/** True when the full message adds detail beyond the title line. */
export function findingBodyNeeded(msg: string): boolean {
  const title = findingTitle(msg)
  if (!msg?.trim() || title === 'Finding') return !!msg?.trim()
  const flat = msg.replace(/\s+/g, ' ').trim()
  return flat.length > title.length + 2 || /\n/.test(msg)
}
