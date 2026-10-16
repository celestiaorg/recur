/** @vitest-environment happy-dom */
import { describe, expect, it } from 'vitest'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { allEditorThemesForPicker, editorChromeExtension, editorChromeOnlyExtension, editorThemeIdForAppChrome, cycleEditorThemeKey, compareEditorThemesForCycle } from './cm-themes'
import { _resetTmRegistryForTests, applyColorMap, getColorMap, getColorMapEpoch, registerThemeMeta, setDisabledBundleContribs } from './tm-registry'
import { parseThemeJson, tmChromeExtension, tmRootContentExtension } from './tm-theme'

function chromeCaretWidth(thickCaret: boolean): string | null {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: 'x',
      extensions: [editorChromeExtension(12, 'monospace', 1.55, thickCaret)],
    }),
  })
  const css = [...document.querySelectorAll('style')]
    .map((el) => el.textContent ?? '')
    .join('\n')
  const classes = [...view.dom.classList]
  view.destroy()
  parent.remove()
  for (const cls of classes) {
    const re = new RegExp(
      `\\.${cls} \\.cm-cursor, \\.${cls} \\.cm-dropCursor \\{border-left-width: ([^;]+);`,
    )
    const m = css.match(re)
    if (m) return m[1]
  }
  return null
}

function themeCss(extensions: Extension): string {
  const parent = document.createElement('div')
  document.body.appendChild(parent)
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: 'x',
      extensions,
    }),
  })
  const css = [...document.querySelectorAll('style')]
    .map((el) => el.textContent ?? '')
    .join('\n')
  view.destroy()
  parent.remove()
  return css
}

describe('editorChromeExtension caret', () => {
  it('sets a 2px caret by default (same visual weight as the native/diff caret)', () => {
    expect(chromeCaretWidth(false)).toBe('2px')
  })

  it('sets a 3px caret when thickCaret is on', () => {
    expect(chromeCaretWidth(true)).toBe('3px')
  })
})

