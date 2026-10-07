import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

// Loop chat cockpit: specialist sessions render as cards interleaved with
// Maestro turns and engine facts (outcomes, deviations) by timestamp. Data is
// UI-ready ChatItems from WorkspaceSessions.

const sessions = [
  {
    id: 's-rv', code: 'RV', name: 'Reviewer', kind: 'reviewer', status: 'idle',
    createdAt: 1_000, updatedAt: 2_000,
    items: [
      { role: 'assistant', kind: 'text', text: 'Reviewing the diff against the base branch.' },
      {
        role: 'assistant', kind: 'tool', tool: 'Bash',
        toolInput: 'python3 -m py_compile tetris.py',
        description: 'Compile check',
        input: '{"command":"python3 -m py_compile tetris.py && echo OK"}',
        result: 'OK',
      },
      { role: 'assistant', kind: 'text', text: 'The smoke test including a piece with rotation is green.' },
    ],
  },
  {
    id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
    createdAt: 3_000, updatedAt: 4_000,
    items: [{ role: 'assistant', kind: 'text', text: 'Implemented the rotation fix.' }],
  },
]

const facts = [
  { id: 'f1', agentCode: 'MA', name: 'Maestro', text: 'Converged: 0 findings.', time: 'now', ts: 2_500, isDeviation: false },
]

