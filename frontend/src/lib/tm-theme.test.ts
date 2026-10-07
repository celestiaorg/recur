import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  colorMapCss,
  cssColorAlpha,
  cssColorKey,
  decodeBackground,
  decodeForeground,
  ensureGlobalThemeChrome,
  extractRootScopeBackgrounds,
  isCssColorDark,
  isCssTransparent,
  isRootLanguageScope,
  isTextMateTextScope,
  parseThemeJson,
  resolveGutterBackground,
  resolveGutterBorder,
  resolveGutterHintColor,
  resolveLineHighlightBackground,
  resolveLineNumberForeground,
  resolveSelectionBackground,
  shouldPaintTokenBackground,
  tokenForegroundReadable,
  compositeCssColor,
  softenCssColor,
  stripRootScopeTokenBackgrounds,
  themeScopeSelectors,
  toRawTheme,
  withCssAlpha,
} from './tm-theme'
import {
  addGrammar,
  applyLanguageMode,
  detectScopeForPath,
  languageDisplayName,
  languageLabelForPath,
  listSyntaxLanguages,
  resolveScopeForPath,
  setDisabledBundleContribs,
  _resetTmRegistryForTests,
} from './tm-registry'
import {
  setSyntaxForFileType,
  setSyntaxForPath,
  syntaxPathOverride,
} from './syntax-mode'
import {
  genericRgbHexToSrgb,
  isGenericRgbColorSpace,
} from '../../../scripts/generic-rgb-to-srgb.mjs'

