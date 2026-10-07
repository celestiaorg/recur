import { describe, expect, test } from 'vitest'
import { emojiForShortcode, emojiQueryAt, expandEmojiShortcodes, expandEmojiShortcodesAt, suggestEmoji } from './emoji'

describe('emoji shortcodes', () => {
  test('resolves GitHub aliases', () => {
    expect(emojiForShortcode('rocket')).toBe('🚀')
    expect(emojiForShortcode('+1')).toBe('👍')
    expect(emojiForShortcode('thumbsup')).toBe('👍')
    expect(emojiForShortcode('smile')).toBe('😄')
  })

  test('expands completed :name: tokens', () => {
    expect(expandEmojiShortcodes('LGTM :rocket: :+1:')).toBe('LGTM 🚀 👍')
  })

  test('leaves unknown names and incomplete tokens', () => {
    expect(expandEmojiShortcodes('see :notanemoji: and :rock')).toBe('see :notanemoji: and :rock')
  })

  test('skips fenced and inline code when asked', () => {
    const src = 'ok :rocket: and `:rocket:`\n```\n:smile:\n```'
    expect(expandEmojiShortcodes(src, { skipCode: true })).toBe('ok 🚀 and `:rocket:`\n```\n:smile:\n```')
  })

  test('keeps the caret after an expansion at the end', () => {
    const hit = expandEmojiShortcodesAt('hi :rocket:', 11)
    expect(hit).toEqual({ text: 'hi 🚀', caret: 5 })
  })

  test('shifts the caret when an earlier shortcode expands', () => {
    const src = ':rocket: later'
    const hit = expandEmojiShortcodesAt(src, src.length)
    expect(hit?.text).toBe('🚀 later')
    expect(hit?.caret).toBe('🚀 later'.length)
  })

  test('detects an in-progress :query at the caret', () => {
    expect(emojiQueryAt('LGTM :roc', 9)).toEqual({ start: 5, query: 'roc' })
    expect(emojiQueryAt('LGTM :roc', 5)).toBeNull()
    expect(emojiQueryAt('see:roc', 7)).toBeNull()
    expect(emojiQueryAt('http://x', 8)).toBeNull()
    expect(emojiQueryAt(':', 1)).toEqual({ start: 0, query: '' })
    expect(emojiQueryAt('LGTM :', 6)).toEqual({ start: 5, query: '' })
    expect(emojiQueryAt('(:', 2)).toEqual({ start: 1, query: '' })
    expect(emojiQueryAt('see:', 4)).toBeNull()
  })

  test('ranks shortcode suggestions', () => {
    const hits = suggestEmoji('rock', 8)
    expect(hits[0]?.name).toBe('rock')
    expect(hits.some((h) => h.name === 'rocket' && h.emoji === '🚀')).toBe(true)
    expect(suggestEmoji('rocket', 8)[0]).toEqual({ name: 'rocket', emoji: '🚀' })
  })

  test('bare : offers GitHub defaults plus rocket and fire', () => {
    expect(suggestEmoji('').map((h) => h.name)).toEqual([
      '+1', '-1', '100', '1st_place_medal', 'rocket', 'fire',
    ])
    expect(suggestEmoji(' ')[0]).toEqual({ name: '+1', emoji: '👍' })
  })
})
