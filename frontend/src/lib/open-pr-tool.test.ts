import { describe, expect, it } from 'vitest'
import { isOpenPRTool, openPRLabel, parseOpenPRCard } from './open-pr-tool'

describe('isOpenPRTool', () => {
  it('matches Recur MCP and bare names', () => {
    expect(isOpenPRTool('mcp__recur__open_pr')).toBe(true)
    expect(isOpenPRTool('open_pr')).toBe(true)
    expect(isOpenPRTool('mcp__recur__rename_workspace')).toBe(false)
    expect(isOpenPRTool(undefined)).toBe(false)
  })
})

describe('parseOpenPRCard', () => {
  const args = {
    title: 'Align screening allow cache TTL to 24 hours',
    body: '## Summary\nSet ALLOW_TTL to 86400.',
    draft: true,
  }

  it('reads title and draft from args, number from the result', () => {
    const card = parseOpenPRCard(args, 'Opened GitHub draft PR #112 (promoted from Local).')
    expect(card.title).toBe('Align screening allow cache TTL to 24 hours')
    expect(card.draft).toBe(true)
    expect(card.number).toBe(112)
    expect(card.source).toBe('gh')
    expect(card.promoted).toBe(true)
    expect(card.failed).toBe(false)
    expect(openPRLabel(card)).toBe('Opened GitHub draft PR #112')
  })

  it('does not treat the markdown body as the label', () => {
    const card = parseOpenPRCard(args, 'Opened GitHub draft PR #112 (promoted from Local).')
    expect(openPRLabel(card)).not.toMatch(/Summary|ALLOW_TTL|\\n/)
    expect(openPRLabel(card)).not.toContain('{')
  })

  it('labels a Local fallback and a later push', () => {
    expect(openPRLabel(parseOpenPRCard({ title: 'Fix' }, 'Opened Local PR #4.'))).toBe('Opened Local PR #4')
    expect(openPRLabel(parseOpenPRCard({ title: 'Fix' }, 'Pushed new commits to GitHub PR #91 (description unchanged).'))).toBe('Pushed to PR #91')
  })

  it('does not treat a Local fallback as a GitHub PR when the result mentions GitHub', () => {
    const card = parseOpenPRCard(
      { title: 'Add client-side acme.dev API' },
      'Opened Local PR #164 (GitHub create failed: gh pr create failed: HTTP 401 — retry Open PR).',
    )
    expect(card.source).toBe('local')
    expect(card.number).toBe(164)
    expect(openPRLabel(card)).toBe('Opened Local PR #164')
    const already = parseOpenPRCard(
      { title: 'Retry' },
      'PR #164 is already open (Local). GitHub create failed (git push failed) — retry Open PR.',
    )
    expect(already.source).toBe('local')
    expect(openPRLabel(already)).toBe('PR #164 already open')
  })

  it('labels pending and engine errors without dumping JSON', () => {
    expect(openPRLabel(parseOpenPRCard(args, '', { pending: true }))).toBe(
      'Opening Align screening allow cache TTL to 24 hours',
    )
    const fail = parseOpenPRCard(args, 'open_pr: the main working copy can\'t open a PR', { error: false })
    expect(fail.failed).toBe(true)
    expect(openPRLabel(fail)).toBe('Open PR failed')
  })
})
