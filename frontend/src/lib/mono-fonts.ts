// Discover installed monospace fonts for the editor font picker.
//
// Strategy (union):
//  1. Native Go list (SettingsService.ListMonospaceFonts) — authoritative on
//     desktop; the webview cannot reliably see ~/Library/Fonts via canvas.
//  2. Local Font Access API + canvas probe — extras for browser/e2e.
//  3. Always include bundled webfonts (JetBrains / Plex / Source / System).

import { SettingsService } from '../../bindings/recur/internal/app'

export type MonoFont = {
  /** CSS font-family name (stored in prefs). */
  family: string
  label: string
}

/** Always available (loaded via index.html Google Fonts + generic stack). */
export const BUNDLED_MONO_FONTS: MonoFont[] = [
  { family: 'JetBrains Mono', label: 'JetBrains Mono' },
  { family: 'IBM Plex Mono', label: 'IBM Plex Mono' },
  { family: 'Source Code Pro', label: 'Source Code Pro' },
  { family: 'System Mono', label: 'System Mono' },
]

/** Special stack for the System Mono option (not a real family name). */
export const SYSTEM_MONO_STACK =
  "ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, monospace"

/** Legacy prefs keys → family names. */
export const LEGACY_FONT_FACE_KEYS: Record<string, string> = {
  jetbrains: 'JetBrains Mono',
  plex: 'IBM Plex Mono',
  source: 'Source Code Pro',
  system: 'System Mono',
}

/** Common mono / nerd-font family names for the permission-free probe path. */
const MONO_CANDIDATES = [
  'JetBrains Mono',
  'JetBrainsMono Nerd Font',
  'JetBrainsMono NF',
  'IBM Plex Mono',
  'Source Code Pro',
  'SauceCodePro Nerd Font',
  'Fira Code',
  'FiraCode Nerd Font',
  'Fira Mono',
  'Cascadia Code',
  'Cascadia Mono',
  'CaskaydiaCove Nerd Font',
  'Hack',
  'Hack Nerd Font',
  'MesloLGS NF',
  'MesloLGL Nerd Font',
  'Meslo LG M',
  'Menlo',
  'Monaco',
  'SF Mono',
  'Consolas',
  'Courier New',
  'Courier',
  'Andale Mono',
  'Roboto Mono',
  'Ubuntu Mono',
  'DejaVu Sans Mono',
  'Liberation Mono',
  'Inconsolata',
  'Inconsolata LGC',
  'Anonymous Pro',
  'Victor Mono',
  'Iosevka',
  'Iosevka Term',
  'Iosevka Nerd Font',
  'Input Mono',
  'Operator Mono',
  'Berkeley Mono',
  'Comic Code',
  'Commit Mono',
  'Geist Mono',
  'MD IO',
  'Sudo',
  'PT Mono',
  'Noto Sans Mono',
  'Droid Sans Mono',
  'Lucida Console',
  'OCR A Extended',
]

const BASELINES = ['monospace', 'serif', 'sans-serif'] as const
const PROBE = 'mmmmmmmmmmlliWQ0Ogq{}[]#@'
const SIZE = 72
/** Allow sub-pixel / hinting drift across a 4-glyph run. */
const MONO_EPS = 2

type FontDataLike = {
  family: string
  fullName?: string
  style?: string
}

function cssFamily(name: string): string {
  return `'${name.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`
}

function measure(ctx: CanvasRenderingContext2D, text: string): number {
  return ctx.measureText(text).width
}

/** True when the named family is installed (differs from at least one generic baseline). */
function isInstalled(ctx: CanvasRenderingContext2D, family: string): boolean {
  if (family === 'System Mono') return true
  for (const b of BASELINES) {
    ctx.font = `${SIZE}px ${b}`
    const base = measure(ctx, PROBE)
    ctx.font = `${SIZE}px ${cssFamily(family)}, ${b}`
    if (measure(ctx, PROBE) !== base) return true
  }
  return false
}

/**
 * True when advance widths of narrow vs wide glyphs match — the practical
 * definition of monospace for UI purposes.
 */
function isMonospace(ctx: CanvasRenderingContext2D, family: string): boolean {
  if (family === 'System Mono') return true
  // serif fallback: a missing face fails the equal-width check.
  ctx.font = `${SIZE}px ${cssFamily(family)}, serif`
  const wi = measure(ctx, 'iiii')
  const wM = measure(ctx, 'MMMM')
  return Math.abs(wi - wM) < MONO_EPS
}

