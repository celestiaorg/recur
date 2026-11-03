import type { PRReaction } from './types'
import type { IconName } from './icons'

export const GH_REACTIONS: ReadonlyArray<{ content: string; icon: IconName; label: string }> = [
  { content: '+1', icon: 'thumbs-up', label: 'thumbs up' },
  { content: '-1', icon: 'thumbs-down', label: 'thumbs down' },
  { content: 'laugh', icon: 'face-grinning', label: 'laugh' },
  { content: 'hooray', icon: 'party-popper', label: 'hooray' },
  { content: 'confused', icon: 'face-slightly-frowning', label: 'confused' },
  { content: 'heart', icon: 'heart', label: 'heart' },
  { content: 'rocket', icon: 'rocket', label: 'rocket' },
  { content: 'eyes', icon: 'eye', label: 'eyes' },
]

const ORDER: string[] = GH_REACTIONS.map((r) => r.content)
const ALIAS: Record<string, string> = {
  thumbs_up: '+1',
  thumbs_down: '-1',
  '+1': '+1',
  '-1': '-1',
}

export function normalizeReactionContent(raw?: string): string {
  const s = (raw ?? '').trim()
  if (!s) return ''
  const lower = s.toLowerCase()
  if (ALIAS[lower]) return ALIAS[lower]
  return ORDER.includes(lower) ? lower : ''
}

export function reactionIcon(content?: string): IconName | null {
  const n = normalizeReactionContent(content)
  return GH_REACTIONS.find((r) => r.content === n)?.icon ?? null
}

export function reactionLabel(content?: string): string {
  const n = normalizeReactionContent(content)
  return GH_REACTIONS.find((r) => r.content === n)?.label ?? n
}

export function visibleReactions(rs?: PRReaction[] | null): PRReaction[] {
  return sortReactions((rs ?? []).filter((r) => (r.count ?? 0) > 0 && normalizeReactionContent(r.content)))
}

export function sortReactions(rs: PRReaction[]): PRReaction[] {
  const rank = new Map(ORDER.map((c, i) => [c, i]))
  return rs
    .map((r) => ({ ...r, content: normalizeReactionContent(r.content) || r.content }))
    .filter((r) => (r.count ?? 0) > 0)
    .slice()
    .sort((a, b) => (rank.get(a.content) ?? 99) - (rank.get(b.content) ?? 99))
}

export function toggleReactions(rs: PRReaction[] | null | undefined, content: string): PRReaction[] {
  const c = normalizeReactionContent(content)
  if (!c) return visibleReactions(rs)
  const next = visibleReactions(rs).map((r) => ({ ...r }))
  const i = next.findIndex((r) => normalizeReactionContent(r.content) === c)
  if (i >= 0) {
    const cur = next[i]
    next[i] = cur.mine
      ? { ...cur, count: cur.count - 1, mine: false }
      : { ...cur, count: cur.count + 1, mine: true }
    return visibleReactions(next)
  }
  return visibleReactions([...next, { content: c, count: 1, mine: true }])
}

export function reactionMine(rs: PRReaction[] | null | undefined, content: string): boolean {
  const c = normalizeReactionContent(content)
  if (!c) return false
  return !!(rs ?? []).find((r) => normalizeReactionContent(r.content) === c)?.mine
}
