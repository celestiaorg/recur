import { describe, expect, test } from 'vitest'
import { isPairChatCode, isLoopCockpitTab, isSpecialistSession, mergeLoopCockpit, parseLoopPRFact, formatLoopElapsed, loopPRCompletion, loopCompletionWrap, stripLoopWrap, specialistLiveCodes, specialistName, specialistMode, isPhaseKickoffFact, isSpecialistWrapFact, needsLiveTurnBreak } from './loop-cockpit'
import type { AgentMsg } from '../components/AgentChat.svelte'

function factText(m: AgentMsg): string | undefined {
  return m.log?.kind === 'fact' ? m.log.text : undefined
}
function sessionMsgs(m: AgentMsg): AgentMsg[] {
  return m.log?.kind === 'session' ? m.log.messages : []
}

describe('isLoopCockpitTab', () => {
  test('MA always; PA* only once the run is pinned', () => {
    expect(isLoopCockpitTab('MA', false)).toBe(true)
    expect(isLoopCockpitTab('PA', false)).toBe(false)
    expect(isLoopCockpitTab('PA', true)).toBe(true)
    expect(isLoopCockpitTab('PA2', true)).toBe(true)
  })
})

describe('mergeLoopCockpit', () => {
  test('orders Maestro, fact, specialist by timestamp and skips PA*', () => {
    const ma: AgentMsg[] = [
      { me: true, body: 'go', ts: 1000 },
      { body: 'on it', ts: 4000 },
    ]
    const merged = mergeLoopCockpit(
      ma,
      [
        { id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 3000, messages: [{ body: 'edited' }] },
        { id: 's-pa', code: 'PA', name: 'Agent', kind: 'pair', ts: 2500, messages: [{ body: 'human chat' }] },
        { id: 's-ma', code: 'MA', name: 'Maestro', kind: 'maestro', ts: 500, messages: [{ body: 'dup' }] },
      ],
      [{ id: 'f1', text: 'Converged.', ts: 2000, isDeviation: false } as never],
      new Set(['CO']),
    )
    expect(merged.map((m) => m.body || m.log?.kind)).toEqual(['go', 'fact', 'session', 'on it'])
    expect(merged[1].log).toMatchObject({ kind: 'fact', text: 'Converged.', code: 'MA', name: 'Maestro' })
    expect(merged[2].log).toMatchObject({ kind: 'session', id: 's-co#0', code: 'CO', live: true })
    expect(merged.some((m) => m.log?.kind === 'session' && m.log.code === 'PA')).toBe(false)
  })

  test('splits one agent session into a card per turn at that turn’s timestamp', () => {
    const merged = mergeLoopCockpit(
      [
        { me: true, body: 'go', ts: 500 },
        { body: 'coder round 2', ts: 2500 },
      ],
      [{
        id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 1000,
        messages: [
          { me: true, body: 'fix rotation', ts: 1000 },
          { body: 'first pass', ts: 1100 },
          { turnEnd: true, durationMs: 1, ts: 1200 },
          { me: true, body: 'fix again', ts: 3000 },
          { body: 'second pass', ts: 3100 },
        ],
      }],
      [],
      new Set(['CO']),
    )
    const cards = merged.filter((m) => m.log?.kind === 'session')
    expect(cards).toHaveLength(2)
    expect(cards[0].log).toMatchObject({
      kind: 'session', id: 's-co#0', code: 'CO', live: false,
      messages: expect.arrayContaining([{ body: 'first pass', ts: 1100 }]),
    })
    expect(sessionMsgs(cards[0]).some((m) => m.body === 'second pass')).toBe(false)
    expect(cards[1].log).toMatchObject({ kind: 'session', id: 's-co#1', code: 'CO', live: true })
    expect(sessionMsgs(cards[1]).map((m) => m.body)).toContain('second pass')
    expect(merged.map((m) => m.body || m.log?.kind)).toEqual([
      'go', 'session', 'coder round 2', 'session',
    ])
  })

  test('keeps queued follow-ups at the end', () => {
    const merged = mergeLoopCockpit(
      [{ body: 'hi', ts: 1 }, { me: true, body: 'later', queued: true, pendingId: 'p1' }],
      [],
      [],
      new Set(),
    )
    expect(merged[merged.length - 1]).toMatchObject({ queued: true, body: 'later' })
  })

  test('pins the opening user prompt before start-of-loop engine facts', () => {
    const merged = mergeLoopCockpit(
      [
        { me: true, body: 'create a cli game of pong', ts: 3000 },
        { thinking: true, body: 'inspect the repo', ts: 3100 },
      ],
      [],
      [
        { id: 'f1', text: 'Starting loop for: create a cli game of pong', ts: 1000, isDeviation: false } as never,
        { id: 'f2', text: 'Converged.', ts: 2000, isDeviation: false } as never,
      ],
      new Set(),
    )
    expect(merged.map((m) => m.body || factText(m))).toEqual([
      'create a cli game of pong',
      'Starting loop for: create a cli game of pong',
      'Converged.',
      'inspect the repo',
    ])
  })

  test('keeps a live Maestro turn in array order when rows have no timestamps', () => {
    const merged = mergeLoopCockpit(
      [
        { me: true, body: 'pong', ts: 500 },
        { body: 'planned', ts: 800 },
        { me: true, body: "so we're done?", ts: 5000 },
        { thinking: true, body: 'check the PR' },
        { body: "I'll check the worktree" },
        { tool: 'Read', description: 'Read file' },
        { thinking: true, body: 'settled', streaming: true },
      ],
      [],
      [
        { id: 'f1', text: 'Starting loop for: pong', ts: 1000, isDeviation: false } as never,
        { id: 'f2', text: 'Planning the change.', ts: 2000, isDeviation: false } as never,
      ],
      new Set(),
    )
    expect(merged.map((m) => m.body || m.tool || factText(m))).toEqual([
      'pong',
      'planned',
      'Starting loop for: pong',
      'Planning the change.',
      "so we're done?",
      'check the PR',
      "I'll check the worktree",
      'Read',
      'settled',
    ])
  })

  test('places start-of-loop facts before a live Maestro turn with no user bubble yet', () => {
    const merged = mergeLoopCockpit(
      [
        { thinking: true, body: 'inspect the repo', streaming: true },
        { body: "I'll inspect the repository" },
      ],
      [],
      [
        { id: 'f1', text: 'Starting loop for: pong', ts: 1000, isDeviation: false } as never,
        { id: 'f2', text: 'Planning the change.', ts: 2000, isDeviation: false } as never,
      ],
      new Set(),
      'create a cli game of pong',
    )
    expect(merged.map((m) => m.body || factText(m))).toEqual([
      'create a cli game of pong',
      'Starting loop for: pong',
      'Planning the change.',
      'inspect the repo',
      "I'll inspect the repository",
    ])
  })

  test('keeps the goal bubble ahead of an optimistic plan-approval answer', () => {
    const goal = 'create a cli game of pong'
    const merged = mergeLoopCockpit(
      [{ me: true, body: 'Approve and proceed', optimistic: true, ts: Date.now() }],
      [],
      [
        { id: 'f1', text: 'Starting loop for: create a cli game of pong', ts: 1000, isDeviation: false } as never,
        { id: 'f2', text: 'Converged.', ts: 2000, isDeviation: false } as never,
      ],
      new Set(),
      goal,
    )
    expect(merged.map((m) => m.body || factText(m))).toEqual([
      goal,
      'Starting loop for: create a cli game of pong',
      'Converged.',
      'Approve and proceed',
    ])
  })

  test('does not duplicate an optimistic opening seed that already is the goal', () => {
    const goal = 'create a cli game of pong'
    const merged = mergeLoopCockpit(
      [{ me: true, body: goal, optimistic: true }],
      [],
      [],
      new Set(),
      goal,
    )
    expect(merged.filter((m) => m.me).map((m) => m.body)).toEqual([goal])
  })

  test('keeps completed-in after the turn that produced it, not between assistant waves', () => {
    const merged = mergeLoopCockpit(
      [
        { me: true, body: 'pong', ts: 100 },
        { me: true, body: "so we're done?", ts: 5000 },
        { thinking: true, body: 'blast', ts: 5100 },
        { body: "I'll check", ts: 5200 },
        { tool: 'Read', ts: 5300 },
        { turnEnd: true, durationMs: 17000, ts: 5100 },
        { thinking: true, body: 'settled', ts: 5400 },
        { body: 'Yes. The loop has settled.', ts: 5500 },
      ],
      [],
      [],
      new Set(),
    )
    expect(merged.map((m) => m.body || m.tool || (m.turnEnd ? 'completed' : ''))).toEqual([
      'pong',
      "so we're done?",
      'blast',
      "I'll check",
      'Read',
      'completed',
      'settled',
      'Yes. The loop has settled.',
    ])
  })
})

