import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge, BridgeError } from './helpers/mock-bridge'
import type { Locator } from '@playwright/test'

async function expectPickersOnOneRow(host: Locator) {
  const modelBtn = host.locator('.sel-btn[aria-label="Model"]')
  const flagsBtn = host.locator('.sel-btn[aria-label="Model flags"]')
  await expect(modelBtn).toBeVisible()
  await expect(flagsBtn).toBeVisible()
  const model = await modelBtn.boundingBox()
  const flags = await flagsBtn.boundingBox()
  expect(model).toBeTruthy()
  expect(flags).toBeTruthy()
  expect(Math.abs(model!.y - flags!.y), 'flags must sit on the same row as the model').toBeLessThan(4)
}

async function expectLabelVisible(btn: Locator) {
  const clipped = await btn.locator('.sel-val').evaluate((el) => el.scrollWidth > el.clientWidth + 1)
  expect(clipped, 'label should not ellipsis').toBe(false)
}

async function openSettings(page: import('@playwright/test').Page) {
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await expect(page.locator('#v-settings')).toHaveClass(/on/)
}

async function openSection(page: import('@playwright/test').Page, label: string) {
  await page.locator('#v-settings .set-nav-item', { hasText: label }).click()
  await expect(page.locator('#v-settings .set-head h2')).toHaveText(label)
}

