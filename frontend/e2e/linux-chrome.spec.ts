import { test, expect, gotoApp, seed } from './helpers/fixtures'
import { installBridge, installLinuxChrome, LINUX_MENU_SPEC, type WindowOps } from './helpers/mock-bridge'
import type { Page } from '@playwright/test'

async function bootLinux(page: Page, custom = true): Promise<WindowOps> {
  const s = structuredClone(seed)
  await installBridge(page, s, {
    resolvers: { HasCustomChrome: () => custom, GetMenuSpec: () => LINUX_MENU_SPEC },
  })
  const ops = await installLinuxChrome(page)
  await gotoApp(page)
  return ops
}

test.describe('Linux custom chrome', () => {
  test('hamburger menu mirrors the spec with disabled, radio, accel and separators', async ({ page }) => {
    await bootLinux(page)
    const toggle = page.getByRole('button', { name: 'Application menu' })
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-expanded', 'true')

    const menu = page.locator('.app-menu')
    await expect(menu.getByRole('menuitem', { name: 'File' })).toBeVisible()
    await menu.getByRole('menuitem', { name: 'File' }).click()
    await expect(menu.getByRole('menuitem', { name: /^New Workspace/ })).toContainText('Ctrl+N')
    await expect(menu.getByRole('menuitem', { name: /^Save/ })).toBeDisabled()
    await expect(menu.getByRole('separator')).toHaveCount(1)

    await menu.getByRole('menuitem', { name: 'View' }).click()
    await menu.getByRole('menuitem', { name: 'Appearance' }).click()
    await menu.getByRole('menuitem', { name: 'App Theme' }).click()
    await expect(menu.getByRole('menuitemradio', { name: 'Charcoal' })).toHaveAttribute('aria-checked', 'true')
    await expect(menu.getByRole('menuitemradio', { name: 'Paper' })).toHaveAttribute('aria-checked', 'false')
  })

  test('menu actions fire, and Esc / outside click close it', async ({ page }) => {
    await bootLinux(page)
    const toggle = page.getByRole('button', { name: 'Application menu' })
    await toggle.click()
    await page.locator('.app-menu').getByRole('menuitem', { name: 'View' }).click()
    await page.locator('.app-menu').getByRole('menuitem', { name: 'Appearance' }).click()
    await page.locator('.app-menu').getByRole('menuitem', { name: 'App Theme' }).click()
    await page.locator('.app-menu').getByRole('menuitemradio', { name: 'Paper' }).click()
    await expect(page.locator('.app-menu')).toHaveCount(0)
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'paper')

    await toggle.click()
    await page.keyboard.press('Escape')
    await expect(page.locator('.app-menu')).toHaveCount(0)
    await expect(toggle).toBeFocused()

    await toggle.click()
    await page.locator('.rail').click({ position: { x: 10, y: 300 } })
    await expect(page.locator('.app-menu')).toHaveCount(0)
  })

  test('Quit item quits the app', async ({ page }) => {
    const ops = await bootLinux(page)
    await page.getByRole('button', { name: 'Application menu' }).click()
    await page.locator('.app-menu').getByRole('menuitem', { name: 'File' }).click()
    await page.locator('.app-menu').getByRole('menuitem', { name: 'Quit' }).click()
    await expect.poll(() => ops.calls).toContain('quit')
  })

  test('keyboard: Alt opens the menu, arrows move, ArrowRight expands', async ({ page }) => {
    await bootLinux(page)
    await page.keyboard.press('Alt')
    const menu = page.locator('.app-menu')
    await expect(menu).toBeVisible()
    await expect(menu.getByRole('menuitem', { name: 'File' })).toBeFocused()
    await page.keyboard.press('ArrowRight')
    await expect(menu.getByRole('menuitem', { name: 'File' })).toHaveAttribute('aria-expanded', 'true')
    await page.keyboard.press('ArrowDown')
    await expect(menu.getByRole('menuitem', { name: /^New Workspace/ })).toBeFocused()
    await page.keyboard.press('ArrowLeft')
    await expect(menu.getByRole('menuitem', { name: 'File' })).toBeFocused()
    await page.keyboard.press('Alt')
    await expect(menu).toHaveCount(0)
  })

  test('window controls call the window API and restore tracks maximise', async ({ page }) => {
    const ops = await bootLinux(page)
    const bar = page.locator('.titlebar')
    await expect(bar.getByRole('button', { name: 'Maximize' })).toBeVisible()
    await bar.getByRole('button', { name: 'Minimize' }).click()
    await bar.getByRole('button', { name: 'Maximize' }).click()
    // The real window resizes on maximise; the browser does not.
    await page.evaluate(() => window.dispatchEvent(new Event('resize')))
    await expect(bar.getByRole('button', { name: 'Restore' })).toBeVisible()
    await bar.getByRole('button', { name: 'Close' }).click()
    await expect.poll(() => ops.calls).toEqual(['minimise', 'toggleMaximise', 'close'])
  })

  test('double-click on empty header toggles maximise, buttons and fullscreen do not', async ({ page }) => {
    const ops = await bootLinux(page)
    const bar = page.locator('.titlebar')
    await expect(bar.locator('.tb-left')).toBeVisible()
    const box = (await bar.locator('.tb-left').boundingBox())!
    await page.mouse.dblclick(box.x + box.width - 8, box.y + box.height / 2)
    await expect.poll(() => ops.calls).toEqual(['toggleMaximise'])

    await bar.getByRole('button', { name: 'Application menu' }).dblclick()
    await page.keyboard.press('Escape')
    expect(ops.calls).toEqual(['toggleMaximise'])

    ops.fullscreen = true
    await page.mouse.dblclick(box.x + box.width - 8, box.y + box.height / 2)
    await page.waitForTimeout(150)
    expect(ops.calls).toEqual(['toggleMaximise'])
  })

  test('drag region is scoped: bar drags, interactive children opt out', async ({ page }) => {
    await bootLinux(page)
    const draggable = (sel: string) =>
      page.locator(sel).first().evaluate((el) => getComputedStyle(el).getPropertyValue('--wails-draggable').trim())
    expect(await draggable('.titlebar')).toBe('drag')
    expect(await draggable('.tb-left')).toBe('drag')
    expect(await draggable('.titlebar .tb-rail')).toBe('no-drag')
    expect(await draggable('.tb-winbtn')).toBe('no-drag')
    expect(await draggable('.tb-menu')).toBe('no-drag')
  })

  test('chrome-custom class is absent without custom chrome', async ({ page }) => {
    await bootLinux(page, false)
    await expect(page.locator('.win.chrome-custom')).toHaveCount(0)
    await expect(page.locator('.tb-winctl')).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Application menu' })).toHaveCount(0)
  })

  test('Settings → Appearance toggle saves useNativeTitleBar with a restart note', async ({ page }) => {
    await bootLinux(page)
    await page.locator('.rail-foot button[aria-label="Settings"]').click()
    await page.locator('.set-nav-item', { hasText: 'Appearance' }).click()
    const tog = page.getByRole('button', { name: 'toggle native title bar and menu' })
    await expect(page.getByText('Takes effect after restarting Recur.')).toBeVisible()
    await expect(tog).toHaveAttribute('aria-pressed', 'false')
    await tog.click()
    await expect(tog).toHaveAttribute('aria-pressed', 'true')
  })
})

test('macOS shows no custom chrome and no Window section in Settings', async ({ page }) => {
  await gotoApp(page)
  await expect(page.locator('.win.chrome-custom')).toHaveCount(0)
  await expect(page.locator('.tb-winctl')).toHaveCount(0)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await page.locator('.set-nav-item', { hasText: 'Appearance' }).click()
  await expect(page.getByRole('button', { name: 'toggle native title bar and menu' })).toHaveCount(0)
})
