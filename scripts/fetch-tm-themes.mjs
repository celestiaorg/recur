#!/usr/bin/env node
/**
 * Fetch all TextMate / VS Code themes from shikijs/tm-themes into
 * bundles/Themes/themes/ and regenerate bundle.toml [[theme]] entries.
 *
 * Preserves featured local ports (Cool Glow, Made of Code, classic TM ports).
 * Aura is fetched from daltonmenezes/aura-theme (not in Shiki). GPL-licensed
 * Shiki themes are skipped (data redistributable under MIT/Apache only for
 * the shipped pack).
 *
 * Local filmgirl ports live in filmgirl-local.json (see port-filmgirl-themes.mjs).
 * Those ids/files are never overwritten by the Shiki fetch.
 *
 *   node scripts/fetch-tm-themes.mjs              # re-fetch Shiki + Aura
 *   node scripts/fetch-tm-themes.mjs --local-only # rebuild bundle.toml from disk
 *   node scripts/fetch-tm-themes.mjs --local-only --prune  # also delete unlisted files
 */
import { mkdirSync, writeFileSync, existsSync, rmSync, readdirSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { pathToFileURL, fileURLToPath } from 'node:url'
import { previewFromTheme } from './theme-preview.mjs'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_DIR = join(ROOT, 'bundles', 'Themes')
const THEME_DIR = join(OUT_DIR, 'themes')
const FILMGIRL_MANIFEST = join(OUT_DIR, 'filmgirl-local.json')
const INDEX_URL =
  'https://raw.githubusercontent.com/shikijs/textmate-grammars-themes/main/packages/tm-themes/index.js'
const SHIKI = (name) =>
  `https://raw.githubusercontent.com/shikijs/textmate-grammars-themes/main/packages/tm-themes/themes/${name}.json`
const UPSTREAM_NOTICE =
  'https://github.com/shikijs/textmate-grammars-themes/blob/main/packages/tm-themes/NOTICE'
const AURA_URL =
  'https://raw.githubusercontent.com/daltonmenezes/aura-theme/main/packages/vscode/themes/aura-dark-color-theme.json'

function writeCompactNotice(path, themes) {
  const lines = [
    'THIRD-PARTY SOFTWARE NOTICES',
    '',
    'Themes in this pack are vendored from shikijs/textmate-grammars-themes',
    '(plus featured local TextMate ports and Aura). Full Shiki license texts:',
    UPSTREAM_NOTICE,
    '',
    'Per-theme attribution (also see provenance.json):',
    '',
  ]
  for (const t of themes) {
    const name = t.id || t.key
    const spdx = t.license || (t.source === 'local' ? 'see theme file' : 'see upstream')
    const src = t.url || (t.source?.startsWith('http') ? t.source : '') || ''
    lines.push(`${name} — ${spdx}${src ? ` — ${src}` : ''}`)
  }
  lines.push('')
  writeFileSync(path, lines.join('\n'))
}

/**
 * Non-filmgirl local ports. filmgirl-local.json is merged in loadLocal().
 * Ids / files listed here are never overwritten by the Shiki fetch.
 */
const HAND_LOCAL = [
  {
    id: 'made-of-code',
    name: 'Made of Code',
    file: 'made-of-code.tmTheme.json',
    uiTheme: 'dark',
    order: 40,
    preview: { bg: '#08091D', fg: '#F9F9F9', fn: '#F4DE62', name: '#FF5366', string: '#9DFA6B' },
    source: 'https://github.com/mkdynamic/made-of-code-tmbundle',
  },
  {
    id: 'birds-of-paradise',
    name: 'Birds of Paradise',
    file: 'birds-of-paradise.tmTheme.json',
    uiTheme: 'dark',
    order: 62,
    preview: { bg: '#473431', fg: '#EBE6CF', fn: '#F47440', name: '#F3B940', string: '#E0DC75' },
    source: 'Joe Bergantine, Birds of Paradise.tmTheme',
  },
]

const AURA = {
  id: 'aura',
  name: 'Aura',
  file: 'aura.tmTheme.json',
  order: 50,
  preview: { bg: '#15141b', fg: '#edecee', fn: '#ffca85', name: '#a277ff', string: '#61ffca' },
  url: AURA_URL,
}

const ALLOWED_LICENSE = /^(MIT|Apache-2\.0|ISC|BSD)/i

function loadFilmgirlLocal() {
  if (!existsSync(FILMGIRL_MANIFEST)) return []
  return JSON.parse(readFileSync(FILMGIRL_MANIFEST, 'utf8'))
}

function loadLocal() {
  return [...HAND_LOCAL, ...loadFilmgirlLocal()]
}

const HAND_IDS = new Set(HAND_LOCAL.map((t) => t.id))

/** Hand-kept ports keep authored swatches; everyone else is derived from the JSON. */
function localThemeEntry(t) {
  const path = join(THEME_DIR, t.file)
  if (!existsSync(path)) throw new Error(`missing local theme ${t.file}`)
  const json = JSON.parse(readFileSync(path, 'utf8'))
  const preview = HAND_IDS.has(t.id) ? t.preview : previewFromTheme(json)
  return { ...t, preview, file: t.file }
}

/** Keep filmgirl-local.json in sync so a full fetch doesn't read stale swatches. */
function refreshFilmgirlPreviews() {
  const list = loadFilmgirlLocal()
  if (!list.length) return
  for (const t of list) {
    const path = join(THEME_DIR, t.file)
    if (!existsSync(path)) throw new Error(`missing local theme ${t.file}`)
    t.preview = previewFromTheme(JSON.parse(readFileSync(path, 'utf8')))
  }
  writeFileSync(FILMGIRL_MANIFEST, JSON.stringify(list, null, 2) + '\n')
}

function prettyThemeName(name, id = '') {
  const s = String(name || id || '').trim().replace(/_/g, ' ')
  if (!s) return id
  if (/[A-Z]/.test(s) || /\s/.test(s)) return s
  if (s.includes('-')) {
    return s.split('-').filter(Boolean).map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(' ')
  }
  return s.charAt(0).toUpperCase() + s.slice(1)
}

function themeLabel(json, id, fallback) {
  const display = typeof json?.displayName === 'string' ? json.displayName.trim() : ''
  if (display) return display
  return prettyThemeName(json?.name || fallback || id, id)
}

function previewToml(p) {
  return `{ bg = "${p.bg}", fg = "${p.fg}", fn = "${p.fn}", name = "${p.name}", string = "${p.string}" }`
}

function themeEntry(t, file) {
  const ui = t.uiTheme === 'light' ? 'light' : 'dark'
  const name = prettyThemeName(t.name, t.id)
  return `[[theme]]
id = "${t.id}"
name = ${JSON.stringify(name)}
path = "themes/${file}"
uiTheme = "${ui}"
order = ${t.order}
preview = ${previewToml(t.preview)}
`
}

function writeBundle(entries, provenance) {
  const parts = [
    `[bundle]
name = "Themes"
id = "recur.themes"
version = "1.0.0"
author = "Recur"

# Editor colour themes. Charcoal/Paper live in bundles/Recur (order 10–20).
# Featured local ports: Cool Glow / Made of Code / Aura / classic TM (30–…).
# Full Shiki tm-themes catalog starts at order 1000 (alpha by fetch index).
# Refresh: wails3 task fetch:tm-themes
# Local filmgirl ports: node scripts/port-filmgirl-themes.mjs
# Theme id is the Settings preference value.
`,
  ]
  for (const t of entries) parts.push(themeEntry(t, t.file))
  writeFileSync(join(OUT_DIR, 'bundle.toml'), parts.join('\n'))
  writeFileSync(join(OUT_DIR, 'provenance.json'), JSON.stringify(provenance, null, 2) + '\n')
  writeCompactNotice(join(OUT_DIR, 'NOTICE'), provenance.themes)
}

function localProvenance(t) {
  return {
    id: t.id,
    source: t.source || 'local',
    file: t.file,
    ...(t.source ? { url: t.source } : {}),
  }
}

async function fetchText(url) {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`${url}: ${res.status}`)
  return res.text()
}