test.describe('new loop + settings', () => {
  test('new-loop modal has repository and base-branch pickers', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    await expect(modal.getByRole('dialog', { name: 'New workspace' })).toBeVisible()
    // Autonomy applies to the autopilot — pick Loop in the footer mode select.
    await modal.locator('.sel-btn[aria-label="Mode"]').click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Loop' }).click()
    const repoBtn = modal.locator('.sel-btn[aria-label="Repository"]')
    await expect(repoBtn).toBeVisible()
    await repoBtn.click()
    await expect(page.locator('body > .sel-menu .sel-opt')).toHaveCount(seed.projects.length)
    await page.keyboard.press('Escape')
    const branchBtn = modal.locator('.sel-btn[aria-label="Base branch"]')
    await expect(branchBtn).toBeVisible()
    await branchBtn.click()
    const branchSearch = page.locator('body > .sel-menu .sel-search input')
    await expect(branchSearch).toBeVisible()
    await branchSearch.fill('auth')
    await expect(page.locator('body > .sel-menu .sel-opt')).toHaveCount(1)
    await expect(page.locator('body > .sel-menu .sel-opt')).toContainText('feature/auth')
    await page.locator('body > .sel-menu .sel-opt').click()
    await expect(branchBtn).toContainText('feature/auth')
    const loopBtn = modal.locator('.sel-btn[aria-label="Loop"]')
    await expect(loopBtn).toBeVisible()
    const policyBtn = modal.locator('.sel-btn[aria-label="Policy"]')
    await expect(policyBtn).toBeVisible()
    await policyBtn.click()
    await expect(page.locator('body > .sel-menu .sel-opt')).toHaveCount(3)
    await page.keyboard.press('Escape')
    const go = modal.locator('.nt-go')
    await expect(go).toBeVisible()
    await expect(modal.locator('.wsv-simplify')).toBeVisible()
    await expect(modal.locator('.wsv-simplify')).toHaveAttribute('aria-pressed', 'true')
    // Hint must match onTaskKey: ⌘/Ctrl+Enter (plain Enter inserts a newline).
    await expect(go.locator('.nt-ret')).toHaveText('⌘↵')
    await page.keyboard.press('Escape')
    await expect(modal).toHaveCount(0)
  })

  test('Escape abandons eager create so a second ⌘N is one row', async ({ page }) => {
    let n = 0
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        CreateWorkspace: (a) => {
          n++
          const id = `WS-EAGER-${n}`
          return {
            id,
            projectId: a[0],
            type: 'solo',
            statusLabel: 'Creating',
            phase: 'creating',
            goal: 'New Workspace',
            short: 'New Workspace',
            branch: `recur/${id}`,
            baseBranch: a[2] || 'main',
            archived: false,
            isMain: false,
          }
        },
      },
    })
    await gotoApp(page)
    const rows = page.locator('.rail .tloop:not(.tmain)')
    const before = await rows.count()
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    await expect(rows).toHaveCount(before + 1)
    await page.keyboard.press('Escape')
    await expect(modal).toHaveCount(0)
    await expect(rows).toHaveCount(before)
    await page.keyboard.press('Meta+N')
    await expect(modal).toBeVisible()
    await expect(rows).toHaveCount(before + 1)
    await page.keyboard.press('Escape')
    await expect(modal).toHaveCount(0)
    await expect(rows).toHaveCount(before)
  })

  test('create-workspace model menu stays inside the window', async ({ page }) => {
    const effort = {
      id: 'effort',
      label: 'Effort',
      options: [
        { value: 'low', label: 'Low' },
        { value: 'high', label: 'High', default: true },
      ],
    }
    const models = Array.from({ length: 24 }, (_, i) => ({
      ref: `claude-code/model-${i}`,
      label: `Model ${String(i + 1).padStart(2, '0')}`,
      flags: [effort],
      contextWindow: 200_000,
    }))
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListModelCatalog: () => [{ provider: 'claude-code', label: 'Claude Code', models }],
      },
    })
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const modelBtn = modal.locator('.sel-btn[aria-label="Model"]')
    await expect(modelBtn).toBeVisible()
    await modelBtn.click()
    const menu = page.locator('body > .sel-menu')
    await expect(menu).toBeVisible()
    await expect.poll(() => menu.locator('.sel-opt').count()).toBeGreaterThanOrEqual(24)

    const vp = page.viewportSize()
    expect(vp).toBeTruthy()
    const box = await menu.boundingBox()
    expect(box).toBeTruthy()
    expect(box!.y, 'menu must not clip past the top of the window').toBeGreaterThanOrEqual(0)
    expect(box!.y + box!.height, 'menu must not clip past the bottom of the window')
      .toBeLessThanOrEqual(vp!.height)

    const list = menu.locator('.sel-list')
    await expect.poll(() => list.evaluate((el) => el.scrollHeight > el.clientHeight + 1)).toBe(true)

    await list.evaluate((el) => { el.scrollTop = 0 })
    const firstBox = await menu.locator('.sel-opt').first().boundingBox()
    expect(firstBox).toBeTruthy()
    expect(firstBox!.y).toBeGreaterThanOrEqual(box!.y - 1)
    expect(firstBox!.y).toBeLessThan(box!.y + box!.height)

    await list.evaluate((el) => { el.scrollTop = el.scrollHeight })
    const lastBox = await menu.locator('.sel-opt').last().boundingBox()
    expect(lastBox).toBeTruthy()
    expect(lastBox!.y + lastBox!.height).toBeLessThanOrEqual(box!.y + box!.height + 1)
  })

  test('model and flags stay on one row in create-workspace and chat composer', async ({ page }) => {
    const flags = [
      {
        id: 'effort',
        label: 'Effort',
        options: [
          { value: 'low', label: 'Low' },
          { value: 'medium', label: 'Medium', default: true },
          { value: 'high', label: 'High' },
        ],
      },
      {
        id: 'context',
        label: 'Context',
        options: [
          { value: '272k', label: '272K', default: true },
          { value: '1m', label: '1M' },
        ],
      },
      {
        id: 'thinking',
        label: 'On',
        options: [
          { value: 'true', label: 'Yes', default: true },
          { value: 'false', label: 'No' },
        ],
      },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListModelCatalog: () => [{
          provider: 'openai',
          label: 'OpenAI',
          models: [{
            ref: 'openai/gpt-6-astra',
            label: 'GPT-6 Astra',
            flags,
            contextWindow: 272_000,
          }],
        }],
      },
    })
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    await expectPickersOnOneRow(modal.locator('.nt-foot'))
    await expect(modal.locator('.wsv-simplify-label')).toBeHidden()
    await expectLabelVisible(modal.locator('.sel-btn[aria-label="Model"]'))
    await expectLabelVisible(modal.locator('.sel-btn[aria-label="Model flags"]'))

    await page.keyboard.press('Escape')
    await expect(modal).toHaveCount(0)
    const working = seed.loops.find((l: { group?: string; goal?: string }) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .wsv-chat-new').click()
    await expectPickersOnOneRow(page.locator('#v-detail .obs-left .chat-in-tools'))
  })

  test('creating a workspace keeps chat visible while the editor is blocked', async ({ page }) => {
    await gotoApp(page)
    await page.locator('button[aria-label="Get started"]').click()
    await expect(page.locator('#v-welcome')).toHaveClass(/on/)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    await modal.locator('.nt-in').fill('review the auth flow')
    const go = modal.locator('.nt-go')
    await expect(go).toBeEnabled()
    await go.click()
    await expect(modal).toHaveCount(0)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    const chat = page.locator('#v-detail .obs-left .chat')
    await expect(chat).toBeVisible()
    await expect(page.locator('#v-detail .chat-row.me', { hasText: 'review the auth flow' })).toBeVisible()
    await expect(page.locator('#v-detail .chat-turn')).toContainText('Setting up the worktree')
    await expect(page.locator('.wsv-creating')).toContainText('Creating workspace')
    // Overlay covers the editor/files, not the chat dock — and must not steal
    // those grid cells (in-flow placement wraps main+files onto a second row).
    await expect(chat).toBeVisible()
    const left = page.locator('#v-detail .obs-left')
    const overlay = page.locator('#v-detail .obs-work-overlay')
    const files = page.locator('#v-detail .obs-side')
    const body = page.locator('#v-detail .obs-body')
    await expect(page.locator('#v-detail .obs-work')).toBeVisible()
    await expect(overlay).toBeVisible()
    const leftBox = await left.boundingBox()
    const overlayBox = await overlay.boundingBox()
    const filesBox = await files.boundingBox()
    const bodyBox = await body.boundingBox()
    const composerBox = await page.locator('#v-detail .obs-left .chat-in').boundingBox()
    expect(leftBox && overlayBox && filesBox && bodyBox && composerBox).toBeTruthy()
    expect(Math.abs(leftBox!.height - bodyBox!.height), 'chat dock fills the body height').toBeLessThan(8)
    expect(overlayBox!.x, 'overlay sits to the right of chat').toBeGreaterThanOrEqual(leftBox!.x + leftBox!.width - 2)
    expect(Math.abs(overlayBox!.y - leftBox!.y), 'overlay shares the chat vertical band').toBeLessThan(8)
    expect(Math.abs(overlayBox!.height - leftBox!.height), 'overlay matches chat height').toBeLessThan(8)
    expect(overlayBox!.x, 'overlay covers files horizontally').toBeLessThanOrEqual(filesBox!.x + 2)
    expect(overlayBox!.y, 'overlay covers files vertically').toBeLessThanOrEqual(filesBox!.y + 2)
    expect(overlayBox!.x + overlayBox!.width).toBeGreaterThanOrEqual(filesBox!.x + filesBox!.width - 2)
    expect(overlayBox!.y + overlayBox!.height).toBeGreaterThanOrEqual(filesBox!.y + filesBox!.height - 2)
    expect(composerBox!.y + composerBox!.height, 'composer stays at the bottom of the dock')
      .toBeGreaterThan(leftBox!.y + leftBox!.height * 0.5)
    await expect(page.locator('#v-detail .obs-editor-toggle')).toBeDisabled()
    await expect(page.locator('#v-detail .obs-term-toggle')).toBeDisabled()
    await expect(page.locator('#v-detail .obs-tb-right .obs-pane-toggle').first()).toBeDisabled()
    await expect(page.locator('#v-detail .obs-tb-left .obs-pane-toggle').first()).toBeEnabled()
    await expect(page.locator('#v-detail .obs-side .vsplit')).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-left .vsplit')).toBeVisible()
    await expect(page.locator('#v-detail .obs-main')).toHaveAttribute('inert')
    await expect(page.locator('#v-detail .obs-side')).toHaveAttribute('inert')
    await expect(page.locator('.titlebar .tb-right button', { hasText: 'Commit' })).toHaveCount(0)
    await expect(page.locator('.titlebar .tb-right button', { hasText: 'Remove' })).toBeVisible()
    await expect(page.locator('.titlebar .tb-open-in')).toHaveCount(0)
    await expect(page.locator('.titlebar .tb-copy-path')).toHaveCount(0)
    await page.locator('.rail-foot button[aria-label="Settings"]').click()
    await expect(page.locator('#v-settings')).toHaveClass(/on/)
  })

  test('inbox create shows the opening prompt after opening the rail row', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    await modal.locator('.nt-in').fill('review the auth flow')
    await modal.locator('.nt-go').click()
    await expect(modal).toHaveCount(0)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await page.locator('.rail .tloop[data-workspace-id="WS-NEW"]').click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .chat-row.me', { hasText: 'review the auth flow' })).toBeVisible()
    await expect(page.locator('#v-detail .chat-row.me.queued', { hasText: 'review the auth flow' })).toBeVisible()
  })

  test('settings exposes editable models and the fixed rail set', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)

    await expect(page.locator('#v-settings .set-head h2')).toHaveText('General')
    await openSection(page, 'Models')
    await expect(page.locator('#v-settings .set-head h2')).toHaveText('Models')
    await expect(page.locator('#v-settings .sel-btn[aria-label="Ask model"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Plan model"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Agent / Coder model"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Review model"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Loop model"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Review flags"]')).toBeVisible()

    await openSection(page, 'Loops')
    await expect(page.locator('#v-settings .sel-btn[aria-label="Loop"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Policy"]')).toBeVisible()
    await expect(page.locator('#v-settings').getByText('Add a rail')).toHaveCount(0)
  })

  test('project Skills links to Settings → Skills', async ({ page }) => {
    await gotoApp(page)
    const proj = seed.projects.find((p) => p.open) ?? seed.projects[0]
    const projRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: proj.name }) })
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.pset').click()
    await expect(page.locator('#v-project')).toHaveClass(/on/)
    await page.locator('#v-project .set-nav-item', { hasText: 'Skills' }).click()
    await page.locator('#v-project button.lnk', { hasText: 'Settings → Skills' }).click()
    await expect(page.locator('#v-settings')).toHaveClass(/on/)
    await expect(page.locator('#v-settings .set-head h2')).toHaveText('Skills')
  })

  test('project MCP links to Settings → MCP', async ({ page }) => {
    await gotoApp(page)
    const proj = seed.projects.find((p) => p.open) ?? seed.projects[0]
    const projRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: proj.name }) })
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.pset').click()
    await expect(page.locator('#v-project')).toHaveClass(/on/)
    await page.locator('#v-project .set-nav-item', { hasText: 'MCP' }).click()
    await page.locator('#v-project button.lnk', { hasText: 'Settings → MCP' }).click()
    await expect(page.locator('#v-settings')).toHaveClass(/on/)
    await expect(page.locator('#v-settings .set-head h2')).toHaveText('MCP')
  })

  test('project settings files to copy previews matching paths', async ({ page }) => {
    await gotoApp(page)
    const proj = seed.projects.find((p) => p.open) ?? seed.projects[0]
    const projRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: proj.name }) })
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.pset').click()
    await expect(page.locator('#v-project')).toHaveClass(/on/)
    await page.locator('#v-project .set-nav-item', { hasText: 'Run' }).click()
    await expect(page.locator('#v-project .set-head h2')).toHaveText('Run')
    const ta = page.getByLabel('Files to copy')
    await expect(ta).toBeVisible()
    await expect(ta).toHaveAttribute('placeholder', /# \.env\*/)
    await expect(page.locator('#v-project .copy-files-meta')).toContainText('Nothing will be copied')
    await ta.fill('.env*')
    await expect(page.locator('#v-project .copy-files-meta')).toContainText('3 files will be copied from /Users/josh/code/storefront')
    await expect(page.locator('#v-project .copy-files-list')).toContainText('apps/web/.env')
    await expect(page.locator('#v-project .copy-files-list')).toContainText('deploy/.env.mainnet')
    await expect(page.locator('.toast.success', { hasText: 'Project saved' })).toBeVisible()
    await expect(page.locator('#v-project .set-head')).not.toContainText('Saved')
  })

  test('project settings highlights the project row', async ({ page }) => {
    await gotoApp(page)
    const proj = seed.projects.find((p) => p.open) ?? seed.projects[0]
    const projRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: proj.name }) })
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.pset').click()
    await expect(page.locator('#v-project')).toHaveClass(/on/)
    await expect(projRow.locator('.tproj-h')).toHaveClass(/on/)
    await expect(page.locator('.tloop.on')).toHaveCount(0)
    // Leave hover so the pic (not the collapse caret) is the one on screen.
    await page.mouse.move(0, 0)
    const header = projRow.locator('.tproj-h')
    await expect(header.locator('.pic')).toBeVisible()
    const paint = await header.evaluate((el) => {
      const pic = el.querySelector('.pic')
      return {
        shadow: getComputedStyle(el).boxShadow.replace(/\s/g, ''),
        picTransform: pic ? getComputedStyle(pic).transform : '',
      }
    })
    expect(paint.shadow).not.toMatch(/inset/)
    expect(paint.picTransform).not.toBe('none')
  })

  test('add project is a create modal', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.sa button[aria-label="Add project"]').click()
    const modal = page.locator('#v-newproject')
    await expect(modal).toBeVisible()
    await expect(page.locator('#v-project')).not.toHaveClass(/on/)
    await modal.getByLabel('Repository path').fill('/tmp/quick-app')
    await expect(modal.getByLabel('Project name')).toHaveValue('quick-app')
    await modal.locator('.nt-go').click()
    await expect(modal).toHaveCount(0)
    await expect(page.locator('.tproj .pn', { hasText: 'quick-app' })).toBeVisible()
  })

  test('Providers warns when Cursor is connected without the SDK and installs it', async ({ page }) => {
    let installed = false
    const status = () => ({ bridgeInstalled: installed, authenticated: installed, email: installed ? 'dev@example.com' : '', error: installed ? '' : 'cursor-sdk-bridge not found' })
    const usage = {
      fetchedAt: Date.now(),
      stale: false,
      providers: [{ id: 'cursor', name: 'Cursor', email: 'dev@example.com', source: 'Cursor Connect', sourceId: 'cursor_connect', windows: [] }],
    }
    await installBridge(page, {
      ...structuredClone(seed),
      settings: { ...seed.settings, cursorApiKey: 'crsr_test' },
    }, {
      resolvers: {
        GetCursorStatus: status,
        InstallCursorBridge: () => { installed = true; return status() },
        GetProviderUsage: () => usage,
        RefreshProviderUsage: () => usage,
      },
    })
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Providers')
    const note = page.locator('#v-settings .acct-sdk-note')
    await expect(page.locator('#v-settings .acct-status-head.warn')).toContainText('SDK not installed')
    await expect(note).toContainText('Cursor SDK is not installed')
    await note.getByRole('button', { name: 'Install SDK' }).click()
    await expect(note).toHaveCount(0)
    await expect(page.locator('#v-settings .acct-status-head.warn')).toHaveCount(0)
  })

  test('changing the agent model is accepted', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Models')
    const agent = page.locator('#v-settings .sel-btn[aria-label="Agent / Coder model"]')
    await agent.click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Haiku 4.5' }).click()
    await expect(agent).toContainText('Haiku 4.5')
  })

  test('changing review flags is accepted', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Models')
    const flags = page.locator('#v-settings .sel-btn[aria-label="Review flags"]')
    await flags.click()
    await page.locator('body > .flags-menu .sel-opt', { hasText: 'Extra High' }).click()
    await expect(flags).toContainText('Extra High')
  })

  test('appearance picker switches and persists the theme', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Appearance')

    // App theme cards — six chrome themes; Charcoal starts selected.
    const appCards = page.locator('#v-settings .thgrid').first().locator('.thcard')
    await expect(appCards).toHaveCount(6)
    await expect(page.locator('html')).not.toHaveAttribute('data-theme', /.+/)

    // Picking Nord sets data-theme, marks the card active, and persists to storage.
    await appCards.filter({ hasText: 'Nord' }).click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'nord')
    await expect(appCards.filter({ hasText: 'Nord' })).toHaveClass(/on/)
    expect(await page.evaluate(() => localStorage.getItem('recur.theme'))).toBe('nord')

    // Paper is the light chrome theme.
    await appCards.filter({ hasText: 'Paper' }).click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'paper')
    await expect(appCards.filter({ hasText: 'Paper' })).toHaveClass(/on/)
    expect(await page.evaluate(() => localStorage.getItem('recur.theme'))).toBe('paper')

    // Back to Charcoal removes the attribute again.
    await appCards.filter({ hasText: 'Charcoal' }).click()
    await expect(page.locator('html')).not.toHaveAttribute('data-theme', /.+/)
  })

  test('match system appearance uses the chosen light and dark themes', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' })
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Appearance')

    await page.getByRole('button', { name: 'Sync with system' }).first().click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'paper')

    const grids = page.locator('#v-settings .thgrid')
    await expect(grids.nth(0).locator('.thcard')).toHaveCount(1)
    await expect(grids.nth(1).locator('.thcard')).toHaveCount(5)
    await grids.nth(1).locator('.thcard', { hasText: 'Nord' }).click()
    await page.emulateMedia({ colorScheme: 'dark' })
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'nord')

    await grids.nth(1).locator('.thcard', { hasText: 'Charcoal' }).click()
    await expect(page.locator('html')).not.toHaveAttribute('data-theme', /.+/)

    // Unchecking shows one grid again and a manual pick sticks.
    await page.getByRole('button', { name: 'Single theme' }).first().click()
    await page.locator('#v-settings .thgrid').first().locator('.thcard', { hasText: 'Slate' }).click()
    await page.emulateMedia({ colorScheme: 'dark' })
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'slate')
  })

  test('editor and terminal themes can sync with the system', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' })
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Appearance')

    const sync = page.getByRole('button', { name: 'Sync with system' })
    const lsGet = (k: string) => page.evaluate((key) => localStorage.getItem(key), k)

    // Editor: light mode picks Paper by default, dark mode picks Charcoal.
    await sync.nth(1).click()
    const edLists = page.locator('#v-settings .edth-list')
    await expect(edLists).toHaveCount(2)
    await expect.poll(() => lsGet('recur.editorTheme')).toBe('paper')
    await page.emulateMedia({ colorScheme: 'dark' })
    await expect.poll(() => lsGet('recur.editorTheme')).toBe('recur')

    // Terminal: dark mode uses its own pick, light mode falls back to Match app.
    await sync.nth(2).click()
    const termDark = page.locator('#v-settings .thgrid').nth(2).locator('.thcard', { hasText: 'Homebrew' })
    await expect(termDark).toBeVisible({ timeout: 15_000 })
    await termDark.click()
    await expect.poll(async () => JSON.parse((await lsGet('recur.terminalPrefs')) || '{}').theme).toBe('homebrew')
    await page.emulateMedia({ colorScheme: 'light' })
    await expect.poll(async () => JSON.parse((await lsGet('recur.terminalPrefs')) || '{}').theme).toBe('')
  })

  test('terminal theme picker is independent and persists', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Appearance')

    const termCards = page.locator('#v-settings .thgrid').nth(1).locator('.thcard')
    await expect(termCards.filter({ hasText: 'Match app' })).toHaveClass(/on/)
    const homebrew = termCards.filter({ hasText: 'Homebrew' })
    await expect(homebrew).toBeVisible({ timeout: 15_000 })
    await homebrew.click()
    await expect(homebrew).toHaveClass(/on/)
    expect(await page.evaluate(() => JSON.parse(localStorage.getItem('recur.terminalPrefs') || '{}').theme)).toBe('homebrew')
  })

  test('code editor theme is independent and persists', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Appearance')

    const edRows = page.locator('#v-settings .edth-list .edth-row')
    await expect(edRows.first()).toBeVisible({ timeout: 15_000 })
    await expect(edRows.filter({ hasText: 'Charcoal' })).toHaveClass(/on/)
    await expect(edRows.filter({ hasText: 'Paper' })).toBeVisible()

    // Bundle catalog loads async — wait for a pack theme, then pick it.
    const coolGlow = edRows.filter({ hasText: 'Cool Glow' })
    await expect(coolGlow).toBeVisible({ timeout: 15_000 })

    await edRows.filter({ hasText: 'Paper' }).click()
    await expect(edRows.filter({ hasText: 'Paper' })).toHaveClass(/on/)
    expect(await page.evaluate(() => localStorage.getItem('recur.editorTheme'))).toBe('paper')

    await coolGlow.click()
    await expect(coolGlow).toHaveClass(/on/)
    expect(await page.evaluate(() => localStorage.getItem('recur.editorTheme'))).toBe('cool-glow')

    // App chrome theme should be untouched.
    await expect(page.locator('html')).not.toHaveAttribute('data-theme', /.+/)
  })

  test('code editor layout prefs persist', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Code editor')

    const prefs = page.locator('#v-settings .editor-prefs')
    const fontSel = prefs.getByRole('combobox', { name: 'Editor font' })
    await expect(fontSel).toBeVisible()
    await fontSel.click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'IBM Plex Mono' }).click()
    await expect(fontSel).toContainText('IBM Plex Mono')

    await prefs.getByRole('group', { name: 'Editor font size' }).getByRole('button', { name: '11', exact: true }).click()
    await prefs.getByRole('group', { name: 'Line wrapping' }).getByRole('button', { name: 'Scroll' }).click()
    await prefs.getByRole('group', { name: 'Tab size' }).getByRole('button', { name: '2', exact: true }).click()
    await prefs.getByRole('group', { name: 'Default indent style' }).getByRole('button', { name: '2 spaces' }).click()
    // Per-language indent lives in a nested SettingsGroup (`.indent-lang-rows`),
    // not inside `.editor-prefs`.
    const indentRows = page.locator('#v-settings .indent-lang-rows')
    // Go already defaults to tabs via language conventions; force an explicit override.
    await indentRows.getByRole('group', { name: 'Go indent' }).getByRole('button', { name: '4', exact: true }).click()
    await expect(indentRows.getByText('Override · 4 spaces')).toBeVisible()
    await expect(indentRows.getByText('Auto · Tabs (gofmt)')).toHaveCount(0)

    // TypeScript should show its Auto convention without an override.
    await expect(indentRows.getByText('Auto · 2 spaces').first()).toBeVisible()

    await prefs.getByRole('group', { name: 'Render whitespace' }).getByRole('button', { name: 'Trailing' }).click()
    await prefs.getByRole('group', { name: 'Special characters' }).getByRole('button', { name: 'Off' }).click()
    await prefs.getByRole('group', { name: 'Line numbers' }).getByRole('button', { name: 'Off' }).click()
    await prefs.getByRole('group', { name: 'Scroll past end' }).getByRole('button', { name: 'On' }).click()
    await prefs.getByRole('group', { name: 'Line height', exact: true }).getByRole('button', { name: 'Relaxed' }).click()
    await prefs.getByRole('group', { name: 'Thick caret' }).getByRole('button', { name: 'On' }).click()
    await prefs.getByRole('group', { name: 'Print margin' }).getByRole('button', { name: '80', exact: true }).click()
    await prefs.getByRole('group', { name: 'Editor keymap' }).getByRole('button', { name: 'Vim' }).click()

    const saved = await page.evaluate(() => JSON.parse(localStorage.getItem('recur.editorPrefs') || '{}'))
    expect(saved).toMatchObject({
      fontFace: 'IBM Plex Mono',
      fontSize: 11,
      lineWrapping: false,
      tabSize: 2,
      indent: 'spaces-2',
      indentByLang: { go: 'spaces-4' },
      whitespace: 'trailing',
      specialChars: false,
      lineNumbers: false,
      scrollPastEnd: true,
      lineHeight: 1.7,
      thickCaret: true,
      rulerColumn: 80,
      keymap: 'vim',
    })
  })

  test('loops section shows loop and policy pickers', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Loops')

    await expect(page.locator('#v-settings .sel-btn[aria-label="Loop"]')).toBeVisible()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Policy"]')).toBeVisible()
    await page.locator('#v-settings .sel-btn[aria-label="Policy"]').click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Aggressive' }).click()
    await expect(page.locator('#v-settings .sel-btn[aria-label="Policy"]')).toContainText('Aggressive')
  })

  test('a failed policy save shows an error toast', async ({ page }) => {
    await installBridge(page, structuredClone(seed), { resolvers: { SetSettings: () => new BridgeError('settings save failed') } })
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Loops')

    await page.locator('#v-settings .sel-btn[aria-label="Policy"]').click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Aggressive' }).click()
    await expect(page.locator('.toast.error')).toBeVisible()
  })

  test('general settings include a branch name prefix field', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'General')
    await expect(page.locator('#v-settings .set-title', { hasText: 'Branch name prefix' })).toBeVisible()
    const input = page.locator('#v-settings input[aria-label="Branch name prefix"]')
    await expect(input).toHaveValue('recur')
    const presets = page.locator('#v-settings [aria-label="Prefix presets"] button')
    await expect(presets.filter({ hasText: 'Recur' })).toHaveClass(/on/)
    await input.fill('typed-prefix')
    await presets.filter({ hasText: 'None' }).click()
    await expect(input).toHaveValue('')
    await expect(page.locator('#v-settings .set-desc code')).toHaveText('crispy-quiche')
    await expect(presets.filter({ hasText: 'GitHub' })).toBeEnabled()
    await presets.filter({ hasText: 'GitHub' }).click()
    await expect(input).toHaveValue('mycodecrafting')
    await expect(page.locator('#v-settings .set-desc code')).toHaveText('mycodecrafting/crispy-quiche')
    await presets.filter({ hasText: 'Recur' }).click()
    await expect(input).toHaveValue('recur')
    await expect(page.locator('#v-settings .set-desc code')).toHaveText('recur/crispy-quiche')
  })

  test('general settings include live concurrency caps', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'General')
    await expect(page.locator('#v-settings .set-title', { hasText: 'Max concurrent loops' })).toBeVisible()
    await expect(page.locator('#v-settings input[aria-label="Max concurrent loops"]')).toHaveValue('4')
    await expect(page.locator('#v-settings input[aria-label="Max concurrent agents"]')).toHaveValue('8')
    await expect(page.locator('#v-settings .set-desc', { hasText: '0 is unlimited' })).toHaveCount(2)
  })

  test('PR settings radio dots share a column', async ({ page }) => {
    await gotoApp(page)
    await openSettings(page)
    await openSection(page, 'Pull requests')
    const dots = page.locator('#v-settings .set-row:has(.merge-method-radios) .mm-radio:first-child .mm-dot')
    await expect(dots).toHaveCount(4)
    const boxes = await dots.evaluateAll((els) => els.map((el) => el.getBoundingClientRect().x))
    for (const x of boxes) expect(Math.abs(x - boxes[0])).toBeLessThan(1)
  })
})

