// @vitest-environment happy-dom
/**
 * Mounts a real EditorView with the TextMate highlighter and edits it — the
 * integration coverage the v1 handoff called out as missing. Guards the two
 * things unit tests cannot see: that a highlighted buffer stays editable, and
 * that marks never outrun the document after an edit.
 */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { Compartment, EditorState, StateEffect } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { textmateLanguage } from './cm-textmate'
import { textmateDocKey } from './cm-tm-doc'
import { editorThemeExtension } from './cm-themes'
import { TmEngine } from './tm-engine'
import { parseThemeJson } from './tm-theme'
import { _setTmEngineForTests, applyColorMap, getColorMap, hasGrammar, registerThemeMeta } from './tm-registry'

const here = dirname(fileURLToPath(import.meta.url))
const wasmPath = join(here, '../../node_modules/vscode-oniguruma/release/onig.wasm')
const tsGrammarPath = join(here, '../../../bundles/Languages/syntaxes/typescript.tmLanguage.json')

let seq = 0
let engine: TmEngine

function mount(doc: string) {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const state = EditorState.create({
    doc,
    extensions: [textmateDocKey(`src/edit${++seq}.ts`), textmateLanguage('source.ts')],
  })
  return new EditorView({ parent, state })
}

/** Mark classes in document order — `cm-tm-<colourIndex>` per token. */
function markClasses(view: EditorView): string[] {
  const out: string[] = []
  for (const set of view.state.facet(EditorView.decorations)) {
    const deco = typeof set === 'function' ? set(view) : set
    const iter = deco.iter()
    while (iter.value) {
      const cls = (iter.value.spec as { class?: string }).class
      if (cls) out.push(cls)
      iter.next()
    }
  }
  return out
}

/** Individual class names across all marks (a mark may carry several). */
function markClassTokens(view: EditorView): Set<string> {
  const out = new Set<string>()
  for (const cls of markClasses(view)) {
    for (const part of cls.split(' ')) if (part) out.add(part)
  }
  return out
}

/** Every decoration must stay inside the document. */
function assertMarksInDocument(view: EditorView) {
  const len = view.state.doc.length
  for (const set of view.state.facet(EditorView.decorations)) {
    const deco = typeof set === 'function' ? set(view) : set
    const iter = deco.iter()
    while (iter.value) {
      expect(iter.from).toBeGreaterThanOrEqual(0)
      expect(iter.to).toBeLessThanOrEqual(len)
      iter.next()
    }
  }
}

beforeAll(async () => {
  engine = new TmEngine()
  await engine.initOnig(readFileSync(wasmPath))
  engine.registerGrammar(
    'source.ts',
    readFileSync(tsGrammarPath, 'utf8'),
    'typescript.tmLanguage.json',
  )
  applyColorMap(
    engine.setTheme({
      name: 'test',
      settings: [{ settings: { foreground: '#cccccc', background: '#111111' } }],
    }),
  )
  expect(await engine.ensureGrammar('source.ts')).toBe(true)
  _setTmEngineForTests(engine)
})