describe('isSpecialistSession', () => {
  test('excludes Maestro and pair tabs', () => {
    expect(isSpecialistSession({ code: 'CO' })).toBe(true)
    expect(isSpecialistSession({ code: 'MA' })).toBe(false)
    expect(isSpecialistSession({ code: 'PA2' })).toBe(false)
  })
})

describe('specialistLiveCodes', () => {
  test('keeps specialists for this workspace and extra stream codes', () => {
    expect([...specialistLiveCodes('ws1', ['ws1-CO', 'ws1-MA', 'ws2-RV', 'ws1-PA'], ['QA'])].sort()).toEqual(['CO', 'QA'])
  })

  test('names the loop specialists', () => {
    expect(specialistName('CO')).toBe('Coder')
    expect(specialistName('RV')).toBe('Reviewer')
    expect(specialistName('PL')).toBe('Planner')
    expect(specialistName('SR')).toBe('Security reviewer')
  })

  test('maps specialists to their chat mode', () => {
    expect(specialistMode('CO')).toBe('agent')
    expect(specialistMode('RV')).toBe('review')
    expect(specialistMode('PL')).toBe('plan')
    expect(specialistMode('SR')).toBe('review')
    expect(specialistMode('QA')).toBe('ask')
    expect(specialistMode('WR')).toBe('agent')
    expect(specialistMode('ED')).toBe('review')
  })
})

