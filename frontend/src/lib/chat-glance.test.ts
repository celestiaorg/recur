import { describe, expect, it } from 'vitest'
import { FIRST_CHECKPOINT_SEC, foldInnerNarration, foldSubagentNotice, hasLaterStep, isFirstCheckpoint, liveGroupOpen, type TurnBoundary } from './chat-glance'

describe('isFirstCheckpoint', () => {
  it('holds for thinking-only rows under the cap', () => {
    expect(isFirstCheckpoint(true, 5, [{ thinking: true }])).toBe(true)
  })

  it('ends once a tool appears', () => {
    expect(isFirstCheckpoint(true, 5, [{ thinking: true }, { tool: 'Read' }])).toBe(false)
  })

  it('ends at the cap even if still thinking', () => {
    expect(isFirstCheckpoint(true, FIRST_CHECKPOINT_SEC, [{ thinking: true }])).toBe(false)
  })

  it('is off when idle or empty', () => {
    expect(isFirstCheckpoint(false, 1, [{ thinking: true }])).toBe(false)
    expect(isFirstCheckpoint(true, 1, [])).toBe(false)
  })
})

describe('liveGroupOpen', () => {
  it('defaults to the checkpoint, then honors an explicit toggle', () => {
    expect(liveGroupOpen(undefined, true)).toBe(true)
    expect(liveGroupOpen(undefined, false)).toBe(false)
    expect(liveGroupOpen(false, true)).toBe(false)
    expect(liveGroupOpen(true, false)).toBe(true)
  })
})

describe('foldInnerNarration', () => {
  it('folds settled text once more tools follow, not the live or final reply', () => {
    const turn: (TurnBoundary & { body?: string })[] = [
      { me: true },
      { body: 'plan' },
      { tool: 'Read' },
      { body: 'inner' },
      { tool: 'Edit' },
      { body: 'final' },
      { turnEnd: true },
    ]
    expect(foldInnerNarration(turn[1], hasLaterStep(turn, 1))).toBe(true)
    expect(foldInnerNarration({ streaming: true }, hasLaterStep(turn, 3))).toBe(false)
    expect(foldInnerNarration(turn[3], hasLaterStep(turn, 3))).toBe(true)
    expect(foldInnerNarration(turn[5], hasLaterStep(turn, 5))).toBe(false)
  })

  it('does not fold the final reply when only compaction follows', () => {
    const turn: (TurnBoundary & { body?: string })[] = [
      { me: true },
      { tool: 'Read' },
      { thinking: true },
      { body: 'final' },
      { compact: true },
      { turnEnd: true },
    ]
    expect(foldInnerNarration(turn[3], hasLaterStep(turn, 3))).toBe(false)
    expect(hasLaterStep(turn, 1)).toBe(true)
  })
})

describe('foldSubagentNotice', () => {
  it('folds only Subagent notes when later steps follow', () => {
    const turn: (TurnBoundary & { note?: boolean; noteLabel?: string })[] = [
      { note: true, noteLabel: 'Subagent' },
      { tool: 'Read' },
      { note: true, noteLabel: 'Subagent' },
      { thinking: true },
      { note: true, noteLabel: 'Commit' },
      { tool: 'Edit' },
      { note: true, noteLabel: 'Handoff' },
      { tool: 'Grep' },
      { note: true, noteLabel: 'Subagent' },
      { turnEnd: true },
    ]
    expect(foldSubagentNotice(turn[0], hasLaterStep(turn, 0))).toBe(true)
    expect(foldSubagentNotice(turn[2], hasLaterStep(turn, 2))).toBe(true)
    expect(foldSubagentNotice(turn[4], hasLaterStep(turn, 4))).toBe(false)
    expect(foldSubagentNotice(turn[6], hasLaterStep(turn, 6))).toBe(false)
    expect(foldSubagentNotice(turn[8], hasLaterStep(turn, 8))).toBe(false)
    expect(foldSubagentNotice({ note: true, noteLabel: 'Subagent' }, false)).toBe(false)
  })
})
