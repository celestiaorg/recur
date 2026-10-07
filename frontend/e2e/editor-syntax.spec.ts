// Change the open file's TextMate language from the editor status bar.
import { test, expect, gotoApp, reloadApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { codeEditor } from './helpers/editor'

async function openLoopCode(page: import('@playwright/test').Page) {
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  return working
}

async function openFile(page: import('@playwright/test').Page, name: string) {
  await page.locator('#v-detail .obs-files .obs-node', { hasText: name }).click()
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await code.count()) {
    await expect(code).toBeEnabled()
    await code.click()
    await expect(code).toHaveClass(/on/)
  }
  await expect(codeEditor(page)).toBeVisible()
}

test('status-bar language is a picker; the pick is remembered for this file', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')

  const lang = page.locator('#v-detail .obs-statusbar[aria-label="Editor status"] button[aria-label="Change language mode"]')
  await expect(lang).toBeVisible()
  await expect(lang).toContainText(/go/i)

  await lang.click()
  const pal = page.locator('.palette-bg.open [role="dialog"][aria-label="Change language mode"]')
  await expect(pal).toBeVisible()
  await pal.locator('input').fill('plain')
  await pal.locator('.pal-row', { hasText: 'Plain Text' }).click()
  await expect(pal).toBeHidden()
  await expect(lang).toHaveText('Plain Text')

  await lang.click()
  await expect(pal).toBeVisible()
  await pal.locator('input').fill('auto detect')
  await pal.locator('.pal-row', { hasText: 'Auto Detect' }).click()
  await expect(lang).toContainText(/go/i)
})

test('language pick survives a restart via the editor session', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')

  const lang = page.locator('#v-detail .obs-statusbar[aria-label="Editor status"] button[aria-label="Change language mode"]')
  await lang.click()
  const pal = page.locator('.palette-bg.open [role="dialog"][aria-label="Change language mode"]')
  await pal.locator('input').fill('plain')
  await pal.locator('.pal-row', { hasText: 'Plain Text' }).click()
  await expect(lang).toHaveText('Plain Text')
  // Debounced session patch must land on the mock backend before "restart".
  await page.waitForTimeout(600)

  await reloadApp(page)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"] button[aria-label="Change language mode"]'))
    .toHaveText('Plain Text')
})
