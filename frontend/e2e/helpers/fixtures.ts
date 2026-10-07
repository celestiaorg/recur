// Shared test fixtures: every test gets the mocked Wails bridge installed (with the
// real seed fixture) BEFORE it navigates, and a post-test assertion that the page
// raised no uncaught JS errors during the run — a strong, cheap regression signal.

import { test as base, expect, type Page } from '@playwright/test'
import { installBridge, loadSeed, gotoApp, reloadApp, type Seed } from './mock-bridge'

export const seed: Seed = loadSeed()

type Fixtures = {
  // Auto fixture: installs the bridge and tracks uncaught page errors.
  bridge: void
}

export const test = base.extend<Fixtures>({
  bridge: [
    async ({ page }, use) => {
      const errors: string[] = []
      page.on('pageerror', (e) => errors.push(e.message))
      // Per-test clone so mutations (CreateWorkspaceChat, SetSettings, …) cannot
      // leak into the next test on a reused Playwright worker.
      await installBridge(page, structuredClone(seed))
      await use()
      expect(errors, 'the page raised uncaught JS errors').toEqual([])
    },
    { auto: true },
  ],
})

/** Sidebar workspace row for a seed goal. Rows show `short`, so match by id. */
export function sidebarWorkspace(page: Page, goal: string) {
  const loop = seed.loops.find((l: { goal?: string; id?: string }) => l.goal === goal)
  if (loop?.id) {
    return page.locator(`.rail .tloop:not(.tmain)[data-workspace-id="${loop.id}"]`)
  }
  return page.locator('.rail .tloop:not(.tmain)', { hasText: goal })
}

/** Open a seeded workspace from the sidebar project tree (matched by goal). */
export async function openWorkspaceByGoal(page: Page, goal: string) {
  await sidebarWorkspace(page, goal).click()
}


/** Wait until the Agent-panel tab code is written to localStorage (survives gotoApp). */
export async function waitForSavedChatTab(page: Page, workspaceId: string, code: string) {
  await expect.poll(async () => {
    const raw = await page.evaluate(() => localStorage.getItem('recur.activeChatTab.v1'))
    if (!raw) return ''
    try {
      const all = JSON.parse(raw) as Record<string, string>
      return all[workspaceId] ?? ''
    } catch {
      return ''
    }
  }).toBe(code)
}

export { expect, gotoApp, reloadApp }
