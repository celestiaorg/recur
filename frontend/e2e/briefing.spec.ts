import { test, expect, gotoApp, reloadApp, seed } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

test.describe('briefing', () => {
  test('boots into the unified list with Now/Next cards and Ask Recur', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('.rail .nav.inbox-nav')).toContainText('Inbox')
    await expect(page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'All' })).toHaveClass(/on/)
    await expect(page.locator('.titlebar .tb-sub')).toContainText('unread')
    await expect(page.locator('#briefing-needs')).toBeVisible()
    await expect(page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' })).toBeVisible()
    await expect(page.locator('.wbrief-row-now').first()).toBeVisible()
    await expect(page.locator('.wbrief-row-when').first()).toBeVisible()
    await expect(page.locator('.app-chat')).toBeVisible()
    await expect(page.locator('.app-chat .chat')).toHaveClass(/compact/)
    await expect(page.getByRole('button', { name: 'What needs me?' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Where did I leave off?' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Which workspace should I open next?' })).toBeVisible()
  })

  test('unread pill recalls its notification titles on hover', async ({ page }) => {
    await gotoApp(page)
    const pill = page.locator('.wbrief-row', { hasText: 'Add Apple & Google sign-in' }).locator('.wbrief-unread')
    await expect(pill).toHaveText('2')
    await expect(pill).toHaveAttribute('title', /#482 is ready for review/)
  })

  test('notifications with no card land in Elsewhere', async ({ page }) => {
    await gotoApp(page)
    const row = page.locator('.briefing-elsewhere .ibrow', { hasText: '#471' })
    await expect(row).toBeVisible()
    await expect(row).toHaveClass(/read/)
  })

  test('cards carry unread badges that clear on open', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' }).locator('.wbrief-unread')).toHaveText('1')
    await expect(page.locator('.wbrief-row', { hasText: 'Add Apple & Google sign-in' }).locator('.wbrief-unread')).toHaveText('2')
    await page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' }).locator('.wbrief-unread')).toHaveCount(0)
  })

  test('mark all read clears every badge', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.titlebar .tb-mark-read').click()
    await expect(page.locator('.wbrief-unread')).toHaveCount(0)
    await expect(page.locator('.titlebar .tb-mark-read')).toHaveCount(0)
  })

  test('bucket chips filter the overview', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('#briefing-working')).toBeVisible()
    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'Needs you' }).click()
    await expect(page.locator('#briefing-working')).toHaveCount(0)
    await expect(page.locator('#briefing-pickup')).toHaveCount(0)
    await expect(page.locator('#briefing-needs')).toBeVisible()
    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'All' }).click()
    await expect(page.locator('#briefing-working')).toBeVisible()
  })

  test('unread filter narrows to workspaces with unread notifications', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('.wbrief-row', { hasText: 'Migrate analytics to PG16' })).toBeVisible()
    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'Unread' }).click()
    await expect(page.locator('.wbrief-row', { hasText: 'Migrate analytics to PG16' })).toHaveCount(0)
    await expect(page.locator('.wbrief-row', { hasText: 'Add Apple & Google sign-in' })).toBeVisible()
    await expect(page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' })).toBeVisible()
  })

  test('project filter scopes the overview', async ({ page }) => {
    await gotoApp(page)
    const picker = page.locator('#v-briefing .sel-btn[aria-label="Filter by project"]')
    await expect(picker).toBeVisible()
    await expect(picker).toContainText('All projects')
    await picker.click()
    const menu = page.locator('body > .sel-menu')
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'payments' }).click()
    await expect(menu).toBeVisible()
    await expect(page.locator('body > .sel-menu .sel-opt', { hasText: 'payments' })).toHaveClass(/on/)
    await expect(page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' })).toBeVisible()
    await expect(page.locator('.wbrief-row', { hasText: 'Add Apple & Google sign-in' })).toHaveCount(0)
    await expect(page.locator('#briefing-working')).toHaveCount(0)
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'storefront' }).click()
    await expect(page.locator('body > .sel-menu .sel-opt.on')).toHaveCount(2)
    await expect(page.locator('.wbrief-row', { hasText: 'Add Apple & Google sign-in' })).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(picker).toContainText('payments')
    await expect(picker).toContainText('storefront')
  })

  test('ask dock toggles from the pane headers like the PR dock', async ({ page }) => {
    await gotoApp(page)
    const ask = page.locator('.briefing-ask')
    await expect(ask.locator('.app-chat')).toBeVisible()
    // Chat dock first in the split — left of the list, like every other chat.
    await expect(page.locator('#v-briefing .briefing-body > *').first()).toHaveClass(/briefing-ask/)
    await expect(ask.locator('.vsplit')).toBeVisible()
    // Hide from the dock's own tab bar; reopen from the filters header bar.
    await ask.locator('.app-chat-head .obs-pane-toggle').click()
    await expect(page.locator('.briefing-ask')).toHaveCount(0)
    await page.locator('#v-briefing .briefing-filters .obs-pane-toggle').click()
    await expect(ask.locator('.app-chat')).toBeVisible()
    await expect(page.locator('#v-briefing .briefing-filters .obs-pane-toggle')).toHaveCount(0)
  })

  test('Ask Recur selected tab lights when the chat has focus', async ({ page }) => {
    await gotoApp(page)
    const strip = page.locator('.briefing-ask .app-chat-tabs')
    const chat = page.locator('.briefing-ask .app-chat')
    const primary = strip.locator('.wsv-chat-tab').first()
    await expect(primary).toHaveClass(/on/)
    await expect(primary).not.toHaveClass(/lit/)

    await chat.locator('.chat-in .chat-composer').click()
    await expect(primary).toHaveClass(/lit/)

    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'All' }).click()
    await expect(primary).toHaveClass(/on/)
    await expect(primary).not.toHaveClass(/lit/)

    await strip.locator('.wsv-chat-new').click()
    const extra = strip.locator('.wsv-chat-tab', { hasText: 'New chat' })
    await expect(extra).toHaveClass(/on/)
    await expect(extra).toHaveClass(/lit/)
    await expect(primary).not.toHaveClass(/on/)
    await expect(primary).not.toHaveClass(/lit/)
  })

  test('Ask Recur tabs show running status like workspace chats', async ({ page }) => {
    await gotoApp(page)
    const strip = page.locator('.briefing-ask .app-chat-tabs')
    const chat = page.locator('.briefing-ask .app-chat')
    await chat.locator('.chat-in .chat-composer').fill('what needs me')
    await chat.locator('.chat-in .send:not(.stop)').click()
    await expect(chat.locator('.chat-row.me', { hasText: 'what needs me' })).toBeVisible()
    // Default mock SendAppMessage returns immediately, which used to drop busy.
    await expect(chat.locator('.chat-in .send.stop')).toHaveCount(0)

    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId: '', agentId: 'app-APP', status: 'working', surface: 'app', code: 'APP' },
      })
    })
    const primary = strip.locator('.wsv-chat-tab').first()
    await expect(chat.locator('.chat-in .send.stop')).toBeVisible()
    await expect(primary).toHaveAttribute('aria-busy', 'true')
    await expect(primary).toHaveAttribute('title', /running/)

    await strip.locator('.wsv-chat-new').click()
    await expect(strip.locator('.wsv-chat-tab')).toHaveCount(2)
    await expect(primary).toHaveClass(/busy/)
    await expect(primary).toHaveAttribute('aria-busy', 'true')
    const extra = strip.locator('.wsv-chat-tab', { hasText: 'New chat' })
    await expect(extra).toHaveClass(/on/)
    await expect(extra).not.toHaveAttribute('aria-busy')
    await expect(chat.locator('.chat-in .send.stop')).toHaveCount(0)

    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId: '', agentId: 'app-APP', status: 'idle', surface: 'app', code: 'APP' },
      })
    })
    await expect(primary).not.toHaveClass(/busy/)
    await expect(primary).not.toHaveAttribute('aria-busy')
  })

  test('ask dock supports multiple chat tabs like the PR dock', async ({ page }) => {
    await gotoApp(page)
    const strip = page.locator('.briefing-ask .app-chat-tabs')
    await expect(strip.locator('.wsv-chat-tab')).toHaveCount(1)
    await expect(strip.locator('.wsv-chat-tab').first()).toContainText('Ask Recur')
    await strip.locator('.wsv-chat-new').click()
    await expect(strip.locator('.wsv-chat-tab')).toHaveCount(2)
    const extra = strip.locator('.wsv-chat-tab', { hasText: 'New chat' })
    await expect(extra).toHaveClass(/on/)
    await extra.hover()
    await extra.locator('.tab-chip-x').click()
    await expect(strip.locator('.wsv-chat-tab')).toHaveCount(1)
    await expect(strip.locator('.wsv-chat-tab').first()).toHaveClass(/on/)
    // The primary closes too — it resets to a fresh Ask Recur chat.
    const primary = strip.locator('.wsv-chat-tab').first()
    await primary.hover()
    await primary.locator('.tab-chip-x').click()
    await expect(strip.locator('.wsv-chat-tab')).toHaveCount(1)
    await expect(strip.locator('.wsv-chat-tab').first()).toContainText('Ask Recur')
  })

  test('unread and project filters survive a reload; bucket does not', async ({ page }) => {
    await gotoApp(page)
    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'Unread' }).click()
    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'Needs you' }).click()
    await reloadApp(page)
    await expect(page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'Unread' })).toHaveClass(/on/)
    await expect(page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'All' })).toHaveClass(/on/)
    await expect(page.locator('.wbrief-row', { hasText: 'Migrate analytics to PG16' })).toHaveCount(0)
  })

  test('restores Briefing from shell state', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await page.locator('.nav.inbox-nav').click()
    await reloadApp(page)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('#briefing-needs')).toBeVisible()
  })

  test('footer Needs you opens the list focused, clearing bucket filters', async ({ page }) => {
    await gotoApp(page)
    await page.locator('#v-briefing .briefing-filters .seg button', { hasText: 'Working' }).click()
    await expect(page.locator('#briefing-needs')).toHaveCount(0)
    await page.locator('.rail-stat.needs').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('#briefing-needs')).toBeVisible()
  })

  test('go-to palette reaches Briefing', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await page.keyboard.press('Meta+k')
    const pal = page.locator('.palette:not(.shortcuts-panel):not(.cmd-palette)')
    await expect(pal).toBeVisible()
    await pal.locator('.pal-in input').fill('Inbox')
    await pal.locator('.pal-row', { hasText: 'Inbox' }).first().click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('#briefing-needs')).toBeVisible()
  })

  test('opening a brief row lands on the workspace', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.wbrief-row', { hasText: 'Rate-limit the public API' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
  })

  test('Ask Recur briefing tools skip JSON; only present_workspace is a card', async ({ page }) => {
    const brief = {
      workspaceId: 'LOOP-2218',
      projectName: 'payments',
      title: 'Rate-limit the public API',
      branch: 'recur/rate-limit',
      bucket: 'needs',
      reasons: ['escalation', 'plan_approval'],
      now: 'Escalated: ambiguous burst-limit spec',
      next: 'Answer the question',
      primary: { label: 'Answer', target: 'workspace:LOOP-2218' },
    }
    const transcript = [
      { role: 'user', kind: 'text', text: 'What needs me?' },
      {
        role: 'assistant', kind: 'tool',
        tool: 'mcp__recur__list_workspace_briefs',
        input: '{}',
        result: JSON.stringify({
          counts: { needs: 1, working: 0, pickup: 0 },
          workspaces: [brief],
        }),
      },
      {
        role: 'assistant', kind: 'tool',
        tool: 'mcp__recur__get_workspace_context',
        input: '{"workspace_id":"LOOP-2218"}',
        result: JSON.stringify({
          brief,
          planSteps: [{ id: 'S1', title: 'Specify burst vs sustained', status: 'pending' }],
        }),
      },
      {
        role: 'assistant', kind: 'tool',
        tool: 'mcp__recur__present_workspace',
        input: '{"target":"workspace:LOOP-2218"}',
        result: JSON.stringify({
          ok: true, target: 'workspace:LOOP-2218', kind: 'workspace', brief,
        }),
      },
      { role: 'assistant', kind: 'text', text: 'Rate-limit is waiting on you.' },
      { role: 'assistant', kind: 'turn', durationMs: 1400 },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: { AppChatItems: () => transcript },
    })
    await gotoApp(page)

    const chat = page.locator('.app-chat')
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    const group = chat.locator('.steps-head')
    if (await group.count()) await group.click()
    await expect(chat.getByText('Listed 1 workspace')).toBeVisible()
    await expect(chat.getByText('Looked up Rate-limit the public API')).toBeVisible()
    await expect(chat.getByText('Presented Rate-limit the public API')).toBeVisible()
    await expect(chat.locator('.tc-pre', { hasText: 'workspaceId' })).toHaveCount(0)
    await expect(chat.getByText('Rate-limit the public API — Escalated: ambiguous burst-limit spec')).toBeVisible()
    await expect(chat.locator('.wbrief-row')).toHaveCount(0)
    await expect(chat.locator('.turn-end-brief .wbrief', { hasText: 'Rate-limit the public API' })).toBeVisible()
    await expect(chat.locator('.turn-end-link', { hasText: 'Open Rate-limit the public API' })).toBeVisible()

    await chat.locator('.turn-end-link', { hasText: 'Open Rate-limit the public API' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
  })
})
