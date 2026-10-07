import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { MAX_IDLE_DOCS, MAX_IDLE_LINES, TmEngine } from './tm-engine'
import { tmDocFingerprint } from './cm-tm-doc'

const here = dirname(fileURLToPath(import.meta.url))
const wasmPath = join(here, '../../node_modules/vscode-oniguruma/release/onig.wasm')
const tsGrammarPath = join(here, '../../../bundles/Languages/syntaxes/typescript.tmLanguage.json')

function synthTs(lines: number): string {
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
  return out.slice(0, lines).join('\n')
}

describe('TmEngine', () => {
  it('retokenizes one line on a same-stack keystroke after a warm fill', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    engine.setTheme({
      name: 'test',
      settings: [{ settings: { foreground: '#ccc', background: '#111' } }],
    })
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    const text = synthTs(200)
    engine.openDoc('d1', 'source.ts', text)
    expect(engine.tokenizeViewport('d1', 1, 200, 1000).mode).toBe('accurate')
    expect(engine.hasFreshTokens('d1', 1, 80)).toBe(true)

    engine.editDoc('d1', {
      fromLine: 37,
      oldLineCount: 1,
      newLines: ['  const y = x + 99'],
    })
    // The pre-edit token array for line 37 is deliberately kept so a
    // re-tokenize can `equals` the old end state and stop early. It must not
    // count as fresh — treating it as usable repainted the viewport from
    // pre-edit tokens and left the correction to the idle sweep.
    expect(engine.hasFreshTokens('d1', 1, 80)).toBe(false)

    const after = engine.tokenizeViewport('d1', 1, 80, 1000)
    expect(after.mode).toBe('accurate')
    expect(after.tokenized).toBeLessThanOrEqual(3)
    expect(engine.hasFreshTokens('d1', 1, 80)).toBe(true)
  })

  it('applies a whole-document replace without exceeding the call stack', () => {
    const engine = new TmEngine()
    engine.openDoc('big', 'source.ts', 'const a = 1')
    const newLines = new Array(200_000).fill('const x = 1')
    // A live refresh of a changed file replaces every line at once; spreading
    // that into Array.prototype.splice throws RangeError.
    expect(() => engine.editDoc('big', { fromLine: 1, oldLineCount: 1, newLines })).not.toThrow()
    expect(engine.lineCount('big')).toBe(200_000)
  })

  it('reuses the line-state cache when reopening the same id+text', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    const text = synthTs(200)
    expect(engine.openDoc('src/a.ts', 'source.ts', text).reused).toBe(false)
    engine.tokenizeViewport('src/a.ts', 1, 200, 1000)
    expect(engine.filledThrough('src/a.ts')).toBe(200)

    expect(engine.openDoc('src/a.ts', 'source.ts', text).reused).toBe(true)
    expect(engine.filledThrough('src/a.ts')).toBe(200)
    expect(engine.hasFreshTokens('src/a.ts', 1, 40)).toBe(true)

    expect(engine.openDoc('src/a.ts', 'source.ts', `${text}\n`).reused).toBe(false)
    expect(engine.filledThrough('src/a.ts')).toBe(0)
    expect(engine.hasFreshTokens('src/a.ts', 1, 40)).toBe(false)
  })

  it('matchDoc reuses a resident doc by fingerprint without shipping text', () => {
    const fpOf = (text: string) => {
      const lines = text.split('\n')
      return tmDocFingerprint((n) => lines[n - 1], lines.length, text.length)
    }
    const engine = new TmEngine()
    const text = synthTs(200)
    engine.openDoc('src/a.ts', 'source.ts', text)
    expect(engine.matchDoc('src/a.ts', 'source.ts', fpOf(text))).toBe(true)
    expect(engine.matchDoc('src/a.ts', 'source.other', fpOf(text))).toBe(false)
    expect(engine.matchDoc('missing', 'source.ts', fpOf(text))).toBe(false)
    // Diverged text: extra line, and a same-length single-char change.
    expect(engine.matchDoc('src/a.ts', 'source.ts', fpOf(`${text}\n`))).toBe(false)
    const lines = text.split('\n')
    lines[0] = lines[0].replace('export', 'exqort')
    expect(engine.matchDoc('src/a.ts', 'source.ts', fpOf(lines.join('\n')))).toBe(false)
    // Edits move the resident doc; a stale fingerprint stops matching.
    engine.editDoc('src/a.ts', { fromLine: 1, oldLineCount: 1, newLines: ['// changed'] })
    expect(engine.matchDoc('src/a.ts', 'source.ts', fpOf(text))).toBe(false)
  })

  it('tokenizeViewport is accurate when the state chain reaches the viewport', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    engine.openDoc('d', 'source.ts', synthTs(300))
    const r = engine.tokenizeViewport('d', 1, 60, 1000)
    expect(r.mode).toBe('accurate')
    expect(r.complete).toBe(true)
    expect(engine.hasFreshTokens('d', 1, 60)).toBe(true)
    expect(engine.provisionalCount('d')).toBe(0)
    expect(engine.filledThrough('d')).toBeGreaterThanOrEqual(60)
  })

  it('tokenizeViewport on a cold deep jump guesses a start and stays provisional', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    engine.openDoc('d', 'source.ts', synthTs(2000))
    const r = engine.tokenizeViewport('d', 1500, 1560, 1000)
    expect(r.mode).toBe('guessed')
    expect(r.complete).toBe(true)
    // Guesses never advance the accurate watermark (VS Code: heuristic tokens
    // are not written to the end-state store).
    expect(engine.filledThrough('d')).toBe(0)
    expect(engine.provisionalCount('d')).toBeGreaterThan(0)
    expect(engine.hasFreshTokens('d', 1500, 1560)).toBe(true)
    const mid = engine.tokensForLine('d', 1530)
    expect(mid && mid.length).toBeTruthy()
  })

  it('fillSlice sweeps to EOF, corrects wrong guesses, and reports the changed range', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    // Line 3 sits inside a template literal; an indentation-walk guess from
    // column 1 cannot know that and will tokenize it as plain code.
    const lines = [
      'const s = `',
      'function not_code() {',
      'const fake = 1',
      '`',
      'const real = 2',
    ]
    engine.openDoc('d', 'source.ts', lines.join('\n'))
    const guess = engine.tokenizeViewport('d', 3, 3, 1000)
    expect(guess.mode).toBe('guessed')
    const guessed = engine.tokensForLine('d', 3)!

    let done = false
    let changedFrom: number | null = null
    let changedTo = 0
    for (let i = 0; i < 100 && !done; i++) {
      const p = engine.fillSlice('d', 50)
      if (p.changedFrom !== null) {
        changedFrom = changedFrom === null ? p.changedFrom : Math.min(changedFrom, p.changedFrom)
        changedTo = Math.max(changedTo, p.changedTo)
      }
      done = p.done
    }
    expect(done).toBe(true)
    expect(engine.filledThrough('d')).toBe(lines.length)
    expect(engine.provisionalCount('d')).toBe(0)
    // The corrected line is inside the reported changed range and its tokens moved.
    expect(changedFrom).not.toBeNull()
    expect(changedFrom!).toBeLessThanOrEqual(3)
    expect(changedTo).toBeGreaterThanOrEqual(3)
    const accurate = engine.tokensForLine('d', 3)!
    expect(Array.from(accurate)).not.toEqual(Array.from(guessed))
  })

  it('fillSlice advances monotonically under a tiny budget until done', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    engine.openDoc('d', 'source.ts', synthTs(400))
    let prev = 0
    let done = false
    for (let i = 0; i < 10_000 && !done; i++) {
      const p = engine.fillSlice('d', 0.05)
      expect(p.filled).toBeGreaterThanOrEqual(prev)
      prev = p.filled
      done = p.done
    }
    expect(done).toBe(true)
    expect(engine.filledThrough('d')).toBe(400)
  })

  it('closeDoc drops a document a view still holds, closeIfIdle does not', () => {
    const engine = new TmEngine()
    engine.openDoc('live.ts', 'source.ts', 'const a = 1')
    // Reload from disk discards the buffer while the file is still on screen.
    // The fingerprint cannot be trusted to notice (see below), so the caller
    // forces the drop and the plugin re-opens from the new text.
    expect(engine.closeIfIdle('live.ts')).toBe(false)
    expect(engine.hasDoc('live.ts')).toBe(true)
    engine.closeDoc('live.ts')
    expect(engine.hasDoc('live.ts')).toBe(false)
  })

  it('matchDoc samples the document, so it cannot police a reload', () => {
    const engine = new TmEngine()
    const a = Array.from({ length: 1000 }, (_, i) => `const v${i} = 1`)
    const b = [...a]
    b[49] = 'const v49 = 2' // same length, line 50 is not sampled
    const fpOf = (lines: string[]) => {
      const text = lines.join('\n')
      return tmDocFingerprint((n) => lines[n - 1], lines.length, text.length)
    }
    engine.openDoc('big.ts', 'source.ts', a.join('\n'))
    // Documents the limit this guard has: only ~32 lines are hashed, so a
    // same-length change elsewhere is invisible. Callers that know the text
    // was replaced must drop the document instead of relying on this.
    expect(engine.matchDoc('big.ts', 'source.ts', fpOf(b))).toBe(true)
    const sampled = [...a]
    sampled[32] = 'const v32 = 2'
    expect(engine.matchDoc('big.ts', 'source.ts', fpOf(sampled))).toBe(false)
  })

  it('retains inactive documents and frees them on close', () => {
    const engine = new TmEngine()
    engine.openDoc('a.ts', 'source.ts', 'const a = 1')
    engine.openDoc('b.ts', 'source.ts', 'const b = 2')
    // Switching away keeps the cache — returning to a tab must be free.
    engine.releaseDoc('a.ts')
    expect(engine.hasDoc('a.ts')).toBe(true)
    // A document a view still holds is never dropped by a close request.
    expect(engine.closeIfIdle('b.ts')).toBe(false)
    expect(engine.hasDoc('b.ts')).toBe(true)
    // Closing the tab frees it (VS Code disposes the TextModel here).
    expect(engine.closeIfIdle('a.ts')).toBe(true)
    expect(engine.hasDoc('a.ts')).toBe(false)
  })

  it('evicts idle documents by total lines, not by count', () => {
    const engine = new TmEngine()
    const big = new Array(Math.ceil(MAX_IDLE_LINES * 0.6)).fill('x').join('\n')
    for (const id of ['b1', 'b2']) {
      engine.openDoc(id, 'source.ts', big)
      engine.releaseDoc(id)
    }
    // Two docs at 60% of the budget each overflow it: the oldest goes, even
    // though the doc count is far under the count backstop.
    expect(engine.hasDoc('b1')).toBe(false)
    expect(engine.hasDoc('b2')).toBe(true)

    // Many small documents stay — count alone must not evict them.
    for (let i = 0; i < MAX_IDLE_DOCS - 1; i++) {
      engine.openDoc(`s${i}`, 'source.ts', 'const x = 1')
      engine.releaseDoc(`s${i}`)
    }
    expect(engine.hasDoc('s0')).toBe(true)
    expect(engine.hasDoc(`s${MAX_IDLE_DOCS - 2}`)).toBe(true)
  })

  it('the sweep after a mid-file edit stops when the stack restabilizes', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)

    engine.openDoc('d2', 'source.ts', synthTs(200))
    engine.tokenizeViewport('d2', 1, 200, 1000)
    expect(engine.filledThrough('d2')).toBe(200)
    engine.editDoc('d2', {
      fromLine: 37,
      oldLineCount: 1,
      newLines: ['  const y = x + 99'],
    })
    expect(engine.filledThrough('d2')).toBe(36)
    // Re-tokenizing the viewport re-converges on the cached stacks and marks
    // the rest of the file valid again rather than sweeping to EOF.
    const after = engine.tokenizeViewport('d2', 1, 80, 1000)
    expect(after.tokenized).toBeLessThanOrEqual(3)
    expect(engine.filledThrough('d2')).toBe(200)
  })

  it('scopesForLine names a TypeScript function', async () => {
    const wasm = readFileSync(wasmPath)
    const engine = new TmEngine()
    await engine.initOnig(wasm)
    engine.registerGrammar('source.ts', readFileSync(tsGrammarPath, 'utf8'), 'typescript.tmLanguage.json')
    expect(await engine.ensureGrammar('source.ts')).toBe(true)
    engine.openDoc('fn.ts', 'source.ts', 'export function hello() {}\n')
    expect(engine.fillSync('fn.ts', 1000)).toBe(true)
    const toks = engine.scopesForLine('fn.ts', 1) ?? []
    const names = toks.filter((t) => t.scopes.some((s) => s.startsWith('entity.name.function')))
    expect(names.map((t) => engine.lineAt('fn.ts', 1)?.slice(t.start, t.end))).toEqual(['hello'])
  })

  it('re-noting a grammar reloads its source from the loader', async () => {
    const engine = new TmEngine()
    await engine.initOnig(readFileSync(wasmPath))
    let version = 1
    let loads = 0
    engine.setSourceLoader(async () => {
      loads++
      return {
        content: JSON.stringify({ scopeName: 'source.demo', patterns: [{ match: 'x', name: `v${version}` }] }),
        pathHint: 'demo.json',
      }
    })
    engine.noteGrammar('source.demo', 'demo.json')
    expect(await engine.ensureGrammar('source.demo')).toBe(true)
    version = 2
    engine.noteGrammar('source.demo', 'demo.json')
    expect(await engine.ensureGrammar('source.demo')).toBe(true)
    expect(loads).toBe(2)
  })
})
