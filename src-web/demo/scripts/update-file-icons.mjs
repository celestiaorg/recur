#!/usr/bin/env node
/**
 * Pull fonts / styles / icon DB from upstream file-icons/atom (and Octicons
 * from file-icons/vscode) into src/lib/file-icons/generated/.
 *
 * Usage: pnpm run update-file-icons
 *
 * See src/lib/file-icons/README.md for details.
 */
import { execFileSync } from 'node:child_process'
import {
  createWriteStream, mkdirSync, readFileSync, writeFileSync, rmSync,
} from 'node:fs'
import { dirname, join } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'
import { Readable } from 'node:stream'

const __dirname = dirname(fileURLToPath(import.meta.url))
const ROOT = join(__dirname, '..')
const OUT = join(ROOT, 'src/lib/file-icons/generated')
const TMP = join(ROOT, '.tmp-file-icons')

const ATOM = 'https://raw.githubusercontent.com/file-icons/atom/master'
const VSCODE = 'https://raw.githubusercontent.com/file-icons/vscode/master'

async function fetchTo(url, dest) {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`${res.status} ${res.statusText} for ${url}`)
  mkdirSync(dirname(dest), { recursive: true })
  await pipeline(Readable.fromWeb(res.body), createWriteStream(dest))
  console.log('  ↓', dest.replace(ROOT + '/', ''))
}

function lessc(input, output) {
  const bin = join(ROOT, 'node_modules/.bin/lessc')
  execFileSync(bin, [input, output], { stdio: 'inherit' })
}

async function main() {
  console.log('Updating file-icons from upstream…')
  rmSync(TMP, { recursive: true, force: true })
  mkdirSync(TMP, { recursive: true })
  mkdirSync(join(OUT, 'fonts'), { recursive: true })

  // --- styles (LESS → CSS) ---
  console.log('Styles')
  await Promise.all([
    fetchTo(`${ATOM}/styles/colours.less`, join(TMP, 'colours.less')),
    fetchTo(`${ATOM}/styles/icons.less`, join(TMP, 'icons.less')),
    fetchTo(`${ATOM}/styles/mixins.less`, join(TMP, 'mixins.less')),
    fetchTo(`${ATOM}/styles/fonts.less`, join(TMP, 'fonts.less')),
  ])
  // Point @font-face at our vendored woff2 files (Atom uses atom:// URLs).
  let fontsLess = readFileSync(join(TMP, 'fonts.less'), 'utf8')
  fontsLess = fontsLess
    .replace(/atom:\/\/file-icons\/fonts\//g, './fonts/')
    .replace(/@custom-font-path:\s*"[^"]+";/, '@custom-font-path: "./fonts/file-icons.woff2";')
  // Octicons ships with file-icons/vscode, not atom — add the face ourselves.
  fontsLess += `

@font-face {
	font-family: Octicons Regular;
	font-weight: normal;
	font-style: normal;
	src: url("./fonts/octicons.woff2");
}
`
  writeFileSync(join(TMP, 'fonts.less'), fontsLess)

  lessc(join(TMP, 'colours.less'), join(OUT, 'colors.css'))
  lessc(join(TMP, 'icons.less'), join(OUT, 'icons.css'))
  lessc(join(TMP, 'fonts.less'), join(OUT, 'fonts.css'))
  console.log('  ✓ compiled LESS → CSS')

  // --- icon database ---
  console.log('Icon DB')
  await fetchTo(`${ATOM}/lib/icons/.icondb.js`, join(TMP, 'icondb.js'))
  let db = readFileSync(join(TMP, 'icondb.js'), 'utf8')
  // Upstream is CJS; drop the header and emit an ESM default export.
  db = db.replace(/^[\s\S]*?module\.exports\s*=\s*/, 'export default ')
  // WKWebView / Safari: strip lookbehind assertions (one checksum pattern uses them).
  db = db.replace(/\(\?<![^)]+\)/g, '').replace(/\(\?<=[^)]+\)/g, '')
  writeFileSync(join(OUT, 'icondb.js'), db)
  writeFileSync(
    join(OUT, 'icondb.d.ts'),
    `import type { IconDb } from '../types'\ndeclare const db: IconDb\nexport default db\n`,
  )
  console.log('  ✓ wrote icondb.js')

  // --- fonts ---
  console.log('Fonts')
  const fonts = [
    ['devopicons.woff2', `${ATOM}/fonts/devopicons.woff2`],
    ['file-icons.woff2', `${ATOM}/fonts/file-icons.woff2`],
    ['fontawesome.woff2', `${ATOM}/fonts/fontawesome.woff2`],
    ['mfixx.woff2', `${ATOM}/fonts/mfixx.woff2`],
    // Not in file-icons/atom — required by text-icon / book-icon / etc.
    ['octicons.woff2', `${VSCODE}/icons/octicons.woff2`],
  ]
  await Promise.all(fonts.map(([name, url]) => fetchTo(url, join(OUT, 'fonts', name))))

  // --- pin revision ---
  console.log('Upstream pin')
  const shaRes = await fetch('https://api.github.com/repos/file-icons/atom/commits/master')
  if (!shaRes.ok) throw new Error(`failed to resolve master SHA: ${shaRes.status}`)
  const { sha, commit } = await shaRes.json()
  const pin = [
    `repo: https://github.com/file-icons/atom`,
    `ref: master`,
    `sha: ${sha}`,
    `date: ${commit?.committer?.date ?? ''}`,
    `message: ${(commit?.message ?? '').split('\n')[0]}`,
    `octicons: https://github.com/file-icons/vscode (icons/octicons.woff2)`,
    `updated: ${new Date().toISOString()}`,
    '',
  ].join('\n')
  writeFileSync(join(OUT, 'UPSTREAM'), pin)
  console.log('  ✓', sha.slice(0, 8), (commit?.message ?? '').split('\n')[0])

  rmSync(TMP, { recursive: true, force: true })
  console.log('Done →', OUT.replace(ROOT + '/', ''))
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
