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
    // SetUIZoom is debounced; wait for the call to land before the "restart".
    const persisted = page.waitForRequest(
      (r) => r.url().endsWith('/wails/runtime') && (r.postData() ?? '').includes('1.25'),
    )
    await page.keyboard.press('Meta+=')
    await page.keyboard.press('Meta+=')
    await expect.poll(() => zoomOf(page)).toBe('1.25')
    await persisted

    await reloadApp(page)
    await expect.poll(() => zoomOf(page)).toBe('1.25')
    await expect(page.locator('.toast', { hasText: /Zoom \d+%/ })).toHaveCount(0)
  })

  test('numpad and shifted chords work', async ({ page }) => {
    await gotoApp(page)

    await page.keyboard.press('Meta+Shift+=')
    await expect.poll(() => zoomOf(page)).toBe('1.1')
    await page.keyboard.press('Meta+NumpadAdd')
    await expect.poll(() => zoomOf(page)).toBe('1.25')
    await page.keyboard.press('Meta+NumpadSubtract')
    await expect.poll(() => zoomOf(page)).toBe('1.1')
    await page.keyboard.press('Meta+Numpad0')
    await expect.poll(() => zoomOf(page)).toBe('1')
  })

  test('zoom clamps between 50% and 200%', async ({ page }) => {
    await gotoApp(page)

    for (let i = 0; i < 12; i++) await page.keyboard.press('Meta+=')
    await expect.poll(() => zoomOf(page)).toBe('2')
    for (let i = 0; i < 12; i++) await page.keyboard.press('Meta+-')
    await expect.poll(() => zoomOf(page)).toBe('0.5')
  })

  test('View menu actions zoom in, out and reset', async ({ page }) => {
    await gotoApp(page)
    const menu = (data: string) =>
      page.evaluate((d) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'menu.action', data: d })
      }, data)

    await menu('zoom-in')
    await expect.poll(() => zoomOf(page)).toBe('1.1')
    await menu('zoom-out')
    await expect.poll(() => zoomOf(page)).toBe('1')
    await menu('zoom-in')
    await menu('zoom-reset')
    await expect.poll(() => zoomOf(page)).toBe('1')
  })
})
