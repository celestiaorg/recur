import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import {
  readDiffLayout, writeDiffLayout,
  readReviewTab, writeReviewTab, readFileListMode, writeFileListMode,
  readReviewSidebar, writeReviewSidebar,
  reuseFileDiffs,
  fileDiffPathLabel,
  fileDiffTreeLabel,
} from './review-diff'

describe('review-diff prefs', () => {
  const store = new Map<string, string>()
  beforeEach(() => {
    store.clear()
    vi.stubGlobal('localStorage', {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => { store.set(k, v) },
      removeItem: (k: string) => { store.delete(k) },
    })
  })
  afterEach(() => { vi.unstubAllGlobals() })

  it('defaults to unified, changes tab, tree file list, sidebar on', () => {
    expect(readDiffLayout()).toBe('unified')
    expect(readReviewTab()).toBe('changes')
    expect(readFileListMode()).toBe('tree')
    expect(readReviewSidebar()).toBe(true)
  })

  it('round-trips layout prefs', () => {
    writeDiffLayout('split')
    writeReviewTab('commits')
    writeFileListMode('flat')
    writeReviewSidebar(false)
    expect(readDiffLayout()).toBe('split')
    expect(readReviewTab()).toBe('commits')
    expect(readFileListMode()).toBe('flat')
    expect(readReviewSidebar()).toBe(false)
    writeReviewSidebar(true)
    expect(readReviewSidebar()).toBe(true)
  })

  it('falls back when stored values are junk', () => {
    store.set('recur.obsReviewTab', 'nope')
    store.set('recur.reviewFileListMode', 'grid')
    store.set('recur.reviewSidebar', 'maybe')
    expect(readReviewTab()).toBe('changes')
    expect(readFileListMode()).toBe('tree')
    expect(readReviewSidebar()).toBe(true)
  })

  it('migrates the old list/stack pref into the sidebar default', () => {
    store.set('recur.prFilesView', 'stack')
    expect(readReviewSidebar()).toBe(false)
    store.set('recur.prFilesView', 'list')
    expect(readReviewSidebar()).toBe(true)
  })

  it('prefers the sidebar key over the migrated list/stack pref', () => {
    store.set('recur.prFilesView', 'stack')
    store.set('recur.reviewSidebar', 'on')
    expect(readReviewSidebar()).toBe(true)
  })
})

describe('reuseFileDiffs', () => {
  it('returns the previous array when bodies are identical', () => {
    const prev = [{ path: 'a.ts', status: 'm', diff: { add: 1, del: 0 }, lines: [{ type: 'add', new: '1', codeHtml: 'x' }] }]
    const next = [{ path: 'a.ts', status: 'm', diff: { add: 1, del: 0 }, lines: [{ type: 'add', new: '1', codeHtml: 'x' }] }]
    expect(reuseFileDiffs(prev, next)).toBe(prev)
  })

  it('keeps object identity per unchanged path', () => {
    const a = { path: 'a.ts', diff: { add: 1, del: 0 }, lines: [{ type: 'add', new: '1', codeHtml: 'x' }] }
    const b = { path: 'b.ts', diff: { add: 1, del: 0 }, lines: [{ type: 'add', new: '1', codeHtml: 'y' }] }
    const prev = [a, b]
    const next = [
      { path: 'a.ts', diff: { add: 1, del: 0 }, lines: [{ type: 'add', new: '1', codeHtml: 'x' }] },
      { path: 'b.ts', diff: { add: 2, del: 0 }, lines: [{ type: 'add', new: '1', codeHtml: 'z' }] },
    ]
    const got = reuseFileDiffs(prev, next)
    expect(got[0]).toBe(a)
    expect(got[1]).not.toBe(b)
    expect(got[1].diff.add).toBe(2)
  })

  it('keeps the previous empty array', () => {
    const prev: { path: string }[] = []
    expect(reuseFileDiffs(prev, [])).toBe(prev)
  })
})

describe('fileDiffPathLabel', () => {
  it('returns the path when there is no rename', () => {
    expect(fileDiffPathLabel({ path: 'a.ts' })).toBe('a.ts')
    expect(fileDiffPathLabel({ path: 'a.ts', oldPath: 'a.ts' })).toBe('a.ts')
  })

  it('shortens the new side when the directory is unchanged', () => {
    expect(fileDiffPathLabel({
      path: 'apps/rindexer/migrations/0016.sql',
      oldPath: 'apps/rindexer/migrations/0015.sql',
    })).toBe('apps/rindexer/migrations/0015.sql → 0016.sql')
  })

  it('keeps both full paths when the directory changes', () => {
    expect(fileDiffPathLabel({ path: 'b/y.ts', oldPath: 'a/x.ts' })).toBe('a/x.ts → b/y.ts')
  })

  it('keeps both full paths when moving into a child directory', () => {
    expect(fileDiffPathLabel({ path: 'apps/archive/a.ts', oldPath: 'apps/a.ts' })).toBe('apps/a.ts → apps/archive/a.ts')
  })
})

describe('fileDiffTreeLabel', () => {
  it('returns the basename when there is no rename', () => {
    expect(fileDiffTreeLabel({ path: 'apps/a.ts' })).toBe('a.ts')
    expect(fileDiffTreeLabel({ path: 'apps/a.ts', oldPath: 'apps/a.ts' })).toBe('a.ts')
  })

  it('shows old → new basename when the directory is unchanged', () => {
    expect(fileDiffTreeLabel({
      path: 'apps/rindexer/migrations/0016.sql',
      oldPath: 'apps/rindexer/migrations/0015.sql',
    })).toBe('0015.sql → 0016.sql')
  })

  it('keeps the old path when the directory changes', () => {
    expect(fileDiffTreeLabel({ path: 'apps/archive/a.ts', oldPath: 'apps/a.ts' })).toBe('apps/a.ts → a.ts')
    expect(fileDiffTreeLabel({ path: 'b/y.ts', oldPath: 'a/x.ts' })).toBe('a/x.ts → y.ts')
  })
})
