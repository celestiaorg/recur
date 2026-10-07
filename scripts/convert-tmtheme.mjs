#!/usr/bin/env node
/**
 * Convert a classic TextMate .tmTheme plist into Recur's VS Code-shaped
 * themes/<id>.tmTheme.json (colors + tokenColors). Generic RGB hex is baked
 * to sRGB and stamped `colorSpaceName: sRGB` (TextMate.app’s default space
 * is Apple Generic RGB; Recur’s webview is sRGB).
 *
 *   node scripts/convert-tmtheme.mjs <in.tmTheme> <out.tmTheme.json>
 *
 * Batch (filmgirl pack, skip mods): scripts/port-filmgirl-themes.mjs
 */
import { execFileSync } from 'node:child_process'
import { writeFileSync } from 'node:fs'
import { basename } from 'node:path'
import { fileURLToPath } from 'node:url'
import { bakeGenericRgbThemeToSrgb } from './generic-rgb-to-srgb.mjs'

const CHROME = [
  ['background', 'editor.background'],
  ['foreground', 'editor.foreground'],
  ['selection', 'editor.selectionBackground'],
  ['lineHighlight', 'editor.lineHighlightBackground'],
  ['caret', 'editorCursor.foreground'],
  ['invisibles', 'editorWhitespace.foreground'],
]

const GUTTER = [
  ['background', 'editorGutter.background'],
  ['foreground', 'editorLineNumber.foreground'],
  ['divider', 'gutter.divider'],
]

function splitScope(scope) {
  if (typeof scope !== 'string') return scope
  const parts = scope.split(',').map((s) => s.trim()).filter(Boolean)
  if (parts.length === 0) return undefined
  return parts.length === 1 ? parts[0] : parts
}

function cleanSettings(raw) {
  if (!raw || typeof raw !== 'object') return {}
  const out = {}
  for (const key of ['foreground', 'background', 'fontStyle']) {
    const v = raw[key]
    if (typeof v !== 'string' || v.trim() === '') continue
    out[key] = v
  }
  return out
}

function isDarkHex(c) {
  const s = (c || '').trim().toLowerCase()
  const m = s.match(/^#([0-9a-f]{6})(?:[0-9a-f]{2})?$/)
  if (!m) return true
  const n = parseInt(m[1], 16)
  const r = (n >> 16) & 255
  const g = (n >> 8) & 255
  const b = n & 255
  return 0.3 * r + 0.59 * g + 0.11 * b < 127.5
}

export function plistThemeToJson(plist) {
  const settings = Array.isArray(plist.settings) ? plist.settings : []
  const global = settings.find((s) => !s.scope)?.settings ?? {}
  const colors = {}
  for (const [from, to] of CHROME) {
    if (typeof global[from] === 'string' && global[from].trim()) colors[to] = global[from]
  }
  const gs = plist.gutterSettings
  if (gs && typeof gs === 'object') {
    for (const [from, to] of GUTTER) {
      if (typeof gs[from] === 'string' && gs[from].trim() && !colors[to]) colors[to] = gs[from]
    }
  }

  const tokenColors = []
  for (const entry of settings) {
    const scoped = Boolean(entry.scope)
    const tok = { settings: cleanSettings(entry.settings) }
    if (entry.name) tok.name = entry.name
    if (scoped) {
      const scope = splitScope(entry.scope)
      if (scope) tok.scope = scope
    }
    if (!scoped && Object.keys(tok.settings).length === 0 && !tok.name) continue
    if (!scoped && Object.keys(tok.settings).length === 0) continue
    tokenColors.push(tok)
  }

  const bg = colors['editor.background'] || '#000000'
  const json = {
    name: plist.name || 'Untitled',
    type: isDarkHex(bg) ? 'dark' : 'light',
    colorSpaceName:
      typeof plist.colorSpaceName === 'string' && plist.colorSpaceName.trim()
        ? plist.colorSpaceName.trim()
        : 'Generic RGB',
    colors,
    tokenColors,
  }
  // Classic TextMate default is Generic RGB. Recur’s webview is sRGB.
  bakeGenericRgbThemeToSrgb(json)
  return json
}

export function convertTmThemeFile(src) {
  const jsonText = execFileSync('plutil', ['-convert', 'json', '-o', '-', src], { encoding: 'utf8' })
  return plistThemeToJson(JSON.parse(jsonText))
}

function main() {
  const [, , src, dest] = process.argv
  if (!src || !dest) {
    console.error('usage: node scripts/convert-tmtheme.mjs <in.tmTheme> <out.tmTheme.json>')
    process.exit(1)
  }
  const out = convertTmThemeFile(src)
  writeFileSync(dest, JSON.stringify(out, null, 2) + '\n')
  console.log('ok', basename(dest), '←', basename(src))
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main()
