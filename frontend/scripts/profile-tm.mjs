/**
 * Profile the current CM6 TextMate paint path vs early-stop + token cache.
 * Run: node scripts/profile-tm.mjs
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
  'source.js': 'bundles/Languages/syntaxes/javascript.tmLanguage.json',
  'source.svelte': 'bundles/Languages/syntaxes/svelte.tmLanguage.json',
  'source.css': 'bundles/Languages/syntaxes/css.tmLanguage.json',
  'text.html.basic': 'bundles/Languages/syntaxes/html.tmLanguage.json',
}

const registry = new Registry({
  onigLib: Promise.resolve({
    createOnigScanner: (patterns) => new OnigScanner(patterns),
    createOnigString: (s) => new OnigString(s),
  }),
  loadGrammar: async (scope) => {
    const rel = grammarFiles[scope]
    if (!rel) return null
    const text = readFileSync(join(root, rel), 'utf8')
    return parseRawGrammar(text, rel)
  },
})

function now() {
  return performance.now()
}

function ms(n) {
  return `${n.toFixed(2)}ms`
}

function synthTs(lines) {
  const out = []
  let i = 0
  while (out.length < lines) {
    out.push(`export function f${i}(x: number): number {`)
    out.push(`  const y = x + ${i}`)
    out.push(`  if (y > 10) return y * 2`)
    out.push(`  return y`)
    out.push(`}`)
    i++
  }
  return out.slice(0, lines)
}

function tokenizeRange(grammar, lines, from, to, startStack) {
  let stack = startStack
  let tokens = 0
  const t0 = now()
  for (let n = from; n <= to; n++) {
    const result = grammar.tokenizeLine2(lines[n - 1], stack)
    stack = result.ruleStack
    tokens += result.tokens.length / 2
  }
  return { ms: now() - t0, stack, tokens }
}

/** Current CM6 path: after invalidateFrom(edit), retokenize whole viewport. */
function currentKeystroke(grammar, lines, viewFrom, viewTo, startStack) {
  // Cache is filled through viewFrom-1; invalidate drops editLine..end.
  // resumeAt(viewFrom) still starts at viewFrom if edit is inside the viewport.
  return tokenizeRange(grammar, lines, viewFrom, viewTo, startStack)
}

/** Proposed: retokenize from edit, stop when end stack matches. */
function earlyStopKeystroke(grammar, lines, editLine, cachedAfter) {
  let stack = editLine === 1 ? INITIAL : cachedAfter[editLine - 1]
  let tokenized = 0
  const t0 = now()
  for (let n = editLine; n <= lines.length; n++) {
    const result = grammar.tokenizeLine2(lines[n - 1], stack)
    stack = result.ruleStack
    tokenized++
    if (cachedAfter[n] && stack.equals(cachedAfter[n])) break
  }
  return { ms: now() - t0, tokenized }
}

function fillCache(grammar, lines) {
  const after = []
  let stack = INITIAL
  const t0 = now()
  for (let n = 1; n <= lines.length; n++) {
    const result = grammar.tokenizeLine2(lines[n - 1], stack)
    stack = result.ruleStack
    after[n] = stack
  }
  return { ms: now() - t0, after }
}

function decoCost(grammar, lines, viewFrom, viewTo, startStack) {
  let stack = startStack
  let marks = 0
  const t0 = now()
  for (let n = viewFrom; n <= viewTo; n++) {
    const text = lines[n - 1]
    const result = grammar.tokenizeLine2(text, stack)
    stack = result.ruleStack
    const tokens = result.tokens
    for (let i = 0; i < tokens.length; i += 2) {
      const start = tokens[i]
      const meta = tokens[i + 1]
      const end = i + 2 < tokens.length ? tokens[i + 2] : text.length
      if (end <= start) continue
      const fg = (meta & 0b00000000111111111000000000000000) >>> 15
      const bg = (meta & 0b11111111000000000000000000000000) >>> 24
      const fs = (meta & 0b00000000000000000111100000000000) >>> 11
      const cls = `cm-tm-${fg}${bg > 0 ? ` cm-tm-bg-${bg}` : ''}${fs ? ` fs${fs}` : ''}`
      marks++
      void cls
    }
  }
  return { ms: now() - t0, marks }
}

function decoOnlyFromCached(lines, tokensByLine, viewFrom, viewTo) {
  let marks = 0
  const t0 = now()
  for (let n = viewFrom; n <= viewTo; n++) {
    const text = lines[n - 1]
    const tokens = tokensByLine[n]
    for (let i = 0; i < tokens.length; i += 2) {
      const start = tokens[i]
      const meta = tokens[i + 1]
      const end = i + 2 < tokens.length ? tokens[i + 2] : text.length
      if (end <= start) continue
      const fg = (meta & 0b00000000111111111000000000000000) >>> 15
      marks++
      void fg
    }
  }
  return { ms: now() - t0, marks }
}

