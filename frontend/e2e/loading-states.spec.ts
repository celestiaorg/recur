import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

test.describe('loading states', () => {
  test('the Loop chat renders session cards once the delayed fetch resolves', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceSessions: async () => {
          await new Promise((r) => setTimeout(r, 300))
          return [{
            id: 's1', code: 'CO', name: 'Coder', kind: 'coder', status: 'idle',
            createdAt: 1, updatedAt: 2,
            items: [{ role: 'assistant', kind: 'text', text: 'implemented the change' }],
          }]
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect(page.locator('#v-detail').getByText('implemented the change')).toBeVisible({ timeout: 5000 })
  })
})
