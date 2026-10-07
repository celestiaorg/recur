import { test, expect, gotoApp, seed } from './helpers/fixtures'
import { installBridge, BridgeError } from './helpers/mock-bridge'

const RICH_PR = '482'

test.describe('toasts', () => {
  test('approving a PR shows a success toast that auto-dismisses', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await page.locator('.titlebar .review-actions button', { hasText: 'Approve' }).click()
    await page.locator('#v-pr-review .nt-go', { hasText: 'Approve' }).click()

    const toast = page.locator('.toast.success', { hasText: 'Review approved' })
    await expect(toast).toBeVisible()
    await expect(toast).toHaveCount(0, { timeout: 5000 })
  })

  test('a failed merge shows an error toast that auto-dismisses', async ({ page }) => {
    await installBridge(page, seed, {
      resolvers: { MergePR: () => new BridgeError('merge did not complete — see the loop activity feed for the reason') },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const merge = page.locator('.titlebar .review-actions .rbtn', { hasText: 'Merge' })
    await merge.click() // arm
    await merge.click() // confirm

    const toast = page.locator('.toast.error')
    await expect(toast).toBeVisible()
    // Errors linger longer than success/info (8s vs 3.5s TTL)…
    await page.waitForTimeout(4500)
    await expect(toast).toBeVisible()
    // …but still self-dismiss once the 8s TTL passes.
    await expect(toast).toHaveCount(0, { timeout: 6000 })
  })

  test('a long error toast keeps the dismiss control reachable', async ({ page }) => {
    // Unbroken JSON/path payloads used to expand past the viewport and hide ✕.
    const longMsg =
      '{"message":"read /Users/josh/.recur/worktrees/caramel-cannoli/packages/contracts/lib/openzeppelin-contracts-upgradeable: is a directory","cause":{"Op":"read","Path":"/Users/josh/.recur/worktrees/caramel-cannoli/packages/contracts/lib/openzeppelin-contracts-upgradeable","Err":21},"kind":"RuntimeError"}'
    await installBridge(page, seed, {
      resolvers: { MergePR: () => new BridgeError(longMsg) },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const merge = page.locator('.titlebar .review-actions .rbtn', { hasText: 'Merge' })
    await merge.click()
    await merge.click()

    const toast = page.locator('.toast.error')
    await expect(toast).toBeVisible()
    const close = toast.locator('.tclose')
    await expect(close).toBeInViewport()
    await close.click()
    await expect(toast).toHaveCount(0)
  })
})