describe('mergeLoopCockpit live specialists', () => {
  test('inserts a working card when the specialist has no session yet', () => {
    const merged = mergeLoopCockpit(
      [{ me: true, body: 'go', ts: 100 }],
      [],
      [],
      new Set(['CO']),
    )
    const card = merged.find((m) => m.log?.kind === 'session')
    expect(card?.log).toMatchObject({ kind: 'session', id: 'live-CO#0', code: 'CO', name: 'Coder', live: true, messages: [] })
  })

  test('omits phase-kickoff facts that the specialist card already covers', () => {
    const fact = { id: 'f-rv', text: 'Reviewing the diff (iteration 1).', ts: 2000, isDeviation: false, name: 'Reviewer', agentCode: 'RV' } as never
    const merged = mergeLoopCockpit(
      [{ me: true, body: 'go', ts: 100 }],
      [],
      [fact],
      new Set(['RV']),
    )
    expect(merged.map((m) => m.log?.kind || m.body)).toEqual(['go', 'session'])
    expect(isPhaseKickoffFact('Reviewing the diff (iteration 1).')).toBe(true)
    expect(isPhaseKickoffFact('Planning the change and definition-of-done.')).toBe(true)
    expect(isPhaseKickoffFact('Converged after 1 iteration(s): 0 open findings.')).toBe(false)
  })

  test('folds commit and handoff facts into the specialist timeline', () => {
    const merged = mergeLoopCockpit(
      [{ me: true, body: 'go', ts: 100 }],
      [{
        id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 1000,
        messages: [
          { body: 'working', ts: 1100 },
          { body: 'done', ts: 2500 },
        ],
      }],
      [
        { id: 'f-c', text: 'Committed <code>abc</code> (+1 −0) to <code>main</code>.', ts: 2000, isDeviation: false, name: 'Coder', agentCode: 'CO' } as never,
        { id: 'f-h', text: 'Handoff: raised the breakpoint.', ts: 2600, isDeviation: false, name: 'Coder', agentCode: 'CO' } as never,
        { id: 'f-m', text: 'Converged.', ts: 3000, isDeviation: false } as never,
      ],
      new Set(),
    )
    expect(merged.map((m) => m.log?.kind || m.body)).toEqual(['go', 'session', 'fact'])
    expect(sessionMsgs(merged[1])).toEqual([
      { body: 'working', ts: 1100 },
      { note: true, noteLabel: 'Commit', noteHtml: true, body: 'Committed <code>abc</code> (+1 −0) to <code>main</code>.', ts: 2000 },
      { body: 'done', ts: 2500 },
      { note: true, noteLabel: 'Handoff', noteHtml: true, body: 'Handoff: raised the breakpoint.', ts: 2600 },
    ])
    expect(merged[2].log).toMatchObject({ kind: 'fact', text: 'Converged.' })
    expect(isSpecialistWrapFact('Committed abc (+1 −0) to main.')).toBe(true)
    expect(isSpecialistWrapFact('Handoff: raised the breakpoint.')).toBe(true)
    expect(isSpecialistWrapFact('Converged.')).toBe(false)
  })

  test('packs consecutive specialists into one fanout group', () => {
    const merged = mergeLoopCockpit(
      [{ me: true, body: 'go', ts: 100 }],
      [
        { id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 1000, messages: [{ body: 'done', ts: 1100 }] },
        { id: 's-rv', code: 'RV', name: 'Reviewer', kind: 'reviewer', ts: 2000, messages: [{ body: 'ok', ts: 2100 }] },
      ],
      [{ id: 'f-m', text: 'Converged.', ts: 3000, isDeviation: false } as never],
      new Set(),
    )
    expect(merged.map((m) => m.log?.kind || m.body)).toEqual(['go', 'sessions', 'fact'])
    expect(merged[1].log).toMatchObject({
      kind: 'sessions',
      id: 's-co#0',
      sessions: [
        { code: 'CO', name: 'Coder' },
        { code: 'RV', name: 'Reviewer' },
      ],
    })
  })

  test('does not pack specialists split by an engine fact', () => {
    const merged = mergeLoopCockpit(
      [{ me: true, body: 'go', ts: 100 }],
      [
        { id: 's-rv', code: 'RV', name: 'Reviewer', kind: 'reviewer', ts: 1000, messages: [{ body: 'ok', ts: 1100 }] },
        { id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 3000, messages: [{ body: 'done', ts: 3100 }] },
      ],
      [{ id: 'f-m', text: 'Converged.', ts: 2000, isDeviation: false } as never],
      new Set(),
    )
    expect(merged.map((m) => m.log?.kind || m.body)).toEqual(['go', 'session', 'fact', 'session'])
  })

  test('keeps the first turn id stable when a second dispatch lands', () => {
    const sess = {
      id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 1000,
      messages: [
        { body: 'first', ts: 1100 },
        { turnEnd: true, durationMs: 1, ts: 1200 },
      ],
    }
    const one = mergeLoopCockpit([], [sess], [], new Set())
    const two = mergeLoopCockpit([], [{
      ...sess,
      messages: [
        ...sess.messages,
        { me: true, body: 'again', ts: 2000 },
        { body: 'second', ts: 2100 },
      ],
    }], [], new Set(['CO']))
    expect(one[0].log).toMatchObject({ id: 's-co#0', live: false })
    expect(two[0].log).toMatchObject({
      kind: 'sessions',
      id: 's-co#0',
      sessions: [
        { id: 's-co#0', live: false },
        { id: 's-co#1', live: true },
      ],
    })
  })

  test('live follow-up without timestamps sits after later Maestro rows, not at createdAt', () => {
    const merged = mergeLoopCockpit(
      [
        { me: true, body: 'go', ts: 100 },
        { body: 'delegated', ts: 200 },
        { me: true, body: 'approved', ts: 5000 },
      ],
      [{
        id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', ts: 300, liveTs: 6000,
        messages: [
          { body: 'first', ts: 300 },
          { turnEnd: true, durationMs: 1, ts: 400 },
          { body: 'second', streaming: true },
        ],
      }],
      [],
      new Set(['CO']),
    )
    expect(merged.map((m) => m.body || m.log?.kind)).toEqual([
      'go', 'delegated', 'session', 'approved', 'session',
    ])
    const cards = merged.filter((m) => m.log?.kind === 'session')
    expect(cards[0].log).toMatchObject({ id: 's-co#0', live: false })
    expect(cards[1].log).toMatchObject({ id: 's-co#1', live: true })
    expect(sessionMsgs(cards[1]).some((m) => m.body === 'second')).toBe(true)
  })

  test('keeps a Maestro resume after the human answer when the stream has no timestamps', () => {
    const merged = mergeLoopCockpit(
      [
        { me: true, body: 'go', ts: 100 },
        { thinking: true, body: 'need a decision', ts: 200 },
        { me: true, body: 'use notes', ts: 5000, optimistic: true },
        { thinking: true, body: 'resuming', streaming: true },
      ],
      [],
      [],
      new Set(),
    )
    expect(merged.map((m) => m.body || (m.thinking ? 'think' : ''))).toEqual([
      'go', 'need a decision', 'use notes', 'resuming',
    ])
  })
})

