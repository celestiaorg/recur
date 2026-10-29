/** GitHub permalink for a PR conversation row. Prefers the URL GitHub sent. */
export function commentPermalink(
  prURL: string | undefined,
  c: { id?: string; kind?: string; url?: string } | undefined,
): string {
  const stored = c?.url?.trim() ?? ''
  if (stored) return stored
  const base = (prURL ?? '').replace(/#.*$/, '').replace(/\/+$/, '').trim()
  if (!base || !c) return ''
  if (c.kind === 'description' || c.id === 'gh-description') return base
  const id = c.id ?? ''
  const node = id.replace(/^gh-(?:inline|comment|review-action|review)-/, '')
  const db = githubNodeDatabaseID(node)
  if (!db) return ''
  if (id.startsWith('gh-inline-')) return `${base}#discussion_r${db}`
  if (id.startsWith('gh-comment-')) return `${base}#issuecomment-${db}`
  if (id.startsWith('gh-review-')) return `${base}#pullrequestreview-${db}`
  return ''
}

function githubRepoUrl(prURL: string | undefined): string {
  const raw = (prURL ?? '').replace(/#.*$/, '').replace(/\/+$/, '').trim()
  if (!raw) return ''
  try {
    const u = new URL(raw)
    const parts = u.pathname.split('/').filter(Boolean)
    if (parts.length < 2) return ''
    return `${u.origin}/${parts[0]}/${parts[1]}`
  } catch {
    return ''
  }
}

/** Commit page for a SHA on the same repo as this pull request. */
export function githubCommitUrl(prURL: string | undefined, sha: string): string {
  const oid = sha.trim()
  const repo = oid ? githubRepoUrl(prURL) : ''
  return repo ? `${repo}/commit/${oid}` : ''
}

/** Database id from a numeric REST id or a GraphQL node id (PRR_ / IC_ / …). */
export function githubNodeDatabaseID(id: string): string {
  const rawId = id.trim()
  if (!rawId) return ''
  if (/^\d+$/.test(rawId)) return rawId
  const split = rawId.indexOf('_')
  const prefix = split > 0 ? rawId.slice(0, split) : ''
  if (prefix && /^[A-Z]{1,8}$/.test(prefix)) {
    const bytes = decodeB64(rawId.slice(split + 1))
    if (!bytes || bytes.length < 5) return ''
    let n = 0n
    for (let i = 4; i < bytes.length; i++) n = (n << 8n) | BigInt(bytes[i])
    return n > 0n ? n.toString() : ''
  }
  const bytes = decodeB64(rawId)
  return bytes ? githubLegacyNodeID(bytes) : ''
}

function decodeB64(s: string): Uint8Array | null {
  const pad = s.length % 4 === 0 ? s : s + '='.repeat(4 - (s.length % 4))
  try {
    const bin = atob(pad.replace(/-/g, '+').replace(/_/g, '/'))
    const out = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
    return out
  } catch {
    return null
  }
}

function githubLegacyNodeID(bytes: Uint8Array): string {
  let text = ''
  for (const b of bytes) {
    if (b < 32 || b > 126) return ''
    text += String.fromCharCode(b)
  }
  const i = text.indexOf(':')
  if (i < 0) return ''
  const rest = text.slice(i + 1)
  const m = rest.match(/^([A-Za-z]+)(\d+)$/)
  return m ? m[2] : ''
}
