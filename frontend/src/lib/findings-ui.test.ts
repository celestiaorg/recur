import { describe, expect, test } from 'vitest'
import {
  findingFlagForPath,
  findingLocLabel,
  findingLocSuffix,
  findingHasPendingComment,
  findingHasSubmittedComment,
  findingReadyToSettle,
  findingIdsOnPending,
  dropFindingIdsRemovedFromPending,
  findingReviewAnchor,
  findingSeverityTally,
  hydrateListedFindings,
  parseListFindingsResult,
  resolveListFindings,
  type FindingFlag,
} from './findings-ui'

function flag(severity: string, count = 1): FindingFlag {
  return { count, severity }
}

describe('findingFlagForPath', () => {
  test('file miss and empty dir still return an object (never null)', () => {
    const empty = new Map<string, FindingFlag>()
    expect(findingFlagForPath('src/a.ts', false, empty)).toEqual({ count: 0, severity: 'info' })
    expect(findingFlagForPath('src', true, empty)).toEqual({ count: 0, severity: 'info' })
  })

  test('file hit is the map entry; dir rolls up children and worst severity', () => {
    const byPath = new Map<string, FindingFlag>([
      ['src/a.ts', flag('low')],
      ['src/b.ts', flag('critical', 2)],
      ['lib/c.ts', flag('high')],
    ])
    expect(findingFlagForPath('src/a.ts', false, byPath)).toEqual({ count: 1, severity: 'low' })
    expect(findingFlagForPath('src', true, byPath)).toEqual({ count: 3, severity: 'critical' })
    expect(findingFlagForPath('lib', true, byPath)).toEqual({ count: 1, severity: 'high' })
  })
})

describe('findingReviewAnchor', () => {
  function files(path: string, right: number[]) {
    return [{ path, lines: right.map((n) => ({ new: String(n), type: 'add' as const })) }]
  }

  test('maps a span onto GitHub line/startLine (end is the comment line)', () => {
    expect(findingReviewAnchor({ file: 'a.go', line: 10, endLine: 14 }, files('a.go', [10, 11, 12, 13, 14])))
      .toEqual({ file: 'a.go', line: 14, startLine: 10 })
    expect(findingReviewAnchor({ file: 'a.go', line: 10 }, files('a.go', [10])))
      .toEqual({ file: 'a.go', line: 10, startLine: 0 })
  })

  test('resolves a suffix path to the PR file and rejects out-of-diff lines', () => {
    const pr = files('src/hooks/useOAuthSession.ts', [2, 8])
    expect(findingReviewAnchor({ file: 'useOAuthSession.ts', line: 2 }, pr))
      .toEqual({ file: 'src/hooks/useOAuthSession.ts', line: 2, startLine: 0 })
    expect(findingReviewAnchor({ file: 'src/hooks/useOAuthSession.ts', line: 99 }, pr)).toBeNull()
    expect(findingReviewAnchor({ file: 'missing.ts', line: 2 }, pr)).toBeNull()
  })

  test('rejects an ambiguous suffix instead of guessing a PR file', () => {
    const pr = [
      { path: 'src/index.ts', lines: [{ new: '1', type: 'add' as const }] },
      { path: 'test/index.ts', lines: [{ new: '1', type: 'add' as const }] },
    ]
    expect(findingReviewAnchor({ file: 'index.ts', line: 1 }, pr)).toBeNull()
    expect(findingReviewAnchor({ file: 'src/index.ts', line: 1 }, pr))
      .toEqual({ file: 'src/index.ts', line: 1, startLine: 0 })
  })

  test('rejects findings that cannot be a line comment', () => {
    const pr = files('a.go', [10])
    expect(findingReviewAnchor({ file: '', line: 10 }, pr)).toBeNull()
    expect(findingReviewAnchor({ file: 'a.go', line: 0 }, pr)).toBeNull()
    expect(findingReviewAnchor({ file: '  ', line: 3 }, pr)).toBeNull()
  })
})

