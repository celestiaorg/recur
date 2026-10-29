import { projectColors, projectPicFromName } from './colors'

export function letterFromName(name: string, fallback = '?'): string {
  return projectPicFromName(name) || fallback
}

export function isProjectPresetColor(color: string): boolean {
  return (projectColors as readonly string[]).includes(color)
}

const HEX6 = /^#([0-9a-f]{6})$/i
const HEX3 = /^#([0-9a-f]{3})$/i

/** Normalize `#rgb` / `#rrggbb` to lowercase `#rrggbb`, or '' if not hex. */
export function parseHexColor(color: string): string {
  const t = color.trim()
  const m6 = HEX6.exec(t)
  if (m6) return `#${m6[1]!.toLowerCase()}`
  const m3 = HEX3.exec(t)
  if (!m3) return ''
  const [r, g, b] = [...m3[1]!]
  return `#${r}${r}${g}${g}${b}${b}`.toLowerCase()
}

/** Resolve a CSS color (hex, `var(--cyan)`, `rgb()`) to `#rrggbb`. */
export function colorToHex(color: string): string {
  const hex = parseHexColor(color)
  if (hex) return hex
  if (typeof document === 'undefined') return ''
  const el = document.createElement('span')
  el.style.color = color
  document.body.append(el)
  const rgb = getComputedStyle(el).color
  el.remove()
  const m = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/i.exec(rgb)
  if (!m) return ''
  const parts = [m[1], m[2], m[3]].map((n) => Number(n).toString(16).padStart(2, '0'))
  return `#${parts.join('')}`
}

const DARK_FG = '#0A0C10'
const LIGHT_FG = '#F4F1EA'

function srgbLin(c: number): number {
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
}

/** Letter/icon stroke. Preset swatches always use dark ink; custom hex
 *  flips to light only when the fill is truly dark (L ≤ 0.12). */
export function fgOnBackground(color: string): string {
  if (!color || isProjectPresetColor(color)) return DARK_FG
  const hex = parseHexColor(color) || colorToHex(color)
  if (!hex) return DARK_FG
  const n = parseInt(hex.slice(1), 16)
  const r = srgbLin(((n >> 16) & 255) / 255)
  const g = srgbLin(((n >> 8) & 255) / 255)
  const b = srgbLin((n & 255) / 255)
  const L = 0.2126 * r + 0.7152 * g + 0.0722 * b
  return L > 0.12 ? DARK_FG : LIGHT_FG
}
