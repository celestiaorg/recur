import { describe, expect, test } from 'vitest'
import {
  contextUsageAfterCompact,
  contextUsageFromChatItems,
  contextUsageFromTurn,
  contextUsageReady,
  contextUsedTokens,
} from './contextUsage'

describe('context usage', () => {
  test('ready requires used tokens and a window', () => {
    expect(contextUsageReady(null)).toBe(false)
    expect(contextUsageReady({
      inputTokens: 0, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 200_000,
    })).toBe(false)
    expect(contextUsageReady({
      inputTokens: 10, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 0,
    })).toBe(false)
    expect(contextUsageReady({
      inputTokens: 10, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 200_000,
    })).toBe(true)
  })

  test('ready after compact with empty fill (reset ring)', () => {
    expect(contextUsageReady({
      inputTokens: 0, cacheCreationTokens: 0, cacheReadTokens: 0, contextWindow: 200_000, compacted: true,
    })).toBe(true)
  })

  test('used tokens are prompt-side (input + cache)', () => {
    expect(contextUsedTokens({
      inputTokens: 10, cacheCreationTokens: 2, cacheReadTokens: 8, contextWindow: 100,
    })).toBe(20)
  })

  test('fromTurn drops rows that cannot fill the ring', () => {
    expect(contextUsageFromTurn({ inputTokens: 0, contextWindow: 100 })).toBeNull()
    expect(contextUsageFromTurn({
      inputTokens: 12, cacheReadTokens: 3, contextWindow: 100, compacted: true,
    })).toEqual({
      inputTokens: 12,
      cacheCreationTokens: 0,
      cacheReadTokens: 3,
      contextWindow: 100,
      compacting: false,
      compacted: true,
    })
  })

  test('afterCompact zeros prompt-side fill and keeps the window', () => {
    expect(contextUsageAfterCompact({
      inputTokens: 150_000, cacheCreationTokens: 0, cacheReadTokens: 50_000, contextWindow: 200_000, compacting: true,
    })).toEqual({
      inputTokens: 0,
      cacheCreationTokens: 0,
      cacheReadTokens: 0,
      contextWindow: 200_000,
      compacting: false,
      compacted: true,
    })
    expect(contextUsageAfterCompact(null)).toBeNull()
  })

  test('fromChatItems takes the newest turn and notes a compact in the log', () => {
    expect(contextUsageFromChatItems([
      { kind: 'text' },
      { kind: 'turn', inputTokens: 1, contextWindow: 10 },
      { kind: 'compact' },
      { kind: 'turn', inputTokens: 4, cacheReadTokens: 6, contextWindow: 10 },
    ])).toEqual({
      inputTokens: 4,
      cacheCreationTokens: 0,
      cacheReadTokens: 6,
      contextWindow: 10,
      compacting: false,
      compacted: true,
    })
  })

  test('fromChatItems resets fill when compact is newer than the last usable turn', () => {
    expect(contextUsageFromChatItems([
      { kind: 'turn', inputTokens: 180_000, cacheReadTokens: 20_000, contextWindow: 200_000 },
      { kind: 'compact' },
    ])).toEqual({
      inputTokens: 0,
      cacheCreationTokens: 0,
      cacheReadTokens: 0,
      contextWindow: 200_000,
      compacting: false,
      compacted: true,
    })
  })

  test('fromChatItems skips a zero-usage turn footer and keeps the last usable fill', () => {
    expect(contextUsageFromChatItems([
      { kind: 'turn', inputTokens: 12, cacheReadTokens: 3, contextWindow: 100 },
      { kind: 'turn', inputTokens: 0, cacheReadTokens: 0, contextWindow: 100 },
    ])).toEqual({
      inputTokens: 12,
      cacheCreationTokens: 0,
      cacheReadTokens: 3,
      contextWindow: 100,
      compacting: false,
      compacted: false,
    })
  })

  test('fromChatItems does not zero fill when a later turn is empty but compact is older', () => {
    expect(contextUsageFromChatItems([
      { kind: 'turn', inputTokens: 4, cacheReadTokens: 6, contextWindow: 10 },
      { kind: 'compact' },
      { kind: 'turn', inputTokens: 30, contextWindow: 10 },
      { kind: 'turn', inputTokens: 0, contextWindow: 10 },
    ])).toEqual({
      inputTokens: 30,
      cacheCreationTokens: 0,
      cacheReadTokens: 0,
      contextWindow: 10,
      compacting: false,
      compacted: true,
    })
  })
})
