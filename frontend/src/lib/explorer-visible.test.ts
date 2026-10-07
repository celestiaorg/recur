import { describe, expect, test } from 'vitest'
import { isPathVisibleInExplorerTree, type ExplorerVisibleNode } from './explorer-visible'

function node(path: string, children?: ExplorerVisibleNode[]): ExplorerVisibleNode {
  return { path, dir: !!children, children }
}

const srcTree: ExplorerVisibleNode[] = [
  node('src', [
    node('src/main.go'),
    node('src/lib', [node('src/lib/util.ts')]),
  ]),
  node('README.md'),
]

describe('isPathVisibleInExplorerTree', () => {
  test('rejects empty path', () => {
    expect(isPathVisibleInExplorerTree(srcTree, '', () => true)).toBe(false)
  })

  test('finds a root-level file', () => {
    expect(isPathVisibleInExplorerTree(srcTree, 'README.md', () => true)).toBe(true)
  })

  test('finds a nested file when ancestors are expanded', () => {
    expect(isPathVisibleInExplorerTree(srcTree, 'src/main.go', () => true)).toBe(true)
    expect(isPathVisibleInExplorerTree(srcTree, 'src/lib/util.ts', () => true)).toBe(true)
  })

  test('finds a directory that is itself in the listing', () => {
    expect(isPathVisibleInExplorerTree(srcTree, 'src', () => true)).toBe(true)
    expect(isPathVisibleInExplorerTree(srcTree, 'src/lib', () => true)).toBe(true)
  })

  test('hides nested files when an ancestor is collapsed', () => {
    const open = (dir: string) => dir !== 'src/lib'
    expect(isPathVisibleInExplorerTree(srcTree, 'src/main.go', open)).toBe(true)
    expect(isPathVisibleInExplorerTree(srcTree, 'src/lib', open)).toBe(true)
    expect(isPathVisibleInExplorerTree(srcTree, 'src/lib/util.ts', open)).toBe(false)
  })

  test('hides everything under a collapsed root folder', () => {
    expect(isPathVisibleInExplorerTree(srcTree, 'src/main.go', () => false)).toBe(false)
    expect(isPathVisibleInExplorerTree(srcTree, 'README.md', () => false)).toBe(true)
  })

  test('returns false for paths not in the tree (e.g. filtered out)', () => {
    expect(isPathVisibleInExplorerTree(srcTree, 'PROGRESS.md', () => true)).toBe(false)
    expect(isPathVisibleInExplorerTree(srcTree, 'src/other.go', () => true)).toBe(false)
  })

  test('uses childEntries when the rendered children differ from node.children', () => {
    const hiddenKid = node('src', [node('src/secret.go'), node('src/main.go')])
    const shown = (n: ExplorerVisibleNode) => (n.children ?? []).filter((c) => c.path !== 'src/secret.go')
    expect(isPathVisibleInExplorerTree([hiddenKid], 'src/main.go', () => true, shown)).toBe(true)
    expect(isPathVisibleInExplorerTree([hiddenKid], 'src/secret.go', () => true, shown)).toBe(false)
  })
})
