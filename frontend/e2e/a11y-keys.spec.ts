import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

test.describe('keyboard: Esc-to-back', () => {
  test('Esc from loop detail returns to briefing', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await page.keyboard.press('Escape')
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
  })

  test('Esc from PR detail returns to the PR list', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow').first().click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await page.keyboard.press('Escape')
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
  })

  test('Esc while typing in the Maestro chat box does not navigate away', async ({ page }) => {
    // Idle roster: the seeded 'directing' MA locks the composer (busy gate).
    await installBridge(page, structuredClone(seed), { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-left .chat-in .chat-composer').click()
    await page.keyboard.press('Escape')
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
  })

  test('icon-only sidebar buttons have accessible names', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('.rail-foot button[aria-label="Settings"]')).toBeVisible()
    await expect(page.locator('.rail-foot button[aria-label="Get started"]')).toBeVisible()
    await expect(page.locator('.sa button[aria-label="Add project"]')).toBeVisible()
  })

  test('Add project is not left focused after launch', async ({ page }) => {
    await gotoApp(page)
    const add = page.locator('.sa button[aria-label="Add project"]')
    await expect(add).toBeVisible()
    // gotoApp settles past the boot timer; simulate the window becoming key
    // and focus inside one evaluate so the 500ms arm window cannot expire
    // between Playwright hops.
    await page.evaluate(() => {
      window.dispatchEvent(new Event('blur'))
      window.dispatchEvent(new Event('focus'))
      document.querySelector<HTMLElement>('.sa button[aria-label="Add project"]')!.focus()
    })
    await expect(add).not.toBeFocused()
    await page.evaluate(() => window.dispatchEvent(new Event('pointerdown', { bubbles: true })))
    await add.focus()
    await expect(add).toBeFocused()
  })
})

