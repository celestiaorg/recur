import { describe, expect, it } from 'vitest'
import {
  parseCommittedPromptLine,
  paramsFromArgs,
  pickPromptTab,
  promptActionErrorMessage,
  promptActionQueryAt,
  promptArgsOptional,
  promptArgSlotOptional,
  promptRequiredArgsFilled,
  promptArgsQueryAt,
  type PromptTab,
} from './promptActions'

describe('promptActionErrorMessage', () => {
  it('unwraps a Wails RuntimeError JSON string', () => {
    const raw =
      '{"message":"prompt \\"recur.chat.update\\" exited 1: error: Your local changes to the following files would be overwritten by merge:\\n\\tREADME.md\\nPlease commit your changes or stash them before you merge.\\nAborting\\nMerge with strategy ort failed.\\ngit merge main failed","cause":{},"kind":"RuntimeError"}'
    expect(promptActionErrorMessage(raw)).toBe(
      'error: Your local changes to the following files would be overwritten by merge:\n\tREADME.md\nPlease commit your changes or stash them before you merge.\nAborting\nMerge with strategy ort failed.\ngit merge main failed',
    )
  })

  it('unwraps a RuntimeError object', () => {
    const err = {
      message: 'prompt "recur.chat.commit" exited 1: nothing to commit',
      cause: {},
      kind: 'RuntimeError',
    }
    expect(promptActionErrorMessage(err)).toBe('nothing to commit')
  })

  it('unwraps an Error whose message is RuntimeError JSON', () => {
    const err = new Error(
      '{"message":"prompt \\"x\\" exited 2: no merge base ref","cause":{},"kind":"RuntimeError"}',
    )
    expect(promptActionErrorMessage(err)).toBe('no merge base ref')
  })

  it('passes through a plain string', () => {
    expect(promptActionErrorMessage('Unknown prompt /update')).toBe('Unknown prompt /update')
  })
})

describe('parseCommittedPromptLine', () => {
  it('parses /linear with no args', () => {
    expect(parseCommittedPromptLine('/linear')).toEqual({ slash: 'linear', args: '' })
    expect(parseCommittedPromptLine('  /linear  ')).toEqual({ slash: 'linear', args: '' })
  })

  it('parses /linear ENG-123 and extra notes', () => {
    expect(parseCommittedPromptLine('/linear ENG-123')).toEqual({ slash: 'linear', args: 'ENG-123' })
    expect(parseCommittedPromptLine('/linear   ENG-123  extra notes')).toEqual({
      slash: 'linear',
      args: 'ENG-123  extra notes',
    })
  })

  it('parses a Linear URL as args', () => {
    expect(parseCommittedPromptLine('/linear https://linear.app/acme/issue/ENG-123/slug')).toEqual({
      slash: 'linear',
      args: 'https://linear.app/acme/issue/ENG-123/slug',
    })
  })

  it('rejects mid-line and uppercase slash', () => {
    expect(parseCommittedPromptLine('please /linear ENG-123')).toBeNull()
    expect(parseCommittedPromptLine('/Linear ENG-123')).toBeNull()
  })
})

describe('promptActionQueryAt', () => {
  it('matches /linear with trailing whitespace only', () => {
    expect(promptActionQueryAt('/linear', 7)).toEqual({ start: 0, end: 7, query: 'linear' })
    expect(promptActionQueryAt('/linear ', 7)).toEqual({ start: 0, end: 7, query: 'linear' })
  })

  it('rejects once args start', () => {
    expect(promptActionQueryAt('/linear ENG', 11)).toBeNull()
  })
})

describe('promptArgsQueryAt', () => {
  const takes = (s: string) => s === 'linear' || s === 'issue'

  it('opens after /linear and a space', () => {
    expect(promptArgsQueryAt('/linear ', 8, takes)).toEqual({
      slash: 'linear',
      args: '',
      argsStart: 8,
    })
    expect(promptArgsQueryAt('/linear ENG-12', 14, takes)).toEqual({
      slash: 'linear',
      args: 'ENG-12',
      argsStart: 8,
    })
  })

  it('stays closed without a trailing space or for non-arg prompts', () => {
    expect(promptArgsQueryAt('/linear', 7, takes)).toBeNull()
    expect(promptArgsQueryAt('/commit ', 8, takes)).toBeNull()
  })
})

