import { test, expect, gotoApp, seed, openWorkspaceByGoal, sidebarWorkspace } from './helpers/fixtures'

test.describe('loop remove', () => {
  test('a settled loop offers Remove; a running loop shows it disabled', async ({ page }) => {
    await gotoApp(page)

    const settled = seed.loops.find((l: any) => l.id === 'LOOP-2224')!
    await openWorkspaceByGoal(page, settled.goal)
    await expect(page.locator('.titlebar .tb-right button', { hasText: 'Remove' })).toBeVisible()
    await expect(page.locator('.titlebar .tb-right button', { hasText: 'Archive' })).toHaveCount(0)
    await expect(page.locator('.titlebar .tb-right button', { hasText: 'Delete' })).toHaveCount(0)

    await page.locator('.titlebar .tb-back').click()
    const running = seed.loops.find((l: any) => l.id === 'LOOP-2229')!
    await openWorkspaceByGoal(page, running.goal)
    const rem = page.locator('.titlebar .tb-right button', { hasText: 'Remove' })
    await expect(rem).toBeVisible()
    await expect(rem).toBeDisabled()
    await expect(rem).toHaveAttribute('title', 'Stop the workspace first')
    await rem.click({ force: true })
    await expect(rem).not.toHaveClass(/armed/)
  })

  test('an armed Remove disarms when an agent turn starts', async ({ page }) => {
    await gotoApp(page)
    const settled = seed.loops.find((l: any) => l.id === 'LOOP-2224')!
    await openWorkspaceByGoal(page, settled.goal)

    const rem = page.locator('.titlebar .tb-right button', { hasText: 'Remove' })
    await rem.click()
    await expect(rem).toHaveClass(/armed/)
    await expect(rem).toHaveText(/Confirm remove/)

    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
    }, { workspaceId: settled.id })

    await expect(rem).toBeDisabled()
    await expect(rem).not.toHaveClass(/armed/)
    await expect(rem).toHaveText(/^Remove$/)
    await expect(rem).toHaveAttribute('title', 'Stop the workspace first')
  })

  test('Remove requires a second click, then hides the loop and returns to inbox', async ({ page }) => {
    await gotoApp(page)
    const settled = seed.loops.find((l: any) => l.id === 'LOOP-2224')!
    await openWorkspaceByGoal(page, settled.goal)

    const rem = page.locator('.titlebar .tb-right button', { hasText: 'Remove' })
    await rem.click()
    await expect(rem).toHaveText(/Confirm remove/)
    await rem.click()

    await expect(page.locator('.toast.success', { hasText: 'Workspace removed' })).toBeVisible()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(sidebarWorkspace(page, settled.goal)).toHaveCount(0)
  })

  test('Remove confirms on a double-click', async ({ page }) => {
    await gotoApp(page)
    const settled = seed.loops.find((l: any) => l.id === 'LOOP-2224')!
    await openWorkspaceByGoal(page, settled.goal)
    await page.locator('.titlebar .tb-right button', { hasText: 'Remove' }).dblclick()
    await expect(page.locator('.toast.success', { hasText: 'Workspace removed' })).toBeVisible()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(sidebarWorkspace(page, settled.goal)).toHaveCount(0)
  })
})