describe('textmate highlighting in a live EditorView', () => {
  it('paints on the first frame without waiting for a second turn', () => {
    const view = mount('const x = 1\nconst y = 2\n')
    // Grammar was resident, so the constructor tokenized synchronously.
    const deco = view.state.facet(EditorView.decorations)
    const built = deco.map((d) => (typeof d === 'function' ? d(view) : d))
    expect(built.some((d) => d.size > 0)).toBe(true)
    view.destroy()
  })

  it('stays editable: typing inserts text', () => {
    const view = mount('const x = 1\n')
    view.dispatch({ changes: { from: view.state.doc.length, insert: 'const y = 2\n' } })
    expect(view.state.doc.toString()).toBe('const x = 1\nconst y = 2\n')
    expect(view.state.readOnly).toBe(false)
    assertMarksInDocument(view)
    view.destroy()
  })

  it('survives a shrinking edit without stale marks running past the line', () => {
    const view = mount('const alpha = "a long string literal here"\nconst beta = 2\n')
    // Delete most of line 1 — stale tokens for it would point far past its end.
    const line = view.state.doc.line(1)
    view.dispatch({ changes: { from: line.from, to: line.to, insert: 'x' } })
    expect(view.state.doc.line(1).text).toBe('x')
    assertMarksInDocument(view)
    // And a follow-up edit still lands.
    view.dispatch({ changes: { from: 1, insert: 'y' } })
    expect(view.state.doc.line(1).text).toBe('xy')
    assertMarksInDocument(view)
    view.destroy()
  })

  it('repaints an open buffer when the colour theme changes', () => {
    const view = mount('const x = 1\n')
    const before = markClasses(view)
    expect(before.length).toBeGreaterThan(0)
    // Theme B gives keywords their own colour, so `const` must land on a
    // different colour index than it had under the flat theme A.
    applyColorMap(
      engine.setTheme({
        name: 'B',
        settings: [
          { settings: { foreground: '#ff0000', background: '#000000' } },
          { scope: 'keyword', settings: { foreground: '#00ff00' } },
          { scope: 'constant.numeric', settings: { foreground: '#0000ff' } },
        ],
      }),
    )
    // What CodeEditor's themeCompartment.reconfigure produces for the view.
    view.dispatch({})
    const after = markClasses(view)
    expect(after).not.toEqual(before)
    expect(new Set(after).size).toBeGreaterThan(1)
    assertMarksInDocument(view)
    view.destroy()
  })

  it('repaints through the app path: themeCompartment.reconfigure', () => {
    // Mirrors CodeEditor: a themeCompartment holding editorThemeExtension(key),
    // reconfigured from the editorTheme store subscriber.
    const themeCompartment = new Compartment()
    const themeJson = (name: string, keywordFg: string) =>
      JSON.stringify({
        name,
        type: 'dark',
        colors: { 'editor.background': '#101010', 'editor.foreground': '#c0c0c0' },
        tokenColors: [
          { scope: 'keyword', settings: { foreground: keywordFg } },
          { scope: 'constant.numeric', settings: { foreground: '#abcdef' } },
        ],
      })
    for (const [id, kw] of [
      ['alpha', '#ff0000'],
      ['beta', '#00ff00'],
    ] as const) {
      registerThemeMeta(
        { id, name: id, uiTheme: 'dark' } as Parameters<typeof registerThemeMeta>[0],
        parseThemeJson(themeJson(id, kw), id),
      )
    }

    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'const x = 1\n',
        extensions: [
          textmateDocKey('src/theme-app.ts'),
          themeCompartment.of(editorThemeExtension('alpha')),
          textmateLanguage('source.ts'),
        ],
      }),
    })
    const before = markClasses(view)
    const cssBefore = document.getElementById('recur-tm-colormap')?.textContent ?? ''
    expect(before.length).toBeGreaterThan(0)

    view.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension('beta')) })

    const cssAfter = document.getElementById('recur-tm-colormap')?.textContent ?? ''
    expect(cssAfter).not.toBe(cssBefore)
    expect(cssAfter.toLowerCase()).toContain('#00ff00')
    // Marks must correspond to the new colour map, not the old one.
    const colorMap = getColorMap()
    const kwIndex = colorMap.findIndex((c) => c?.toLowerCase() === '#00ff00')
    expect(kwIndex).toBeGreaterThan(0)
    expect(markClassTokens(view)).toContain(`cm-tm-${kwIndex}`)
    assertMarksInDocument(view)
    view.destroy()
  })

  it('repaints a parked tab restored after a theme change', () => {
    // CodeEditor's tab model: one EditorView, inactive tabs keep their
    // EditorState, and a switch back is setState(parked + reconfigure).
    // Diff parks the outgoing tab read-only; restore must clear that too.
    const themeCompartment = new Compartment()
    const roCompartment = new Compartment()
    const editableCompartment = new Compartment()
    const langOf = () => textmateLanguage('source.ts')
    const extensionsFor = (key: string, docKey: string, ro: boolean) => [
      textmateDocKey(docKey),
      themeCompartment.of(editorThemeExtension(key)),
      langOf(),
      roCompartment.of(EditorState.readOnly.of(ro)),
      editableCompartment.of(EditorView.editable.of(!ro)),
    ]

    const parent = document.createElement('div')
    document.body.appendChild(parent)
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: 'const parked = 1\n',
        extensions: extensionsFor('alpha', 'src/parked.ts', true),
      }),
    })
    expect(markClasses(view).length).toBeGreaterThan(0)
    expect(view.state.readOnly).toBe(true)

    // Switch away: park this state, point the view at another file.
    const parked = view.state
    view.setState(
      EditorState.create({
        doc: 'const other = 2\n',
        extensions: extensionsFor('alpha', 'src/other.ts', true),
      }),
    )

    // Theme changes while the tab is parked — only the live view is told.
    view.dispatch({ effects: themeCompartment.reconfigure(editorThemeExtension('beta')) })
    const colorMap = getColorMap()
    const kwIndex = colorMap.findIndex((c) => c?.toLowerCase() === '#00ff00')
    expect(kwIndex).toBeGreaterThan(0)

    // StateEffect.reconfigure.of(compartment.of(x)) keeps the parked map
    // entry — the same footgun mountEditor has to paper over.
    const naive = parked.update({
      effects: StateEffect.reconfigure.of(extensionsFor('beta', 'src/parked.ts', false)),
    }).state
    expect(naive.readOnly).toBe(true)
    expect(naive.facet(EditorView.editable)).toBe(false)

    // Switch back: same effects as mountEditor (reconfigure.of + slot reconfigure).
    const themeExt = editorThemeExtension('beta')
    const restored = parked.update({
      effects: [
        StateEffect.reconfigure.of(extensionsFor('beta', 'src/parked.ts', false)),
        themeCompartment.reconfigure(themeExt),
        roCompartment.reconfigure(EditorState.readOnly.of(false)),
        editableCompartment.reconfigure(EditorView.editable.of(true)),
      ],
    }).state
    view.setState(restored)

    expect(themeCompartment.get(view.state)).toBe(themeExt)
    expect(view.state.readOnly).toBe(false)
    expect(view.state.facet(EditorView.editable)).toBe(true)
    // The restored tab must show the NEW theme's colours, not the old indices.
    expect(markClassTokens(view)).toContain(`cm-tm-${kwIndex}`)
    // Nothing may still carry a colour index that only existed under alpha.
    const liveIndices = new Set(colorMap.map((_, i) => `cm-tm-${i}`))
    for (const cls of markClasses(view)) {
      for (const part of cls.split(' ')) {
        if (part.startsWith('cm-tm-') && /^cm-tm-\d+$/.test(part)) {
          expect(liveIndices.has(part)).toBe(true)
        }
      }
    }
    assertMarksInDocument(view)
    view.destroy()
  })

  it('a theme change drops engine state so any later paint is fresh', () => {
    // The user-visible contract: after a theme change nothing anywhere can
    // still be tokenized under the old theme, so a tab restored later paints
    // exactly as a close-and-reopen would.
    const view = mount('const themed = 1\n')
    const docKey = `src/edit${seq}.ts`
    expect(engine.hasDoc(docKey)).toBe(true)
    applyColorMap(
      engine.setTheme({
        name: 'reset',
        settings: [
          { settings: { foreground: '#123456' } },
          { scope: 'keyword', settings: { foreground: '#654321' } },
        ],
      }),
    )
    // Every document is gone — no stale tokens can survive anywhere.
    expect(engine.hasDoc(docKey)).toBe(false)
    // The live view re-opens itself and repaints under the new theme.
    view.dispatch({})
    expect(engine.hasDoc(docKey)).toBe(true)
    const idx = getColorMap().findIndex((c) => c?.toLowerCase() === '#654321')
    expect(idx).toBeGreaterThan(0)
    expect(markClassTokens(view)).toContain(`cm-tm-${idx}`)
    assertMarksInDocument(view)
    view.destroy()
  })

  it('decorates every line CodeMirror renders, not just the visible subset', async () => {
    const lines: string[] = []
    for (let i = 0; i < 400; i++) lines.push(`const v${i} = ${i}`)
    const view = mount(lines.join('\n'))
    // The constructor paints before the first measure, so the rendered span
    // can widen afterwards. Let layout settle, then drive one more update.
    await new Promise((r) => setTimeout(r, 60))
    view.dispatch({})

    const doc = view.state.doc
    const from = doc.lineAt(Math.min(view.viewport.from, doc.length)).number
    const to = doc.lineAt(Math.min(view.viewport.to, doc.length)).number
    const covered = new Set<number>()
    for (const set of view.state.facet(EditorView.decorations)) {
      const d = typeof set === 'function' ? set(view) : set
      const iter = d.iter()
      while (iter.value) {
        covered.add(doc.lineAt(iter.from).number)
        iter.next()
      }
    }
    const missing: number[] = []
    for (let n = from; n <= to; n++) if (!covered.has(n)) missing.push(n)
    // A rendered line without marks is an unstyled band on screen.
    expect(missing).toEqual([])
    view.destroy()
  })

  it('retokenizes after an edit instead of reusing stale tokens', () => {
    const view = mount('const s = 1\nconst t = 2\n')
    /** Colour index per token on the given line. */
    const lineClasses = (n: number) => {
      const doc = view.state.doc
      const line = doc.line(n)
      const out: string[] = []
      for (const set of view.state.facet(EditorView.decorations)) {
        const d = typeof set === 'function' ? set(view) : set
        const iter = d.iter()
        while (iter.value) {
          if (iter.from >= line.from && iter.to <= line.to) {
            const cls = (iter.value.spec as { class?: string }).class
            if (cls) out.push(cls)
          }
          iter.next()
        }
      }
      return out
    }

    const line2Before = lineClasses(2)
    expect(line2Before.length).toBeGreaterThan(0)
    // Turn line 1 into an unterminated template literal. Line 2 is now inside
    // a string, so its marks must change — with stale tokens they would not.
    const line = view.state.doc.line(1)
    view.dispatch({ changes: { from: line.from, to: line.to, insert: 'const s = `' } })
    assertMarksInDocument(view)
    expect(lineClasses(2)).not.toEqual(line2Before)
    view.destroy()
  })

  it('re-opens when its engine document is dropped underneath it', () => {
    // Reload from disk force-drops the document while the view is still live.
    const view = mount('const reloaded = 1\n')
    const docKey = `src/edit${seq}.ts`
    expect(engine.hasDoc(docKey)).toBe(true)
    const before = markClasses(view)
    expect(before.length).toBeGreaterThan(0)

    engine.closeDoc(docKey)
    expect(engine.hasDoc(docKey)).toBe(false)

    // The next paint must rebuild from the current text, not render unstyled.
    view.dispatch({ changes: { from: view.state.doc.length, insert: 'const b = 2\n' } })
    expect(engine.hasDoc(docKey)).toBe(true)
    expect(markClasses(view).length).toBeGreaterThan(0)
    assertMarksInDocument(view)
    view.destroy()
  })

  it('a shortening edit on a single-line document keeps marks in range', () => {
    // No trailing line to absorb an over-long mark: an unclamped stale token
    // offset lands past doc.length, which CodeMirror rejects when the view
    // consumes the set (outside the build's try/catch).
    const view = mount('const alpha = "a long string literal here"')
    const line = view.state.doc.line(1)
    view.dispatch({ changes: { from: line.from, to: line.to, insert: 'x' } })
    expect(view.state.doc.toString()).toBe('x')
    assertMarksInDocument(view)
    view.destroy()
  })

  it('re-highlights after a bundle reload drops the compiled grammar', async () => {
    const pathHint = 'typescript.tmLanguage.json'
    const content = readFileSync(tsGrammarPath, 'utf8')
    engine.setSourceLoader(async () => ({ content, pathHint }))
    try {
      const view = mount('const reloaded = 1\n')
      expect(markClasses(view).length).toBeGreaterThan(0)

      // Reload re-notes every grammar (dropping the compiled one), then the theme is re-applied.
      engine.noteGrammar('source.ts', pathHint)
      applyColorMap(engine.setTheme({ name: 'reload', settings: [{ settings: { foreground: '#cccccc' } }] }), true)
      view.dispatch({})
      expect(hasGrammar('source.ts')).toBe(false)
      expect(markClasses(view)).toEqual([])

      await vi.waitFor(() => {
        view.dispatch({})
        expect(markClasses(view).length).toBeGreaterThan(0)
      })
      expect(hasGrammar('source.ts')).toBe(true)
      assertMarksInDocument(view)
      view.destroy()
    } finally {
      engine.setSourceLoader(null)
      engine.registerGrammar('source.ts', content, pathHint)
      await engine.ensureGrammar('source.ts')
    }
  })
})
