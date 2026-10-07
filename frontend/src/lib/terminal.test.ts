/** @vitest-environment happy-dom */
import { afterEach, describe, expect, test } from 'vitest'
import { get } from 'svelte/store'
import type { ITheme } from '@xterm/xterm'
import { TerminalThemeInfo, TerminalPaletteInfo } from '../../bindings/recur/internal/app/models'
import { resolveTerminalTheme, terminalFrosts, terminalSurfaceBg, terminalTheme, terminalThemeCatalog } from './terminal'

afterEach(() => {
  terminalThemeCatalog.set([])
})

function bundled(id: string, palette: ITheme, uiTheme = 'dark') {
  return new TerminalThemeInfo({
    id,
    name: id,
    path: `themes/${id}.json`,
    uiTheme,
    palette: new TerminalPaletteInfo(palette),
    bundleName: 'Terminal',
    sourceRoot: 'default',
  })
}

describe('resolveTerminalTheme', () => {
  test('empty id uses app chrome fallbacks', () => {
    const t = resolveTerminalTheme('')
    expect(t.background).toBe('#141414')
    expect(t.foreground).toBe('#e6e4e1')
  })

  test('bundle id applies the ITheme palette', () => {
    terminalThemeCatalog.set([bundled('dracula', {
      background: '#282a36',
      foreground: '#f8f8f2',
      red: '#ff5555',
    })])
    const t = resolveTerminalTheme('dracula')
    expect(t.background).toBe('#282a36')
    expect(t.foreground).toBe('#f8f8f2')
    expect(t.red).toBe('#ff5555')
  })

  test('unknown id falls back to app chrome', () => {
    const t = resolveTerminalTheme('missing')
    expect(t.background).toBe('#141414')
  })

  test('surface bg stays opaque for pane chrome', () => {
    terminalThemeCatalog.set([bundled('homebrew', {
      background: '#000000',
      foreground: '#00ff00',
    })])
    expect(terminalSurfaceBg('homebrew')).toBe('#000000')
    expect(resolveTerminalTheme('homebrew', { transparentBg: true }).background).toBe('#00000000')
  })

  test('only dark palettes frost', () => {
    terminalThemeCatalog.set([
      bundled('solarized-light', { background: '#fdf6e3', foreground: '#657b83' }, 'light'),
      bundled('homebrew', { background: '#000000', foreground: '#00ff00' }),
    ])
    expect(terminalFrosts('solarized-light')).toBe(false)
    expect(terminalFrosts('homebrew')).toBe(true)
    expect(terminalFrosts('')).toBe(true)
  })
})

describe('terminalTheme catalog store', () => {
  test('exposes registered palettes', () => {
    terminalThemeCatalog.set([bundled('nord', { background: '#2e3440', foreground: '#d8dee9' })])
    expect(get(terminalThemeCatalog)).toHaveLength(1)
    expect(terminalTheme({ palette: { background: '#000', foreground: '#fff' } }).background).toBe('#000')
  })
})