async function loadShikiIndex() {
  const src = await fetchText(INDEX_URL)
  const tmp = join(THEME_DIR, '.tm-themes-index.mjs')
  mkdirSync(THEME_DIR, { recursive: true })
  writeFileSync(tmp, src)
  try {
    const mod = await import(pathToFileURL(tmp).href + `?t=${Date.now()}`)
    return mod.themes
  } finally {
    try { rmSync(tmp) } catch { /* ignore */ }
  }
}

function rebuildLocalOnly() {
  const prune = process.argv.includes('--prune')
  refreshFilmgirlPreviews()
  const local = loadLocal()
  const keepFiles = new Set([...local.map((t) => t.file), AURA.file])
  const prevPath = join(OUT_DIR, 'provenance.json')
  const prev = existsSync(prevPath) ? JSON.parse(readFileSync(prevPath, 'utf8')) : { themes: [] }
  const prevByFile = Object.fromEntries((prev.themes || []).map((t) => [t.file, t]))

  const entries = []
  const provenance = { fetchedAt: prev.fetchedAt || new Date().toISOString(), themes: [] }

  for (const t of local) {
    entries.push(localThemeEntry(t))
    provenance.themes.push(localProvenance(t))
  }

  if (!existsSync(join(THEME_DIR, AURA.file))) {
    throw new Error(`missing ${AURA.file}`)
  }
  entries.push({ ...AURA, uiTheme: 'dark', file: AURA.file })
  provenance.themes.push({
    id: AURA.id,
    source: 'aura-theme',
    file: AURA.file,
    url: AURA.url,
  })

  entries.sort((a, b) => (a.order ?? 0) - (b.order ?? 0))

  let order = 1000
  const shikiFiles = []
  for (const name of readdirSync(THEME_DIR).filter((n) => n.endsWith('.tmTheme.json')).sort()) {
    if (keepFiles.has(name)) continue
    const json = JSON.parse(readFileSync(join(THEME_DIR, name), 'utf8'))
    const prevT = prevByFile[name] || {}
    const isShiki = Boolean(
      json.semanticHighlighting
      || json.semanticTokenColors
      || json.displayName
      || json.colors?.['activityBar.background']
      || json.colors?.['statusBar.background'],
    )
    if (!isShiki) {
      if (prune) {
        rmSync(join(THEME_DIR, name), { force: true })
        console.log('drop orphan', name)
      } else {
        console.warn(`skip unlisted ${name} (not in manifest; pass --prune to delete)`)
      }
      continue
    }
    shikiFiles.push(name)
    const id = name.replace(/\.tmTheme\.json$/, '')
    if (!prevT.license) {
      throw new Error(
        `missing license for ${name} in provenance.json — re-run a full fetch or add it by hand`,
      )
    }
    entries.push({
      id,
      name: themeLabel(json, id, prevT.displayName),
      uiTheme: json.type === 'light' || prevT.uiTheme === 'light' ? 'light' : 'dark',
      order: order++,
      preview: previewFromTheme(json),
      file: name,
    })
    provenance.themes.push({
      id,
      source: prevT.source || `shiki:${id}`,
      file: name,
      url: prevT.url || SHIKI(id),
      license: prevT.license,
      displayName: prevT.displayName || json.displayName || json.name,
    })
  }

  writeBundle(entries, provenance)
  const shikiCount = shikiFiles.length
  console.log(`wrote ${entries.length} themes (${shikiCount} from Shiki on disk) → ${join(OUT_DIR, 'bundle.toml')}`)
}