/** Name looks like a coding/mono face — keep specific; bare "code"/"fira" match proportional faces. */
function looksMonospaceName(family: string): boolean {
  return /mono|consolas|inconsolata|menlo|monaco|iosevka|meslo|jetbrains|hack|fira\s*(code|mono)|cascadia\s*(code|mono)|source\s*code|courier|anonymous\s*pro|nerd\s*font|\bnf\b/i.test(
    family,
  )
}

function uniqueFamilies(names: string[]): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const n of names) {
    const t = n.trim()
    if (!t) continue
    const key = t.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    out.push(t)
  }
  return out
}

/**
 * Returns family names from Local Font Access, or null if unavailable / denied.
 * Empty array means "API worked but returned nothing" — caller should still probe.
 */
async function familiesFromLocalFonts(): Promise<string[] | null> {
  const w = window as Window & {
    queryLocalFonts?: (opts?: { postscriptNames?: string[] }) => Promise<FontDataLike[]>
  }
  if (typeof w.queryLocalFonts !== 'function') return null
  try {
    const fonts = await w.queryLocalFonts()
    // Prefer Regular/plain styles; still keep every distinct family.
    const regularish = fonts.filter((f) => {
      const s = (f.style ?? 'Regular').toLowerCase()
      return s === 'regular' || s === 'normal' || s === 'book' || s === 'roman' || s === ''
    })
    const pool = regularish.length ? regularish : fonts
    return uniqueFamilies(pool.map((f) => f.family))
  } catch {
    // Permission denied or unsupported in this embed — fall back to probe only.
    return null
  }
}

function keepMono(ctx: CanvasRenderingContext2D, family: string, fromOsInventory: boolean): boolean {
  if (family === 'System Mono') return true
  if (fromOsInventory) {
    // OS already listed it; accept obvious mono names, else require metrics.
    return looksMonospaceName(family) || isMonospace(ctx, family)
  }
  return isInstalled(ctx, family) && (looksMonospaceName(family) || isMonospace(ctx, family))
}

function familiesFromProbe(): string[] {
  const canvas = document.createElement('canvas')
  const ctx = canvas.getContext('2d')
  if (!ctx) return []
  return MONO_CANDIDATES.filter((name) => keepMono(ctx, name, false))
}

function filterMonospace(families: string[], fromOsInventory: boolean): string[] {
  const canvas = document.createElement('canvas')
  const ctx = canvas.getContext('2d')
  if (!ctx) return families
  return families.filter((name) => keepMono(ctx, name, fromOsInventory))
}

/** CSS font-family stack for a stored prefs value. */
export function monoFontStack(family: string): string {
  const resolved = LEGACY_FONT_FACE_KEYS[family] ?? family
  if (resolved === 'System Mono') return SYSTEM_MONO_STACK
  return `${cssFamily(resolved)}, ${SYSTEM_MONO_STACK}`
}

async function familiesFromNative(): Promise<string[]> {
  try {
    const list = await SettingsService.ListMonospaceFonts()
    return Array.isArray(list) ? list.filter((s): s is string => typeof s === 'string' && !!s.trim()) : []
  } catch {
    return []
  }
}

/**
 * List monospace fonts for the settings picker.
 * Order: bundled → other installed (A–Z).
 */
export async function listMonospaceFonts(): Promise<MonoFont[]> {
  const [native, local] = await Promise.all([familiesFromNative(), familiesFromLocalFonts()])
  const fromLocal = local && local.length > 0 ? filterMonospace(local, true) : []
  const fromProbe = familiesFromProbe()
  const discovered = uniqueFamilies([...native, ...fromLocal, ...fromProbe])

  const bundledKeys = new Set(BUNDLED_MONO_FONTS.map((f) => f.family.toLowerCase()))
  const extras = discovered
    .filter((f) => !bundledKeys.has(f.toLowerCase()))
    .sort((a, b) => a.localeCompare(b))
    .map((family) => ({ family, label: family }))

  // Keep bundled entries even when not "installed" as system fonts (webfonts).
  return [...BUNDLED_MONO_FONTS, ...extras]
}