describe('tmChromeExtension root fills', () => {
  const parsed = parseThemeJson(JSON.stringify({
    name: 'Birds of Paradise',
    type: 'dark',
    colors: { 'editor.background': '#372725', 'editor.foreground': '#E6E1C4' },
    tokenColors: [
      { settings: { foreground: '#E6E1C4', background: '#372725' } },
      { scope: 'source', settings: { background: '#322323' } },
    ],
  }))

  it('exposes source fill as a CSS variable and TextMate-softened gutter chrome', () => {
    const css = themeCss(tmChromeExtension(parsed))
    expect(css).toContain('--cm-tm-source-bg: #322323')
    expect(css).toContain('background-color: #514341')
    expect(parsed.rootBackgrounds.source).toBe('#322323')
  })

  it('puts the pane fill on the scroller so selection behind content still paints', () => {
    const css = themeCss(tmChromeExtension(parsed))
    expect(css).toMatch(/\.cm-scroller \{[^}]*background-color:\s*var\(--cm-tm-content-bg/)
    expect(css).not.toMatch(/\.cm-content \{[^}]*background-color:\s*var\(--cm-tm-content-bg/)
  })

  it('binds source grammars to the source fill variable', () => {
    const css = themeCss([tmChromeExtension(parsed), tmRootContentExtension('source.ts')])
    expect(css).toContain('--cm-tm-content-bg: var(--cm-tm-source-bg)')
  })

  it('binds text grammars to the text fill variable', () => {
    const css = themeCss([tmChromeExtension(parsed), tmRootContentExtension('text.html.markdown')])
    expect(css).toContain('--cm-tm-content-bg: var(--cm-tm-text-bg)')
  })
})

describe('tmChromeExtension selection', () => {
  it('uses the theme selection wash, not a hardcoded purple', () => {
    const charcoal = parseThemeJson(JSON.stringify({
      name: 'Charcoal',
      type: 'dark',
      colors: {
        'editor.background': '#0e0e0e',
        'editor.foreground': '#e6e4e1',
        'editor.selectionBackground': '#e07a5f47',
        'editorGutter.background': '#161616',
      },
      tokenColors: [{ settings: { foreground: '#e6e4e1', background: '#0e0e0e' } }],
    }))
    const css = themeCss(tmChromeExtension(charcoal))
    expect(css).toContain('#e07a5f47')
    expect(css).not.toContain('rgba(128,128,255')
    expect(css).toContain(
      '.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground',
    )
  })
})

describe('compareEditorThemesForCycle', () => {
  it('orders dark A–Z with Charcoal first, then light A–Z with Paper first', () => {
    const cards = [
      { key: 'zenburn', name: 'Zenburn', kind: 'dark' as const },
      { key: 'paper', name: 'Paper', kind: 'light' as const },
      { key: 'amy', name: 'Amy', kind: 'dark' as const },
      { key: 'recur', name: 'Charcoal', kind: 'dark' as const },
      { key: 'dawn', name: 'Dawn', kind: 'light' as const },
    ]
    expect([...cards].sort(compareEditorThemesForCycle).map((t) => t.key)).toEqual([
      'recur', 'amy', 'zenburn', 'paper', 'dawn',
    ])
  })
})

describe('cycleEditorThemeKey', () => {
  const keys = ['recur', 'amy', 'zenburn', 'paper', 'dawn']

  it('steps forward through darks then lights and wraps', () => {
    expect(cycleEditorThemeKey('recur', 1, keys)).toBe('amy')
    expect(cycleEditorThemeKey('zenburn', 1, keys)).toBe('paper')
    expect(cycleEditorThemeKey('dawn', 1, keys)).toBe('recur')
  })

  it('steps backward and wraps', () => {
    expect(cycleEditorThemeKey('paper', -1, keys)).toBe('zenburn')
    expect(cycleEditorThemeKey('recur', -1, keys)).toBe('dawn')
  })

  it('starts at the first theme when the current key is unknown', () => {
    expect(cycleEditorThemeKey('missing', 1, keys)).toBe('amy')
    expect(cycleEditorThemeKey('missing', -1, keys)).toBe('dawn')
  })
})

describe('editorThemeIdForAppChrome', () => {
  it('pairs each app chrome theme with a matching editor theme', () => {
    _resetTmRegistryForTests()
    for (const [id, name, ui] of [
      ['recur', 'Charcoal', 'dark'],
      ['paper', 'Paper', 'light'],
      ['github-dark-default', 'GitHub Dark Default', 'dark'],
      ['cool-glow', 'Cool Glow', 'dark'],
      ['catppuccin-mocha', 'Catppuccin Mocha', 'dark'],
      ['catppuccin-latte', 'Catppuccin Latte', 'light'],
      ['nord', 'Nord', 'dark'],
    ] as const) {
      registerThemeMeta(
        { id, name, uiTheme: ui } as Parameters<typeof registerThemeMeta>[0],
        parseThemeJson(JSON.stringify({ name, type: ui, colors: {} }), id),
      )
    }
    expect(editorThemeIdForAppChrome('default')).toBe('recur')
    expect(editorThemeIdForAppChrome('paper')).toBe('paper')
    expect(editorThemeIdForAppChrome('slate')).toBe('github-dark-default')
    expect(editorThemeIdForAppChrome('coolglow')).toBe('cool-glow')
    expect(editorThemeIdForAppChrome('catppuccin-mocha')).toBe('catppuccin-mocha')
    expect(editorThemeIdForAppChrome('catppuccin-latte')).toBe('catppuccin-latte')
    expect(editorThemeIdForAppChrome('nord')).toBe('nord')
    expect(editorThemeIdForAppChrome('unknown')).toBe('recur')
    _resetTmRegistryForTests()
  })

  it('skips a matching editor theme the user disabled in Bundles', () => {
    _resetTmRegistryForTests()
    registerThemeMeta(
      { id: 'recur', name: 'Charcoal', uiTheme: 'dark' } as Parameters<typeof registerThemeMeta>[0],
      parseThemeJson(JSON.stringify({ name: 'Charcoal', type: 'dark', colors: {} }), 'recur'),
    )
    registerThemeMeta(
      { id: 'nord', name: 'Nord', uiTheme: 'dark' } as Parameters<typeof registerThemeMeta>[0],
      parseThemeJson(JSON.stringify({ name: 'Nord', type: 'dark', colors: {} }), 'nord'),
    )
    expect(editorThemeIdForAppChrome('nord')).toBe('nord')
    setDisabledBundleContribs({ themeIds: ['nord'] })
    expect(editorThemeIdForAppChrome('nord')).toBe('recur')
    _resetTmRegistryForTests()
  })
})

describe('editorChromeOnlyExtension', () => {
  it('paints Charcoal chrome without publishing a global colour map', () => {
    _resetTmRegistryForTests()
    registerThemeMeta(
      { id: 'recur', name: 'Charcoal', uiTheme: 'dark' } as Parameters<typeof registerThemeMeta>[0],
      parseThemeJson(JSON.stringify({
        name: 'Charcoal',
        type: 'dark',
        colors: {
          'editor.background': '#0e0e0e',
          'editor.foreground': '#e6e4e1',
          'editor.selectionBackground': '#3c3936',
        },
        tokenColors: [{ settings: { foreground: '#e6e4e1', background: '#0e0e0e' } }],
      }), 'recur'),
    )
    applyColorMap(['', '#ff00ff'])
    const epoch = getColorMapEpoch()
    const css = themeCss(editorChromeOnlyExtension('recur'))
    expect(css).toContain('#0e0e0e')
    expect(getColorMap()).toEqual(['', '#ff00ff'])
    expect(getColorMapEpoch()).toBe(epoch)
    _resetTmRegistryForTests()
  })
})

describe('allEditorThemesForPicker', () => {
  it('omits themes disabled in Settings', () => {
    _resetTmRegistryForTests()
    registerThemeMeta(
      { id: 'recur', name: 'Charcoal', uiTheme: 'dark', bundleName: 'Recur' } as Parameters<typeof registerThemeMeta>[0],
      parseThemeJson(JSON.stringify({ name: 'Charcoal', type: 'dark', colors: {} }), 'recur'),
    )
    registerThemeMeta(
      { id: 'cool-glow', name: 'Cool Glow', uiTheme: 'dark', bundleName: 'Themes' } as Parameters<typeof registerThemeMeta>[0],
      parseThemeJson(JSON.stringify({ name: 'Cool Glow', type: 'dark', colors: {} }), 'cool-glow'),
    )
    expect(allEditorThemesForPicker().map((t) => t.key).sort()).toEqual(['cool-glow', 'recur'])
    setDisabledBundleContribs({ themeIds: ['cool-glow'] })
    expect(allEditorThemesForPicker().map((t) => t.key)).toEqual(['recur'])
    _resetTmRegistryForTests()
  })
})
