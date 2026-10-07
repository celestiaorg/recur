import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

test.describe('workspace target branch', () => {
  test('loop workspaces show name and branch, not the id', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!
    await openWorkspaceByGoal(page, working.goal)

    const crumb = page.locator('.titlebar .crumb')
    await expect(crumb).toContainText(working.short)
    await expect(crumb.locator('.crumb-branch')).toHaveText(working.branch)
    await expect(crumb).not.toContainText(working.id)
    await expect(page.locator('.titlebar .sel-btn[aria-label="Target branch"]')).toBeVisible()
  })

  test('titlebar offers a target-branch picker that can change after creation', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.id === 'LOOP-2229')!
    await openWorkspaceByGoal(page, working.goal)

    const picker = page.locator('.titlebar .sel-btn[aria-label="Target branch"]')
    await expect(picker).toBeVisible()
    // Seeded loops inherit the project default (main) when baseBranch is unset.
    await expect(picker).toContainText('main')

    await picker.click()
    const menu = page.locator('body > .sel-menu')
    await expect(menu).toBeVisible()
    await expect(menu.locator('.sel-opt', { hasText: 'develop' })).toBeVisible()
    await menu.locator('.sel-opt', { hasText: 'develop' }).click()

    await expect(page.locator('.toast.success', { hasText: 'Target branch is now develop' })).toBeVisible()
    await expect(picker).toContainText('develop')
  })

  test('target-branch picker does not refetch on workspace.updated', async ({ page }) => {
    let listCalls = 0
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListBranches: (a, s) => {
          listCalls++
          const p = s.projects.find((x: { id: string }) => x.id === a[0])
          if (!p?.baseBranch) return []
          const names = [p.baseBranch, 'origin/main', 'develop'].filter((b, i, arr) => arr.indexOf(b) === i)
          const remote = names.filter((n) => n.startsWith('origin/')).map((name) => ({ name, group: 'origin' }))
          const local = names.filter((n) => !n.startsWith('origin/')).map((name) => ({ name, group: 'Local' }))
          return [...remote, ...local]
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.id === 'LOOP-2229')!
    await openWorkspaceByGoal(page, working.goal)

    const picker = page.locator('.titlebar .sel-btn[aria-label="Target branch"]')
    await expect(picker).toBeVisible()
    await expect(picker).toBeEnabled()
    await expect(picker).toContainText('main')
    const afterLoad = listCalls

    await page.evaluate((id) => {
      const w = window as any
      w._wails.dispatchWailsEvent({ name: 'workspace.updated', data: { workspaceId: id, diff: { add: 4, del: 1 } } })
      w._wails.dispatchWailsEvent({ name: 'workspace.event', data: { workspaceId: id, event: { text: 'edited a file' } } })
    }, working.id)

    // Wait until the store actually applied workspace.updated (sidebar +N/−M).
    await expect(page.locator('.rail .tloop[data-workspace-id="LOOP-2229"] .df .a')).toHaveText('+4')
    await expect(picker).toBeEnabled()
    await expect(picker).toContainText('main')
    expect(listCalls, 'ListBranches must not re-run when workspace.updated replaces the row').toBe(afterLoad)
  })

  test('target-branch picker stays disabled until the new project\'s branches land', async ({ page }) => {
    let holdPayments = false
    let release = () => {}
    let gate = Promise.resolve()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListBranches: async (a) => {
          if (a[0] === 'payments' && holdPayments) await gate
          if (a[0] === 'payments') return [{ name: 'release', group: 'Local' }, { name: 'main', group: 'Local' }]
          return [{ name: 'main', group: 'Local' }, { name: 'develop', group: 'Local' }, { name: 'feature/auth', group: 'Local' }]
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    const picker = page.locator('.titlebar .sel-btn[aria-label="Target branch"]')
    await expect(picker).toBeEnabled()
    await picker.click()
    await expect(page.locator('body > .sel-menu .sel-opt', { hasText: 'feature/auth' })).toBeVisible()
    await page.keyboard.press('Escape')

    holdPayments = true
    gate = new Promise<void>((r) => { release = r })
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2218')!.goal)
    await expect(picker).toBeDisabled()
    release()
    await expect(picker).toBeEnabled()
    await picker.click()
    const menu = page.locator('body > .sel-menu')
    await expect(menu.locator('.sel-opt', { hasText: 'release' })).toBeVisible()
    await expect(menu.locator('.sel-opt', { hasText: 'feature/auth' })).toHaveCount(0)
  })

  test('switching workspaces in the same project reloads branch choices', async ({ page }) => {
    let extra: { name: string; group: string }[] = []
    let listCalls = 0
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListBranches: () => {
          listCalls++
          return [
            { name: 'main', group: 'Local' },
            { name: 'develop', group: 'Local' },
            ...extra,
          ]
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    const picker = page.locator('.titlebar .sel-btn[aria-label="Target branch"]')
    await expect(picker).toBeEnabled()
    const afterLoad = listCalls
    extra = [{ name: 'feature/new-base', group: 'Local' }]

    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2235')!.goal)
    await expect(picker).toBeEnabled()
    expect(listCalls, 'ListBranches must re-run when switching workspaces in the same project').toBeGreaterThan(afterLoad)
    await picker.click()
    await expect(page.locator('body > .sel-menu .sel-opt', { hasText: 'feature/new-base' })).toBeVisible()
  })

  test('the main working copy has no target-branch picker', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.rail button.tmain').first().click()
    await expect(page.locator('.titlebar .tb-back')).toBeVisible()
    await expect(page.locator('.titlebar .sel-btn[aria-label="Target branch"]')).toHaveCount(0)
  })
})

