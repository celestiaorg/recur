import { describe, expect, it } from 'vitest'
import { buildTreeFromPaths, capVisibleTree, flattenVisibleTree, orderChangedPaths, pathsUnderDir, sortFilesByChangedTree } from './changed-tree'

describe('buildTreeFromPaths', () => {
  it('returns empty for no paths', () => {
    expect(buildTreeFromPaths([])).toEqual([])
  })

  it('nests directories and sorts dirs first', () => {
    const tree = buildTreeFromPaths(['b.txt', 'a/z.go', 'a/b.go'])
    expect(tree.map((e) => e.name)).toEqual(['a', 'b.txt'])
    expect(tree[0].dir).toBe(true)
    expect(tree[0].children?.map((e) => e.name)).toEqual(['b.go', 'z.go'])
    expect(tree[1].dir).toBe(false)
    expect(tree[1].path).toBe('b.txt')
  })

  it('handles a single nested file', () => {
    const tree = buildTreeFromPaths(['src/main.go'])
    expect(tree).toHaveLength(1)
    expect(tree[0]).toMatchObject({ name: 'src', path: 'src', dir: true })
    expect(tree[0].children).toEqual([
      expect.objectContaining({ name: 'main.go', path: 'src/main.go', dir: false }),
    ])
  })

  it('sorts siblings with localeCompare (underscore before dot)', () => {
    const tree = buildTreeFromPaths(['internal/store/seed.go', 'internal/store/seed_prs.go'])
    const store = tree[0]?.children?.[0]
    expect(store?.children?.map((e) => e.name)).toEqual(
      ['seed.go', 'seed_prs.go'].sort((a, b) => a.localeCompare(b)),
    )
  })

  it('keeps a type-changed file beside its descendants', () => {
    for (const paths of [
      ['a', 'a/b.txt'],
      ['a/b.txt', 'a'],
    ]) {
      const tree = buildTreeFromPaths(paths)
      expect(tree).toHaveLength(1)
      expect(tree[0]).toMatchObject({ name: 'a', path: 'a', dir: true })
      expect(tree[0].children?.map((e) => [e.path, e.dir])).toEqual([
        ['a', false],
        ['a/b.txt', false],
      ])
    }
  })
})

describe('flattenVisibleTree', () => {
  it('includes dirs and files in DFS order', () => {
    const tree = buildTreeFromPaths(['b.txt', 'a/z.go', 'a/b.go'])
    expect(flattenVisibleTree(tree).map((r) => [r.entry.path, r.depth])).toEqual([
      ['a', 0],
      ['a/b.go', 1],
      ['a/z.go', 1],
      ['b.txt', 0],
    ])
  })

  it('omits children of collapsed dirs', () => {
    const tree = buildTreeFromPaths(['a/b.go', 'a/c.go', 'd.txt'])
    expect(flattenVisibleTree(tree, new Set(['a'])).map((r) => r.entry.path)).toEqual(['a', 'd.txt'])
  })

  it('shows both sides of a file/dir type change', () => {
    const rows = flattenVisibleTree(buildTreeFromPaths(['a', 'a/b.txt']))
    expect(rows.map((r) => [r.entry.path, !!r.entry.dir, r.depth])).toEqual([
      ['a', true, 0],
      ['a', false, 1],
      ['a/b.txt', false, 1],
    ])
  })
})

describe('capVisibleTree', () => {
  it('keeps nested structure and stops at the limit', () => {
    const tree = buildTreeFromPaths(['a/b.go', 'a/c.go', 'd.txt'])
    const capped = capVisibleTree(tree, new Set(), 3)
    expect(capped).not.toBe(tree)
    expect(flattenVisibleTree(capped).map((r) => r.entry.path)).toEqual(['a', 'a/b.go', 'a/c.go'])
  })

  it('returns the same tree when under the limit', () => {
    const tree = buildTreeFromPaths(['a/b.go', 'd.txt'])
    expect(capVisibleTree(tree, new Set(), 10)).toBe(tree)
    expect(capVisibleTree(tree, new Set(), Infinity)).toBe(tree)
  })

  it('omits children of collapsed dirs from the budget', () => {
    const tree = buildTreeFromPaths(['a/1.go', 'a/2.go', 'b/1.go', 'b/2.go'])
    const collapsed = new Set(['a'])
    const capped = capVisibleTree(tree, collapsed, 3)
    expect(flattenVisibleTree(capped, collapsed).map((r) => r.entry.path)).toEqual(['a', 'b', 'b/1.go'])
  })
})

describe('pathsUnderDir', () => {
  it('keeps descendants and ignores a same-prefix sibling', () => {
    expect(pathsUnderDir('a', ['a/b.go', 'a/c/d.go', 'ab/e.go', 'a'])).toEqual(['a/b.go', 'a/c/d.go'])
  })
})

describe('orderChangedPaths', () => {
  it('is a DFS of the sorted tree (dirs first)', () => {
    expect(orderChangedPaths(['b.txt', 'a/z.go', 'a/b.go'])).toEqual(['a/b.go', 'a/z.go', 'b.txt'])
  })

  it('matches the TOC for seed.go vs seed_prs.go', () => {
    const paths = ['internal/store/seed.go', 'internal/store/seed_prs.go']
    expect(orderChangedPaths(paths)).toEqual(
      [...paths].sort((a, b) => a.split('/').pop()!.localeCompare(b.split('/').pop()!)),
    )
  })
})

describe('sortFilesByChangedTree', () => {
  it('reorders git-byte-order files to tree order', () => {
    const files = [{ path: 'internal/store/seed.go' }, { path: 'internal/store/seed_prs.go' }]
    expect(sortFilesByChangedTree(files).map((f) => f.path)).toEqual(
      orderChangedPaths(files.map((f) => f.path)),
    )
  })
})
