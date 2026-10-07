import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { TmEngine } from './tm-engine'
import { decodeForeground, parseThemeJson } from './tm-theme'

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '../../..')
const wasmPath = join(here, '../../node_modules/vscode-oniguruma/release/onig.wasm')
const dotenvPath = join(root, 'bundles/Languages/syntaxes/dotenv.tmLanguage.json')

const LINE = 'WAILS_VITE_PORT=$RECUR_PORT/extra'
const QUOTED = 'PATH="$HOME/bin"'

function colorAt(engine: TmEngine, text: string, needle: string): string {
  const tokens = engine.tokensForLine('env', 1)
  if (!tokens) throw new Error('no tokens')
  const map = engine.getColorMap()
  const at = text.indexOf(needle)
  expect(at).toBeGreaterThanOrEqual(0)
  for (let i = 0; i < tokens.length; i += 2) {
    const start = tokens[i]
    const end = i + 2 < tokens.length ? tokens[i + 2] : text.length
    if (at >= start && at < end) {
      const fg = decodeForeground(tokens[i + 1])
      return (map[fg] ?? '').toLowerCase()
    }
  }
  throw new Error(`no token covering ${needle}`)
}

async function engineWith(themeRel: string): Promise<TmEngine> {
  const engine = new TmEngine()
  await engine.initOnig(readFileSync(wasmPath))
  engine.registerGrammar('source.dotenv', readFileSync(dotenvPath, 'utf8'), 'dotenv.tmLanguage.json')
  const parsed = parseThemeJson(readFileSync(join(root, themeRel), 'utf8'))
  engine.setTheme(parsed.rawTheme)
  expect(await engine.ensureGrammar('source.dotenv')).toBe(true)
  engine.openDoc('env', 'source.dotenv', LINE)
  engine.tokenizeViewport('env', 1, 1, 1000)
  return engine
}

describe('dotenv token colors', () => {
  it('Charcoal paints keys, =, values, and $vars as distinct roles', async () => {
    const engine = await engineWith('bundles/Recur/themes/charcoal.tmTheme.json')
    const key = colorAt(engine, LINE, 'WAILS_VITE_PORT')
    const eq = colorAt(engine, LINE, '=')
    const dollar = colorAt(engine, LINE, '$RECUR_PORT')
    const extra = colorAt(engine, LINE, '/extra')
    expect(key).toBe('#6eb6ff')
    expect(eq).toBe('#969490')
    expect(dollar).toBe('#6eb6ff')
    expect(extra).toBe('#9fe3b5')
    expect(new Set([key, eq, extra]).size).toBe(3)
  })

  it('Paper paints keys, =, values, and $vars as distinct roles', async () => {
    const engine = await engineWith('bundles/Recur/themes/paper.tmTheme.json')
    const key = colorAt(engine, LINE, 'WAILS_VITE_PORT')
    const eq = colorAt(engine, LINE, '=')
    const dollar = colorAt(engine, LINE, '$RECUR_PORT')
    const extra = colorAt(engine, LINE, '/extra')
    expect(key).toBe('#2a7eb5')
    expect(eq).toBe('#5a616c')
    expect(dollar).toBe('#2a7eb5')
    expect(extra).toBe('#2d9a5a')
    expect(new Set([key, eq, extra]).size).toBe(3)
  })

  it('highlights $VAR inside double-quoted values', async () => {
    const engine = await engineWith('bundles/Recur/themes/charcoal.tmTheme.json')
    engine.openDoc('q', 'source.dotenv', QUOTED)
    engine.tokenizeViewport('q', 1, 1, 1000)
    const tokens = engine.tokensForLine('q', 1)
    if (!tokens) throw new Error('no tokens')
    const map = engine.getColorMap()
    const at = QUOTED.indexOf('$HOME')
    let color = ''
    for (let i = 0; i < tokens.length; i += 2) {
      const start = tokens[i]
      const end = i + 2 < tokens.length ? tokens[i + 2] : QUOTED.length
      if (at >= start && at < end) {
        color = (map[decodeForeground(tokens[i + 1])] ?? '').toLowerCase()
        break
      }
    }
    expect(color).toBe('#6eb6ff')
  })
})