test.describe('main working copy titlebar git actions', () => {
  async function openMain(page: Parameters<typeof gotoApp>[0]) {
    await gotoApp(page)
    await page.locator('.rail button.tmain').first().click()
    await expect(page.locator('.titlebar .tb-back')).toBeVisible()
  }

  test('shows Commit when the checkout is dirty', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 0, dirty: true, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
      },
    })
    await openMain(page)
    await expect(page.locator('.titlebar button', { hasText: 'Commit' })).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Remove' })).toHaveCount(0)
  })

  test('recalls Commit disabled until git status lands after a switch', async ({ page }) => {
    const storefrontLand = { behindBase: 0, aheadBase: 0, dirty: true, unmerged: false, base: 'origin/main', remote: 'origin' }
    const otherLand = { behindBase: 0, aheadBase: 0, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    let hold = false
    let release = () => {}
    let gate = Promise.resolve()
    const landFor = async (a: unknown[], s: { loopById?: Record<string, { isMain?: boolean; projectId?: string }> }) => {
      if (hold) await gate
      const w = s.loopById?.[String(a[0] ?? '')]
      if (w?.isMain && w.projectId === 'storefront') return storefrontLand
      return otherLand
    }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: (a, s) => landFor(a, s),
        FetchWorkingCopyUpstream: (a, s) => landFor(a, s),
      },
    })
    await openMain(page)
    const commit = page.locator('.titlebar .tb-right button', { hasText: 'Commit' })
    await expect(commit).toBeVisible()
    await expect(commit).toBeEnabled()

    const payments = page.locator('.tproj', { has: page.locator('.pn', { hasText: 'payments-api' }) })
    await payments.locator('button.tloop.tmain').click()
    await expect(commit).toHaveCount(0)

    hold = true
    gate = new Promise<void>((r) => { release = r })
    const storefront = page.locator('.tproj', { has: page.locator('.pn', { hasText: 'storefront' }) })
    await storefront.locator('button.tloop.tmain').click()
    await expect(commit).toBeVisible()
    await expect(commit).toBeDisabled()
    await expect(commit).toHaveAttribute('title', 'Checking git status…')
    release()
    await expect(commit).toBeEnabled()
  })

  test('shows Update from the tracking branch when the remote is ahead', async ({ page }) => {
    const land = { behindBase: 2, aheadBase: 0, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
      },
    })
    await openMain(page)
    await expect(page.locator('.titlebar button', { hasText: 'Update from origin/main' })).toBeVisible()
  })

  test('clears Update after an external git pull (workspace.fs_changed)', async ({ page }) => {
    const land = { behindBase: 2, aheadBase: 0, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => ({ ...land }),
        FetchWorkingCopyUpstream: () => ({ ...land }),
      },
    })
    await openMain(page)
    await expect(page.locator('.titlebar button', { hasText: 'Update from origin/main' })).toBeVisible()

    land.behindBase = 0
    const wsId = await page.locator('.rail button.tmain.on').getAttribute('data-workspace-id')
    await page.evaluate((id) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.fs_changed',
        data: { workspaceId: id },
      })
    }, wsId)
    await expect(page.locator('.titlebar button', { hasText: 'Update from origin/main' })).toHaveCount(0)
  })

  test('shows Push to the remote when local is ahead and clean', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
      },
    })
    await openMain(page)
    const push = page.locator('.titlebar .rbtn.merge', { hasText: 'Push to origin' })
    await expect(push).toBeVisible()
    await push.click()
    await expect(page.locator('.toast.success', { hasText: 'Pushed to origin' })).toBeVisible()
  })

  test('shows Commit and Push together when ahead with leftover dirty files', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: true, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
      },
    })
    await openMain(page)
    await expect(page.locator('.titlebar button', { hasText: 'Commit' })).toBeVisible()
    const push = page.locator('.titlebar .rbtn.merge', { hasText: 'Push to origin' })
    await expect(push).toBeVisible()
    await push.click()
    await expect(page.locator('.toast.success', { hasText: 'Pushed to origin' })).toBeVisible()
  })

  test('Update from origin offers merge and rebase', async ({ page }) => {
    const land = { behindBase: 2, aheadBase: 0, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
      },
    })
    await openMain(page)
    await expect(page.locator('.titlebar button', { hasText: 'Update from origin/main' })).toBeVisible()
    await page.locator('.titlebar .update-split button[aria-label="Choose update method"]').click()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Merge' })).toBeVisible()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Rebase' })).toBeVisible()
    await page.locator('.titlebar .merge-menu-item', { hasText: 'Rebase' }).click()
    await expect(page.locator('.titlebar button', { hasText: 'Rebase onto origin/main' })).toBeVisible()
  })

  test('shows Commit and Update together when behind with leftover dirty files', async ({ page }) => {
    const land = { behindBase: 2, aheadBase: 0, dirty: true, unmerged: false, base: 'origin/main', remote: 'origin' }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
      },
    })
    await openMain(page)
    await expect(page.locator('.titlebar button', { hasText: 'Commit' })).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Update from origin/main' })).toBeVisible()
  })
})

