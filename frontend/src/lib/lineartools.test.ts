import { describe, expect, it } from 'vitest'
import {
  isLinearTool,
  linearCountLabel,
  linearInputBody,
  linearItemUrl,
  linearSubject,
  linearToolLabel,
  linearToolMeta,
  parseLinearArgs,
  parseLinearResult,
} from './lineartools'

const apollo = {
  id: '712025ee-a70e-4000-8000-000000000001',
  name: 'Apollo',
  summary: 'QA and pre-launch testing',
  description: "there's a lot going on in the app, so the immediate need is simply more people using it hard and filing clear bug reports.\n\n## What Apollo is\n\nA browser app… (truncated, use `get_project` for full description)",
  url: 'https://linear.app/acme/project/apollo-qa-and-pre-launch-testing-712025eea70e',
  createdAt: '2026-05-29T10:31:35.161Z',
  updatedAt: '2026-05-29T10:31:35.161Z',
  startedAt: null,
  completedAt: null,
  canceledAt: null,
  startDate: null,
  targetDate: '2026-06-16',
  priority: { value: 1, name: 'Urgent' },
  status: { name: 'In Progress', type: 'started' },
}

const acme = {
  id: 'e61f6417-68c4-41e3-a2b1-a55a334a8d79',
  icon: 'Robot',
  name: 'Acme',
  visibility: 'public',
  createdAt: '2026-06-16T12:31:54.884Z',
  updatedAt: '2026-09-14T04:10:51.227Z',
}

describe('isLinearTool', () => {
  it('matches Recur and plugin Linear MCP names', () => {
    expect(isLinearTool('mcp__linear__list_projects')).toBe(true)
    expect(isLinearTool('mcp__plugin_linear_linear__list_teams')).toBe(true)
    expect(isLinearTool('mcp__recur__list_findings')).toBe(false)
    expect(isLinearTool('list_projects')).toBe(false)
    expect(isLinearTool('WebSearch')).toBe(false)
  })
})

describe('linear labels', () => {
  it('uses tense + query, never the args JSON', () => {
    const args = parseLinearArgs({ query: 'Apollo' })
    expect(linearSubject(args)).toBe('Apollo')
    expect(linearToolLabel({
      tool: 'mcp__linear__list_projects',
      pending: true,
      failed: false,
      args,
      view: null,
    })).toBe('Listing projects')
    const view = parseLinearResult(JSON.stringify({ projects: [apollo], hasNextPage: false }))
    const label = linearToolLabel({
      tool: 'mcp__linear__list_projects',
      pending: false,
      failed: false,
      args,
      view,
    })
    expect(linearToolLabel({
      tool: 'mcp__linear__list_projects',
      pending: false,
      failed: true,
      args,
      view: null,
    })).toBe('List projects failed')
    const meta = linearToolMeta(args, view, label)
    expect(meta).toContain('"Apollo"')
    expect(meta).not.toContain('createdAt')
    expect(meta).not.toContain('{')
  })

  it('names a get_ lookup after the identifier', () => {
    const args = parseLinearArgs({ id: 'ACME-114' })
    expect(linearToolLabel({
      tool: 'mcp__linear__get_issue',
      pending: false,
      failed: false,
      args,
      view: null,
    })).toBe('Looked up ACME-114')
  })
})

describe('parseLinearResult', () => {
  it('lifts list_projects into rows without the truncated description', () => {
    const view = parseLinearResult(JSON.stringify({
      projects: [apollo],
      hasNextPage: false,
    }))
    expect(view?.kind).toBe('list')
    expect(view?.noun).toBe('projects')
    expect(view?.items).toHaveLength(1)
    const row = view!.items[0]!
    expect(row.title).toBe('Apollo')
    expect(row.url).toContain('linear.app')
    expect(row.chips).toContain('Urgent')
    expect(row.chips).toContain('In Progress')
    expect(row.summary).toBe('QA and pre-launch testing')
    expect(row.body).toBe('')
    expect(linearCountLabel(view!)).toBe('1 project')
    expect(JSON.stringify(row)).not.toContain('createdAt')
  })

  it('renders list_teams as a name + visibility row, not a uuid dump', () => {
    const view = parseLinearResult(JSON.stringify({
      teams: [acme],
      hasNextPage: false,
    }))
    expect(view?.items[0]?.title).toBe('Acme')
    expect(view?.items[0]?.chips).toContain('public')
    expect(view?.items[0]?.title).not.toMatch(/e61f6417/)
  })

  it('keeps label color and drops the team filter JSON as a title', () => {
    const view = parseLinearResult(JSON.stringify({
      labels: [
        { id: 'lab-1', name: 'Bug', color: '#EB5757', description: 'Something is broken' },
      ],
    }))
    expect(view?.items[0]?.title).toBe('Bug')
    expect(view?.items[0]?.color).toBe('#EB5757')
    expect(view?.items[0]?.summary).toBe('Something is broken')
  })

  it('shows get_issue description as markdown body', () => {
    const view = parseLinearResult(JSON.stringify({
      id: 'ACME-114',
      identifier: 'ACME-114',
      title: 'Apollo transcript cards',
      description: '## Context\n\nRender Linear MCP results as cards.',
      url: 'https://linear.app/acme/issue/ACME-114',
      status: { name: 'In Progress' },
      assignee: { name: 'Josh' },
    }))
    expect(view?.kind).toBe('item')
    expect(view?.items[0]?.identifier).toBe('ACME-114')
    expect(view?.items[0]?.title).toBe('Apollo transcript cards')
    expect(view?.items[0]?.body).toContain('Render Linear MCP results')
    expect(view?.items[0]?.chips).toEqual(['In Progress', 'Josh'])
    expect(linearItemUrl(view)).toContain('ACME-114')
  })

  it('keeps get_issue over relationship arrays and nested project/team', () => {
    const view = parseLinearResult(JSON.stringify({
      id: '712025ee-a70e-4000-8000-000000000114',
      identifier: 'ACME-114',
      title: 'Apollo transcript cards',
      description: '## Context\n\nRender Linear MCP results as cards.',
      url: 'https://linear.app/acme/issue/ACME-114',
      status: { name: 'In Progress' },
      labels: ['Bug'],
      attachments: [{ id: 'att-1', title: 'shot.png' }],
      documents: [{ id: 'doc-1', title: 'Spec' }],
      project: apollo,
      team: acme,
    }))
    expect(view?.kind).toBe('item')
    expect(view?.noun).toBe('')
    expect(view?.items).toHaveLength(1)
    expect(view?.items[0]?.identifier).toBe('ACME-114')
    expect(view?.items[0]?.title).toBe('Apollo transcript cards')
    expect(view?.items[0]?.body).toContain('Render Linear MCP results')
    expect(linearItemUrl(view)).toContain('ACME-114')
  })

  it('rejects a non-entity envelope', () => {
    expect(parseLinearResult('{"error":"nope"}')).toBeNull()
    expect(parseLinearResult('Refused: no')).toBeNull()
    expect(parseLinearResult('')).toBeNull()
  })
})

describe('linearInputBody', () => {
  it('reads comment/issue markdown from args, not as JSON', () => {
    expect(linearInputBody({ body: '## Hello\n\nWorld' })).toBe('## Hello\n\nWorld')
    expect(linearInputBody({ query: 'Apollo' })).toBe('')
  })
})
