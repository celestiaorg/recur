import { test, expect, gotoApp, seed } from './helpers/fixtures'
import { installBridge, BridgeError } from './helpers/mock-bridge'

test.describe('welcome / first run', () => {
  const codexLoginSeed = { ...seed, settings: { ...seed.settings, codexChatGPTAuth: true } }

  test('a fresh app with a detected agent lands on step 2 and opens the project dialog', async ({ page }) => {
    await installBridge(page, codexLoginSeed, { resolvers: { ListProjects: () => [], ListWorkspaces: () => [] } })
    await gotoApp(page)
    const welcome = page.locator('#v-welcome')
    await expect(welcome).toHaveClass(/on/)
    await expect(welcome.locator('.wprov li[data-provider="claude"]')).toHaveClass(/on/)
    await expect(welcome.locator('.wprov li[data-provider="claude"]')).toContainText('dev@example.com')
    await expect(welcome.locator('.wprov li[data-provider="codex"]')).not.toHaveClass(/on/)
    await expect(welcome.locator('.wprov li[data-provider="codex"]')).toContainText('codex login')
    await expect(welcome.locator('.wprov li[data-provider="cursor"]').getByRole('button', { name: 'Connect' })).toBeVisible()
    await expect(welcome.locator('.ws[data-step="2"]')).toHaveClass(/current/)
    await welcome.getByRole('button', { name: 'Choose a repository' }).click()
    await expect(page.getByRole('dialog', { name: 'New project' })).toBeVisible()
  })

  test('without an agent, step 1 offers providers and can be skipped', async ({ page }) => {
    await installBridge(page, codexLoginSeed, {
      resolvers: { ListProjects: () => [], ListWorkspaces: () => [], DetectAgent: () => new BridgeError('no agent') },
    })
    await gotoApp(page)
    const welcome = page.locator('#v-welcome')
    await expect(welcome.locator('.ws[data-step="1"]')).toHaveClass(/current/)
    await expect(welcome.locator('.wprov li.on')).toHaveCount(0)
    await expect(welcome.getByRole('button', { name: 'Check again' })).toHaveCount(2)
    await welcome.getByRole('button', { name: 'Continue without an agent' }).click()
    await expect(welcome.locator('.ws[data-step="2"]')).toHaveClass(/current/)
    await expect(welcome.locator('.wprov li')).toHaveCount(3)
    await expect(welcome.getByRole('button', { name: 'Continue without an agent' })).toHaveCount(0)
  })

  test('a Cursor key counts as a connected agent without Claude', async ({ page }) => {
    await installBridge(page, {
      ...seed,
      settings: { ...seed.settings, cursorApiKey: 'crsr_test' },
    }, {
      resolvers: { ListProjects: () => [], ListWorkspaces: () => [], DetectAgent: () => new BridgeError('no agent') },
    })
    await gotoApp(page)
    await expect(page.locator('#v-welcome .wprov li[data-provider="cursor"]')).toHaveClass(/on/)
    await expect(page.locator('#v-welcome .wprov li[data-provider="claude"]')).not.toHaveClass(/on/)
    await expect(page.locator('#v-welcome .ws[data-step="2"]')).toHaveClass(/current/)
  })

  test('a Cursor key without the SDK is not ready until Install SDK completes', async ({ page }) => {
    let installed = false
    const status = () => ({ bridgeInstalled: installed, authenticated: installed, email: installed ? 'dev@example.com' : '', error: installed ? '' : 'cursor-sdk-bridge not found' })
    await installBridge(page, {
      ...seed,
      settings: { ...seed.settings, cursorApiKey: 'crsr_test' },
    }, {
      resolvers: {
        ListProjects: () => [], ListWorkspaces: () => [], DetectAgent: () => new BridgeError('no agent'),
        GetCursorStatus: status,
        InstallCursorBridge: () => { installed = true; return status() },
      },
    })
    await gotoApp(page)
    const welcome = page.locator('#v-welcome')
    const cursor = welcome.locator('.wprov li[data-provider="cursor"]')
    await expect(welcome.locator('.ws[data-step="1"]')).toHaveClass(/current/)
    await expect(cursor).toHaveClass(/warn/)
    await expect(cursor).toContainText('SDK is not installed')
    await cursor.getByRole('button', { name: 'Install SDK' }).click()
    await expect(cursor).toHaveClass(/on/)
    await expect(cursor).toContainText('dev@example.com')
    await expect(welcome.locator('.ws[data-step="2"]')).toHaveClass(/current/)
  })

  test('a signed-in Codex counts and the Cursor connect link lands on Providers settings', async ({ page }) => {
    await installBridge(page, seed, {
      resolvers: {
        ListProjects: () => [], ListWorkspaces: () => [], DetectAgent: () => new BridgeError('no agent'),
        GetCodexStatus: () => ({ binInstalled: true, authenticated: true, loginMethod: 'ChatGPT', email: 'dev@example.com', error: '' }),
      },
    })
    await gotoApp(page)
    const welcome = page.locator('#v-welcome')
    await expect(welcome.locator('.wprov li[data-provider="codex"]')).toHaveClass(/on/)
    await expect(welcome.locator('.ws[data-step="2"]')).toHaveClass(/current/)
    await welcome.locator('.wprov li[data-provider="cursor"]').getByRole('button', { name: 'Connect' }).click()
    await expect(page.locator('#v-settings .set-head h2')).toHaveText('Providers')
  })

  test('agents are probed only while Get started is shown, and again on return from Settings', async ({ page }) => {
    let probes = 0
    let authed = false
    await installBridge(page, seed, {
      resolvers: {
        GetCodexStatus: () => {
          probes++
          return { binInstalled: true, authenticated: authed, loginMethod: 'ChatGPT', email: authed ? 'dev@example.com' : '', error: '' }
        },
      },
    })
    await gotoApp(page)
    await expect(page.locator('#v-welcome')).not.toHaveClass(/on/)
    expect(probes).toBe(0)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const step1 = page.locator('#v-welcome .ws[data-step="1"]')
    await expect.poll(() => probes).toBe(1)
    await expect(step1.locator('.ws-chip')).toHaveText(['Claude Code'])
    authed = true
    await step1.getByRole('button', { name: 'Providers' }).click()
    await expect(page.locator('#v-settings .set-head h2')).toHaveText('Providers')
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    await expect(step1.locator('.ws-chip')).toHaveText(['Claude Code', 'Codex'])
  })

  test('with a project but no workspace, a mode row opens an empty New workspace dialog in that mode', async ({ page }) => {
    await installBridge(page, seed, { resolvers: { ListWorkspaces: () => [] } })
    await page.addInitScript(() => localStorage.setItem('recur.newWorkspaceMode', 'loop'))
    await gotoApp(page)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.locator('.ws[data-step="3"]')).toHaveClass(/current/)
    await expect(welcome.getByRole('button', { name: /^Start in .* mode$/ })).toHaveCount(5)
    await welcome.getByRole('button', { name: 'Start in Review mode' }).click()
    const modal = page.getByRole('dialog', { name: 'New workspace' })
    await expect(modal.locator('.nt-in')).toHaveValue('')
    await expect(modal.locator('.sel-btn[aria-label="Mode"]')).toContainText('Review')
    await modal.locator('.nt-in').fill('Review the auth changes.')
    await expect(modal.locator('.nt-in')).toHaveValue('Review the auth changes.')
  })

  test('a project whose only workspace is the working copy still needs a first task', async ({ page }) => {
    const main = { ...seed.loops[0], id: 'MAIN-storefront', isMain: true, archived: false, hostPrId: '' }
    await installBridge(page, seed, { resolvers: { ListWorkspaces: () => [main] } })
    await gotoApp(page)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.locator('.ws[data-step="3"]')).toHaveClass(/current/)
    await expect(welcome.getByRole('heading', { level: 1 })).toHaveText('Set up Recur')
  })

  test('opening and cancelling the New workspace dialog does not complete the first task', async ({ page }) => {
    await installBridge(page, seed, {
      resolvers: {
        ListWorkspaces: () => [],
        CreateWorkspace: (a) => ({ id: 'WS-NEW', projectId: a[0], type: 'solo', goal: 'New Workspace', short: 'New Workspace', statusLabel: 'Creating', phase: 'creating', branch: 'recur/new', baseBranch: 'main' }),
      },
    })
    await gotoApp(page)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.locator('.ws[data-step="3"]')).toHaveClass(/current/)
    await page.keyboard.press('Meta+N')
    const modal = page.getByRole('dialog', { name: 'New workspace' })
    await expect(modal).toBeVisible()
    await expect(welcome.locator('.ws[data-step="3"]')).toHaveClass(/current/)
    await page.keyboard.press('Escape')
    await expect(modal).toHaveCount(0)
    await expect(welcome.locator('.ws[data-step="3"]')).toHaveClass(/current/)
    await expect(welcome.getByRole('heading', { level: 1 })).toHaveText('Set up Recur')
  })

  test('a hand-driven workspace counts once its first message named it', async ({ page }) => {
    const solo = { ...seed.loops[0], id: 'WS-1', type: 'solo', runStarted: false, isMain: false, archived: false, hostPrId: '', goal: 'Add a timeout to the session refresh', short: 'Add a timeout', lastActivityAt: 0 }
    await installBridge(page, seed, { resolvers: { ListWorkspaces: () => [solo] } })
    await gotoApp(page)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.getByRole('heading', { level: 1 })).toHaveText('You’re set up.')
    await expect(welcome.locator('.ws[data-step="3"] .ws-chip')).toHaveText('1 workspace')
  })

  test('the plain workspace shortcut still uses the remembered mode', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('recur.newWorkspaceMode', 'plan'))
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.getByRole('dialog', { name: 'New workspace' })
    await expect(modal.locator('.nt-in')).toHaveValue('')
    await expect(modal.locator('.sel-btn[aria-label="Mode"]')).toContainText('Plan')
  })

  test('once a workspace exists, the page shows the completed steps and links onward', async ({ page }) => {
    await gotoApp(page)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.getByRole('heading', { level: 1 })).toHaveText('You’re set up.')
    await expect(welcome.locator('.ws-num.done')).toHaveCount(3)
    await expect(welcome.locator('.ws[data-step="1"] .ws-chip')).toHaveText(['Claude Code'])
    await expect(welcome.locator('.ws[data-step="2"] .ws-chip')).toHaveCount(3)
    await expect(welcome.locator('.ws[data-step="3"] .ws-chip')).toContainText('workspaces')
    await expect(welcome.locator('.ws[data-step="1"]').getByRole('button', { name: 'Providers' })).toBeVisible()
    await expect(welcome.locator('.wexplore-foot .lnk')).toHaveCount(0)
    await welcome.getByRole('button', { name: /^Inbox/ }).click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
  })

  test('a set-up user does not see the setup flow while agents are still being probed', async ({ page }) => {
    let release = () => {}
    const gate = new Promise<void>((r) => { release = r })
    await installBridge(page, seed, {
      resolvers: { DetectAgent: () => gate.then(() => 'Claude Code · dev@example.com (Acme)') },
    })
    await gotoApp(page)
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.getByRole('heading', { level: 1 })).toHaveText('You’re set up.')
    await expect(welcome.locator('.ws[data-step="1"]')).not.toHaveClass(/current/)
    await expect(welcome.getByRole('button', { name: 'Continue without an agent' })).toHaveCount(0)
    release()
    await expect(welcome.locator('.ws[data-step="1"] .ws-chip')).toHaveText(['Claude Code'])
  })

  test('fits a narrow pane in the light theme', async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 760, height: 900 })
    await installBridge(page, seed, { resolvers: { ListWorkspaces: () => [] } })
    await gotoApp(page)
    await page.evaluate(() => { document.documentElement.dataset.theme = 'paper' })
    await page.getByRole('button', { name: 'Get started', exact: true }).click()
    const welcome = page.locator('#v-welcome')
    await expect(welcome.locator('.ws[data-step="3"]')).toHaveClass(/current/)
    expect(await welcome.locator('.scroll').evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true)
    await page.screenshot({ path: testInfo.outputPath('welcome.png') })
  })
})