test.describe('workspace GitHub PR titlebar', () => {
  function githubWorkspaceSeed(pr?: { id: string; number: number; source: string; state?: string; group?: string }) {
    const s = structuredClone(seed)
    const proj = s.projects.find((p: { id: string }) => p.id === 'storefront')
    proj.prOnGithub = true
    for (const ws of [s.loops.find((l: { id: string }) => l.id === 'LOOP-2229'), s.loopById['LOOP-2229']]) {
      ws.type = 'agent'
      ws.runStarted = false
      ws.state = ''
    }
    if (pr) {
      const rec = {
        id: pr.id,
        workspaceId: 'LOOP-2229',
        group: pr.group ?? 'you',
        number: pr.number,
        source: pr.source,
        branch: 'recur/pg16',
        base: 'main',
        title: 'Migrate analytics to Postgres 16',
        state: pr.state ?? 'In review',
        diff: { add: 1, del: 0 },
        fileCount: 1,
        checksOk: true,
        when: 'now',
        commits: [],
        files: [],
        comments: [],
      }
      s.prs.push(rec)
      s.prById[pr.id] = rec
    }
    return s
  }

  test('workspace behind its base offers merge/rebase Update and auto-pushes the GitHub PR', async ({ page }) => {
    const land = {
      behindBase: 2, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 0, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
        UpdateWorkspaceWithBase: () => ({
          status: 'updated', workspace: {}, base: 'main', pushedPR: 91, forcePushed: true,
        }),
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Update with main' })).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
    await page.locator('.titlebar .update-split button[aria-label="Choose update method"]').click()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Merge' })).toBeVisible()
    await page.locator('.titlebar .merge-menu-item', { hasText: 'Rebase' }).click()
    const rebase = page.locator('.titlebar .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Rebase onto main' })
    await expect(rebase).toBeVisible()
    await rebase.click()
    await expect(page.locator('.toast.success', { hasText: 'Updated with latest main and force-pushed to PR #91' })).toBeVisible()
  })

  test('clears Update with base after an external git pull (workspace.fs_changed)', async ({ page }) => {
    const land = { behindBase: 2, aheadBase: 1, dirty: false, unmerged: false, base: 'main' }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: {
        WorkspaceLandState: () => ({ ...land }),
        FetchWorkingCopyUpstream: () => ({ ...land }),
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Update with main' })).toBeVisible()

    land.behindBase = 0
    await page.evaluate((id) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.fs_changed',
        data: { workspaceId: id },
      })
    }, 'LOOP-2229')
    await expect(page.locator('.titlebar button', { hasText: 'Update with main' })).toHaveCount(0)
  })

  test('shows Open PR before a pull request exists', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main' }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toBeVisible()
    await expect(page.locator('.titlebar button[aria-label="Open as draft"]')).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'View PR' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'Push updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar .rbtn.merge')).toBeVisible()
  })

  test('hides Merge when GitHub requires a pull request on the remote target', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin', requirePR: true }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .rbtn.merge')).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toBeVisible()
  })

  test('shows Merge on a GitHub-PR remote when GitHub allows a direct land', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin', requirePR: false }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .rbtn.merge')).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toBeVisible()
  })

  test('hides Merge when a background GitHub branch-rules probe starts requiring a PR', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin', requirePR: false }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .rbtn.merge')).toBeVisible()
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'github.branch_rules',
        data: { projectId: 'storefront', branch: 'main', requirePR: true },
      })
    })
    await expect(page.locator('.titlebar .rbtn.merge')).toHaveCount(0)
  })

  test('hides Merge for a slash-containing base branch when GitHub requires a PR', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/release/1.0', remote: 'origin', requirePR: false }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .rbtn.merge')).toBeVisible()
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'github.branch_rules',
        data: { projectId: 'storefront', branch: '1.0', remote: 'origin', requirePR: true },
      })
    })
    await expect(page.locator('.titlebar .rbtn.merge')).toBeVisible()
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'github.branch_rules',
        data: { projectId: 'storefront', branch: 'release/1.0', remote: 'origin', requirePR: true },
      })
    })
    await expect(page.locator('.titlebar .rbtn.merge')).toHaveCount(0)
  })

  test('keeps a branch-rules event when an in-flight land returns a stale requirePR', async ({ page }) => {
    const stale = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin', requirePR: false }
    let releaseFetch: (value: typeof stale) => void
    const fetchP = new Promise<typeof stale>((resolve) => { releaseFetch = resolve })
    let fetchStarted!: () => void
    const started = new Promise<void>((resolve) => { fetchStarted = resolve })
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: {
        WorkspaceLandState: () => stale,
        FetchWorkingCopyUpstream: () => {
          fetchStarted()
          return fetchP
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar .rbtn.merge')).toBeVisible()
    await started
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'github.branch_rules',
        data: { projectId: 'storefront', branch: 'main', remote: 'origin', requirePR: true },
      })
    })
    await expect(page.locator('.titlebar .rbtn.merge')).toHaveCount(0)
    releaseFetch!(stale)
    await expect(page.locator('.titlebar .rbtn.merge')).toHaveCount(0)
  })

  test('keeps a branch-rules event that arrives before the first land resolves', async ({ page }) => {
    const stale = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin', requirePR: false }
    let releaseLand: (value: typeof stale) => void
    const landP = new Promise<typeof stale>((resolve) => { releaseLand = resolve })
    let releaseFetch: (value: typeof stale) => void
    const fetchP = new Promise<typeof stale>((resolve) => { releaseFetch = resolve })
    let landStarted!: () => void
    const landGo = new Promise<void>((resolve) => { landStarted = resolve })
    let fetchStarted!: () => void
    const fetchGo = new Promise<void>((resolve) => { fetchStarted = resolve })
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: {
        WorkspaceLandState: () => {
          landStarted()
          return landP
        },
        FetchWorkingCopyUpstream: () => {
          fetchStarted()
          return fetchP
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await Promise.all([landGo, fetchGo])
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'github.branch_rules',
        data: { projectId: 'storefront', branch: 'main', remote: 'origin', requirePR: true },
      })
    })
    releaseLand!(stale)
    releaseFetch!(stale)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toBeEnabled()
    await expect(page.locator('.titlebar .rbtn.merge')).toHaveCount(0)
  })

  test('Open PR split offers Open draft PR', async ({ page }) => {
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main' }
    await installBridge(page, githubWorkspaceSeed(), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await page.locator('.titlebar button[aria-label="Open as draft"]').click()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Open draft PR' })).toBeVisible()
    await page.locator('.titlebar .merge-menu-item', { hasText: 'Open draft PR' }).click()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Open draft PR' })).toHaveCount(0)
  })

  test('shows View PR after a pull request is open', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 0, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toHaveCount(0)
    const view = page.locator('.titlebar button', { hasText: 'View PR #91' })
    await expect(view).toBeVisible()
    await view.click()
    await expect(page.locator('#v-prdetail')).toBeVisible()
    const workspace = page.locator('.titlebar button', { hasText: 'View workspace' })
    await expect(workspace).toBeVisible()
    await workspace.click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
  })

  test('shows View draft when land.prDraft is set', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', prDraft: true, aheadRemote: 0, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    const viewDraft = page.locator('.titlebar button', { hasText: 'View draft #91' })
    await expect(viewDraft).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toHaveCount(0)
    // Dotted draft glyph (lucide git-pull-request-draft), not the solid PR icon.
    await expect(viewDraft.locator('svg path[d="M18 6V5"]')).toHaveCount(1)
  })

  test('keeps View PR after the GitHub PR is merged', async ({ page }) => {
    const s = githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' })
    for (const ws of [s.loops.find((l: { id: string }) => l.id === 'LOOP-2229'), s.loopById['LOOP-2229']]) {
      ws.state = 'merged'
    }
    const leftover = {
      id: '4', workspaceId: 'LOOP-2229', group: 'you', number: 4, source: 'local',
      branch: 'recur/pg16', base: 'main', title: 'Migrate analytics to Postgres 16',
      state: 'In review', diff: { add: 1, del: 0 }, fileCount: 1, checksOk: true, when: 'now',
      commits: [], files: [], comments: [],
    }
    s.prs.unshift(leftover)
    s.prById['4'] = leftover
    const rec = s.prById['gh-91']
    rec.state = 'Merged'
    rec.group = 'merged'
    const listed = s.prs.find((p: { id: string }) => p.id === 'gh-91')
    listed.state = 'Merged'
    listed.group = 'merged'
    const land = { behindBase: 0, aheadBase: 0, dirty: false, unmerged: false, base: 'main' }
    await installBridge(page, s, {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toHaveCount(0)
  })

  test('shows Push updates next to View PR when HEAD is ahead of the GitHub head', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 2, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
        OpenWorkspacePR: () => 'gh-91',
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
    const push = page.locator('.titlebar button.tbtn.solid', { hasText: 'Push updates' })
    await expect(push).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Pull updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'Force push' })).toHaveCount(0)
    await expect(page.locator('.titlebar button[aria-label="More pull request actions"]')).toHaveCount(0)
    await push.click()
    await expect(page.locator('.toast.success', { hasText: 'Pushed to PR #91' })).toBeVisible()
  })

  test('PR view shows Push updates when HEAD is ahead of the GitHub head', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 2, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
        OpenWorkspacePR: () => 'gh-91',
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await page.locator('.titlebar button', { hasText: 'View PR #91' }).click()
    await expect(page.locator('#v-prdetail')).toBeVisible()
    const push = page.locator('.titlebar .review-actions button.tbtn.solid', { hasText: 'Push updates' })
    await expect(push).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Pull updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'Force push' })).toHaveCount(0)
    await push.click()
    await expect(page.locator('.toast.success', { hasText: 'Pushed to PR #91' })).toBeVisible()
  })

  test('PR view hides Pull / Push / Force push when the GitHub PR is closed', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 2, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh', state: 'Closed' }), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await page.locator('.titlebar button', { hasText: 'View PR #91' }).click()
    await expect(page.locator('#v-prdetail')).toBeVisible()
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Push updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Pull updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Force push' })).toHaveCount(0)
  })

  test('PR view disables Merge while Push updates is in flight', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 2, behindRemote: 0,
    }
    let release = () => {}
    const gate = new Promise<void>((r) => { release = r })
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
        OpenWorkspacePR: async () => { await gate; return 'gh-91' },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await page.locator('.titlebar button', { hasText: 'View PR #91' }).click()
    const push = page.locator('.titlebar .review-actions button.tbtn.solid', { hasText: /Push(ing)?/ })
    const merge = page.locator('.titlebar .review-actions .rbtn.merge')
    await expect(push).toBeEnabled()
    await expect(merge).toBeEnabled()
    await push.click()
    await expect(push).toContainText('Pushing…')
    await expect(merge).toBeDisabled()
    release()
    await expect(page.locator('.toast.success', { hasText: 'Pushed to PR #91' })).toBeVisible()
  })

  test('PR view recalls Push disabled until git status lands', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 2, behindRemote: 0,
    }
    let hold = false
    let release = () => {}
    let gate = Promise.resolve()
    const landFor = async () => {
      if (hold) await gate
      return land
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => landFor(),
        FetchWorkingCopyUpstream: () => landFor(),
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await page.locator('.titlebar button', { hasText: 'View PR #91' }).click()
    const push = page.locator('.titlebar .review-actions button.tbtn.solid', { hasText: 'Push updates' })
    await expect(push).toBeVisible()
    await expect(push).toBeEnabled()

    await page.locator('.titlebar button', { hasText: 'View workspace' }).click()
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()

    hold = true
    gate = new Promise<void>((r) => { release = r })
    await page.locator('.titlebar button', { hasText: 'View PR #91' }).click()
    await expect(push).toBeVisible()
    await expect(push).toBeDisabled()
    await expect(push).toHaveAttribute('title', 'Checking git status…')
    release()
    await expect(push).toBeEnabled()
  })

  test('shows Pull updates next to View PR when GitHub is ahead of HEAD', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 0, behindRemote: 2,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
        PullWorkspacePR: () => ({ status: 'updated', workspace: {}, base: 'origin/recur/pg16' }),
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
    const pull = page.locator('.titlebar button.tbtn.solid', { hasText: 'Pull updates' })
    await expect(pull).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Push updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'Force push' })).toHaveCount(0)
    await pull.click()
    await expect(page.locator('.toast.success', { hasText: 'Pulled updates from PR #91' })).toBeVisible()
  })

  test('hides Open PR when the workspace has no worktree', async ({ page }) => {
    const s = githubWorkspaceSeed()
    for (const ws of [s.loops.find((l: { id: string }) => l.id === 'LOOP-2229'), s.loopById['LOOP-2229']]) {
      ws.worktreePath = ''
      ws.phase = 'failed'
      ws.lastError = 'allocate worktree: bad base'
      ws.statusLabel = "Couldn't create"
    }
    const land = { behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main' }
    await installBridge(page, s, {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toHaveCount(0)
  })

  test('shows View PR and Open PR when the existing PR is only Local', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: '7', prNumber: 7, prSource: 'local', aheadRemote: 0, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: '7', number: 7, source: 'local' }), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'Open PR' })).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'View PR #7' })).toBeVisible()
  })

  test('shows Pull updates and Force push when the GitHub head has diverged', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 2, behindRemote: 2,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: {
        WorkspaceLandState: () => land,
        FetchWorkingCopyUpstream: () => land,
        OpenWorkspacePR: () => 'gh-91',
        PullWorkspacePR: () => ({ status: 'updated', workspace: {}, base: 'origin/recur/pg16' }),
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
    const pull = page.locator('.titlebar button.tbtn.solid', { hasText: 'Pull updates' })
    const force = page.locator('.titlebar button.tbtn.solid', { hasText: 'Force push' })
    await expect(pull).toBeVisible()
    await expect(force).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'Push updates' })).toHaveCount(0)
    await force.click()
    await expect(page.locator('.toast.success', { hasText: 'Force-pushed to PR #91' })).toBeVisible()
  })

  test('hides Pull / Push / Force push when the GitHub PR is already in sync', async ({ page }) => {
    const land = {
      behindBase: 0, aheadBase: 1, dirty: false, unmerged: false, base: 'main',
      prId: 'gh-91', prNumber: 91, prSource: 'gh', aheadRemote: 0, behindRemote: 0,
    }
    await installBridge(page, githubWorkspaceSeed({ id: 'gh-91', number: 91, source: 'gh' }), {
      resolvers: { WorkspaceLandState: () => land, FetchWorkingCopyUpstream: () => land },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, seed.loops.find((l: { id: string }) => l.id === 'LOOP-2229')!.goal)
    await expect(page.locator('.titlebar button', { hasText: 'View PR #91' })).toBeVisible()
    await expect(page.locator('.titlebar button[aria-label="More pull request actions"]')).toHaveCount(0)
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Push updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Pull updates' })).toHaveCount(0)
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Force push' })).toHaveCount(0)
  })
})
