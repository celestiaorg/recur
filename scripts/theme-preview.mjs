/**
 * Settings / Appearance swatch colours from a theme JSON.
 * Prefers the earliest exact (or prefix) scope rule over last-wins substring.
 */

const PREVIEW_MIN_ALPHA = 32 / 255
const PREVIEW_MIN_CONTRAST = 1.6

export function asHex(c, fallback) {
  if (typeof c !== 'string') return fallback
  const s = c.trim()
  if (/^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(s)) return s
  return fallback
}

function hexRgb(c) {
  let s = String(c).replace(/^#/, '').toLowerCase()
  if (s.length === 3 || s.length === 4) {
    s = `${s[0]}${s[0]}${s[1]}${s[1]}${s[2]}${s[2]}${s[3] ? s[3] + s[3] : ''}`
  }
  if (s.length !== 6 && s.length !== 8) return null
  return {
    r: parseInt(s.slice(0, 2), 16),
    g: parseInt(s.slice(2, 4), 16),
    b: parseInt(s.slice(4, 6), 16),
  }
}

function cssColorAlpha(c) {
  const s = String(c).replace(/^#/, '')
  if (s.length === 8) return parseInt(s.slice(6), 16) / 255
  if (s.length === 4) return parseInt(s[3] + s[3], 16) / 255
  return 1
}

function srgbLuma({ r, g, b }) {
  const lin = (n) => {
    const x = n / 255
    return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

function contrastRatio(fg, bg) {
  const a = hexRgb(fg)
  const b = hexRgb(bg)
  if (!a || !b) return 99
  const l1 = srgbLuma(a)
  const l2 = srgbLuma(b)
  const hi = Math.max(l1, l2)
  const lo = Math.min(l1, l2)
  return (hi + 0.05) / (lo + 0.05)
}

function fallbackFg(bg) {
  const rgb = hexRgb(bg)
  if (!rgb) return '#cccccc'
  return srgbLuma(rgb) < 0.5 ? '#cccccc' : '#333333'
}

function swatchOk(color, bg) {
  if (!color) return false
  if (cssColorAlpha(color) < PREVIEW_MIN_ALPHA) return false
  return contrastRatio(color, bg) >= PREVIEW_MIN_CONTRAST
}

function scopeSelectors(scope) {
  if (!scope) return []
  const parts = Array.isArray(scope) ? scope : [scope]
  const out = []
  for (const s of parts) {
    if (typeof s !== 'string') continue
    for (const piece of s.split(',')) {
      const t = piece.trim().toLowerCase()
      if (t) out.push(t)
    }
  }
  return out
}

/** Higher is better. Exact selector wins; trailing-scope substring matches are weak. */
function rankAgainst(selectors, want) {
  let best = 0
  for (const sel of selectors) {
    if (sel === want) return 100
    if (sel.startsWith(`${want}.`)) best = Math.max(best, 70)
    const last = sel.split(/\s+/).pop()
    if (last === want) best = Math.max(best, 25)
    else if (last.startsWith(`${want}.`)) best = Math.max(best, 15)
  }
  return best
}

function pickToken(entries, wants, bg, fallback) {
  let best = { rank: 0, color: '' }
  for (const entry of entries) {
    const fgTok = asHex(entry?.settings?.foreground, '')
    if (!fgTok || !swatchOk(fgTok, bg)) continue
    const sels = scopeSelectors(entry.scope)
    for (const want of wants) {
      const rank = rankAgainst(sels, want)
      if (rank > best.rank) best = { rank, color: fgTok }
    }
  }
  return best.color || fallback
}

export function previewFromTheme(json) {
  const colors = json.colors ?? {}
  const bg = asHex(colors['editor.background'], '#1e1e1e')
  const fg = asHex(colors['editor.foreground'], '')
  const entries = json.tokenColors ?? []
  const fgOk = fg && swatchOk(fg, bg) ? fg : fallbackFg(bg)
  const cursor = asHex(colors['editorCursor.foreground'], '')
  const fnFallback = cursor && swatchOk(cursor, bg) ? cursor : fgOk
  return {
    bg,
    fg: fgOk,
    fn: pickToken(entries, ['keyword', 'storage'], bg, fnFallback),
    name: pickToken(entries, ['entity.name.function', 'support.function'], bg, fnFallback),
    string: pickToken(entries, ['string'], bg, fgOk),
  }
}