describe('needsLiveTurnBreak', () => {
  test('breaks after a settled assistant turn, not mid-stream or after a user prompt', () => {
    expect(needsLiveTurnBreak([{ body: 'done' }])).toBe(true)
    expect(needsLiveTurnBreak([{ body: '…', streaming: true }])).toBe(false)
    expect(needsLiveTurnBreak([{ me: true, body: 'fix again' }])).toBe(false)
    expect(needsLiveTurnBreak([{ turnEnd: true }])).toBe(false)
    expect(needsLiveTurnBreak([])).toBe(false)
  })
})

describe('parseLoopPRFact', () => {
  test('parses the legacy long Local PR sentence', () => {
    const html = 'Opened a Local PR <code>#157</code> (+721 −0 across 8 files) — ready for your review.'
    expect(parseLoopPRFact(html)).toEqual({ number: 157, source: 'local' })
  })

  test('parses the completed-in line with data-pr', () => {
    const html = 'Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>'
    expect(parseLoopPRFact(html)).toEqual({
      number: 157, prId: 'p1', duration: '12m 04s', source: 'local',
    })
  })

  test('parses a data-pr link with no Completed in / Opened prefix', () => {
    const html = '<a class="loop-pr-link" data-pr="p1">Local PR #12</a>'
    expect(parseLoopPRFact(html)).toEqual({ number: 12, prId: 'p1', source: 'local' })
  })

  test('ignores non-PR facts', () => {
    expect(parseLoopPRFact('Converged after 2 iteration(s): 0 open findings.')).toBeNull()
    expect(parseLoopPRFact('Could not open the PR: gh missing')).toBeNull()
  })

  test('does not treat a GitHub fallback note as an opened GitHub PR', () => {
    const html = 'GitHub PR not opened (gh pr create failed: a pull request already exists: https://github.com/acme/r/pull/39) — opening a Local PR instead.'
    expect(parseLoopPRFact(html)).toBeNull()
    expect(loopPRCompletion(html, {
      factTs: 1000 + 37 * 60_000 + 11_000,
      startTs: 1000,
    })).toBeNull()
  })

  test('ignores wrap-body prose that would collide with PR parsing', () => {
    const wrap = (body: string) => `<div class="loop-wrap">${body}</div>`
    expect(parseLoopPRFact(
      'Completed in 12s.' + wrap('<b>Done</b> Reviewed PR #39 and recorded the findings'),
    )).toBeNull()
    expect(loopPRCompletion(
      'Completed in 12s.' + wrap('<b>Done</b> Reviewed PR #39 and recorded the findings'),
    )).toBeNull()
    expect(parseLoopPRFact(
      'Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">GitHub PR #157</a>' +
      wrap('<b>Done</b> Fixed Local PR rendering'),
    )).toEqual({ number: 157, prId: 'p1', duration: '12m 04s', source: 'gh' })
    expect(parseLoopPRFact(
      'Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>' +
      wrap('<b>Notes</b> Settings dialog was not opened during testing'),
    )).toEqual({ number: 157, prId: 'p1', duration: '12m 04s', source: 'local' })
  })
})