async function openLoopCockpit(page: import('@playwright/test').Page) {
  await gotoApp(page)
  const working = seed.loops.find((l: any) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  const loopTab = page.locator('#v-detail .wsv-chat-tab', { hasText: 'Loop' })
  await expect(loopTab).toBeVisible()
  await loopTab.click()
}

function rowNamed(scope: import('@playwright/test').Locator, name: string) {
  return scope.getByRole('button', { name: new RegExp(`^${name}\\b`) })
}

test.describe('loop chat cockpit', () => {
  test('session cards render chronologically and expand to the full transcript', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => sessions,
        WorkspaceFacts: () => facts,
      },
    })
    await openLoopCockpit(page)

    // Chronological: Reviewer, then Coder + live QA packed in one fanout card.
    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    const names = page.locator('#v-detail .chat > .chat-main .sesscard .sess-name')
    await expect(names).toHaveText(['Reviewer', 'Coder', 'QA'])
    await expect(cards).toHaveCount(2)
    await expect(cards.nth(0).locator('.sess-name')).toHaveText('Reviewer')
    await expect(cards.nth(1).locator('.sess-name')).toHaveText(['Coder', 'QA'])
    await expect(cards.nth(1).locator('.sess-live')).toHaveText('working')

    // Collapsed: fanout-style row with the final message as meta.
    await expect(cards.nth(0).locator('.sess-meta')).toContainText('rotation is green')
    await rowNamed(cards.nth(0), 'Reviewer').first().click()
    await expect(cards.nth(0).locator('.sess-body')).toContainText('Reviewing the diff')
    await expect(cards.nth(0).locator('.sess-body')).toHaveCSS('max-height', '420px')

    // The tool call renders as a card; expanding shows the full command + result.
    const toolRow = cards.nth(0).locator('.tc', { hasText: 'Compile check' })
    await toolRow.locator('.tc-head').click()
    await expect(toolRow.locator('.tc-pre')).toContainText('python3 -m py_compile tetris.py')
    await expect(toolRow.locator('.tc-pre')).toContainText('OK')
  })

  test('sibling specialists share one fanout card', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceSessions: () => [
          {
            id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
            createdAt: 1_000, updatedAt: 2_000,
            items: [{ role: 'assistant', kind: 'text', text: 'Implemented the rotation fix.' }],
          },
          {
            id: 's-rv', code: 'RV', name: 'Reviewer', kind: 'reviewer', status: 'idle',
            createdAt: 3_000, updatedAt: 4_000,
            items: [{ role: 'assistant', kind: 'text', text: 'No new findings.' }],
          },
        ],
        WorkspaceFacts: () => [
          { id: 'f1', agentCode: 'MA', name: 'Maestro', text: 'Converged: 0 findings.', time: 'now', ts: 5_000, isDeviation: false },
        ],
      },
    })
    await openLoopCockpit(page)

    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    await expect(cards).toHaveCount(1)
    await expect(cards.first().locator('.sess-name')).toHaveText(['Coder', 'Reviewer'])
    await expect(cards.first().locator('.sess-head')).toHaveCount(2)
  })

  test('commit and handoff land in the specialist timeline; collapsed shows the last line', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => [{
          id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
          createdAt: 1_000, updatedAt: 4_000,
          items: [
            { role: 'assistant', kind: 'text', text: 'Implemented the rotation fix.', ts: 1_100 },
          ],
        }],
        WorkspaceFacts: () => [
          { id: 'f-c', agentCode: 'CO', name: 'Coder', text: 'Committed <code>abc</code> (+1 −0) to <code>main</code>.', time: 'now', ts: 2_000, isDeviation: false },
          { id: 'f-h', agentCode: 'CO', name: 'Coder', text: 'Handoff: raised the breakpoint.', time: 'now', ts: 2_100, isDeviation: false },
        ],
      },
    })
    await openLoopCockpit(page)

    const card = page.locator('#v-detail .chat > .chat-main .sesscard').filter({ has: page.locator('.sess-name', { hasText: 'Coder' }) }).first()
    const coder = rowNamed(card, 'Coder').first()
    await expect(coder.locator('.sess-meta')).toHaveCount(1)
    await expect(coder.locator('.sess-meta')).toContainText('Handoff: raised the breakpoint.')
    await expect(coder.locator('.sess-meta')).not.toContainText('Committed')
    const nameBox = await coder.locator('.sess-name').boundingBox()
    const metaBox = await coder.locator('.sess-meta').boundingBox()
    expect(nameBox && metaBox && Math.abs(nameBox.y - metaBox.y) < 4).toBeTruthy()
    await coder.click()
    const body = card.locator('.sess-body').first()
    await expect(body).toContainText('Implemented the rotation fix.')
    await expect(body).toContainText('abc')
    await expect(body).toContainText('Handoff: raised the breakpoint.')
  })

  test('readLints tool card shows a clean summary instead of raw JSON', async ({ page }) => {
    const lintSessions = [
      {
        id: 's-co-lint', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
        createdAt: 1_000, updatedAt: 2_000,
        items: [
          {
            role: 'assistant', kind: 'tool', tool: 'readLints',
            toolInput: '{"paths":["/tmp/ws/frontend/src/a.ts","/tmp/ws/frontend/src/b.ts"]}',
            input: JSON.stringify({
              paths: [
                '/tmp/ws/frontend/src/lib/agentchat.ts',
                '/tmp/ws/frontend/src/components/AgentChat.svelte',
                '/tmp/ws/frontend/src/styles/app.css',
              ],
            }),
            result: JSON.stringify({
              fileDiagnostics: [
                { path: '/tmp/ws/frontend/src/lib/agentchat.ts', diagnostics: [], diagnosticsCount: 0 },
                { path: '/tmp/ws/frontend/src/components/AgentChat.svelte', diagnostics: [], diagnosticsCount: 0 },
                { path: '/tmp/ws/frontend/src/styles/app.css', diagnostics: [], diagnosticsCount: 0 },
              ],
              totalFiles: 3,
              totalDiagnostics: 0,
            }),
          },
        ],
      },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => lintSessions,
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)

    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    await rowNamed(cards.nth(0), 'Coder').first().click()
    const toolRow = cards.nth(0).locator('.tc', { hasText: 'Read lints' })
    await expect(toolRow.locator('.tc-meta')).toContainText('clean')
    await toolRow.locator('.tc-head').click()
    await expect(toolRow.locator('.tc-finding-empty')).toContainText('No linter issues')
    await expect(toolRow.locator('.tc-lint-paths .tc-lint-rel')).toHaveCount(3)
    await expect(toolRow.locator('.tc-pre')).toHaveCount(0)
  })

  test('Delete tool card names the file instead of dumping raw JSON args', async ({ page }) => {
    const deleteSessions = [
      {
        id: 's-co-del', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
        createdAt: 1_000, updatedAt: 2_000,
        items: [
          {
            role: 'assistant', kind: 'tool', tool: 'Delete',
            // Stale Cursor summary: truncated JSON used to leak into tc-meta.
            toolInput: '{"file_path":"/tmp/ws/frontend/src/views/Escalation.svelte"}',
            input: '{"file_path":"/tmp/ws/frontend/src/views/Escalation.svelte"}',
            result: '',
          },
        ],
      },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => deleteSessions,
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)

    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    await rowNamed(cards.nth(0), 'Coder').first().click()
    const toolRow = cards.nth(0).locator('.tc', { hasText: 'Deleted Escalation.svelte' })
    await expect(toolRow.locator('.tc-label')).toHaveText('Deleted Escalation.svelte')
    await expect(toolRow.locator('.tc-file')).toHaveText('Escalation.svelte')
    await expect(toolRow.locator('.tc-ico svg path')).toHaveAttribute('d', /M5 7h14/)
    await expect(toolRow.locator('.tc-meta')).toHaveCount(0)
    await expect(toolRow.locator('.tc-pre')).toHaveCount(0)
    await expect(toolRow.locator('.tc-head')).toHaveClass(/static/)
    await expect(toolRow).not.toContainText('{"file_path"')
  })

  test('WebFetch tool card shows a URL chip instead of dumping raw JSON args', async ({ page }) => {
    const url = 'https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token'
    const fetchSessions = [
      {
        id: 's-co-fetch', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
        createdAt: 1_000, updatedAt: 2_000,
        items: [
          {
            role: 'assistant', kind: 'tool', tool: 'WebFetch',
            // Stale summary: truncated JSON used to leak into tc-meta.
            toolInput: `{"url":"${url}","prompt":"List the request body fields. Is clientIp required?"}`,
            input: `{"url":"${url}","prompt":"List the request body fields. Is clientIp required?"}`,
            result: '[![CI](https://img.shields.io/badge.svg)](https://example.com)\n\n# Onramp Session Token Request Body Fields\n\n**addresses** (Required)\n\n![](https://tracker.example/pixel.png)',
          },
        ],
      },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => fetchSessions,
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)

    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    await rowNamed(cards.nth(0), 'Coder').first().click()
    const toolRow = cards.nth(0).locator('.tc', { hasText: 'Fetched a page' })
    await expect(toolRow.locator('.tc-label')).toHaveText('Fetched a page')
    const chip = toolRow.locator('a.tc-url')
    await expect(chip).toHaveText('docs.cdp.coinbase.com/…/create-session-token')
    await expect(chip).toHaveAttribute('href', url)
    await expect(toolRow.locator('.tc-meta')).toHaveCount(0)
    await expect(toolRow).not.toContainText('{"url"')
    await expect(toolRow.locator('.tc-pre')).toHaveCount(0)

    await toolRow.locator('.tc-label').click()
    await expect(toolRow.locator('.tc-head')).toHaveAttribute('aria-expanded', 'true')
    await expect(toolRow.locator('.tc-web-prompt')).toContainText('Is clientIp required?')
    await expect(toolRow.locator('.tc-web-body h1')).toHaveText('Onramp Session Token Request Body Fields')
    await expect(toolRow.locator('.tc-web-body strong')).toHaveText('addresses')
    await expect(toolRow.locator('.tc-web-body img')).toHaveCount(0)
    await expect(toolRow.locator('.tc-web-body a', { hasText: 'CI' })).toHaveAttribute('href', 'https://example.com')
    await expect(toolRow.locator('.tc-web-body a', { hasText: 'image' })).toHaveAttribute('href', 'https://tracker.example/pixel.png')
    await expect(toolRow).not.toContainText('{"url"')
    await expect(toolRow.locator('.tc-pre')).toHaveCount(0)
  })

  test('failed WebSearch with an empty result still shows a failed placeholder', async ({ page }) => {
    const searchSessions = [
      {
        id: 's-co-search', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
        createdAt: 1_000, updatedAt: 2_000,
        items: [
          {
            role: 'assistant', kind: 'tool', tool: 'WebSearch',
            toolInput: 'onramp session token',
            input: '{"query":"onramp session token"}',
            result: '',
            resultError: true,
          },
        ],
      },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => searchSessions,
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)

    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    await rowNamed(cards.nth(0), 'Coder').first().click()
    const toolRow = cards.nth(0).locator('.tc', { hasText: 'Searched the web' })
    await expect(toolRow.locator('.tc-errbadge')).toHaveText('failed')
    await toolRow.locator('.tc-label').click()
    await expect(toolRow.locator('.tc-pre.tc-outerr')).toHaveText('failed')
    await expect(toolRow).not.toContainText('{"query"')
  })

  test('loop facts interleave between session cards by timestamp', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => sessions,
        WorkspaceFacts: () => facts,
      },
    })
    await openLoopCockpit(page)

    // Reviewer (ts 1000) → fact (ts 2500) → Coder (ts 3000).
    await expect(page.locator('#v-detail .chat > .chat-main .loop-fact')).toContainText('Converged: 0 findings.')
    await expect(page.locator('#v-detail .chat > .chat-main .loop-fact .tc-label')).toHaveText('Maestro')
    const factBox = await page.locator('#v-detail .chat > .chat-main .loop-fact').boundingBox()
    const firstCard = await page.locator('#v-detail .chat > .chat-main .sesscard').nth(0).boundingBox()
    const lastCard = await page.locator('#v-detail .chat > .chat-main .sesscard').nth(1).boundingBox()
    expect(factBox && firstCard && lastCard && firstCard.y < factBox.y && factBox.y < lastCard.y).toBeTruthy()
  })

  test('long loop-fact errors wrap instead of ellipsizing', async ({ page }) => {
    const text = 'Loop failed: Coder turn failed: cursor run ended without a result (bridge log: /Users/josh/Library/Application Support/Recur/logs/cursor-bridges/7d6963206f17cc84/cursor-bridge.log)'
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => sessions,
        WorkspaceFacts: () => [
          { id: 'f-err', agentCode: 'MA', name: 'Maestro', text, time: 'now', ts: 2_500, isDeviation: false },
        ],
      },
    })
    await openLoopCockpit(page)

    const meta = page.locator('#v-detail .chat > .chat-main .loop-fact .tc-meta')
    await expect(meta).toContainText('cursor-bridge.log')
    const clipped = await meta.evaluate((el) => {
      const s = getComputedStyle(el)
      return s.whiteSpace === 'nowrap' && s.textOverflow === 'ellipsis' && el.scrollWidth > el.clientWidth
    })
    expect(clipped).toBe(false)
  })

  test('PR-opened fact shows completed time and opens the PR', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => sessions,
        WorkspaceFacts: () => [
          {
            id: 'f-pr', agentCode: 'MA', name: 'Maestro', ts: 5_000, isDeviation: false,
            text: 'Completed in 12m 04s · <a class="loop-pr-link" data-pr="482">Local PR #482</a><div class="loop-wrap"><b>Done</b> Retry the payment token once</div>',
          },
        ],
      },
    })
    await openLoopCockpit(page)

    const fact = page.locator('#v-detail .chat > .chat-main .loop-fact.loop-pr')
    await expect(fact).toContainText('Completed in 12m 04s')
    await expect(fact).toContainText('Local PR #482')
    await expect(fact).toContainText('Retry the payment token once')
    await expect(fact).not.toContainText('ready for your review')
    await fact.locator('.turn-end-link', { hasText: 'Local PR #482' }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
  })

  test('one specialist session with two turns renders two cards at those times', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => [{
          id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
          createdAt: 1_000, updatedAt: 4_000,
          items: [
            { role: 'user', kind: 'text', text: 'fix rotation', ts: 1_000 },
            { role: 'assistant', kind: 'text', text: 'first pass', ts: 1_100 },
            { role: 'assistant', kind: 'turn', durationMs: 1, ts: 1_200 },
            { role: 'user', kind: 'text', text: 'fix again', ts: 3_000 },
            { role: 'assistant', kind: 'text', text: 'second pass', ts: 3_100 },
          ],
        }],
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)

    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    const coderRows = cards.locator('.sess-head', { has: page.locator('.sess-name', { hasText: 'Coder' }) })
    await expect(coderRows).toHaveCount(2)
    await expect(cards.filter({ has: page.locator('.sess-name', { hasText: 'Coder' }) })).toHaveCount(1)
    await expect(coderRows.nth(0).locator('.sess-name')).toHaveText('Coder')
    await expect(coderRows.nth(1).locator('.sess-name')).toHaveText('Coder')
    await expect(coderRows.nth(0).locator('.sess-meta')).toContainText('first pass')
    await expect(coderRows.nth(1).locator('.sess-meta')).toContainText('second pass')
    await expect(cards.locator('.sess-live')).toHaveText('working')
  })

  test('coder fix-turn prompt shows the short label, not the MCP dispatch wire', async ({ page }) => {
    const wire = [
      'You are the Coder agent. A reviewer found issues with the current diff. ',
      'Fix every finding below with minimal, correct edits. For each finding, call ',
      'mcp__recur__update_finding with its id: status resolved if you fixed it, or ',
      'invalid with a clear reason if it is wrong/out of scope. Then call mcp__recur__done.',
      '',
      'Original task: create a cli game of pong',
      '',
      'Reviewer findings to fix:',
      '1. id=F-test [medium] pong/game.py:135 — paddle centering mismatch',
    ].join('\n')
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: () => [{
          id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
          createdAt: 1_000, updatedAt: 2_000,
          items: [
            { role: 'user', kind: 'text', text: wire, ts: 1_000 },
            { role: 'assistant', kind: 'text', text: 'Centered the paddle on integer coords.', ts: 1_100 },
          ],
        }],
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)

    const card = page.locator('#v-detail .chat > .chat-main .sesscard').first()
    await rowNamed(card, 'Coder').first().click()
    const body = card.locator('.sess-body').first()
    await expect(body.locator('.chat-row.me')).toContainText('Fix open reviewer findings')
    await expect(body).not.toContainText('mcp__recur__update_finding')
    await expect(body).not.toContainText('You are the Coder agent')
  })

  test('late turn_end and stream after idle do not keep the running timer', async ({ page }) => {
    // Wails can deliver session.turn_end / leftover deltas after agent.updated
    // idle. Those must not re-light the elapsed clock — the sidebar is already idle.
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceSessions: () => [],
        WorkspaceFacts: () => [],
      },
    })
    await openLoopCockpit(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const cards = page.locator('#v-detail .chat > .chat-main .sesscard')
    await expect(cards).toHaveCount(0)

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-CO`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'CO', text: 'editing the rotation', seq: 1, replace: true },
      })
    }, { workspaceId: working.id })

    const card = cards.filter({ has: page.locator('.sess-name', { hasText: 'Coder' }) }).last()
    await expect(card.locator('.sess-live')).toHaveText('working')
    await expect(card.locator('.chat-turn-elapsed')).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-CO`, status: 'idle' },
      })
    }, { workspaceId: working.id })
    await expect(card.locator('.sess-live')).toHaveCount(0)
    await expect(page.locator('#v-detail .chat > .chat-main .chat-turn-elapsed')).toHaveCount(0)

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'CO', durationMs: 4200 },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'CO', text: 'late leftover chunk', seq: 2, replace: true },
      })
    }, { workspaceId: working.id })
    // turn_end splits the session; expand the settled slice so the footer is
    // in the DOM, then assert the timer stayed dark (events have landed).
    const ended = cards.filter({ has: page.locator('.sess-name', { hasText: 'Coder' }) }).first()
    await rowNamed(ended, 'Coder').first().click()
    await expect(ended).toContainText('completed in')
    await expect(ended.locator('.sess-live')).toHaveCount(0)
    await expect(page.locator('#v-detail .chat > .chat-main .chat-turn-elapsed')).toHaveCount(0)
  })
})

