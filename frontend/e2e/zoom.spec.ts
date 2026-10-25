import { test, expect, gotoApp, reloadApp } from './helpers/fixtures'
import type { Page } from '@playwright/test'

// The mock bridge reports macOS, so the app-level chord modifier is Meta.
const zoomOf = (page: Page) => page.evaluate(() => document.documentElement.style.zoom)
const toast = (page: Page, text: string) => page.locator('.toast', { hasText: text })

async function wheelWithMeta(page: Page, deltaY: number) {
  await page.mouse.move(400, 300)
  await page.keyboard.down('Meta')
  await page.mouse.wheel(0, deltaY)
  await page.keyboard.up('Meta')
}

test.describe('ui zoom', () => {
  test('chords step the zoom, show a toast and reset', async ({ page }) => {
    await gotoApp(page)

    await page.keyboard.press('Meta+=')
    await expect.poll(() => zoomOf(page)).toBe('1.1')
    await expect(toast(page, 'Zoom 110%')).toBeVisible()

    await page.keyboard.press('Meta+-')
    await page.keyboard.press('Meta+-')
    await expect.poll(() => zoomOf(page)).toBe('0.9')
    await expect(toast(page, 'Zoom 90%')).toBeVisible()
    await expect(page.locator('.toast', { hasText: /Zoom \d+%/ })).toHaveCount(1)

    await page.keyboard.press('Meta+0')
    await expect.poll(() => zoomOf(page)).toBe('1')
    await expect(toast(page, 'Zoom 100%')).toBeVisible()
  })

  test('Ctrl/Cmd+wheel zooms and a plain wheel does not', async ({ page }) => {
    await gotoApp(page)

    await page.mouse.move(400, 300)
    await page.mouse.wheel(0, -100)
    await page.waitForTimeout(200)
    expect(await zoomOf(page)).toBe('')

    await wheelWithMeta(page, -100)
    await expect.poll(() => zoomOf(page)).toBe('1.1')

    await page.waitForTimeout(200)
    await wheelWithMeta(page, 100)
    await expect.poll(() => zoomOf(page)).toBe('1')
  })

  test('the level survives a reload', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+=')
    await page.keyboard.press('Meta+=')
    await expect.poll(() => zoomOf(page)).toBe('1.25')
    // SetUIZoom is debounced; let it land before the "restart".
    await page.waitForTimeout(600)

    await reloadApp(page)
    expect(await zoomOf(page)).toBe('1.25')
    await expect(page.locator('.toast', { hasText: /Zoom \d+%/ })).toHaveCount(0)
  })
})