test.describe('keyboard: new workspace modal', () => {
  test('Tab reaches every field and Enter/arrows change them', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()
    const prompt = modal.locator('.nt-in')
    await expect(prompt).toBeFocused()
    await expect(modal.locator('.sel-btn[aria-label="Model"]')).toBeVisible()
    await expect(modal.locator('.nt-go')).toBeEnabled()

    await page.keyboard.press('Tab')
    const modeBtn = modal.locator('.sel-btn[aria-label="Mode"]')
    await expect(modeBtn).toBeFocused()
    await expect(modeBtn).toHaveClass(/kb-focus/)
    await expect.poll(async () => modeBtn.evaluate((el) => {
      const s = getComputedStyle(el)
      return s.outlineStyle !== 'none' && parseFloat(s.outlineWidth) > 0
    })).toBe(true)
    await page.keyboard.press('Tab')
    await expect(modal.locator('.sel-btn[aria-label="Model"]')).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(modal.locator('.sel-btn[aria-label="Model flags"]')).toBeFocused()
    await page.keyboard.press('Tab')
    const simplifyBtn = modal.locator('.wsv-simplify')
    await expect(simplifyBtn).toBeFocused()
    await expect(simplifyBtn).toHaveClass(/kb-focus/)
    await expect(simplifyBtn).toHaveAttribute('aria-pressed', 'true')
    await page.keyboard.press('Tab')
    await expect(modal.locator('.nt-go')).toBeFocused()
    await expect(modal.locator('.nt-go')).toHaveClass(/kb-focus/)
    await page.keyboard.press('Tab')
    const repoBtn = modal.locator('.sel-btn[aria-label="Repository"]')
    await expect(repoBtn).toBeFocused()
    await expect(repoBtn).toHaveClass(/kb-focus/)
    await page.keyboard.press('Tab')
    await expect(modal.locator('.sel-btn[aria-label="Base branch"]')).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(prompt).toBeFocused()
    await expect(prompt).not.toHaveClass(/kb-focus/)
    await expect.poll(async () => prompt.evaluate((el) => getComputedStyle(el).outlineStyle)).toBe('none')

    await page.keyboard.press('Shift+Tab')
    await expect(modal.locator('.sel-btn[aria-label="Base branch"]')).toBeFocused()
    await page.keyboard.press('Enter')
    await expect(page.locator('body > .sel-menu .sel-search input')).toBeFocused()
    await page.locator('body > .sel-menu .sel-search input').fill('auth')
    await expect(page.locator('body > .sel-menu .sel-opt')).toHaveCount(1)
    await page.keyboard.press('Enter')
    await expect(modal.locator('.sel-btn[aria-label="Base branch"]')).toContainText('feature/auth')
    await expect(modal.locator('.sel-btn[aria-label="Base branch"]')).toBeFocused()

    await modal.locator('.sel-btn[aria-label="Mode"]').focus()
    await page.keyboard.press('Enter')
    await expect(page.locator('body > .sel-menu .sel-opt', { hasText: 'Loop' })).toBeVisible()
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('Enter')
    await expect(modal.locator('.sel-btn[aria-label="Mode"]')).toContainText('Loop')
    await expect(modal.locator('.sel-btn[aria-label="Mode"]')).toBeFocused()

    await page.keyboard.press('Tab')
    await expect(modal.locator('.sel-btn[aria-label="Loop"]')).toBeFocused()
    await page.keyboard.press('Tab')
    await expect(modal.locator('.sel-btn[aria-label="Policy"]')).toBeFocused()
    await page.keyboard.press('Enter')
    await expect(page.locator('body > .sel-menu .sel-opt')).toHaveCount(3)
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('Enter')
    await expect(modal.locator('.sel-btn[aria-label="Policy"]')).toBeFocused()
    await expect(page.locator('body > .sel-menu')).toHaveCount(0)

    await modeBtn.click()
    await page.keyboard.press('Escape')
    await expect(modeBtn).not.toHaveClass(/kb-focus/)
  })

  test('select menus ignore a stationary cursor during keyboard nav', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+N')
    const modal = page.locator('#v-newtask')
    await expect(modal).toBeVisible()

    const gitNames = seed.projects.filter((p) => p.isGit).map((p) => p.name as string)
    expect(gitNames.length).toBeGreaterThan(1)

    const repoBtn = modal.locator('.sel-btn[aria-label="Repository"]')
    const current = (await repoBtn.locator('.sel-val').innerText()).trim()
    expect(gitNames).toContain(current)
    const nextName = gitNames[gitNames.indexOf(current) + 1]
    expect(nextName).toBeTruthy()

    await repoBtn.focus()
    await page.keyboard.press('Enter')
    const menu = page.locator('body > .sel-menu')
    await expect(menu).toBeVisible()
    const last = menu.locator('.sel-opt').last()
    const lastLabel = (await last.locator('.sel-opt-lbl').innerText()).trim()
    expect(lastLabel).not.toBe(current)
    const lastBox = await last.boundingBox()
    expect(lastBox).toBeTruthy()

    await page.keyboard.press('Escape')
    await expect(menu).toHaveCount(0)

    // Park the pointer where a different option will appear, then reopen with
    // the keyboard — a synthetic pointerenter must not steal the highlight.
    await page.mouse.move(lastBox!.x + lastBox!.width / 2, lastBox!.y + lastBox!.height / 2)
    await repoBtn.focus()
    await page.keyboard.press('Enter')
    await expect(menu).toBeVisible()
    const activeOpt = menu.locator('.sel-opt[data-active="true"]')
    await expect(activeOpt.locator('.sel-opt-lbl')).toHaveText(current)
    await expect(last).not.toHaveAttribute('data-active', 'true')
    // CSS :hover must not paint the parked row like the keyboard highlight.
    const lastBg = await last.evaluate((el) => getComputedStyle(el).backgroundColor)
    const activeBg = await activeOpt.evaluate((el) => getComputedStyle(el).backgroundColor)
    expect(lastBg).not.toBe(activeBg)

    await page.keyboard.press('ArrowDown')
    await expect(menu.locator('.sel-opt[data-active="true"] .sel-opt-lbl')).toHaveText(nextName)

    // A real move across the last row (still on that option) should arm hover.
    const lastAfter = await last.boundingBox()
    expect(lastAfter).toBeTruthy()
    const midY = lastAfter!.y + lastAfter!.height / 2
    await page.mouse.move(lastAfter!.x + 8, midY)
    await page.mouse.move(lastAfter!.x + lastAfter!.width - 8, midY)
    await expect(menu.locator('.sel-opt[data-active="true"] .sel-opt-lbl')).toHaveText(lastLabel)

    await page.keyboard.press('Escape')

    // Mode opens upward; same idle-cursor rule.
    const modeBtn = modal.locator('.sel-btn[aria-label="Mode"]')
    const modeCurrent = (await modeBtn.locator('.sel-val').innerText()).trim()
    await modeBtn.focus()
    await page.keyboard.press('Enter')
    await expect(menu).toBeVisible()
    const modeLast = menu.locator('.sel-opt').last()
    const modeLastLabel = (await modeLast.locator('.sel-opt-lbl').innerText()).trim()
    expect(modeLastLabel).not.toBe(modeCurrent)
    const modeLastBox = await modeLast.boundingBox()
    expect(modeLastBox).toBeTruthy()
    await page.keyboard.press('Escape')
    await page.mouse.move(modeLastBox!.x + modeLastBox!.width / 2, modeLastBox!.y + modeLastBox!.height / 2)
    await modeBtn.focus()
    await page.keyboard.press('Enter')
    await expect(menu).toBeVisible()
    await expect(menu.locator('.sel-opt[data-active="true"] .sel-opt-lbl')).toHaveText(modeCurrent)

    await page.keyboard.press('Escape')

    // Searchable menus must keep the gate armed across consecutive row
    // crossings. Assert after *each* jump: the menu may open with active ≠ 0,
    // so the dropped crossing (pre-fix) is whichever jump follows the first
    // `active` write — only checking the last row can pass both implementations.
    const branchBtn = modal.locator('.sel-btn[aria-label="Base branch"]')
    await branchBtn.click()
    await expect(menu).toBeVisible()
    const branchOpts = menu.locator('.sel-opt')
    expect(await branchOpts.count()).toBeGreaterThanOrEqual(3)
    const box0 = await branchOpts.nth(0).boundingBox()
    const box1 = await branchOpts.nth(1).boundingBox()
    const box2 = await branchOpts.nth(2).boundingBox()
    expect(box0 && box1 && box2).toBeTruthy()
    const row1Label = (await branchOpts.nth(1).locator('.sel-opt-lbl').innerText()).trim()
    const row2Label = (await branchOpts.nth(2).locator('.sel-opt-lbl').innerText()).trim()
    const y0 = box0!.y + box0!.height / 2
    await page.mouse.move(box0!.x + 8, y0)
    await page.mouse.move(box0!.x + 18, y0)
    await page.mouse.move(box1!.x + box1!.width / 2, box1!.y + box1!.height / 2)
    await expect(menu.locator('.sel-opt[data-active="true"] .sel-opt-lbl')).toHaveText(row1Label)
    await page.mouse.move(box2!.x + box2!.width / 2, box2!.y + box2!.height / 2)
    await expect(menu.locator('.sel-opt[data-active="true"] .sel-opt-lbl')).toHaveText(row2Label)
  })
})

test.describe('keyboard: new blank workspace', () => {
  test('⇧⌘N creates a blank Agent workspace without the modal', async ({ page }) => {
    const creates: unknown[][] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        CreateWorkspace: (a) => {
          creates.push(a)
          return {
            id: 'WS-NEW',
            projectId: a[0],
            type: 'solo',
            goal: 'New Workspace',
            short: 'New Workspace',
            statusLabel: 'Creating',
            phase: 'creating',
            branch: 'recur/new',
            baseBranch: a[2] || 'main',
          }
        },
      },
    })
    await gotoApp(page)
    await page.keyboard.press('Shift+Meta+N')
    await expect(page.locator('#v-newtask')).toHaveCount(0)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('#v-detail')).not.toHaveClass(/on/)
    await expect.poll(() => creates.length).toBe(1)
    expect(creates[0][3]).toBe('agent')
    expect(creates[0][1]).toBe('')
    expect(creates[0][4]).toBe('')
    await page.locator('.rail .tloop[data-workspace-id="WS-NEW"]').click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .sel-btn[aria-label="Chat mode"]')).toContainText('Agent')
  })
})