describe('formatLoopElapsed', () => {
  test('mirrors Go formatLoopElapsed', () => {
    expect(formatLoopElapsed(3000)).toBe('3s')
    expect(formatLoopElapsed(90_000)).toBe('1m 30s')
    expect(formatLoopElapsed(5 * 60_000)).toBe('5m')
    expect(formatLoopElapsed(62 * 60_000)).toBe('1h 2m')
  })
})

describe('loopPRCompletion', () => {
  test('keeps baked-in duration and data-pr', () => {
    const html = 'Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>'
    expect(loopPRCompletion(html)).toEqual({
      duration: '12m 04s', prId: 'p1', prLabel: 'Local PR #157',
    })
  })

  test('resolves a legacy fact via workspace PRs and cockpit span', () => {
    const html = 'Opened a Local PR <code>#157</code> (+721 −0 across 8 files) — ready for your review.'
    expect(loopPRCompletion(html, {
      prs: [{ id: 'p1', number: 157, workspaceId: 'ws1' }],
      workspaceId: 'ws1',
      factTs: 1000 + 12 * 60_000 + 4_000,
      startTs: 1000,
    })).toEqual({
      duration: '12m 04s', prId: 'p1', prLabel: 'Local PR #157',
    })
  })

  test('does not treat the ts=1 opening-prompt sentinel as loop start', () => {
    const html = 'Opened a Local PR <code>#157</code> (+721 −0 across 8 files) — ready for your review.'
    expect(loopPRCompletion(html, {
      factTs: Date.now(),
      startTs: 1,
    })).toEqual({ duration: '', prLabel: 'Local PR #157' })
  })
})

describe('loopCompletionWrap', () => {
  const wrap = '<div class="loop-wrap"><b>Issue</b> ACME-1 — Flaky checkout<br><b>Done</b> Retry the token</div>'
  const html = 'Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>' + wrap

  test('extracts the wrap body and leaves parseLoopPRFact intact', () => {
    expect(loopCompletionWrap(html)).toBe('<b>Issue</b> ACME-1 — Flaky checkout<br><b>Done</b> Retry the token')
    expect(stripLoopWrap(html)).toBe('Completed in 12m 04s · <a class="loop-pr-link" data-pr="p1">Local PR #157</a>')
    expect(parseLoopPRFact(html)).toEqual({
      number: 157, prId: 'p1', duration: '12m 04s', source: 'local',
    })
  })

  test('returns empty when there is no wrap', () => {
    expect(loopCompletionWrap('Completed in 12s.')).toBe('')
    expect(stripLoopWrap('Completed in 12s.')).toBe('Completed in 12s.')
  })
})
