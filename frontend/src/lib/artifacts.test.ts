import { describe, expect, test } from 'vitest'
import {
  addUnseenArtifactId,
  artifactCanOpen,
  artifactFromEvent,
  artifactIcon,
  artifactIdFromScratchPath,
  artifactKindTally,
  artifactWhen,
  clearUnseenArtifactIds,
  parseListArtifactsResult,
  resolveListArtifacts,
} from './artifacts'

describe('artifactFromEvent', () => {
  test('accepts JSON-tagged fields', () => {
    expect(artifactFromEvent({
      id: 'a1',
      title: 'Report',
      kind: 'md',
      path: 'workspaces/w/artifacts/a1.md',
    })).toEqual({
      id: 'a1',
      title: 'Report',
      kind: 'md',
      path: 'workspaces/w/artifacts/a1.md',
    })
  })

  test('rejects exported Go field names (json tags are the wire shape)', () => {
    expect(artifactFromEvent({
      ID: 'a1',
      Title: 'Report',
      Kind: 'HTML',
      Path: 'workspaces/w/artifacts/a1.html',
    })).toBeNull()
  })

  test('rejects missing id or path', () => {
    expect(artifactFromEvent(null)).toBeNull()
    expect(artifactFromEvent({ title: 'x' })).toBeNull()
    expect(artifactFromEvent({ id: 'a1' })).toBeNull()
    expect(artifactFromEvent({ path: 'workspaces/w/artifacts/a1.md' })).toBeNull()
  })
})

describe('addUnseenArtifactId', () => {
  test('create then present of the same id stays one', () => {
    let ids = addUnseenArtifactId({}, 'ws-b', 'art-1')
    ids = addUnseenArtifactId(ids, 'ws-b', 'art-1')
    expect(ids['ws-b']).toEqual(['art-1'])
  })

  test('distinct ids and workspaces accumulate', () => {
    let ids = addUnseenArtifactId({}, 'ws-a', 'art-1')
    ids = addUnseenArtifactId(ids, 'ws-a', 'art-2')
    ids = addUnseenArtifactId(ids, 'ws-b', 'art-1')
    expect(ids['ws-a']).toEqual(['art-1', 'art-2'])
    expect(ids['ws-b']).toEqual(['art-1'])
  })
})

describe('clearUnseenArtifactIds', () => {
  test('clears one workspace only', () => {
    const ids = clearUnseenArtifactIds(
      { 'ws-a': ['art-1'], 'ws-b': ['art-2'] },
      'ws-a',
    )
    expect(ids['ws-a']).toEqual([])
    expect(ids['ws-b']).toEqual(['art-2'])
  })
})

describe('artifactIcon', () => {
  test('maps known kinds', () => {
    expect(artifactIcon('html')).toBe('file-type-html')
    expect(artifactIcon('md')).toBe('markdown')
  })
})

describe('artifactIdFromScratchPath', () => {
  test('reads the id from an artifact scratch tab', () => {
    expect(artifactIdFromScratchPath('scratch://artifact-a1.md')).toBe('a1')
    expect(artifactIdFromScratchPath('scratch://artifact-52f1b2e3-0555.html')).toBe('52f1b2e3-0555')
  })

  test('keeps dots inside the id (lang is the last segment)', () => {
    expect(artifactIdFromScratchPath('scratch://artifact-shot.v2.png')).toBe('shot.v2')
  })

  test('ignores other editor paths', () => {
    expect(artifactIdFromScratchPath('')).toBe('')
    expect(artifactIdFromScratchPath('src/lib/artifacts.ts')).toBe('')
    expect(artifactIdFromScratchPath('scratch://untitled-1.ts')).toBe('')
  })
})

const row = {
  id: '52f1b2e3-0555-471e-bbef-13e9c9b408fb',
  workspaceId: 'recur.bubbly-parfait',
  title: 'Modes as bundle items',
  kind: 'html',
  path: 'workspaces/recur.bubbly-parfait/artifacts/52f1b2e3.html',
  createdAt: '2026-08-20T04:32:46Z',
  createdBy: 'PA',
}

describe('parseListArtifactsResult', () => {
  test('parses the structured list_artifacts envelope', () => {
    const got = parseListArtifactsResult(JSON.stringify({
      count: 1, artifacts: [row],
    }))
    expect(got).toEqual({
      count: 1,
      artifacts: [{
        id: row.id,
        title: 'Modes as bundle items',
        kind: 'html',
        path: row.path,
        createdAt: row.createdAt,
        createdBy: 'PA',
      }],
    })
  })

  test('accepts a prose prefix so wrapped MCP text still renders rows', () => {
    const got = parseListArtifactsResult('Listed artifacts\n' + JSON.stringify({
      artifacts: [row],
    }))
    expect(got?.count).toBe(1)
    expect(got?.artifacts[0].title).toBe('Modes as bundle items')
  })

  test('rejects a sliced mid-JSON blob (card must not dump it)', () => {
    const raw = JSON.stringify({ count: 2, artifacts: [row, row] })
    expect(parseListArtifactsResult(raw.slice(0, 40))).toBeNull()
  })

  test('treats a Go nil-slice envelope as an empty list', () => {
    expect(parseListArtifactsResult(JSON.stringify({ count: 0, artifacts: null })))
      .toEqual({ count: 0, artifacts: [] })
  })

  test('rejects an unrelated object so the card does not invent an empty list', () => {
    expect(parseListArtifactsResult(JSON.stringify({ status: 'ok' }))).toBeNull()
  })
})

describe('artifactCanOpen', () => {
  test('requires an id and a workspaces/ path', () => {
    expect(artifactCanOpen(row)).toBe(true)
    expect(artifactCanOpen({ ...row, path: '/' + row.path })).toBe(true)
    expect(artifactCanOpen({ id: row.id, path: '' })).toBe(false)
    expect(artifactCanOpen({ id: row.id, path: 'artifacts/x.html' })).toBe(false)
    expect(artifactCanOpen({ id: '', path: row.path })).toBe(false)
  })
})

describe('resolveListArtifacts', () => {
  test('does not invent rows when the tool reported an error', () => {
    expect(resolveListArtifacts(JSON.stringify({ artifacts: [row] }), true)).toBeNull()
  })

  test('does not invent rows when the tool result is unparseable', () => {
    expect(resolveListArtifacts('{"count":2,"artifacts":[{', false)).toBeNull()
  })
})

describe('artifactKindTally', () => {
  test('orders known kinds and keeps extras', () => {
    expect(artifactKindTally([
      { kind: 'md' },
      { kind: 'html' },
      { kind: 'html' },
      { kind: 'mystery' },
    ])).toEqual([
      { kind: 'html', count: 2 },
      { kind: 'md', count: 1 },
      { kind: 'mystery', count: 1 },
    ])
  })
})

describe('artifactWhen', () => {
  test('returns compact age and a full stamp', () => {
    const now = Date.parse('2026-08-20T05:32:46Z')
    const got = artifactWhen('2026-08-20T04:32:46Z', now)
    expect(got?.rel).toBe('1h')
    expect(got?.full).toBeTruthy()
  })

  test('blank createdAt is empty', () => {
    expect(artifactWhen('')).toBeNull()
  })
})
