#!/usr/bin/env node
/**
 * Fetch TextMate grammars from shikijs/textmate-grammars-themes into
 * bundles/Languages/syntaxes/ and regenerate bundle.toml [[grammar]] entries.
 *
 * Sources are the same grammars VS Code / Shiki use (mostly microsoft/vscode
 * plus language-specific upstreams). Metadata comes from @shikijs/tm-grammars index.
 *
 * LOCAL entries cover MIT grammars Shiki omits or only ships under GPL (e.g. nginx),
 * plus first-party wrappers (`grammar:` inline, no download).
 */
import { mkdirSync, writeFileSync, mkdtempSync, rmSync, readdirSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { tmpdir } from 'node:os'
import { pathToFileURL, fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_DIR = join(ROOT, 'bundles', 'Languages')
const SYNTAX_DIR = join(OUT_DIR, 'syntaxes')
const INDEX_URL =
  'https://raw.githubusercontent.com/shikijs/textmate-grammars-themes/main/packages/tm-grammars/index.js'
const GRAMMAR_URL = (name) =>
  `https://raw.githubusercontent.com/shikijs/textmate-grammars-themes/main/packages/tm-grammars/grammars/${name}.json`
const UPSTREAM_NOTICE =
  'https://github.com/shikijs/textmate-grammars-themes/blob/main/packages/tm-grammars/NOTICE'

/** Compact attribution — the upstream Shiki NOTICE inlines full license texts and is >512KiB. */
function writeCompactNotice(path, rows) {
  const lines = [
    'THIRD-PARTY SOFTWARE NOTICES',
    '',
    'Grammars in this pack are vendored from shikijs/textmate-grammars-themes',
    '(primarily microsoft/vscode and language upstreams). Full license texts:',
    UPSTREAM_NOTICE,
    '',
    'Per-grammar attribution (also see provenance.json):',
    '',
  ]
  for (const g of rows) {
    const spdx = g.license || 'see upstream'
    const src = g.source || ''
    lines.push(`${g.name} — ${spdx}${src ? ` — ${src}` : ''}`)
  }
  lines.push('')
  writeFileSync(path, lines.join('\n'))
}

/**
 * Skip strong copyleft only (GPL/AGPL/LGPL). Keep MIT/Apache/BSD/ISC, MPL-2.0
 * (file-level; hcl/terraform), and unasserted/empty licenses (provenance still
 * records upstream). Themes use a tighter allowlist; grammars keep SEED langs
 * like elixir/nim/cue (NOASSERTION) rather than silently deleting them on refresh.
 */
const BLOCKED_LICENSE = /^(AGPL|GPL|LGPL)/i

/**
 * Extra grammars not taken from Shiki (missing, or Shiki’s copy is copyleft).
 * `format: 'plist'` is converted to JSON via macOS `plutil`.
 */
const LOCAL = [
  {
    name: 'nginx',
    scopeName: 'source.nginx',
    license: 'MIT',
    source: 'https://github.com/SublimeText/nginx',
    url: 'https://raw.githubusercontent.com/SublimeText/nginx/master/Syntaxes/nginx.tmLanguage',
    format: 'plist',
    // Avoid bare `conf` — that belongs to ini. Prefer nginx-specific basenames.
    fileTypes: ['ngx'],
    basenameTypes: [
      'nginx.conf',
      'mime.types',
      'fastcgi_params',
      'scgi_params',
      'uwsgi_params',
    ],
  },
  {
    name: 'caddyfile',
    scopeName: 'source.Caddyfile',
    license: 'MIT',
    source: 'https://github.com/caddyserver/vscode-caddyfile',
    url: 'https://raw.githubusercontent.com/caddyserver/vscode-caddyfile/master/syntaxes/caddyfile.tmLanguage.json',
    format: 'json',
    fileTypes: ['caddyfile', 'caddy'],
    basenameTypes: ['caddyfile'],
  },
  {
    name: 'svg',
    scopeName: 'text.xml.svg',
    license: 'MIT',
    source: 'first-party (includes text.xml)',
    fileTypes: ['svg'],
    grammar: {
      displayName: 'SVG',
      name: 'svg',
      patterns: [{ include: 'text.xml' }],
      scopeName: 'text.xml.svg',
    },
  },
]

/** Languages we currently (or want to) highlight — seed set; embedded deps expand this. */
const SEED = [
  'javascript', 'jsx', 'typescript', 'tsx', 'vue', 'svelte', 'html', 'css', 'scss', 'sass', 'less',
  'go', 'python', 'rust', 'java', 'c', 'cpp', 'php', 'json', 'jsonc', 'yaml', 'xml', 'markdown', 'sql',
  'graphql', 'objective-c', 'objective-cpp', 'csharp', 'scala', 'kotlin', 'dart', 'swift', 'ruby',
  'lua', 'perl', 'r', 'erlang', 'haskell', 'clojure', 'elm', 'julia', 'groovy', 'ocaml', 'fsharp',
  'fortran-free-form', 'toml', 'protobuf', // proto in shiki is often `proto`
  'ini', 'cmake', 'docker', 'diff', 'shellscript', 'powershell', 'make',
  'solidity', 'elixir', 'zig', 'hcl', 'terraform', 'nix', 'nim', 'cue', 'wasm', 'mermaid',
  'crystal', 'verilog', 'system-verilog', 'vhdl', 'tcl', 'coffee', 'haxe', 'pascal', 'scheme',
  'common-lisp', 'd', 'asm', 'pug', 'stylus', 'jinja', 'gherkin', 'latex', 'tex', 'bibtex',
  'dotenv', 'protobuf', 'proto', 'handlebars', 'prisma', 'astro', 'move', 'vyper', 'gleam',
  'fish', 'bat', 'regexp',
]

/** Extra fileTypes beyond Shiki aliases (no leading dot). */
const FILE_TYPES = {
  javascript: ['js', 'mjs', 'cjs'],
  jsx: ['jsx'],
  typescript: ['ts', 'mts', 'cts'],
  tsx: ['tsx'],
  vue: ['vue'],
  svelte: ['svelte'],
  html: ['html', 'htm'],
  css: ['css'],
  scss: ['scss'],
  sass: ['sass'],
  less: ['less'],
  go: ['go'],
  python: ['py', 'pyi', 'pyw'],
  rust: ['rs'],
  java: ['java'],
  c: ['c', 'h'],
  cpp: ['cc', 'cpp', 'cxx', 'hpp', 'hxx', 'hh'],
  php: ['php'],
  json: ['json'],
  jsonc: ['jsonc'],
  yaml: ['yaml', 'yml'],
  xml: ['xml'],
  markdown: ['md', 'mdx', 'markdown'],
  sql: ['sql'],
  graphql: ['graphql', 'gql'],
  'objective-c': ['m'],
  'objective-cpp': ['mm'],
  csharp: ['cs'],
  scala: ['scala', 'sc'],
  kotlin: ['kt', 'kts'],
  dart: ['dart'],
  swift: ['swift'],
  ruby: ['rb', 'rake', 'gemspec'],
  lua: ['lua'],
  perl: ['pl', 'pm'],
  r: ['r'],
  erlang: ['erl', 'hrl'],
  haskell: ['hs', 'lhs'],
  clojure: ['clj', 'cljs', 'cljc', 'edn'],
  elm: ['elm'],
  julia: ['jl'],
  groovy: ['groovy', 'gradle'],
  ocaml: ['ml', 'mli'],
  fsharp: ['fs', 'fsi', 'fsx'],
  'fortran-free-form': ['f90', 'f95', 'f'],
  toml: ['toml'],
  proto: ['proto'],
  ini: ['ini', 'cfg', 'conf', 'properties'],
  dotenv: ['env'],
  cmake: ['cmake'],
  docker: ['dockerfile'],
  diff: ['diff', 'patch'],
  shellscript: ['sh', 'bash', 'zsh', 'ksh', 'command'],
  powershell: ['ps1', 'psm1'],
  make: ['mk', 'makefile'],
  solidity: ['sol'],
  elixir: ['ex', 'exs'],
  zig: ['zig'],
  hcl: ['hcl'],
  terraform: ['tf', 'tfvars'],
  nix: ['nix'],
  nim: ['nim', 'nims'],
  cue: ['cue'],
  wasm: ['wat', 'wast'],
  mermaid: ['mmd', 'mermaid'],
  crystal: ['cr'],
  verilog: ['v'],
  'system-verilog': ['sv'],
  vhdl: ['vhd', 'vhdl'],
  tcl: ['tcl'],
  coffee: ['coffee'],
  haxe: ['hx'],
  pascal: ['pas', 'pp'],
  scheme: ['scm', 'ss'],
  'common-lisp': ['lisp', 'cl', 'el'],
  d: ['d'],
  asm: ['s', 'asm'],
  pug: ['pug'],
  stylus: ['styl'],
  jinja: ['j2', 'jinja', 'jinja2'],
  gherkin: ['feature'],
  latex: ['tex', 'ltx'],
  // `tex` is a helper grammar — don't claim .tex (latex wins).
  tex: [],
  bibtex: ['bib'],
  handlebars: ['hbs', 'handlebars'],
  prisma: ['prisma'],
  astro: ['astro'],
  move: ['move'],
  vyper: ['vy'],
  gleam: ['gleam'],
  fish: ['fish'],
  bat: ['bat', 'cmd'],
}

const BASENAME_TYPES = {
  docker: ['dockerfile', 'containerfile'],
  make: ['makefile', 'gnumakefile'],
  cmake: ['cmakelists.txt'],
  ruby: ['gemfile', 'rakefile', 'guardfile', 'capfile', 'vagrantfile', 'brewfile', 'podfile'],
  go: ['go.mod', 'go.sum'],
  toml: ['cargo.lock', 'poetry.lock', 'uv.lock'],
  python: ['build', 'build.bazel', 'workspace', 'workspace.bazel'],
  nginx: ['nginx.conf', 'mime.types', 'fastcgi_params', 'scgi_params', 'uwsgi_params'],
  caddyfile: ['caddyfile'],
}

// Merge LOCAL file-type overlays into the maps used for bundle.toml.
for (const loc of LOCAL) {
  if (loc.fileTypes?.length) {
    FILE_TYPES[loc.name] = [...new Set([...(FILE_TYPES[loc.name] || []), ...loc.fileTypes])]
  }
  if (loc.basenameTypes?.length) {
    BASENAME_TYPES[loc.name] = [
      ...new Set([...(BASENAME_TYPES[loc.name] || []), ...loc.basenameTypes]),
    ]
  }
}

async function fetchText(url) {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`${url}: ${res.status}`)
  return res.text()
}

/** Write a LOCAL grammar to `syntaxes/<name>.tmLanguage.json` (inline or download). */
async function writeLocalGrammar(loc) {
  let obj
  if (loc.grammar) {
    obj = { ...loc.grammar }
  } else {
    const body = await fetchText(loc.url)
    if (loc.format === 'plist') {
      const dir = mkdtempSync(join(tmpdir(), 'tm-plist-'))
      try {
        const plistPath = join(dir, `${loc.name}.tmLanguage`)
        writeFileSync(plistPath, body)
        const jsonText = execFileSync('plutil', ['-convert', 'json', '-o', '-', plistPath], {
          encoding: 'utf8',
        })
        obj = JSON.parse(jsonText)
      } finally {
        rmSync(dir, { recursive: true, force: true })
      }
    } else {
      obj = JSON.parse(body)
    }
  }
  if (!obj.scopeName) obj.scopeName = loc.scopeName
  if (!obj.name) obj.name = loc.name
  const dest = join(SYNTAX_DIR, `${loc.name}.tmLanguage.json`)
  writeFileSync(dest, JSON.stringify(obj, null, 2) + '\n')
}

function tomlString(s) {
  return JSON.stringify(s)
}

function tomlArray(arr) {
  if (!arr.length) return '[]'
  return `[${arr.map(tomlString).join(', ')}]`
}

async function main() {
  mkdirSync(SYNTAX_DIR, { recursive: true })
  console.log('Fetching Shiki grammar index…')
  const indexSrc = await (await fetch(INDEX_URL)).text()
  const tmp = mkdtempSync(join(tmpdir(), 'tm-grammars-'))
  const indexPath = join(tmp, 'index.mjs')
  writeFileSync(indexPath, indexSrc)
  const { grammars } = await import(pathToFileURL(indexPath).href)
  rmSync(tmp, { recursive: true, force: true })
  const byName = new Map(grammars.map((g) => [g.name, g]))

  // Expand seed with transitive embedded langs (skip disallowed licenses).
  const needed = new Set()
  const skippedLicense = []
  const queue = [...SEED]
  while (queue.length) {
    const name = queue.pop()
    if (needed.has(name) || !byName.has(name)) continue
    const meta = byName.get(name)
    const license = meta.license || ''
    if (license && BLOCKED_LICENSE.test(license)) {
      skippedLicense.push(`${name} (${license})`)
      continue
    }
    needed.add(name)
    const emb = meta.embedded || []
    for (const e of emb) queue.push(e)
  }

  // Prefer `proto` over misspelled protobuf seed.
  if (!byName.has('protobuf')) needed.delete('protobuf')

  const missing = SEED.filter((n) => n !== 'protobuf' && !byName.has(n))
  if (missing.length) console.warn('Not in Shiki index:', missing.join(', '))
  if (skippedLicense.length) {
    console.log('skip license:', [...new Set(skippedLicense)].sort().join(', '))
  }

  console.log(`Downloading ${needed.size} grammars…`)
  const selected = [...needed].sort().map((n) => byName.get(n))
  const localNames = new Set(LOCAL.map((l) => l.name))
  const keep = new Set([...needed, ...localNames])

  // Drop stale grammar files (e.g. previously vendored GPL entries).
  for (const name of readdirSync(SYNTAX_DIR)) {
    if (!name.endsWith('.tmLanguage.json')) continue
    const stem = name.slice(0, -'.tmLanguage.json'.length)
    if (!keep.has(stem)) {
      rmSync(join(SYNTAX_DIR, name), { force: true })
      console.log('removed stale', name)
    }
  }

  const concurrency = 12
  let done = 0
  async function downloadOne(g) {
    const dest = join(SYNTAX_DIR, `${g.name}.tmLanguage.json`)
    const res = await fetch(GRAMMAR_URL(g.name))
    if (!res.ok) throw new Error(`fetch ${g.name}: ${res.status}`)
    const body = await res.text()
    JSON.parse(body) // validate
    writeFileSync(dest, body)
    done++
    process.stdout.write(`  [${done}/${selected.length}] ${g.name}                    \r`)
  }
  for (let i = 0; i < selected.length; i += concurrency) {
    await Promise.all(selected.slice(i, i + concurrency).map(downloadOne))
  }
  console.log(`\nWrote ${selected.length} Shiki grammars to ${SYNTAX_DIR}`)

  console.log(`Downloading ${LOCAL.length} local grammars…`)
  for (const loc of LOCAL) {
    await writeLocalGrammar(loc)
    console.log('  ok', loc.name, `(${loc.license})`)
  }

  const allSelected = [
    ...selected,
    ...LOCAL.map((l) => ({
      name: l.name,
      scopeName: l.scopeName,
      license: l.license,
      source: l.source,
      aliases: [],
    })),
  ].sort((a, b) => a.name.localeCompare(b.name))

  // Generate bundle.toml
  const lines = [
    '[bundle]',
    'name = "Languages"',
    'id = "recur.languages"',
    'version = "1.0.0"',
    'author = "Recur"',
    '',
    '# TextMate grammars vendored from shikijs/textmate-grammars-themes',
    '# (primarily microsoft/vscode + language upstreams), plus LOCAL MIT',
    '# grammars (nginx, caddyfile, svg). Regenerated by scripts/fetch-tm-grammars.mjs',
    '# — do not hand-edit [[grammar]] blocks.',
    '',
  ]

  for (const g of allSelected) {
    const types = new Set([
      ...(FILE_TYPES[g.name] || []),
      ...(g.aliases || []).map((a) => a.replace(/^\./, '')),
      ...(BASENAME_TYPES[g.name] || []),
    ])
    // Skip injection-only / helper grammars from claiming file types unless mapped.
    const fileTypes = [...types].filter(Boolean).sort()
    lines.push('[[grammar]]')
    lines.push(`id = ${tomlString(`recur.languages.${g.name}`)}`)
    lines.push(`scopeName = ${tomlString(g.scopeName)}`)
    lines.push(`path = ${tomlString(`syntaxes/${g.name}.tmLanguage.json`)}`)
    lines.push(`language = ${tomlString(g.name)}`)
    if (fileTypes.length) lines.push(`fileTypes = ${tomlArray(fileTypes)}`)
    lines.push('')
  }

  writeFileSync(join(OUT_DIR, 'bundle.toml'), lines.join('\n'))
  console.log(`Wrote ${join(OUT_DIR, 'bundle.toml')} (${allSelected.length} grammars)`)

  // Provenance index for humans + compact NOTICE (editor can open ≤512KiB text).
  const provenance = allSelected.map((g) => ({
    name: g.name,
    scopeName: g.scopeName,
    source: g.source,
    license: g.license,
  }))
  writeFileSync(join(OUT_DIR, 'provenance.json'), JSON.stringify(provenance, null, 2) + '\n')
  writeCompactNotice(join(OUT_DIR, 'NOTICE'), provenance)
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
