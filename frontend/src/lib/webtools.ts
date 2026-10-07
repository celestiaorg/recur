// Display helpers for WebFetch / WebSearch chat cards.
// Keep these pure so the "no JSON dump / friendly URL" contracts stay testable
// without mounting AgentChat.

function asObject(raw: unknown): Record<string, unknown> | null {
  if (raw && typeof raw === 'object') return raw as Record<string, unknown>
  return null
}

function str(raw: unknown, key: string): string {
  const v = asObject(raw)?.[key]
  return typeof v === 'string' ? v.trim() : ''
}

function parseFallback(fallback?: string): Record<string, unknown> | null {
  const fb = (fallback ?? '').trim()
  if (!fb.startsWith('{')) return null
  try {
    const v = JSON.parse(fb)
    return asObject(v)
  } catch {
    return null
  }
}

/** Absolute http(s) URL from WebFetch args, else a bare-URL fallback (feed summary). */
export function webFetchUrl(raw: unknown, fallback?: string): string {
  const fromArgs = str(raw, 'url') || str(parseFallback(fallback), 'url')
  if (fromArgs) return fromArgs
  const fb = (fallback ?? '').trim()
  if (/^https?:\/\//i.test(fb) && !fb.startsWith('{')) return fb
  return ''
}

/** Claude WebFetch extraction prompt — optional, never dumped as JSON. */
export function webFetchPrompt(raw: unknown, fallback?: string): string {
  return str(raw, 'prompt') || str(parseFallback(fallback), 'prompt')
}

/** WebSearch query. Claude uses `query`; some providers send `search_term`. */
export function webSearchQuery(raw: unknown, fallback?: string): string {
  const parsed = parseFallback(fallback)
  const q = str(raw, 'query') || str(raw, 'search_term')
    || str(parsed, 'query') || str(parsed, 'search_term')
  if (q) return q
  const fb = (fallback ?? '').trim()
  if (fb && !fb.startsWith('{')) return fb
  return ''
}

/**
 * Chip label: host + path, no scheme. Long paths keep host and the last
 * segment (`docs.example.com/…/create-token`) so the row stays readable.
 */
export function compactUrl(url: string): string {
  const raw = url.trim()
  if (!raw) return ''
  try {
    const u = new URL(raw)
    if (u.protocol !== 'http:' && u.protocol !== 'https:') return raw
    const host = u.host.replace(/^www\./i, '')
    const parts = u.pathname.replace(/\/+$/, '').split('/').filter(Boolean)
    const tail = `${u.search}${u.hash}`
    if (parts.length === 0) return host + tail
    if (parts.length <= 2) return `${host}/${parts.join('/')}${tail}`
    return `${host}/…/${parts[parts.length - 1]}${tail}`
  } catch {
    return raw
  }
}

/**
 * True only when `text` is a JSON object or array. A first-byte sniff of
 * `[`/`{` is not enough: fetched READMEs often start with a badge link
 * (`[![CI](…)](…)`), which must stay on the markdown path.
 */
export function webResultIsJson(text: string): boolean {
  const t = text.trim()
  if (!t || (t[0] !== '{' && t[0] !== '[')) return false
  try {
    JSON.parse(t)
    return true
  } catch {
    return false
  }
}