describe('paramsFromArgs', () => {
  it('maps a single named arg to the remainder (spaces kept)', () => {
    const info = { slash: 'linear', takesArgs: true, args: [{ name: 'issue', placeholder: 'id' }] } as never
    expect(paramsFromArgs(info, 'ENG-1 extra')).toEqual({ issue: 'ENG-1 extra' })
  })

  it('splits multiple args on whitespace and keeps the remainder on the last', () => {
    const info = {
      slash: 'poll',
      takesArgs: true,
      args: [
        { name: 'question', placeholder: '' },
        { name: 'choice_1', placeholder: '' },
        { name: 'choice_2', placeholder: '' },
      ],
    } as never
    expect(paramsFromArgs(info, 'Which color? Red Blue extra')).toEqual({
      question: 'Which',
      choice_1: 'color?',
      choice_2: 'Red Blue extra',
    })
    expect(paramsFromArgs(info, '"Which color?" Red Blue extra')).toEqual({
      question: 'Which color?',
      choice_1: 'Red',
      choice_2: 'Blue extra',
    })
  })
})

describe('promptArgsOptional', () => {
  it('is true when every named slot is optional', () => {
    const info = {
      slash: 'fix-checks',
      takesArgs: true,
      args: [{ name: 'check', placeholder: 'all', optional: true }],
    } as never
    expect(promptArgsOptional(info)).toBe(true)
  })

  it('is false for Linear-style required slots', () => {
    const info = { slash: 'linear', takesArgs: true, args: [{ name: 'issue' }] } as never
    expect(promptArgsOptional(info)).toBe(false)
  })

  it('is false when only some slots are optional', () => {
    const info = {
      slash: 'mixed',
      takesArgs: true,
      args: [{ name: 'note', optional: true }, { name: 'issue' }],
    } as never
    expect(promptArgsOptional(info)).toBe(false)
  })
})

describe('promptArgSlotOptional', () => {
  const mixed = {
    slash: 'mixed',
    takesArgs: true,
    args: [{ name: 'note', optional: true }, { name: 'issue' }],
  } as never

  it('follows the focused slot', () => {
    expect(promptArgSlotOptional(mixed, 'note')).toBe(true)
    expect(promptArgSlotOptional(mixed, 'issue')).toBe(false)
  })

  it('falls back to the first slot', () => {
    expect(promptArgSlotOptional(mixed)).toBe(true)
  })
})

describe('promptRequiredArgsFilled', () => {
  const mixed = {
    slash: 'mixed',
    takesArgs: true,
    args: [{ name: 'note', optional: true }, { name: 'issue' }],
  } as never

  it('allows an empty optional slot when required slots are filled', () => {
    expect(promptRequiredArgsFilled(mixed, ['', 'ENG-1'])).toBe(true)
    expect(promptRequiredArgsFilled(mixed, ['later', 'ENG-1'])).toBe(true)
  })

  it('rejects a missing required slot', () => {
    expect(promptRequiredArgsFilled(mixed, ['', ''])).toBe(false)
    expect(promptRequiredArgsFilled(mixed, ['note'])).toBe(false)
  })

  it('is false when info is missing or has no arg specs', () => {
    expect(promptRequiredArgsFilled(undefined, [])).toBe(false)
    expect(promptRequiredArgsFilled(undefined, ['ENG-1'])).toBe(false)
    expect(promptRequiredArgsFilled(null, [])).toBe(false)
    expect(promptRequiredArgsFilled({ slash: 'commit' } as never, [])).toBe(false)
  })

  it('is true for all-optional slots even when empty', () => {
    const info = {
      slash: 'fix-checks',
      takesArgs: true,
      args: [{ name: 'check', placeholder: 'all', optional: true }],
    } as never
    expect(promptRequiredArgsFilled(info, [])).toBe(true)
    expect(promptRequiredArgsFilled(info, [''])).toBe(true)
  })
})

