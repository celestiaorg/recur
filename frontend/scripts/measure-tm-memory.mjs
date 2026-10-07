/**
 * Measure retained heap per tokenized line (tokens + rule stacks), so the
 * idle-retention budget in tm-engine.ts is grounded in bytes rather than a
 * guess. Run: node --expose-gc scripts/measure-tm-memory.mjs
 */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const { loadWASM, OnigScanner, OnigString } = require('vscode-oniguruma')
const { Registry, parseRawGrammar, INITIAL } = require('vscode-textmate')

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '../..')
const frontend = join(here, '..')

const wasm = readFileSync(join(frontend, 'node_modules/vscode-oniguruma/release/onig.wasm'))
await loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength))

const grammarFiles = {
  'source.ts': 'bundles/Languages/syntaxes/typescript.tmLanguage.json',
  'source.svelte': 'bundles/Languages/syntaxes/svelte.tmLanguage.json',
  'source.css': 'bundles/Languages/syntaxes/css.tmLanguage.json',
  'text.html.basic': 'bundles/Languages/syntaxes/html.tmLanguage.json',
  'source.js': 'bundles/Languages/syntaxes/javascript.tmLanguage.json',
}

const registry = new Registry({
  onigLib: Promise.resolve({
    createOnigScanner: (patterns) => new OnigScanner(patterns),
    createOnigString: (s) => new OnigString(s),
  }),
  loadGrammar: async (scope) => {
    const rel = grammarFiles[scope]
    if (!rel) return null
    return parseRawGrammar(readFileSync(join(root, rel), 'utf8'), rel)
  },
})
registry.setTheme({
  name: 'measure',
  settings: [{ settings: { foreground: '#ccc', background: '#111' } }],
})

function settle() {
  globalThis.gc?.()
  globalThis.gc?.()
  return process.memoryUsage().heapUsed
}

/** Tokenize `lines` under `scope`, retaining tokens + per-line end stacks. */
async function measure(label, lines, scope) {
  const grammar = await registry.loadGrammar(scope)
  // Warm the grammar's lazy rule compilation so it is not counted per-doc.
  grammar.tokenizeLine2(lines[0] ?? '', INITIAL)

  const before = settle()
  const tokens = []
  const stacks = []
  let stack = INITIAL
  for (const text of lines) {
    const r = grammar.tokenizeLine2(text, stack)
    stack = r.ruleStack
    // Same shapes the engine retains: a plain number[] and the end stack.
    tokens.push(Array.from(r.tokens))
    stacks.push(stack)
  }
  const after = settle()
  const bytes = after - before
  console.log(
    `${label.padEnd(28)} ${String(lines.length).padStart(6)} lines  ` +
      `${(bytes / 1024 / 1024).toFixed(1).padStart(6)} MB  ` +
      `${(bytes / lines.length).toFixed(0).padStart(5)} B/line`,
  )
  // Keep references alive until after the measurement.
  return tokens.length + stacks.length
}

const ts = []
for (let i = 0; ts.length < 20_000; i++) {
  ts.push(
    `export function f${i}(x: number): number {`,
    `  const y = x + ${i} // note`,
    `  if (y > 10) return y * 2`,
    `  return y`,
    `}`,
  )
}

const agentChat = readFileSync(join(frontend, 'src/components/AgentChat.svelte'), 'utf8').split('\n')
const observatory = readFileSync(join(frontend, 'src/components/Observatory.svelte'), 'utf8').split('\n')

console.log('\nRetained heap for tokens + per-line rule stacks\n')
let sink = 0
sink += await measure('synth TS 20k', ts.slice(0, 20_000), 'source.ts')
sink += await measure('AgentChat.svelte', agentChat, 'source.svelte')
sink += await measure('Observatory.svelte', observatory, 'source.svelte')
if (!globalThis.gc) console.log('\n(run with --expose-gc for stable numbers)')
console.log(`\n(sink ${sink})`)
