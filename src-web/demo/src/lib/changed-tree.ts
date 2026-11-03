import type { TreeEntry } from './types'

/**
 * The nested structure implied by a flat list of repo-relative paths.
 * Dirs first, then names: same sort as the Observatory Changed explorer.
 */
export function buildTreeFromPaths(paths: readonly string[]): TreeEntry[] {
  type Node = { entry: TreeEntry; kids: Map<string, Node> }
  const leaves = new Set(paths)
  const roots = new Map<string, Node>()
  for (const p of paths) {
    const parts = p.split('/').filter(Boolean)
    let level = roots
    let acc = ''
    for (let i = 0; i < parts.length; i++) {
      acc = acc ? `${acc}/${parts[i]}` : parts[i]
      let node = level.get(parts[i])
      if (!node) {
        node = { entry: { name: parts[i], path: acc, dir: i < parts.length - 1 }, kids: new Map() }
        level.set(parts[i], node)
      }
      level = node.kids
    }
  }
  const toEntries = (m: Map<string, Node>): TreeEntry[] =>
    [...m.values()]
      .map((n) => {
        const children = toEntries(n.kids)
        if (!children.length) return n.entry
        // Git can list both a path and descendants when a file is replaced by a
        // directory (or vice versa). Keep the leaf beside the new children.
        if (leaves.has(n.entry.path)) {
          children.push({ name: n.entry.name, path: n.entry.path, dir: false })
          children.sort(compareTreeEntry)
        }
        return { ...n.entry, dir: true, children }
      })
      .sort(compareTreeEntry)
  return toEntries(roots)
}

function compareTreeEntry(a: TreeEntry, b: TreeEntry): number {
  if (!!a.dir !== !!b.dir) return a.dir ? -1 : 1
  return a.name.localeCompare(b.name)
}

/** File paths in the same DFS order the Changed tree / review TOC shows. */
export function orderChangedPaths(paths: readonly string[]): string[] {
  const out: string[] = []
  const walk = (nodes: TreeEntry[]) => {
    for (const n of nodes) {
      if (n.dir) walk(n.children ?? [])
      else out.push(n.path)
    }
  }
  walk(buildTreeFromPaths(paths))
  return out
}

/** Reorder a file list to match the Changed-tree / TOC, not git's byte order. */
export function sortFilesByChangedTree<T extends { path: string }>(files: readonly T[]): T[] {
  if (files.length < 2) return files as T[]
  const rank = new Map(orderChangedPaths(files.map((f) => f.path)).map((p, i) => [p, i]))
  return [...files].sort((a, b) => (rank.get(a.path) ?? 0) - (rank.get(b.path) ?? 0))
}
