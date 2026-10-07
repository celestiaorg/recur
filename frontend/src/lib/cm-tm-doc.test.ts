import { describe, expect, it } from 'vitest'
import { tmDocFingerprint, tmDocIdFor } from './cm-tm-doc'

describe('tmDocIdFor', () => {
  it('keeps panes on separate engine documents', () => {
    // Both panes can show the same file; sharing an id would make each apply
    // the same edit to one engine document.
    expect(tmDocIdFor('main', 'src/a.ts')).not.toBe(tmDocIdFor('split', 'src/a.ts'))
    expect(tmDocIdFor('main', '')).toBe('')
  })
})

describe('tmDocFingerprint', () => {
  const fpOf = (text: string) => {
    const lines = text.split('\n')
    return tmDocFingerprint((n) => lines[n - 1], lines.length, text.length)
  }

  it('is stable for identical text and moves on any sampled change', () => {
    const text = ['const a = 1', 'const b = 2', 'const c = 3'].join('\n')
    expect(fpOf(text)).toEqual(fpOf(text))
    expect(fpOf(text).sample).not.toBe(fpOf(text.replace('a', 'x')).sample)
  })

  it('distinguishes line-boundary shifts of equal length', () => {
    expect(fpOf('ab\n').sample).not.toBe(fpOf('a\nb').sample)
  })
})