async function main() {
  mkdirSync(THEME_DIR, { recursive: true })
  refreshFilmgirlPreviews()
  const local = loadLocal()
  const provenance = { fetchedAt: new Date().toISOString(), themes: [] }
  const entries = []
  const keepFiles = new Set([...local.map((t) => t.file), AURA.file])
  const localIds = new Set(local.map((t) => t.id))

  // Drop stale pack files from prior curated fetches (e.g. vscode.tmTheme.json).
  for (const name of readdirSync(THEME_DIR)) {
    if (!name.endsWith('.tmTheme.json')) continue
    if (keepFiles.has(name)) continue
    rmSync(join(THEME_DIR, name), { force: true })
  }

  for (const t of local) {
    const entry = localThemeEntry(t)
    provenance.themes.push(localProvenance(t))
    entries.push(entry)
  }

  {
    const text = await fetchText(AURA.url)
    JSON.parse(text)
    writeFileSync(join(THEME_DIR, AURA.file), text)
    provenance.themes.push({ id: AURA.id, source: 'aura-theme', file: AURA.file, url: AURA.url })
    entries.push({ ...AURA, uiTheme: 'dark', file: AURA.file })
    console.log('ok', AURA.id)
  }

  // Featured ports first in Settings order (Cool Glow → classics → …), then Shiki.
  entries.sort((a, b) => (a.order ?? 0) - (b.order ?? 0))

  const index = await loadShikiIndex()
  let order = 1000
  const kept = []
  for (const meta of index) {
    const license = meta.license || ''
    if (license && !ALLOWED_LICENSE.test(license)) {
      console.log('skip', meta.name, `(license ${license})`)
      continue
    }
    const file = `${meta.name}.tmTheme.json`
    if (keepFiles.has(file) || localIds.has(meta.name)) {
      console.log('skip', meta.name, '(local port)')
      continue
    }
    const text = await fetchText(SHIKI(meta.name))
    const json = JSON.parse(text)
    writeFileSync(join(THEME_DIR, file), text)
    const uiTheme = meta.type === 'light' || json.type === 'light' ? 'light' : 'dark'
    const preview = previewFromTheme(json)
    const entry = {
      id: meta.name,
      name: meta.displayName || meta.name,
      uiTheme,
      order: order++,
      preview,
      file,
    }
    entries.push(entry)
    kept.push(meta.name)
    provenance.themes.push({
      id: meta.name,
      source: `shiki:${meta.name}`,
      file,
      url: SHIKI(meta.name),
      license: meta.license,
      displayName: meta.displayName,
    })
    console.log('ok', meta.name)
  }

  writeBundle(entries, provenance)
  console.log(`wrote ${entries.length} themes (${kept.length} from Shiki) → ${join(OUT_DIR, 'bundle.toml')}`)
}

if (process.argv.includes('--local-only')) {
  rebuildLocalOnly()
} else {
  main().catch((err) => {
    console.error(err)
    process.exit(1)
  })
}
