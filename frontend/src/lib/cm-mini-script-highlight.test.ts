/** @vitest-environment happy-dom */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeAll, describe, expect, it } from 'vitest'
import { EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { miniScriptHighlight } from './cm-mini-script-highlight'
import { TmEngine } from './tm-engine'
import {
  _resetTmRegistryForTests,
  _setTmEngineForTests,
  addGrammar,
  applyColorMap,
  ensureChromePairedTmEngine,
  getColorMap,
  getColorMapEpoch,
  getParsedTheme,
  registerThemeMeta,
} from './tm-registry'
import { parseThemeJson } from './tm-theme'

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '../../..')
const wasmPath = join(here, '../../node_modules/vscode-oniguruma/release/onig.wasm')
const dotenvPath = join(root, 'bundles/Languages/syntaxes/dotenv.tmLanguage.json')
const charcoalPath = join(root, 'bundles/Recur/themes/charcoal.tmTheme.json')

const LINE = 'KEY="green"'

let main: TmEngine

beforeAll(async () => {
  main = new TmEngine()
  await main.initOnig(readFileSync(wasmPath))
})

afterEach(() => {
  _resetTmRegistryForTests()
})

function markStyles(view: EditorView): string[] {
  const out: string[] = []
  for (const set of view.state.facet(EditorView.decorations)) {
    const deco = typeof set === 'function' ? set(view) : set
    const iter = deco.iter()
    while (iter.value) {
      const style = (iter.value.spec as { attributes?: { style?: string } }).attributes?.style
      if (style) out.push(style)
      iter.next()
    }
  }
  return out
}

describe('ensureChromePairedTmEngine', () => {
  it('tokenizes Charcoal without replacing the global colour map', async () => {
    const other = parseThemeJson(JSON.stringify({
      name: 'Other',
      type: 'dark',
      colors: { 'editor.background': '#111111', 'editor.foreground': '#eeeeee' },
      tokenColors: [{ settings: { foreground: '#eeeeee', background: '#111111' } }],
    }))
    main.setTheme(other.rawTheme)
    applyColorMap(main.getColorMap(), true)
    _setTmEngineForTests(main)

    const charcoal = parseThemeJson(readFileSync(charcoalPath, 'utf8'), 'Charcoal')
    registerThemeMeta(
      { id: 'recur', name: 'Charcoal', uiTheme: 'dark' } as Parameters<typeof registerThemeMeta>[0],
      charcoal,
    )
    addGrammar(
      {
        id: 'recur.languages.dotenv',
        scopeName: 'source.dotenv',
        path: 'syntaxes/dotenv.tmLanguage.json',
        language: 'dotenv',
        fileTypes: ['env'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => readFileSync(dotenvPath, 'utf8'),
    )

    const epoch = getColorMapEpoch()
    const globalMap = getColorMap().slice()
    const mini = await ensureChromePairedTmEngine('recur')
    expect(mini).toBeTruthy()
    expect(mini).not.toBe(main)
    expect(getColorMap()).toEqual(globalMap)
    expect(getColorMapEpoch()).toBe(epoch)

    expect(await mini!.ensureGrammar('source.dotenv')).toBe(true)
    mini!.openDoc('env', 'source.dotenv', LINE)
    mini!.tokenizeViewport('env', 1, 1, 1000)
    const tokens = mini!.tokensForLine('env', 1)
    expect(tokens && tokens.length).toBeGreaterThan(0)
    // Charcoal strings are mint; the deco test below asserts the actual hex.
    expect(mini!.getColorMap().some((c) => c.toLowerCase() === '#9fe3b5')).toBe(true)
  })
})

describe('miniScriptHighlight', () => {
  it('paints Charcoal string colour as an inline style', async () => {
    const other = parseThemeJson(JSON.stringify({
      name: 'Other',
      type: 'dark',
      colors: { 'editor.background': '#111111', 'editor.foreground': '#eeeeee' },
      tokenColors: [
        { settings: { foreground: '#eeeeee', background: '#111111' } },
        { scope: 'string', settings: { foreground: '#ff00ff' } },
      ],
    }))
    main.setTheme(other.rawTheme)
    applyColorMap(main.getColorMap(), true)
    _setTmEngineForTests(main)

    const charcoal = parseThemeJson(readFileSync(charcoalPath, 'utf8'), 'Charcoal')
    registerThemeMeta(
      { id: 'recur', name: 'Charcoal', uiTheme: 'dark' } as Parameters<typeof registerThemeMeta>[0],
      charcoal,
    )
    addGrammar(
      {
        id: 'recur.languages.dotenv',
        scopeName: 'source.dotenv',
        path: 'syntaxes/dotenv.tmLanguage.json',
        language: 'dotenv',
        fileTypes: ['env'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => readFileSync(dotenvPath, 'utf8'),
    )

    const engine = await ensureChromePairedTmEngine('recur')
    expect(engine).toBeTruthy()
    expect(await engine!.ensureGrammar('source.dotenv')).toBe(true)
    const parsed = getParsedTheme('recur')
    expect(parsed).toBeTruthy()

    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: LINE,
        extensions: [miniScriptHighlight('source.dotenv', engine!, parsed!, 'recur')],
      }),
    })
    const styles = markStyles(view).join(' ')
    expect(styles.toLowerCase()).toContain('#9fe3b5')
    expect(styles.toLowerCase()).not.toContain('#ff00ff')
    view.destroy()
    parent.remove()
  })
})