test.describe('new workspace composer', () => {
  test('a late project refresh does not reload branches or disable Create', async ({ page }) => {
    let gate = false
    let listBranches = 0
    let releaseRefresh = () => {}
    const refreshed = new Promise<void>((r) => { releaseRefresh = r })
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        // Boot list returns at once; the modal's open-time refresh lands late
        // with fresh objects (renamed so the test can see it apply).
        ListProjects: async (_a, s) => {
          if (!gate) return s.projects
          await refreshed
          return s.projects.map((p) => ({ ...p, name: `${p.name}-late` }))
        },
        ListBranches: () => {
          listBranches++
          return [{ name: 'main', group: 'Local' }, { name: 'develop', group: 'Local' }]
        },
      },
    })
    await gotoApp(page)
    gate = true
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    const go = modal.locator('.nt-go')
    await expect(go).toBeEnabled()
    const repoPicker = modal.locator('.sel-btn[aria-label="Repository"]')
    await expect(repoPicker).not.toContainText('-late')
    releaseRefresh()
    await expect(repoPicker).toContainText('-late')
    await expect(go).toBeEnabled()
    expect(listBranches).toBe(1)
  })

  test('slash menu offers prompts and skills; @ searches the project', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListFiles: () => ['README.md', 'src/main.go', 'PROGRESS.md'],
      },
    })
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const composer = modal.locator('.chat-composer')
    await expect(composer).toBeFocused()

    await composer.pressSequentially('/', { delay: 15 })
    const menu = modal.locator('.chat-mention')
    await expect(menu).toBeVisible()
    await expect(modal.locator('.chat-mention-name', { hasText: /^\/commit$/ })).toBeVisible()
    const vp = page.viewportSize()
    expect(vp).toBeTruthy()
    const box = await menu.boundingBox()
    const composerBox = await composer.boundingBox()
    expect(box).toBeTruthy()
    expect(composerBox).toBeTruthy()
    expect(box!.y, 'slash menu must not clip past the top of the window').toBeGreaterThanOrEqual(0)
    expect(box!.y + box!.height, 'slash menu must not clip past the bottom of the window')
      .toBeLessThanOrEqual(vp!.height)
    expect(box!.y, 'slash menu sits just below the caret line, not under the empty composer').toBeGreaterThan(composerBox!.y)
    expect(box!.y).toBeLessThan(composerBox!.y + 72)
    await composer.pressSequentially('design', { delay: 15 })
    await expect(modal.locator('.chat-mention-row', { hasText: 'design-audit' })).toBeVisible()

    await page.keyboard.press('Escape')
    await expect(menu).toHaveCount(0)
    await expect(modal).toBeVisible()

    await page.keyboard.press('Meta+A')
    await page.keyboard.press('Backspace')
    await composer.pressSequentially('@PROG', { delay: 15 })
    await expect(modal.locator('.chat-mention')).toBeVisible()
    await expect(modal.locator('.chat-mention-row', { hasText: 'PROGRESS.md' })).toBeVisible()
    await page.keyboard.press('Enter')
    await expect(composer.locator('.chat-composer-chip', { hasText: 'PROGRESS.md' })).toBeVisible()
  })

  test('picking a skill chips it and Create sends it as the opening message', async ({ page }) => {
    await gotoApp(page)
    await page.locator('button[aria-label="Get started"]').click()
    await expect(page.locator('#v-welcome')).toHaveClass(/on/)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const composer = modal.locator('.chat-composer')
    await composer.click()
    await composer.pressSequentially('/design', { delay: 15 })
    const row = modal.locator('.chat-mention-row', { hasText: 'design-audit' })
    await expect(row).toBeVisible()
    await row.click()
    await expect(composer.locator('.chat-composer-chip', { hasText: 'design-audit' })).toBeVisible()
    await modal.locator('.nt-go').click()
    await expect(modal).toHaveCount(0)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .chat-row.me')).toContainText('design-audit')
  })

  test('picking /review runs the prompt and opens the workspace', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        CreateWorkspace: (a) => ({
          id: 'WS-NEW',
          projectId: a[0],
          type: 'solo',
          statusLabel: 'Creating',
          phase: 'creating',
          branch: 'recur/new',
          baseBranch: a[2] || 'main',
          worktreePath: '/tmp/recur-e2e/WS-NEW',
        }),
      },
    })
    await gotoApp(page)
    await page.locator('button[aria-label="Get started"]').click()
    await expect(page.locator('#v-welcome')).toHaveClass(/on/)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const composer = modal.locator('.chat-composer')
    await composer.click()
    await page.keyboard.type('/review')
    await modal.locator('.chat-mention-name', { hasText: /^\/review$/ }).click()
    await expect(modal).toHaveCount(0)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .chat-row.me', {
      hasText: 'Please review the changes in this worktree and file any real issues',
    })).toBeVisible()
  })

  test('plain Enter inserts a newline', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const composer = modal.locator('.chat-composer')
    await expect(composer).toBeFocused()
    await page.keyboard.type('first line')
    await page.keyboard.press('Enter')
    await expect(modal).toBeVisible()
    await page.keyboard.type('second line')
    await expect.poll(() => composer.innerText()).toMatch(/first line\s+second line/)
  })

  test('empty ⌘Enter from the focused composer creates a workspace', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const composer = modal.locator('.chat-composer')
    await expect(composer).toBeFocused()
    await expect(modal.locator('.nt-go')).toBeEnabled()
    await page.keyboard.press('Meta+Enter')
    await expect(modal).toHaveCount(0)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('#v-detail')).not.toHaveClass(/on/)
  })
})