describe('pickPromptTab', () => {
  const pa: PromptTab = { code: 'PA', title: 'New chat', mode: 'agent' }
  const work: PromptTab = { code: 'PA', title: 'Ship dark mode', mode: 'agent' }
  const ops: PromptTab = { code: 'PA2', title: 'Commit all changes', mode: 'agent', promptGroup: 'ops' }
  const rv: PromptTab = { code: 'PA3', title: 'Review', mode: 'review' }
  const ma: PromptTab = { code: 'MA', title: 'Loop', mode: 'loop' }
  const unusedRv: PromptTab = { code: 'PA4', title: 'New chat', mode: 'review' }

  function pick(
    tabs: PromptTab[],
    active: string,
    mode: string,
    extra?: { group?: string; busy?: string[]; started?: string[]; drafted?: string[]; locked?: string[] },
  ) {
    const busy = new Set(extra?.busy ?? [])
    const started = new Set(extra?.started ?? tabs.filter((t) => t.title && t.title !== 'New chat').map((t) => t.code))
    const drafted = new Set(extra?.drafted ?? [])
    const locked = new Set(extra?.locked ?? [])
    return pickPromptTab({
      tabs,
      activeCode: active,
      mode,
      group: extra?.group,
      isBusy: (c) => busy.has(c),
      tabMode: (t) => t.mode ?? 'agent',
      started: (t) => started.has(t.code),
      drafted: (t) => drafted.has(t.code),
      conductorLocked: (c) => locked.has(c),
    })
  }

  it('stays on the current idle tab in that mode when ungrouped', () => {
    expect(pick([ma, pa], 'PA', 'agent')).toEqual({ action: 'current' })
  })

  it('takes the nearest started tab in mode when ungrouped', () => {
    expect(pick([ma, work], 'MA', 'agent')).toEqual({ action: 'select', code: 'PA' })
  })

  it('does not take over a started working agent tab when grouped', () => {
    expect(pick([ma, work], 'PA', 'agent', { group: 'ops' })).toEqual({ action: 'create' })
  })

  it('reuses the designated group tab instead of the working chat', () => {
    expect(pick([ma, work, ops], 'PA', 'agent', { group: 'ops' })).toEqual({
      action: 'select',
      code: 'PA2',
    })
  })

  it('stays on the designated group tab', () => {
    expect(pick([ma, ops], 'PA2', 'agent', { group: 'ops' })).toEqual({ action: 'current' })
  })

  it('claims an unused agent tab when no group tab exists yet', () => {
    const unused: PromptTab = { code: 'PA2', title: 'New chat', mode: 'agent' }
    expect(pick([ma, work, unused], 'PA', 'agent', { group: 'ops' })).toEqual({
      action: 'select',
      code: 'PA2',
    })
  })

  it('does not claim a New chat the caller marks started (unsent draft)', () => {
    const draft: PromptTab = { code: 'PA', title: 'New chat', mode: 'agent' }
    expect(pick([ma, draft], 'PA', 'agent', { group: 'ops', started: ['PA'] })).toEqual({
      action: 'create',
    })
  })

  it('does not reuse a grouped tab with an unsent draft', () => {
    expect(pick([ma, work, ops], 'PA', 'agent', { group: 'ops', drafted: ['PA2'] })).toEqual({
      action: 'create',
    })
  })

  it('does not stay on a grouped tab that has an unsent draft', () => {
    expect(pick([ma, ops], 'PA2', 'agent', { group: 'ops', drafted: ['PA2'] })).toEqual({
      action: 'create',
    })
  })

  it('review group opens a new tab instead of a started review chat', () => {
    expect(pick([ma, work, rv], 'PA', 'review', { group: 'review' })).toEqual({ action: 'create' })
  })

  it('does not reuse a tab from a different review group', () => {
    const reviewOps: PromptTab = { code: 'PA3', title: 'Please review', mode: 'review', promptGroup: 'review' }
    expect(pick([ma, work, reviewOps], 'PA', 'review', { group: 'simplify-review' })).toEqual({ action: 'create' })
  })

  it('reuses an unused tab in mode when claiming a group', () => {
    expect(pick([ma, work, unusedRv], 'PA', 'review', { group: 'review' })).toEqual({
      action: 'select',
      code: 'PA4',
    })
  })

  it('re-review (ungrouped) from the fixes tab reuses the newest idle review tab', () => {
    expect(pick([ma, ops, rv], 'PA2', 'review')).toEqual({
      action: 'select',
      code: 'PA3',
    })
  })

  it('stays on a started review tab when ungrouped', () => {
    expect(pick([ma, rv], 'PA3', 'review')).toEqual({ action: 'current' })
  })

  it('picks the newest unused tab when claiming a group', () => {
    const a: PromptTab = { code: 'PA', title: 'New chat', mode: 'agent' }
    const b: PromptTab = { code: 'PA2', title: 'New chat', mode: 'agent' }
    expect(pick([ma, a, b], 'MA', 'agent', { group: 'ops' })).toEqual({
      action: 'select',
      code: 'PA2',
    })
  })

  it('never selects MA or a busy tab', () => {
    expect(pick([ma, pa], 'MA', 'loop', { locked: ['MA'] })).toEqual({ action: 'create' })
    expect(pick([ma, pa], 'PA', 'agent', { busy: ['PA'] })).toEqual({ action: 'create' })
  })
})
