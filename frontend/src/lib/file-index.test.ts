import { describe, expect, test } from 'vitest'
import { mapFileSearchHits } from './file-index'

describe('mapFileSearchHits', () => {
  test('marks a path as a directory when Go sets dir', () => {
    const hits = mapFileSearchHits([
      { path: 'src/', score: 10, dir: true },
      { path: 'src/app.ts', score: 8 },
    ])
    expect(hits.find((h) => h.item === 'src')?.dir).toBe(true)
    expect(hits.find((h) => h.item === 'src/app.ts')?.dir).toBe(false)
  })

  test('infers directories from parent paths even when dir is dropped', () => {
    const hits = mapFileSearchHits([
      { path: 'src', score: 10 },
      { path: 'src/app.ts', score: 8 },
    ])
    expect(hits.find((h) => h.item === 'src')?.dir).toBe(true)
    expect(hits.find((h) => h.item === 'src/app.ts')?.dir).toBe(false)
  })

  test('treats a trailing slash as a directory', () => {
    const hits = mapFileSearchHits([{ path: 'internal/', score: 5 }])
    expect(hits).toEqual([{ item: 'internal', score: 5, label: 'internal', dir: true }])
  })
})
