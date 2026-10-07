import { describe, expect, it } from 'vitest'
import { classForMeta, sameTokens } from './cm-tm-deco'

describe('sameTokens', () => {
  it('compares length and values', () => {
    expect(sameTokens([0, 1, 2, 3], [0, 1, 2, 3])).toBe(true)
    expect(sameTokens([0, 1], [0, 1, 2, 3])).toBe(false)
    expect(sameTokens(undefined, [0, 1])).toBe(false)
    expect(sameTokens([0, 2], [0, 1])).toBe(false)
  })
})

describe('classForMeta', () => {
  it('skips the editor-background wash', () => {
    const colorMap = ['', '#cccccc', '#1e1e1e']
    const meta = (1 << 15) | (2 << 24)
    expect(classForMeta(meta, true, colorMap, ['#1e1e1e'])).toBe('cm-tm-1')
    expect(classForMeta(meta, true, colorMap, ['#000000'])).toBe('cm-tm-1 cm-tm-bg-2')
    expect(classForMeta(meta, false, colorMap, ['#000000'])).toBe('cm-tm-1')
  })

  it('judges foreground contrast against the pane fill, not editor.background', () => {
    const colorMap = ['', '#666666']
    const meta = 1 << 15
    const documentBgs = ['#000000', '#555454']
    expect(classForMeta(meta, false, colorMap, documentBgs, '#000000')).toBe('cm-tm-1')
    expect(classForMeta(meta, false, colorMap, documentBgs, '#555454')).toBe('')
  })
})
