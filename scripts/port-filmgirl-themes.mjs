#!/usr/bin/env node
/**
 * Convert classic TextMate plists from filmgirl/TextMate-Themes into Recur
 * JSON ports. Skips mods/remixes/customs and a few duplicate/junk files.
 *
 *   node scripts/port-filmgirl-themes.mjs [dir]
 *
 * Default dir: ~/Downloads/TextMate-Themes-master
 * Writes bundles/Themes/themes/*.tmTheme.json and filmgirl-local.json.
 * Rebuild the Settings manifest (no network): node scripts/fetch-tm-themes.mjs --local-only
 */
import { readdirSync, writeFileSync, mkdirSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { convertTmThemeFile } from './convert-tmtheme.mjs'
import { previewFromTheme } from './theme-preview.mjs'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const THEME_DIR = join(ROOT, 'bundles', 'Themes', 'themes')
const MANIFEST = join(ROOT, 'bundles', 'Themes', 'filmgirl-local.json')
const DEFAULT_SRC = join(homedir(), 'Downloads', 'TextMate-Themes-master')

/** Keep existing Settings order for themes Recur already shipped. */
const ORDER_PIN = {
  'cool-glow': 30,
  cobalt: 60,
  'vibrant-ink': 61,
  sunburst: 63,
  'idle-fingers': 64,
  'ir-black': 65,
  railscasts: 66,
  'pastels-on-dark': 67,
  monokai: 68,
  'monokai-dark': 69,
}

/** Not filmgirl’s copy: Recur already has a better source. */
const SKIP_FILES = new Set([
  'Birds of Paradise.tmTheme',
  'Made of Code.tmTheme',
  'Tomorrow Night.tmTheme', // duplicate of Tomorrow-Night.tmTheme
])

const SKIP_RE =
  /mod\s*\d*|remix|custom|boost|chopped|screwed|menlo|philtr|seangaffney|sweyla|^fake\.|^barf\.|inkdeep|lighter comments|blackboard new|twilight bright|vibrant fin|vibrant tango|upstream vibrant|boys & girls|zachstronaut|forlatex|multimarkdown|heroku ?code ?samples|daniel fischer|ilife/i

/** Stable ids that camelCase splitting would mangle. */
const SLUG_OVERRIDE = {
  'GitHub.tmTheme': 'github',
  'MagicWB (Amiga).tmTheme': 'magicwb-amiga',
  'iPlastic.tmTheme': 'iplastic',
}

function slugFromPlist(filename) {
  if (SLUG_OVERRIDE[filename]) return SLUG_OVERRIDE[filename]
  return filename
    .replace(/\.tmTheme$/i, '')
    .replace(/^\s*\[\s*|\s*\]\s*$/g, '')
    .replace(/[()']/g, '')
    .replace(/&/g, 'and')
    .replace(/([a-z]{2,})([A-Z])/g, '$1-$2')
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .toLowerCase()
}

function githubBlob(filename) {
  return `https://github.com/filmgirl/TextMate-Themes/blob/master/${filename.replace(/ /g, '%20')}`
}

function shouldSkip(filename) {
  if (SKIP_FILES.has(filename)) return true
  return SKIP_RE.test(filename.replace(/_/g, ' '))
}

function main() {
  const srcDir = process.argv[2] || DEFAULT_SRC
  const files = readdirSync(srcDir).filter((n) => n.endsWith('.tmTheme')).sort()
  mkdirSync(THEME_DIR, { recursive: true })

  const converted = []
  const seenIds = new Set()
  let skipped = 0
  for (const filename of files) {
    if (shouldSkip(filename)) {
      skipped++
      continue
    }
    const id = slugFromPlist(filename)
    if (seenIds.has(id)) {
      throw new Error(`slug collision: ${id} ← ${filename} (add SKIP_FILES or SLUG_OVERRIDE)`)
    }
    seenIds.add(id)
    const json = convertTmThemeFile(join(srcDir, filename))
    const file = `${id}.tmTheme.json`
    writeFileSync(join(THEME_DIR, file), JSON.stringify(json, null, 2) + '\n')
    converted.push({
      id,
      name: json.name || id,
      file,
      plist: filename,
      uiTheme: json.type === 'light' ? 'light' : 'dark',
      preview: previewFromTheme(json),
      source: githubBlob(filename),
    })
    console.log('ok', id, '←', filename)
  }

  converted.sort((a, b) => {
    const pa = ORDER_PIN[a.id] ?? 1000
    const pb = ORDER_PIN[b.id] ?? 1000
    if (pa !== pb) return pa - pb
    return a.name.localeCompare(b.name)
  })
  let next = 70
  for (const t of converted) {
    t.order = ORDER_PIN[t.id] ?? next++
  }
  converted.sort((a, b) => a.order - b.order)

  writeFileSync(MANIFEST, JSON.stringify(converted, null, 2) + '\n')
  console.log(`wrote ${converted.length} ports (${skipped} skipped) → ${MANIFEST}`)
}

main()
