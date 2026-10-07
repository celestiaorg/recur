import { describe, expect, it } from 'vitest'
import {
  isListWorkspaceBriefsTool,
  isGetWorkspaceContextTool,
  isPresentWorkspaceTool,
  parseListWorkspaceBriefs,
  parseWorkspaceContext,
  parsePresentWorkspace,
  parsePresentWorkspaceTarget,
  workspaceIdFromPresentTarget,
  hydratePresentBrief,
  listWorkspaceBriefsLabel,
  getWorkspaceContextLabel,
  presentWorkspaceLabel,
  briefingCountsMeta,
  listWorkspaceBriefsSummary,
  workspaceContextSummary,
} from './briefingtools'

const brief = {
  workspaceId: 'recur.zesty-quiche',
  projectName: 'recur',
  title: 'Account linking architecture',
  branch: 'mycodecrafting/multi-account-linking',
  bucket: 'needs',
  reasons: ['plan_approval', 'unfinished_plan', 'recent_chat'],
  now: 'Plan is ready but not approved',
  next: 'Review and approve the plan',
  primary: { label: 'Review plan', target: 'workspace:recur.zesty-quiche' },
}

describe('briefing tool detectors', () => {
  it('matches Recur MCP and bare names', () => {
    expect(isListWorkspaceBriefsTool('mcp__recur__list_workspace_briefs')).toBe(true)
    expect(isGetWorkspaceContextTool('mcp__recur__get_workspace_context')).toBe(true)
    expect(isPresentWorkspaceTool('mcp__recur__present_workspace')).toBe(true)
    expect(isPresentWorkspaceTool('present_workspace')).toBe(true)
    expect(isListWorkspaceBriefsTool('mcp__recur__list_findings')).toBe(false)
  })
})

describe('parseListWorkspaceBriefs', () => {
  it('reads counts and cards without treating the dump as a label', () => {
    const listed = parseListWorkspaceBriefs(JSON.stringify({
      counts: { needs: 1, working: 0, pickup: 2 },
      workspaces: [brief, { ...brief, workspaceId: 'w2', title: 'Fix build', bucket: 'pickup' }],
    }))
    expect(listed?.counts.needs).toBe(1)
    expect(listed?.workspaces).toHaveLength(2)
    expect(listed?.workspaces[0]?.title).toBe('Account linking architecture')
    expect(listWorkspaceBriefsLabel(false, listed)).toBe('Listed 2 workspaces')
    expect(listWorkspaceBriefsLabel(true, listed)).toBe('Listing workspaces')
    expect(briefingCountsMeta(listed!.counts)).toBe('1 need you · 2 pick up')
    expect(listWorkspaceBriefsLabel(false, listed)).not.toContain('{')
  })

  it('rejects a non-envelope', () => {
    expect(parseListWorkspaceBriefs('{"error":"nope"}')).toBeNull()
    expect(parseListWorkspaceBriefs('Refused: no')).toBeNull()
  })
})

describe('parseWorkspaceContext', () => {
  it('lifts the nested brief and extras', () => {
    const ctx = parseWorkspaceContext(JSON.stringify({
      brief,
      planSteps: [{ id: 'S1', title: 'Model identities', status: 'done' }],
      openAsks: [{ kind: 'question', id: 'q1', summary: 'Burst limit?' }],
      recentChats: [{ code: 'PA', title: 'Multi-account linking model' }],
    }))
    expect(ctx?.brief.title).toBe('Account linking architecture')
    expect(ctx?.planSteps[0]?.title).toBe('Model identities')
    expect(ctx?.openAsks[0]?.summary).toBe('Burst limit?')
    expect(getWorkspaceContextLabel(false, ctx!.brief.title, false)).toBe('Looked up Account linking architecture')
    expect(getWorkspaceContextLabel(false, ctx!.brief.title, false)).not.toContain('{')
  })
})

describe('parsePresentWorkspace', () => {
  it('reads the JSON card payload', () => {
    const card = parsePresentWorkspace(JSON.stringify({
      ok: true,
      target: 'workspace:recur.zesty-quiche',
      kind: 'workspace',
      brief,
    }))
    expect(card.ok).toBe(true)
    expect(card.brief?.title).toBe('Account linking architecture')
    expect(presentWorkspaceLabel(false, card)).toBe('Presented Account linking architecture')
    expect(presentWorkspaceLabel(false, card)).not.toContain('{')
    expect(presentWorkspaceLabel(true, card)).toBe('Presenting Account linking architecture')
  })

  it('understands the legacy Opening string and a refusal', () => {
    const legacy = parsePresentWorkspace('Opening workspace:recur.zesty-quiche.')
    expect(legacy.ok).toBe(true)
    expect(legacy.target).toBe('workspace:recur.zesty-quiche')
    const refused = parsePresentWorkspace('Refused: that workspace is not available', 'workspace:missing')
    expect(refused.ok).toBe(false)
    expect(presentWorkspaceLabel(false, refused)).toBe('Present workspace failed')
  })

  it('reads the target from args, never dumps JSON as the label', () => {
    expect(parsePresentWorkspaceTarget({ target: 'workspace:recur.zesty-quiche' })).toBe('workspace:recur.zesty-quiche')
    expect(parsePresentWorkspaceTarget({ target: 'workspace:recur.zesty-quiche' })).not.toContain('{')
  })

  it('hydrates a missing brief from the overview so the pin is a real card', () => {
    const card = parsePresentWorkspace('Opening workspace:recur.zesty-quiche.')
    expect(card.brief).toBeNull()
    expect(workspaceIdFromPresentTarget(card.target)).toBe('recur.zesty-quiche')
    expect(workspaceIdFromPresentTarget('workspace:recur.zesty-quiche:plan')).toBe('recur.zesty-quiche')
    const hydrated = hydratePresentBrief(card, [brief as never], [], [])
    expect(hydrated.brief?.title).toBe('Account linking architecture')
    expect(presentWorkspaceLabel(false, hydrated)).toBe('Presented Account linking architecture')
  })

  it('hydrates from the workspace list when the overview omits it', () => {
    const card = parsePresentWorkspace('Opening workspace:W1.')
    const hydrated = hydratePresentBrief(
      card,
      [],
      [{ id: 'W1', short: 'Auth rewrite', branch: 'feat/auth', projectId: 'storefront' }],
      [{ id: 'storefront', name: 'storefront' }],
    )
    expect(hydrated.brief?.title).toBe('Auth rewrite')
    expect(hydrated.brief?.projectName).toBe('storefront')
    expect(hydrated.brief?.branch).toBe('feat/auth')
  })
})

describe('list and context summaries', () => {
  it('collapses a list to a titled line', () => {
    const listed = parseListWorkspaceBriefs(JSON.stringify({
      counts: { needs: 1, working: 0, pickup: 2 },
      workspaces: [brief, { ...brief, workspaceId: 'w2', title: 'Fix build', bucket: 'pickup' }],
    }))
    expect(listWorkspaceBriefsSummary(listed!)).toBe('1 need you · 2 pick up')
    expect(listWorkspaceBriefsSummary({ counts: { needs: 0, working: 0, pickup: 0 }, workspaces: [] })).toBe('Nothing in flight.')
    expect(listWorkspaceBriefsSummary({
      counts: { needs: 1, working: 0, pickup: 0 },
      workspaces: [brief as never],
    })).toBe('Account linking architecture — Plan is ready but not approved')
  })

  it('collapses context to Now copy', () => {
    const ctx = parseWorkspaceContext(JSON.stringify({ brief }))
    expect(workspaceContextSummary(ctx!)).toBe('Plan is ready but not approved')
  })
})