describe('tm-theme', () => {
  it('converts VS Code tokenColors to IRawTheme settings', () => {
    const raw = toRawTheme({
      name: 'Demo',
      tokenColors: [
        { settings: { foreground: '#fff', background: '#000' } },
        { scope: 'keyword', settings: { foreground: '#f0f' } },
      ],
    })
    expect(raw.name).toBe('Demo')
    expect(raw.settings.length).toBe(2)
    expect(raw.settings[1].scope).toBe('keyword')
  })

  it('parses cool-glow-shaped JSON', () => {
    const parsed = parseThemeJson(JSON.stringify({
      name: 'Cool Glow',
      type: 'dark',
      colors: { 'editor.background': '#06071D' },
      tokenColors: [
        { settings: { foreground: '#E0E0E0', background: '#06071D' } },
        { scope: ['keyword'], settings: { foreground: '#2BF1DC' } },
      ],
    }))
    expect(parsed.uiTheme).toBe('dark')
    expect(parsed.colors['editor.background']).toBe('#06071D')
    expect(parsed.rawTheme.settings.length).toBeGreaterThan(0)
  })

  it('decodes tokenizeLine2 foreground and background bits', () => {
    // fg index 9 at offset 15, bg index 4 at offset 24
    const meta = (9 << 15) | (4 << 24)
    expect(decodeForeground(meta)).toBe(9)
    expect(decodeBackground(meta)).toBe(4)
    expect(decodeBackground(9 << 15)).toBe(0)
  })

  it('colorMapCss emits foreground and background classes', () => {
    const css = colorMapCss(['', '#fff', '#102622FA'])
    expect(css).toContain('.cm-tm-1 { color: #fff; }')
    expect(css).toContain('.cm-tm-bg-1 { background-color: #fff;')
    expect(css).toContain('.cm-tm-2 { color: #102622FA; }')
    expect(css).toContain('.cm-tm-bg-2 { background-color: #102622FA;')
    expect(css).not.toContain('.cm-tm-bg-0')
    expect(css).toContain('.cm-tm-strike')
  })

  it('fills missing global background so textmate does not default to #ffffff', () => {
    const raw = toRawTheme({
      name: 'Gruv',
      colors: { 'editor.background': '#1d2021', 'editor.foreground': '#ebdbb2' },
      tokenColors: [
        { settings: { foreground: '#ebdbb2' } },
        { scope: 'keyword', settings: { foreground: '#fb4934' } },
      ],
    })
    const global = raw.settings.find((s) => !s.scope)
    expect(global?.settings?.background).toBe('#1d2021')
    expect(global?.settings?.foreground).toBe('#ebdbb2')
  })

  it('cssColorKey ignores alpha and expands short hex', () => {
    expect(cssColorKey('#1D2021')).toBe('#1d2021')
    expect(cssColorKey('#102622FA')).toBe('#102622')
    expect(cssColorKey('#abc')).toBe('#aabbcc')
  })

  it('ensureGlobalThemeChrome does not overwrite an explicit background', () => {
    const settings = [{ settings: { foreground: '#fff', background: '#090916' } }]
    ensureGlobalThemeChrome(settings, { 'editor.background': '#1e1e1e' })
    expect(settings[0].settings.background).toBe('#090916')
  })

  it('isCssTransparent detects alpha-0 hex and transparent keyword', () => {
    expect(isCssTransparent(undefined)).toBe(true)
    expect(isCssTransparent('transparent')).toBe(true)
    expect(isCssTransparent('#2d353b00')).toBe(true)
    expect(isCssTransparent('#0000')).toBe(true)
    expect(isCssTransparent('#161616')).toBe(false)
  })

  it('resolveGutterBackground uses explicit gutter or TextMate soften of editor bg', () => {
    expect(
      resolveGutterBackground(
        { 'editorGutter.background': '#161616' },
        '#0e0e0e',
      ),
    ).toBe('#161616')
    expect(
      resolveGutterBackground(
        { 'editorGutter.background': '#2d353b00' },
        '#372725',
      ),
    ).toBe(softenCssColor('#372725', 0.87))
    expect(resolveGutterBackground({}, '#372725')).toBe('#514341')
  })

  it('resolveGutterBorder prefers divider / editorGroup.border, else theme-aware fallback', () => {
    expect(resolveGutterBorder({ 'editorGroup.border': '#3c3836' }, '#E6E1C4', 'dark')).toBe(
      '#3c3836',
    )
    expect(resolveGutterBorder({ 'gutter.divider': '#5c5a4e' }, '#E6E1C4', 'dark')).toBe(
      '#5c5a4e',
    )
    // Classic TM (no VS Code gutter fill): TextMate soften of foreground.
    expect(resolveGutterBorder({}, '#E6E1C4', 'dark')).toBe('#5c5a4e')
    // Charcoal/Paper-style: explicit gutter fill keeps the Recur hairline.
    expect(
      resolveGutterBorder(
        { 'editorGutter.background': '#161616' },
        '#e6e4e1',
        'dark',
      ),
    ).toBe('#282828')
    expect(
      resolveGutterBorder(
        { 'editorGutter.background': '#ebecef' },
        '#1a1c20',
        'light',
      ),
    ).toBe('#cfd3db')
  })

  it('softens Birds of Paradise chrome the way TextMate theme.cc does', () => {
    expect(isCssColorDark('#372725')).toBe(true)
    expect(isCssColorDark('#E6E1C4')).toBe(false)
    expect(softenCssColor('#372725', 0.87)).toBe('#514341')
    expect(softenCssColor('#E6E1C4', 0.4)).toBe('#5c5a4e')
    expect(softenCssColor('#E6E1C4', 0.5)).toBe('#737162')
    expect(resolveLineNumberForeground({}, '#E6E1C4')).toBe('#737162')
    expect(
      resolveLineNumberForeground({ 'editorLineNumber.foreground': '#888' }, '#E6E1C4'),
    ).toBe('#888')
  })

  it('resolveGutterHintColor prefers opaque theme keys over git tokens', () => {
    expect(
      resolveGutterHintColor(
        { 'editorGutter.addedBackground': '#1e754f' },
        'editorGutter.addedBackground',
        'var(--git-a)',
      ),
    ).toBe('#1e754f')
    expect(
      resolveGutterHintColor(
        { 'editorGutter.addedBackground': '#2d353b00' },
        'editorGutter.addedBackground',
        'var(--git-a)',
      ),
    ).toBe('var(--git-a)')
  })

  it('maps classic gutterSettings onto VS Code colour keys', () => {
    const parsed = parseThemeJson(JSON.stringify({
      name: 'Guttered',
      type: 'dark',
      colors: { 'editor.background': '#000000', 'editor.foreground': '#ffffff' },
      gutterSettings: {
        background: '#111111',
        foreground: '#aaaaaa',
        divider: '#333333',
      },
      tokenColors: [{ settings: { foreground: '#ffffff', background: '#000000' } }],
    }))
    expect(parsed.colors['editorGutter.background']).toBe('#111111')
    expect(parsed.colors['editorLineNumber.foreground']).toBe('#aaaaaa')
    expect(parsed.colors['gutter.divider']).toBe('#333333')
  })

  it('lifts classic unscoped selection / lineHighlight / caret into colors', () => {
    const parsed = parseThemeJson(JSON.stringify({
      name: 'Birds',
      type: 'dark',
      tokenColors: [
        {
          settings: {
            foreground: '#E6E1C4',
            background: '#372725',
            caret: '#E6E1C4',
            selection: '#16120E',
            lineHighlight: '#1F1611',
            invisibles: '#42302D',
          },
        },
      ],
    }))
    expect(parsed.colors['editor.selectionBackground']).toBe('#16120E')
    expect(parsed.colors['editor.lineHighlightBackground']).toBe('#1F1611')
    expect(parsed.colors['editorCursor.foreground']).toBe('#E6E1C4')
    expect(parsed.colors['editorWhitespace.foreground']).toBe('#42302D')
  })

  it('keeps explicit selection and derives a theme-tinted wash when omitted', () => {
    expect(withCssAlpha('#E6E1C4', 0x4d)).toBe('#e6e1c44d')
    expect(
      resolveSelectionBackground({ 'editor.selectionBackground': '#e07a5f47' }, '#e6e4e1', 'dark'),
    ).toBe('#e07a5f47')
    expect(resolveSelectionBackground({}, '#E6E1C4', 'dark')).toBe('#e6e1c44d')
    expect(
      resolveLineHighlightBackground(
        { 'editor.lineHighlightBackground': '#ffffff0f' },
        '#0e0e0e',
        'dark',
      ),
    ).toBe('#ffffff0f')
    expect(resolveLineHighlightBackground({}, '#372725', 'dark')).toBe(
      softenCssColor('#372725', 0.95),
    )
  })

  it('themeScopeSelectors splits comma lists but keeps compound selectors', () => {
    expect(themeScopeSelectors('source')).toEqual(['source'])
    expect(themeScopeSelectors('source, text')).toEqual(['source', 'text'])
    expect(themeScopeSelectors(['text source', 'meta.embedded'])).toEqual([
      'text source',
      'meta.embedded',
    ])
    expect(isRootLanguageScope('source')).toBe(true)
    expect(isRootLanguageScope('text')).toBe(true)
    expect(isRootLanguageScope('source, text')).toBe(true)
    expect(isRootLanguageScope('source.ts')).toBe(false)
    expect(isRootLanguageScope('text source')).toBe(false)
    expect(isRootLanguageScope('meta.embedded')).toBe(false)
    expect(isTextMateTextScope('text.html')).toBe(true)
    expect(isTextMateTextScope('source.svelte')).toBe(false)
  })

  it('strips root source/text token backgrounds and keeps them as pane fills', () => {
    const parsed = parseThemeJson(JSON.stringify({
      name: 'Birds of Paradise',
      type: 'dark',
      colors: { 'editor.background': '#372725', 'editor.foreground': '#E6E1C4' },
      tokenColors: [
        { settings: { foreground: '#E6E1C4', background: '#372725' } },
        {
          name: 'Source',
          scope: 'source',
          settings: { foreground: '#E6E1C4', background: '#322323' },
        },
        { name: 'Keyword', scope: 'keyword', settings: { foreground: '#EF5D32' } },
        {
          name: 'Invalid',
          scope: 'invalid',
          settings: { foreground: '#FFFFFF', background: '#990000' },
        },
      ],
    }))
    expect(parsed.rootBackgrounds.source).toBe('#322323')
    expect(parsed.rootBackgrounds.text).toBeUndefined()
    const sourceRule = parsed.rawTheme.settings.find((s) => s.scope === 'source')
    expect(sourceRule?.settings?.background).toBeUndefined()
    expect(sourceRule?.settings?.foreground).toBe('#E6E1C4')
    const invalid = parsed.rawTheme.settings.find((s) => s.scope === 'invalid')
    expect(invalid?.settings?.background).toBe('#990000')
  })

  it('does not strip compound selectors like text source / meta.embedded', () => {
    const settings = [
      { scope: ['text source', 'meta.embedded'], settings: { background: '#B1B3BA08' } },
    ]
    expect(extractRootScopeBackgrounds(settings).source).toBeUndefined()
    stripRootScopeTokenBackgrounds(settings)
    expect(settings[0].settings.background).toBe('#B1B3BA08')
  })

  it('shouldPaintTokenBackground skips editor fill and near-transparent washes', () => {
    expect(cssColorAlpha('#B1B3BA08')).toBeCloseTo(8 / 255)
    expect(cssColorAlpha('#102622FA')).toBeCloseTo(250 / 255)
    expect(cssColorAlpha('#990000')).toBe(1)
    const doc = ['#372725', '#322323']
    expect(shouldPaintTokenBackground('#322323', doc)).toBe(false)
    expect(shouldPaintTokenBackground('#372725', doc)).toBe(false)
    expect(shouldPaintTokenBackground('#B1B3BA08', ['#090916'])).toBe(false)
    expect(shouldPaintTokenBackground('#102622FA', ['#090916'])).toBe(true)
    expect(shouldPaintTokenBackground('#990000', doc)).toBe(true)
    expect(shouldPaintTokenBackground('#803D0033', ['#0D0D0DFA'])).toBe(true)
    expect(shouldPaintTokenBackground(undefined, doc)).toBe(false)
  })

  it('drops token foregrounds that vanish into the editor background', () => {
    expect(tokenForegroundReadable('#00000080', '#0D0D0D')).toBe(false)
    expect(tokenForegroundReadable('#EEEEEE', '#0D0D0D')).toBe(true)
    expect(tokenForegroundReadable('#FFFC80', '#0D0D0D')).toBe(true)
    expect(compositeCssColor('#803D0033', '#0D0D0D')).toMatch(/^#[0-9a-f]{6}$/)
  })

  it('maps Apple Generic RGB hex to sRGB (tmcolorconv)', () => {
    expect(isGenericRgbColorSpace('Generic RGB')).toBe(true)
    expect(isGenericRgbColorSpace('Apple Generic RGB')).toBe(true)
    expect(isGenericRgbColorSpace('sRGB')).toBe(false)
    expect(isGenericRgbColorSpace(undefined)).toBe(false)
    // Vectors from jibsen/tmcolorconv (gamma 461/256, GenericRGBtosRGB matrix).
    expect(genericRgbHexToSrgb('#272822')).toBe('#34352D')
    expect(genericRgbHexToSrgb('#372725')).toBe('#473431')
    expect(genericRgbHexToSrgb('#A40042')).toBe('#B51C53')
    expect(genericRgbHexToSrgb('#281C1C66')).toBe('#35262566')
    expect(genericRgbHexToSrgb('#000000')).toBe('#000000')
    expect(genericRgbHexToSrgb('#FFFFFF')).toBe('#FFFFFF')
    expect(genericRgbHexToSrgb('#808080')).toBe('#929292')
  })

  it('paints theme hex as-is; classic ports are converted at port time', () => {
    const leftover = parseThemeJson(JSON.stringify({
      name: 'Monokai',
      type: 'dark',
      colorSpaceName: 'Generic RGB',
      colors: { 'editor.background': '#272822', 'editor.foreground': '#F8F8F2' },
      tokenColors: [
        { settings: { foreground: '#F8F8F2', background: '#272822' } },
        { scope: 'keyword', settings: { foreground: '#F92672' } },
      ],
    }))
    expect(leftover.colors['editor.background']).toBe('#272822')
    expect(leftover.colors['editor.foreground']).toBe('#F8F8F2')

    const srgb = parseThemeJson(JSON.stringify({
      name: 'Charcoal',
      type: 'dark',
      colors: { 'editor.background': '#272822', 'editor.foreground': '#F8F8F2' },
      tokenColors: [{ settings: { foreground: '#F8F8F2', background: '#272822' } }],
    }))
    expect(srgb.colors['editor.background']).toBe('#272822')
    expect(srgb.colors['editor.foreground']).toBe('#F8F8F2')
  })

  it('bakes classic ports to sRGB at port time', () => {
    const root = join(dirname(fileURLToPath(import.meta.url)), '../../..')
    const themeDir = join(root, 'bundles/Themes/themes')
    const filmgirl = JSON.parse(
      readFileSync(join(root, 'bundles/Themes/filmgirl-local.json'), 'utf8'),
    ) as Array<{ file: string }>
    const classic = [
      ...filmgirl.map((t) => t.file),
      'made-of-code.tmTheme.json',
      'birds-of-paradise.tmTheme.json',
    ]
    for (const file of new Set(classic)) {
      const json = JSON.parse(readFileSync(join(themeDir, file), 'utf8')) as { colorSpaceName?: string }
      expect(json.colorSpaceName, file).toBe('sRGB')
    }

    const srgbOnly = ['catppuccin-frappe.tmTheme.json', 'aura.tmTheme.json']
    for (const file of srgbOnly) {
      const json = JSON.parse(readFileSync(join(themeDir, file), 'utf8')) as { colorSpaceName?: string }
      expect(json.colorSpaceName, file).toBeUndefined()
    }
    const charcoal = JSON.parse(
      readFileSync(join(root, 'bundles/Recur/themes/charcoal.tmTheme.json'), 'utf8'),
    ) as { colorSpaceName?: string; colors: Record<string, string> }
    expect(charcoal.colorSpaceName).toBeUndefined()
    expect(parseThemeJson(JSON.stringify(charcoal)).colors['editor.background']).toBe(
      charcoal.colors['editor.background'],
    )

    const birds = parseThemeJson(
      readFileSync(join(themeDir, 'birds-of-paradise.tmTheme.json'), 'utf8'),
    )
    const monokai = parseThemeJson(
      readFileSync(join(themeDir, 'monokai.tmTheme.json'), 'utf8'),
    )
    const coolGlow = parseThemeJson(
      readFileSync(join(themeDir, 'cool-glow.tmTheme.json'), 'utf8'),
    )
    const madeOfCode = parseThemeJson(
      readFileSync(join(themeDir, 'made-of-code.tmTheme.json'), 'utf8'),
    )
    expect(birds.colors['editor.background']).toBe('#473431')
    expect(birds.colors['editor.selectionBackground']).toBe('#B51C53')
    expect(monokai.colors['editor.background']).toBe('#34352D')
    expect(monokai.colors['editor.selectionBackground']).toBe('#5B5A4F')
    expect(coolGlow.colors['editor.background']).toBe('#040727FA')
    expect(madeOfCode.colors['editor.background']).toBe('#08091D')
  })
})

describe('tm-registry resolveScopeForPath', () => {
  it('matches fileTypes including dotted extensions', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.demo',
        scopeName: 'source.demo',
        path: 'syntaxes/demo.tmLanguage.json',
        fileTypes: ['demo'],
        bundleName: 'Demo',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    expect(resolveScopeForPath('foo/bar.demo')).toBe('source.demo')
    expect(resolveScopeForPath('readme.md')).toBeNull()
    _resetTmRegistryForTests()
  })

  it('prefers basename matches and maps .env.* to dotenv', () => {
    _resetTmRegistryForTests()
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
      async () => '{}',
    )
    addGrammar(
      {
        id: 'recur.languages.go',
        scopeName: 'source.go',
        path: 'syntaxes/go.tmLanguage.json',
        fileTypes: ['go', 'go.mod'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    expect(resolveScopeForPath('.env')).toBe('source.dotenv')
    expect(resolveScopeForPath('.env.local')).toBe('source.dotenv')
    expect(resolveScopeForPath('go.mod')).toBe('source.go')
    _resetTmRegistryForTests()
  })

  it('languageLabelForPath prefers [[grammar]].language', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.solidity',
        scopeName: 'source.solidity',
        path: 'syntaxes/solidity.tmLanguage.json',
        language: 'solidity',
        fileTypes: ['sol'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    expect(languageLabelForPath('contracts/Token.sol')).toBe('Solidity')
    expect(languageLabelForPath('readme.unknown')).toBe('')
    _resetTmRegistryForTests()
  })

  it('skips grammars disabled in Settings', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.demo',
        scopeName: 'source.demo',
        path: 'syntaxes/demo.tmLanguage.json',
        language: 'demo',
        fileTypes: ['demo'],
        bundleName: 'Demo',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    expect(resolveScopeForPath('foo/bar.demo')).toBe('source.demo')
    setDisabledBundleContribs({ grammarIds: ['recur.languages.demo'] })
    expect(resolveScopeForPath('foo/bar.demo')).toBeNull()
    expect(languageLabelForPath('foo/bar.demo')).toBe('')
    setDisabledBundleContribs({ grammarIds: [] })
    expect(resolveScopeForPath('foo/bar.demo')).toBe('source.demo')
    _resetTmRegistryForTests()
  })

  it('honours a user language-mode override', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.go',
        scopeName: 'source.go',
        path: 'syntaxes/go.tmLanguage.json',
        language: 'go',
        fileTypes: ['go'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    addGrammar(
      {
        id: 'recur.languages.yaml',
        scopeName: 'source.yaml',
        path: 'syntaxes/yaml.tmLanguage.json',
        language: 'yaml',
        fileTypes: ['yaml', 'yml'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    expect(resolveScopeForPath('src/main.go')).toBe('source.go')
    setSyntaxForPath('src/main.go', 'source.yaml')
    expect(resolveScopeForPath('src/main.go')).toBe('source.yaml')
    expect(languageLabelForPath('src/main.go')).toBe('YAML')
    expect(detectScopeForPath('src/main.go')).toBe('source.go')
    setSyntaxForPath('src/main.go', null)
    expect(resolveScopeForPath('src/main.go')).toBe('source.go')
    applyLanguageMode('src/main.go', 'source.yaml')
    expect(syntaxPathOverride('src/main.go')).toBe('source.yaml')
    applyLanguageMode('src/main.go', 'source.go')
    expect(syntaxPathOverride('src/main.go')).toBeUndefined()
    _resetTmRegistryForTests()
  })

  it('stores a path override when the pick differs from a file-type association', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.markdown',
        scopeName: 'text.html.markdown',
        path: 'syntaxes/markdown.tmLanguage.json',
        language: 'markdown',
        fileTypes: ['md'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    addGrammar(
      {
        id: 'recur.languages.yaml',
        scopeName: 'source.yaml',
        path: 'syntaxes/yaml.tmLanguage.json',
        language: 'yaml',
        fileTypes: ['yaml', 'yml'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    setSyntaxForFileType('*.md', 'source.yaml')
    expect(resolveScopeForPath('README.md')).toBe('source.yaml')
    applyLanguageMode('README.md', 'text.html.markdown')
    expect(syntaxPathOverride('README.md')).toBe('text.html.markdown')
    expect(resolveScopeForPath('README.md')).toBe('text.html.markdown')
    applyLanguageMode('README.md', null)
    expect(syntaxPathOverride('README.md')).toBe('text.html.markdown')
    applyLanguageMode('README.md', 'source.yaml')
    expect(syntaxPathOverride('README.md')).toBeUndefined()
    expect(resolveScopeForPath('README.md')).toBe('source.yaml')
    _resetTmRegistryForTests()
  })

  it('omits injection-only grammars from the language picker', () => {
    _resetTmRegistryForTests()
    addGrammar(
      {
        id: 'recur.languages.go',
        scopeName: 'source.go',
        path: 'syntaxes/go.tmLanguage.json',
        language: 'go',
        fileTypes: ['go'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    addGrammar(
      {
        id: 'recur.languages.jsdoc',
        scopeName: 'source.jsdoc',
        path: 'syntaxes/jsdoc.tmLanguage.json',
        language: 'jsdoc',
        injectTo: ['source.js', 'source.ts'],
        bundleName: 'Languages',
        sourceRoot: 'default',
      },
      async () => '{}',
    )
    const langs = listSyntaxLanguages()
    expect(langs.map((l) => l.scopeName)).toEqual(['source.go'])
    _resetTmRegistryForTests()
  })
})

describe('languageDisplayName', () => {
  it('title-cases and aliases common ids', () => {
    expect(languageDisplayName('go')).toBe('Go')
    expect(languageDisplayName('cpp')).toBe('C++')
    expect(languageDisplayName('json')).toBe('JSON')
    expect(languageDisplayName('objective-c')).toBe('Objective-C')
  })
})