function cacheTokens(grammar, lines) {
  const tokensByLine = []
  const after = []
  let stack = INITIAL
  for (let n = 1; n <= lines.length; n++) {
    const result = grammar.tokenizeLine2(lines[n - 1], stack)
    stack = result.ruleStack
    after[n] = stack
    tokensByLine[n] = result.tokens
  }
  return { tokensByLine, after }
}

const tsGrammar = await registry.loadGrammar('source.ts')
const svelteGrammar = await registry.loadGrammar('source.svelte')

const agentChat = readFileSync(
  join(frontend, 'src/components/AgentChat.svelte'),
  'utf8',
).split('\n')

const ts5k = synthTs(5000)
const ts80 = synthTs(80)

console.log('=== TextMate paint profile ===')
console.log(`AgentChat.svelte: ${agentChat.length} lines`)
console.log(`synth TS: ${ts5k.length} lines`)

const fillTs = fillCache(tsGrammar, ts5k)
console.log(`\n[A] Fill 5000-line TS from line 1 (fling-scroll / jump below fold)`)
console.log(`    tokenizeLine2 1..5000: ${ms(fillTs.ms)}`)

const jump = tokenizeRange(tsGrammar, ts5k, 1, 5000, INITIAL)
console.log(`    repeat (warm grammar): ${ms(jump.ms)}`)

const view = tokenizeRange(tsGrammar, ts5k, 4920, 5000, fillTs.after[4919])
console.log(`\n[B] Viewport-only with cache (80 lines at EOF, stacks filled)`)
console.log(`    tokenize 4920..5000: ${ms(view.ms)}`)

const keyCur = currentKeystroke(tsGrammar, ts5k, 2460, 2540, fillTs.after[2459])
console.log(`\n[C] Keystroke in middle of 5k TS — CURRENT (retokenize 80-line viewport)`)
console.log(`    tokenize 2460..2540: ${ms(keyCur.ms)}`)

const keyNew = earlyStopKeystroke(tsGrammar, ts5k, 2500, fillTs.after)
console.log(`    PROPOSED early-stop from edit line: ${ms(keyNew.ms)} (${keyNew.tokenized} lines)`)

const svelteFill = fillCache(svelteGrammar, agentChat)
console.log(`\n[D] AgentChat.svelte (${agentChat.length} lines) source.svelte`)
console.log(`    tokenize 1..end: ${ms(svelteFill.ms)}`)

const svelteViewFrom = Math.max(1, agentChat.length - 79)
const svelteView = tokenizeRange(
  svelteGrammar,
  agentChat,
  svelteViewFrom,
  agentChat.length,
  svelteFill.after[svelteViewFrom - 1] ?? INITIAL,
)
console.log(`    viewport last 80 (cached prefix): ${ms(svelteView.ms)}`)

const svelteEdit = Math.floor(agentChat.length / 2)
const svelteViewStart = Math.max(1, svelteEdit - 40)
const svelteKeyCur = currentKeystroke(
  svelteGrammar,
  agentChat,
  svelteViewStart,
  Math.min(agentChat.length, svelteEdit + 40),
  svelteFill.after[svelteViewStart - 1] ?? INITIAL,
)
const svelteKeyNew = earlyStopKeystroke(svelteGrammar, agentChat, svelteEdit, svelteFill.after)
console.log(`    keystroke CURRENT (80-line viewport): ${ms(svelteKeyCur.ms)}`)
console.log(`    keystroke early-stop: ${ms(svelteKeyNew.ms)} (${svelteKeyNew.tokenized} lines)`)

const { tokensByLine, after } = cacheTokens(tsGrammar, ts80)
const decoTok = decoCost(tsGrammar, ts80, 1, 80, INITIAL)
const decoOnly = decoOnlyFromCached(ts80, tokensByLine, 1, 80)
console.log(`\n[E] Decoration churn vs tokenize (80-line TS viewport)`)
console.log(`    tokenize + className loop: ${ms(decoTok.ms)} (${decoTok.marks} marks)`)
console.log(`    className loop on cached tokens only: ${ms(decoOnly.ms)} (${decoOnly.marks} marks)`)

const mid = tokenizeRange(tsGrammar, ts5k, 1, 80, INITIAL)
console.log(`\n[F] First paint top-of-file 80 lines TS: ${ms(mid.ms)}`)

void after
