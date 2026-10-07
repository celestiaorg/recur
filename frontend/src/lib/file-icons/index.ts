/**
 * File-type icons for the Observatory explorer.
 *
 * Glyphs + match DB come from https://github.com/file-icons/atom (vendored under
 * ./generated/). Re-sync with: pnpm run update-file-icons
 */
import './generated/fonts.css'
import './generated/colors.css'
import './generated/icons.css'
import './dotenv-fix.css'

import { get } from 'svelte/store'
import { isLightTheme, theme } from '../stores'
import IconTables from './icon-tables'
import iconDb from './generated/icondb.js'

const db = new IconTables(iconDb)

/** Fallback when no match — text-icon is defined in icons.css (unlike Atom's icon-file-text). */
const FALLBACK = 'text-icon medium-blue'

/** Same rule as file-icons/atom dotenv-icon — applied before the linear DB scan. */
const DOTENV_RE = /^\.env(?:\.|$)/i
const DOTENV_CLASS = ['dotenv-icon medium-yellow', 'dotenv-icon dark-yellow'] as const

/**
 * CSS classes for an empty <span>; the glyph is painted on :before.
 * Pass themeKey from a reactive `$theme` read so Svelte re-renders on switch;
 * otherwise the active store value is used (index 0 = light, 1 = dark).
 */
export function fileIconClass(name: string, themeKey?: string): string {
  const key = themeKey ?? get(theme)
  const slot = isLightTheme(key) ? 0 : 1
  // Matchers are basename-oriented (`^\.env…`). Callers sometimes pass a path.
  const base = name.split(/[/\\]/).pop() || name
  if (DOTENV_RE.test(base)) return `icon ${DOTENV_CLASS[slot]}`
  const match = db.matchName(base, false)
  const cls = match ? match.getClass(slot) : FALLBACK
  return `icon ${cls}`
}