describe('findingHasPendingComment', () => {
  test('matches the resolved GitHub anchor, not the raw finding path', () => {
    const pr = [{ path: 'src/a.ts', lines: [{ new: '2', type: 'add' }] }]
    const f = { file: 'a.ts', line: 2, message: 'race' }
    expect(findingHasPendingComment(f, [{ file: 'src/a.ts', line: 2, body: 'race' }], pr)).toBe(true)
    expect(findingHasPendingComment(f, [{ file: 'a.ts', line: 2, body: 'race' }], pr)).toBe(false)
    expect(findingHasPendingComment(f, [{ file: 'src/a.ts', line: 2, body: 'other' }], pr)).toBe(false)
  })

  test('requires the RIGHT-side span, not only end line and body', () => {
    const pr = [{ path: 'src/a.ts', lines: [10, 11, 12].map((n) => ({ new: String(n), type: 'add' as const })) }]
    const f = { file: 'src/a.ts', line: 10, endLine: 12, message: 'race' }
    expect(findingHasPendingComment(f, [{ file: 'src/a.ts', line: 12, startLine: 10, side: 'RIGHT', body: 'race' }], pr)).toBe(true)
    expect(findingHasPendingComment(f, [{ file: 'src/a.ts', line: 12, body: 'race' }], pr)).toBe(false)
    expect(findingHasPendingComment(f, [{ file: 'src/a.ts', line: 12, startLine: 10, side: 'LEFT', body: 'race' }], pr)).toBe(false)
  })

  test('trims finding and pending bodies the way AddPRComment persists them', () => {
    const pr = [{ path: 'src/a.ts', lines: [{ new: '2', type: 'add' }] }]
    const f = { file: 'src/a.ts', line: 2, message: '  race\n' }
    expect(findingHasPendingComment(f, [{ file: 'src/a.ts', line: 2, body: 'race' }], pr)).toBe(true)
  })
})

describe('findingHasSubmittedComment', () => {
  test('matches a submitted RIGHT-side comment at the resolved anchor', () => {
    const pr = [{ path: 'src/a.ts', lines: [{ new: '2', type: 'add' }] }]
    const f = { file: 'a.ts', line: 2, message: 'race' }
    expect(findingHasSubmittedComment(f, [{ path: 'src/a.ts', line: 2, side: 'RIGHT', bodyMd: 'race' }], pr)).toBe(true)
    expect(findingHasSubmittedComment(f, [{ path: 'src/a.ts', line: 2, side: 'LEFT', bodyMd: 'race' }], pr)).toBe(false)
    expect(findingHasSubmittedComment(f, [{ path: 'src/a.ts', line: 2, bodyMd: 'other' }], pr)).toBe(false)
  })

  test('trims finding and submitted bodies the way GitHub returns them', () => {
    const pr = [{ path: 'src/a.ts', lines: [{ new: '2', type: 'add' }] }]
    const f = { file: 'src/a.ts', line: 2, message: '\trace  ' }
    expect(findingHasSubmittedComment(f, [{ path: 'src/a.ts', line: 2, side: 'RIGHT', bodyMd: 'race' }], pr)).toBe(true)
  })
})

describe('findingReadyToSettle', () => {
  const pr = [{ path: 'src/a.ts', lines: [{ new: '2', type: 'add' }] }]
  const f = { id: 'F-1', file: 'src/a.ts', line: 2, message: 'race' }
  const submitted = [{ path: 'src/a.ts', line: 2, side: 'RIGHT', bodyMd: 'race' }]

  test('requires this session queued the finding — a matching thread comment is not enough', () => {
    expect(findingReadyToSettle(f, [], submitted, pr, new Set())).toBe(false)
    expect(findingReadyToSettle(f, [], submitted, pr, new Set(['F-other']))).toBe(false)
    expect(findingReadyToSettle(f, [], submitted, pr, new Set(['F-1']))).toBe(true)
  })

  test('waits until the pending draft is gone', () => {
    expect(findingReadyToSettle(f, [{ file: 'src/a.ts', line: 2, body: 'race' }], submitted, pr, new Set(['F-1']))).toBe(false)
  })
})

describe('findingIdsOnPending', () => {
  test('collects unique trimmed finding ids', () => {
    expect(findingIdsOnPending([
      { findingId: 'F-1' },
      { findingId: ' F-1 ' },
      { findingId: 'F-2' },
      { findingId: '' },
      {},
    ])).toEqual(['F-1', 'F-2'])
  })
})

describe('dropFindingIdsRemovedFromPending', () => {
  test('retires a deleted draft and keeps one that is still pending', () => {
    const posted = new Set(['F-1', 'F-2'])
    const next = dropFindingIdsRemovedFromPending(
      posted,
      [{ findingId: 'F-1' }, { findingId: 'F-2' }],
      [{ findingId: 'F-2' }],
    )
    expect([...next]).toEqual(['F-2'])
  })

  test('returns the same set when nothing was removed', () => {
    const posted = new Set(['F-1'])
    expect(dropFindingIdsRemovedFromPending(posted, [{ findingId: 'F-1' }], [{ findingId: 'F-1' }])).toBe(posted)
  })
})

