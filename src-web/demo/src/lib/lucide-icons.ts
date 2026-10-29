import { icons } from 'lucide'
import type { IconNode } from 'lucide'

/** PascalCase / camelCase icon export → lucide.dev kebab-case name. */
export function lucideKebab(name: string): string {
  return name
    .replace(/([a-z])([A-Z])/g, '$1-$2')
    .replace(/([A-Z])([A-Z][a-z])/g, '$1-$2')
    .replace(/([a-z])(\d)/g, '$1-$2')
    .toLowerCase()
}

export type LucideIconEntry = {
  name: string
  aliases: string[]
  node: IconNode
}

const byName = new Map<string, IconNode>()
const entries: LucideIconEntry[] = []

const seen = new Map<IconNode, LucideIconEntry>()
for (const [pascal, node] of Object.entries(icons)) {
  if (!Array.isArray(node)) continue
  const kebab = lucideKebab(pascal)
  byName.set(kebab, node)
  const existing = seen.get(node)
  if (existing) {
    if (kebab !== existing.name && !existing.aliases.includes(kebab)) existing.aliases.push(kebab)
    continue
  }
  const entry: LucideIconEntry = { name: kebab, aliases: [], node }
  seen.set(node, entry)
  entries.push(entry)
}
entries.sort((a, b) => a.name.localeCompare(b.name))

export const lucideIcons: readonly LucideIconEntry[] = entries

const SUGGESTED = [
  'box', 'folder', 'folder-git', 'rocket', 'house', 'globe', 'database', 'server',
  'code', 'terminal', 'sparkles', 'hexagon', 'layers', 'package', 'app-window',
  'briefcase', 'building-2', 'shopping-bag', 'credit-card', 'wallet', 'music',
  'image', 'camera', 'gamepad-2', 'heart', 'star', 'zap', 'leaf', 'flower-2',
  'bird', 'cat', 'dog', 'fish', 'bug', 'cpu', 'wifi', 'cloud', 'lock', 'shield',
  'key', 'users', 'mail', 'message-circle', 'map-pin', 'bookmark', 'flag',
  'puzzle', 'gem', 'crown', 'trophy', 'target', 'anchor', 'plane', 'car', 'store',
  'sun', 'moon', 'flame', 'droplet', 'atom', 'flask-conical', 'coffee', 'gift',
  'party-popper',
]

export const lucideSuggested: readonly LucideIconEntry[] = SUGGESTED
  .map((name) => {
    const node = byName.get(name)
    return node ? lucideIcons.find((e) => e.node === node) : undefined
  })
  .filter((e): e is LucideIconEntry => !!e)

export function lucideNode(name: string): IconNode | undefined {
  return byName.get(name.trim().toLowerCase())
}

export function hasLucideIcon(name: string): boolean {
  return byName.has(name.trim().toLowerCase())
}

function esc(v: string): string {
  return v.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;')
}

/** Inner SVG markup for a lucide icon node. Trusted catalog data only. */
export function lucideInnerHtml(name: string): string {
  const node = lucideNode(name)
  if (!node) return ''
  return node.map(([tag, attrs]) => {
    const a = Object.entries(attrs)
      .filter(([, v]) => v != null && v !== '')
      .map(([k, v]) => `${k}="${esc(String(v))}"`)
      .join(' ')
    return a ? `<${tag} ${a}/>` : `<${tag}/>`
  }).join('')
}
