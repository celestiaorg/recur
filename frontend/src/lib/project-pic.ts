import { projectColors, projectPicFromName } from './colors'

export const LUCIDE_PIC_PREFIX = 'lucide:'
export const DEFAULT_LUCIDE_PIC = `${LUCIDE_PIC_PREFIX}box`

export type ProjectPicKind = 'letter' | 'icon' | 'image'

export type ParsedProjectPic =
  | { kind: 'letter'; text: string }
  | { kind: 'icon'; name: string }
  | { kind: 'image'; src: string }

export function parseProjectPic(pic: string, fallback = '?'): ParsedProjectPic {
  const t = pic.trim()
  if (t.startsWith('data:image/')) return { kind: 'image', src: t }
  if (t.toLowerCase().startsWith(LUCIDE_PIC_PREFIX)) {
    const name = t.slice(LUCIDE_PIC_PREFIX.length).trim().toLowerCase()
    if (name) return { kind: 'icon', name }
  }
  if (t) {
    const chars = [...t]
    return { kind: 'letter', text: chars.slice(0, 2).join('') }
  }
  return { kind: 'letter', text: fallback }
}

export function formatLucidePic(name: string): string {
  return `${LUCIDE_PIC_PREFIX}${name.trim().toLowerCase()}`
}

export function isLetterPic(pic: string): boolean {
  return parseProjectPic(pic).kind === 'letter'
}

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

const PIC_SIZE = 128

/** Cover-fit a picked image into a small PNG data URL for `project.pic`. */
export async function picFromImageFile(file: File): Promise<string> {
  if (!file.type.startsWith('image/')) throw new Error('Not an image')
  const url = URL.createObjectURL(file)
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const el = new Image()
      el.onload = () => resolve(el)
      el.onerror = () => reject(new Error('Could not read image'))
      el.src = url
    })
    const canvas = document.createElement('canvas')
    canvas.width = PIC_SIZE
    canvas.height = PIC_SIZE
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('Could not draw image')
    const scale = Math.max(PIC_SIZE / Math.max(img.width, 1), PIC_SIZE / Math.max(img.height, 1))
    const w = img.width * scale
    const h = img.height * scale
    ctx.drawImage(img, (PIC_SIZE - w) / 2, (PIC_SIZE - h) / 2, w, h)
    return canvas.toDataURL('image/png')
  } finally {
    URL.revokeObjectURL(url)
  }
}