describe('findingLocLabel', () => {
  test('basename plus en-dash range', () => {
    expect(findingLocSuffix(294, 300)).toBe(':294–300')
    expect(findingLocLabel('docs/bundles.md', 294, 300))
      .toBe('bundles.md:294–300')
    expect(findingLocLabel('internal/engine/loop.go', 765)).toBe('loop.go:765')
    expect(findingLocLabel('', 0)).toBe('')
  })
})

describe('parseListFindingsResult', () => {
  const row = {
    id: 'F-1',
    file: 'docs/plan.md',
    line: 10,
    endLine: 12,
    severity: 'high',
    message: 'Deadlock in qa verify.',
    status: 'open',
  }

  test('parses the structured list_findings envelope', () => {
    const got = parseListFindingsResult(JSON.stringify({
      status: 'all', count: 1, findings: [row],
    }))
    expect(got).toEqual({
      status: 'all',
      count: 1,
      findings: [{
        id: 'F-1',
        file: 'docs/plan.md',
        line: 10,
        endLine: 12,
        severity: 'high',
        message: 'Deadlock in qa verify.',
        summary: '',
        status: 'open',
        statusReason: '',
      }],
    })
  })

  test('accepts a prose prefix so wrapped MCP text still renders rows', () => {
    const got = parseListFindingsResult('Listed findings\n' + JSON.stringify({
      status: 'open', findings: [row],
    }))
    expect(got?.count).toBe(1)
    expect(got?.findings[0].id).toBe('F-1')
  })

  test('rejects a sliced mid-JSON blob (card must not dump it)', () => {
    const raw = JSON.stringify({ status: 'all', count: 15, findings: [row, row] })
    expect(parseListFindingsResult(raw.slice(0, 40))).toBeNull()
  })
})

describe('resolveListFindings', () => {
  const store = [{
    id: 'F-1',
    file: 'docs/plan.md',
    line: 10,
    endLine: 12,
    severity: 'high',
    message: 'Full store message that the truncated payload lost.',
    status: 'open',
  }]

  test('hydrates a truncated-but-parseable row from the store', () => {
    const got = resolveListFindings(
      JSON.stringify({
        status: 'all',
        count: 1,
        findings: [{ id: 'F-1', file: 'docs/plan.md', severity: 'high', message: 'Deadlock…', status: 'open' }],
      }),
      false,
      store,
    )
    expect(got?.findings[0].message).toBe('Full store message that the truncated payload lost.')
    expect(got?.findings[0].line).toBe(10)
  })

  test('keeps payload status so a historical open list does not grow RESOLVED badges', () => {
    const got = resolveListFindings(
      JSON.stringify({
        status: 'open',
        count: 1,
        findings: [{ id: 'F-1', file: 'docs/plan.md', severity: 'high', message: 'Deadlock in qa verify.', status: 'open' }],
      }),
      false,
      [{ ...store[0], status: 'resolved', severity: 'low' }],
    )
    expect(got?.findings[0].status).toBe('open')
    expect(got?.findings[0].severity).toBe('high')
    expect(got?.findings[0].message).toBe('Deadlock in qa verify.')
  })

  test('does not invent today\'s store rows when the tool result is unparseable', () => {
    expect(resolveListFindings('{"status":"all","count":15,"findings":[{', false, store)).toBeNull()
  })

  test('does not invent rows when the tool reported an error', () => {
    expect(resolveListFindings('boom', true, store)).toBeNull()
  })
})

describe('findingSeverityTally', () => {
  test('orders known severities and keeps extras', () => {
    expect(findingSeverityTally([
      { severity: 'low' },
      { severity: 'high' },
      { severity: 'high' },
      { severity: 'mystery' },
    ])).toEqual([
      { severity: 'high', count: 2 },
      { severity: 'low', count: 1 },
      { severity: 'mystery', count: 1 },
    ])
  })
})

describe('hydrateListedFindings', () => {
  test('leaves unknown ids untouched', () => {
    const listed = parseListFindingsResult(JSON.stringify({
      status: 'open',
      findings: [{ id: 'missing', file: 'a.go', severity: 'low', message: 'x', status: 'open' }],
    }))!
    expect(hydrateListedFindings(listed, []).findings[0].message).toBe('x')
  })
})
