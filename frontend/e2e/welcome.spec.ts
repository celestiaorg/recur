import { test, expect, gotoApp, seed } from './helpers/fixtures'
import { installBridge, BridgeError } from './helpers/mock-bridge'

test.describe('welcome / first run', () => {
  test('an empty workspace boots into Welcome', async ({ page }) => {
    await installBridge(page, seed, { resolvers: { ListProjects: () => [] } })
    await gotoApp(page)
    await expect(page.locator('#v-welcome')).toHaveClass(/on/)
    await expect(page.locator('#v-welcome h1')).toContainText('Workspaces')
    await expect(page.locator('#v-welcome h1 em', { hasText: 'mode' })).toBeVisible()
    await expect(page.locator('#v-welcome .wmode .wm-n', { hasText: 'Ask' })).toBeVisible()
    await expect(page.locator('#v-welcome .wmode .wm-n', { hasText: 'Loop' })).toBeVisible()
    await expect(page.locator('#v-welcome .tbtn.amber', { hasText: 'Add a project' })).toBeVisible()
  })

  test('a failed agent detection shows an honest "not detected" state', async ({ page }) => {
    await installBridge(page, seed, {
      resolvers: { ListProjects: () => [], DetectAgent: () => new BridgeError('no agent') },
    })
    await gotoApp(page)
    await expect(page.locator('#v-welcome')).toHaveClass(/on/)
    await expect(page.locator('#v-welcome .detected.bad', { hasText: 'No agent connected' })).toBeVisible()
    // The auto fixture (helpers/fixtures.ts) asserts no uncaught page errors on
    // teardown — a rejected DetectAgent call must not throw unhandled.
  })

  test('a Cursor key counts as a connected agent without Claude', async ({ page }) => {
    await installBridge(page, {
      ...seed,
      settings: { ...seed.settings, cursorApiKey: 'crsr_test' },
    }, {
      resolvers: { ListProjects: () => [], DetectAgent: () => new BridgeError('no agent') },
    })
    await gotoApp(page)
    await expect(page.locator('#v-welcome .detected', { hasText: /^Cursor$/ })).toBeVisible()
    await expect(page.locator('#v-welcome .detected.bad')).toHaveCount(0)
  })
})
