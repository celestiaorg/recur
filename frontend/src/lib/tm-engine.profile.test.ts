/**
 * Re-profile the shipped TmEngine paint path with the same cases as
 * frontend/scripts/profile-tm.mjs. Skipped in the default suite.
 *
 *   TM_PROFILE=1 pnpm exec vitest run src/lib/tm-engine.profile.test.ts
 */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it } from 'vitest'
import { TmEngine } from './tm-engine'
import { classForMeta } from './cm-tm-deco'

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '../../..')
const frontend = join(here, '../..')

const grammarFiles: Record<string, string> = {
  'source.ts': 'bundles/Languages/syntaxes/typescript.tmLanguage.json',
  'source.js': 'bundles/Languages/syntaxes/javascript.tmLanguage.json',
  'source.svelte': 'bundles/Languages/syntaxes/svelte.tmLanguage.json',
  'source.css': 'bundles/Languages/syntaxes/css.tmLanguage.json',
  'text.html.basic': 'bundles/Languages/syntaxes/html.tmLanguage.json',
}

function ms(n: number) {
  return `${n.toFixed(2)}ms`
}

function synthTs(lines: number): string[] {
  const out: string[] = []
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

async function makeEngine(): Promise<TmEngine> {
  const wasm = readFileSync(join(frontend, 'node_modules/vscode-oniguruma/release/onig.wasm'))
  const engine = new TmEngine()
  await engine.initOnig(wasm)
  for (const [scope, rel] of Object.entries(grammarFiles)) {
    engine.registerGrammar(scope, readFileSync(join(root, rel), 'utf8'), rel)
  }
  engine.setTheme({
    name: 'profile',
    settings: [{ settings: { foreground: '#cccccc', background: '#1e1e1e' } }],
  })
  return engine
}

describe.skipIf(!process.env.TM_PROFILE)('TmEngine paint path (same cases as profile-tm.mjs)', () => {
  it('prints timings', async () => {
    const engine = await makeEngine()
    if (!(await engine.ensureGrammar('source.ts'))) throw new Error('source.ts failed')
    if (!(await engine.ensureGrammar('source.svelte'))) throw new Error('source.svelte failed')

    const ts5k = synthTs(5000)
    const ts80 = synthTs(80)
    const agentChat = readFileSync(join(frontend, 'src/components/AgentChat.svelte'), 'utf8').split('\n')

    console.log('\n=== TmEngine paint path (new) ===')
    console.log(`AgentChat.svelte: ${agentChat.length} lines`)
    console.log(`synth TS: ${ts5k.length} lines`)

    engine.openDoc('ts5k', 'source.ts', ts5k.join('\n'))
    let t0 = performance.now()
    engine.tokenizeViewport('ts5k', 1, 5000, 60_000)
    const fillTs = performance.now() - t0
    console.log(`\n[A] Tokenize all 5000 lines of TS from line 1 (worst case)`)
    console.log(`    tokenizeViewport 1..5000: ${ms(fillTs)} (filled=${engine.filledThrough('ts5k')})`)

    t0 = performance.now()
    engine.tokenizeViewport('ts5k', 1, 5000, 60_000)
    console.log(`    repeat (already filled): ${ms(performance.now() - t0)}`)

    t0 = performance.now()
    const view = engine.tokenizeViewport('ts5k', 4920, 5000, 1000)
    console.log(`\n[B] Viewport-only with cache (80 lines at EOF, stacks+tokens filled)`)
    console.log(`    tokenizeViewport 4920..5000: ${ms(performance.now() - t0)} (tokenized=${view.tokenized})`)

    const editLine = 2500
    engine.editDoc('ts5k', { fromLine: editLine, oldLineCount: 1, newLines: [ts5k[editLine - 1]] })
    t0 = performance.now()
    const keySame = engine.tokenizeViewport('ts5k', 2460, 2540, 1000)
    console.log(`\n[C] Keystroke in middle of 5k TS — TmEngine (same text, early-stop)`)
    console.log(
      `    tokenizeViewport 2460..2540: ${ms(performance.now() - t0)} (tokenized=${keySame.tokenized})`,
    )

    const realEdit = 2497
    engine.editDoc('ts5k', { fromLine: realEdit, oldLineCount: 1, newLines: ['  const y = x + 99'] })
    t0 = performance.now()
    const keyReal = engine.tokenizeViewport('ts5k', 2460, 2540, 1000)
    console.log(
      `    number edit on line ${realEdit}: ${ms(performance.now() - t0)} (tokenized=${keyReal.tokenized})`,
    )

    engine.openDoc('sv', 'source.svelte', agentChat.join('\n'))
    t0 = performance.now()
    engine.tokenizeViewport('sv', 1, agentChat.length, 60_000)
    const svelteFill = performance.now() - t0
    console.log(`\n[D] AgentChat.svelte (${agentChat.length} lines) source.svelte`)
    console.log(`    tokenize 1..end: ${ms(svelteFill)} (filled=${engine.filledThrough('sv')})`)

    const svelteViewFrom = Math.max(1, agentChat.length - 79)
    t0 = performance.now()
    const svelteView = engine.tokenizeViewport('sv', svelteViewFrom, agentChat.length, 1000)
    console.log(
      `    viewport last 80 (cached): ${ms(performance.now() - t0)} (tokenized=${svelteView.tokenized})`,
    )

    const svelteEdit = Math.floor(agentChat.length / 2)
    const svelteViewStart = Math.max(1, svelteEdit - 40)
    const svelteViewEnd = Math.min(agentChat.length, svelteEdit + 40)
    engine.editDoc('sv', {
      fromLine: svelteEdit,
      oldLineCount: 1,
      newLines: [agentChat[svelteEdit - 1] ?? ''],
    })
    t0 = performance.now()
    const svelteKey = engine.tokenizeViewport('sv', svelteViewStart, svelteViewEnd, 1000)
    console.log(
      `    keystroke same-text: ${ms(performance.now() - t0)} (tokenized=${svelteKey.tokenized})`,
    )

    engine.openDoc('ts80', 'source.ts', ts80.join('\n'))
    t0 = performance.now()
    const first = engine.tokenizeViewport('ts80', 1, 80, 1000)
    const paintMs = performance.now() - t0
    const colorMap = engine.getColorMap()
    t0 = performance.now()
    let marks = 0
    for (let n = 1; n <= 80; n++) {
      const text = ts80[n - 1] ?? ''
      const tokens = engine.tokensForLine('ts80', n) ?? []
      for (let i = 0; i < tokens.length; i += 2) {
        const start = tokens[i]
        const end = i + 2 < tokens.length ? tokens[i + 2] : text.length
        if (end <= start) continue
        void classForMeta(tokens[i + 1], true, colorMap, ['#1e1e1e'])
        marks++
      }
    }
    const decoMs = performance.now() - t0
    console.log(`\n[E] Decoration from TmEngine tokens (80-line TS viewport)`)
    console.log(`    tokenizeViewport 1..80 (cold): ${ms(paintMs)} (tokenized=${first.tokenized})`)
    console.log(`    classForMeta on cached tokens: ${ms(decoMs)} (${marks} marks)`)

    engine.openDoc('tsTop', 'source.ts', ts5k.join('\n'))
    t0 = performance.now()
    const top = engine.tokenizeViewport('tsTop', 1, 80, 1000)
    console.log(`\n[F] First paint top-of-file 80 lines TS: ${ms(performance.now() - t0)} (tokenized=${top.tokenized})`)

    // The v2 headline: cold deep jump with a guessed start state instead of
    // tokenizing the 4k-line prefix (compare against [A]).
    engine.openDoc('tsJump', 'source.ts', ts5k.join('\n'))
    t0 = performance.now()
    const jump = engine.tokenizeViewport('tsJump', 4200, 4280, 1000)
    console.log(
      `\n[G] Cold jump to line 4200 (guessed start): ${ms(performance.now() - t0)} (mode=${jump.mode}, tokenized=${jump.tokenized})`,
    )
    engine.openDoc('svJump', 'source.svelte', agentChat.join('\n'))
    t0 = performance.now()
    const svJump = engine.tokenizeViewport('svJump', 3000, 3080, 1000)
    console.log(
      `    Svelte cold jump to line 3000: ${ms(performance.now() - t0)} (mode=${svJump.mode}, tokenized=${svJump.tokenized})`,
    )
  }, 60_000)
})
