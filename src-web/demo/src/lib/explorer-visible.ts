/** Minimal tree node used to decide whether a path is already shown in the explorer. */
export type ExplorerVisibleNode = {
  path: string
  dir?: boolean
  children?: ExplorerVisibleNode[] | null
}

/**
 * True when `path` is already rendered in the explorer listing: present in the
 * current (filtered) tree, with every ancestor directory expanded. Does not
 * load or expand anything — collapsed or filtered-out files are not visible.
 */
export function isPathVisibleInExplorerTree(
  roots: readonly ExplorerVisibleNode[],
  path: string,
  isDirExpanded: (dirPath: string) => boolean,
  childEntries?: (node: ExplorerVisibleNode) => readonly ExplorerVisibleNode[],
): boolean {
  if (!path) return false
  const parts = path.split('/').filter(Boolean)
  if (!parts.length) return false
  let nodes: readonly ExplorerVisibleNode[] = roots
  let acc = ''
  for (let i = 0; i < parts.length; i++) {
    acc = acc ? `${acc}/${parts[i]}` : parts[i]
    const node = nodes.find((e) => e.path === acc)
    if (!node) return false
    if (i === parts.length - 1) return true
    if (!node.dir || !isDirExpanded(node.path)) return false
    nodes = childEntries ? childEntries(node) : (node.children ?? [])
  }
  return false
}
