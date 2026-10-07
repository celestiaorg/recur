/** @vitest-environment happy-dom */
import { beforeEach, describe, expect, it } from 'vitest'
import { get } from 'svelte/store'
import {
  CODE_PANEL_KEYS, PANEL_DEFAULTS, codePanelSides, initCodePanelSides, obsToolbarMinWidth,
  panelWidths, setCodePanelSide, setPanelWidth,
} from './stores'

beforeEach(() => {
  localStorage.clear()
  codePanelSides.set({
    maestro: 'left', plan: 'left', findings: 'left',
    files: 'right', scm: 'right', outline: 'right', search: 'right', refs: 'right', artifacts: 'right',
  })
  panelWidths.set({ ...PANEL_DEFAULTS })
})

describe('obsToolbarMinWidth', () => {
  it('is hide-toggle chrome plus each 28px pane icon', () => {
    expect(obsToolbarMinWidth(0)).toBe(47)
    expect(obsToolbarMinWidth(3)).toBe(140)
    expect(obsToolbarMinWidth(9)).toBe(314)
  })
})

describe('observatory dock min', () => {
  it('does not shrink the left dock below its header icons', () => {
    for (const key of CODE_PANEL_KEYS) setCodePanelSide(key, 'left')
    setPanelWidth('maestroDock', 80)
    expect(get(panelWidths).maestroDock).toBe(obsToolbarMinWidth(CODE_PANEL_KEYS.length))
  })

  it('floors a saved narrow right dock after sides restore', () => {
    localStorage.setItem('recur.panelWidths', JSON.stringify({ obsSide: 80 }))
    localStorage.setItem('recur.codePanelSides', JSON.stringify(
      Object.fromEntries(CODE_PANEL_KEYS.map((k) => [k, 'right'])),
    ))
    panelWidths.set({ ...PANEL_DEFAULTS, obsSide: 80 })
    initCodePanelSides()
    expect(get(panelWidths).obsSide).toBe(obsToolbarMinWidth(CODE_PANEL_KEYS.length))
  })
})
