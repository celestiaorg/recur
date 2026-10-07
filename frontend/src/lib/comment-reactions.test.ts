import { describe, expect, it } from 'vitest'
import {
  applyReactionHolds,
  normalizeReactionContent,
  reactionIcon,
  reactionMine,
  toggleReactions,
  visibleReactions,
} from './comment-reactions'

describe('normalizeReactionContent', () => {
  it('maps REST and GraphQL names', () => {
    expect(normalizeReactionContent('+1')).toBe('+1')
    expect(normalizeReactionContent('THUMBS_UP')).toBe('+1')
    expect(normalizeReactionContent('Heart')).toBe('heart')
    expect(normalizeReactionContent('wave')).toBe('')
  })
})

describe('toggleReactions', () => {
  const start = [
    { content: 'heart', count: 1, mine: true },
    { content: '+1', count: 2 },
  ]

  it('adds, joins, and removes', () => {
    expect(toggleReactions(start, 'rocket')).toEqual([
      { content: '+1', count: 2 },
      { content: 'heart', count: 1, mine: true },
      { content: 'rocket', count: 1, mine: true },
    ])
    expect(toggleReactions(start, 'heart')).toEqual([{ content: '+1', count: 2 }])
    expect(toggleReactions([{ content: '+1', count: 2 }], '+1')).toEqual([
      { content: '+1', count: 3, mine: true },
    ])
  })

  it('ignores unknown content', () => {
    expect(toggleReactions(start, 'wave')).toEqual(visibleReactions(start))
  })
})

describe('visibleReactions', () => {
  it('drops empty groups and keeps GitHub order', () => {
    expect(reactionIcon('heart')).toBe('heart')
    expect(visibleReactions([
      { content: 'rocket', count: 1 },
      { content: '+1', count: 0 },
      { content: 'heart', count: 2 },
    ])).toEqual([
      { content: 'heart', count: 2 },
      { content: 'rocket', count: 1 },
    ])
  })
})

describe('applyReactionHolds', () => {
  it('reapplies a hold over a stale GetPR snapshot', () => {
    const stale = [
      { id: 'c1', reactions: [{ content: '+1', count: 2 }] },
      { id: 'c2', reactions: [] },
    ]
    const held = [{ content: '+1', count: 3, mine: true }]
    expect(applyReactionHolds(stale, { c1: held })).toEqual([
      { id: 'c1', reactions: held },
      stale[1],
    ])
    expect(reactionMine(held, '+1')).toBe(true)
  })
})