test.describe('plan panel', () => {
  test('shows the workspace checklist and agent todos', async ({ page }) => {
    const s = structuredClone(seed)
    const working = s.loops.find((l: any) => l.group === 'active')!
    const plan = {
      summary: 'Ship the Loop cockpit',
      body: 'Merge specialist cards so the cockpit is one surface. Keep agent todos distinct from plan steps.',
      source: 'human',
      approved: false,
      steps: [
        { id: 's1', title: 'Merge specialist cards into Loop chat', status: 'doing' },
        { id: 's2', title: 'Retarget Activity e2e', status: 'pending' },
      ],
    }
    working.plan = plan
    if (s.loopById[working.id]) s.loopById[working.id].plan = plan

    await installBridge(page, s, {
      resolvers: {
        WorkspaceSessions: () => [{
          id: 's-co', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
          createdAt: 1_000, updatedAt: 2_000,
          items: [{
            role: 'assistant', kind: 'tool', tool: 'TodoWrite',
            input: JSON.stringify({
              todos: [
                { content: 'Wire Plan panel', status: 'in_progress' },
                { content: 'Retarget e2e', status: 'pending' },
              ],
            }),
          }],
        }],
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-pane-toggle[aria-label^="Plan"]').first().click()

    const pane = page.locator('#v-detail .ovw.plan-pane')
    await expect(pane.locator('.plan-summary')).toContainText('Ship the Loop cockpit')
    await expect(pane.locator('.plan-md')).toContainText('Keep agent todos distinct')
    await page.locator('#v-detail .obs-side-head').getByRole('tab', { name: /Todos/ }).click()
    await expect(pane.locator('.plan-step-title').first()).toHaveValue('Merge specialist cards into Loop chat')
    await expect(pane.locator('.plan-todo-name')).toHaveText('Coder')
    await expect(pane.locator('.plan-todos .todo-text').first()).toContainText('Wire Plan panel')
  })
})

test.describe('closed chat recents', () => {
  test('reopens a closed pair tab from the recents menu', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-new').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)

    await dock.locator('.wsv-chat-tab').nth(2).locator('[aria-label="Close New chat"]').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(2)

    await dock.locator('[aria-label="Recent closed chats"]').click()
    const recent = dock.locator('.wsv-recents-row')
    await expect(recent).toContainText('New chat')
    await expect(recent).not.toContainText('PA2')
    await expect(recent.locator('.wsv-recents-ico')).toBeVisible()
    await expect(recent.locator('.wsv-recents-age')).toHaveText('just now')
    await recent.locator('.wsv-recents-item').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)
    await expect(dock.locator('[aria-label="Recent closed chats"]')).toHaveCount(0)
  })

  test('permanently deletes a closed pair tab from the recents menu', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-new').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)

    await dock.locator('.wsv-chat-tab').nth(2).locator('[aria-label="Close New chat"]').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(2)

    await dock.locator('[aria-label="Recent closed chats"]').click()
    const recent = dock.locator('.wsv-recents-row')
    await expect(recent).toContainText('New chat')
    await recent.hover()
    const del = recent.locator('[aria-label="Delete New chat permanently"]')
    await del.click()
    await expect(dock.locator('[aria-label="Recent closed chats"]')).toHaveCount(1)
    await recent.locator('[aria-label="Confirm delete New chat"]').click()
    await expect(dock.locator('[aria-label="Recent closed chats"]')).toHaveCount(0)
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(2)
  })

  test('closes the Loop tab like other chats', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const loop = dock.locator('.wsv-chat-tab', { hasText: 'Loop' })
    await expect(loop).toBeVisible()
    await expect(loop.locator('[aria-label="Close Loop"]')).toBeVisible()
    await loop.locator('[aria-label="Close Loop"]').click()
    await expect(loop).toHaveCount(0)
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'New chat' })).toBeVisible()

    await dock.locator('[aria-label="Recent closed chats"]').click()
    const recent = dock.locator('.wsv-recents-item')
    await expect(recent).toContainText('Loop')
    await expect(recent.locator('.wsv-recents-ico')).toBeVisible()
    await recent.click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'Loop' })).toBeVisible()
  })
})
