import { test, expect, gotoApp, reloadApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'
import { SCAN_COMMIT_MS } from '../src/lib/workspace-nav'

test.describe('app shell', () => {
  test('boots into the briefing with seeded data', async ({ page }) => {
    await gotoApp(page)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    // Seeded workspaces appear in the sidebar project tree (not pre-selected).
    await expect(page.locator('.rail .tloop:not(.tmain)')).toHaveCount(seed.loops.length)
    await expect(page.locator('.tloop.on')).toHaveCount(0)
  })

  test('sidebar flags a workspace waiting on the user', async ({ page }) => {
    await gotoApp(page)
    const paused = seed.loops.find((l) => l.state === 'escalated')!
    const row = page.locator(`.rail .tloop[data-workspace-id="${paused.id}"]`)
    await expect(row.locator('.ct')).toHaveText('1')
    await expect(row.locator('.ct')).toHaveAttribute('title', 'Needs you')
    await expect(page.locator('.rail-stat.needs')).toContainText('needs you')
  })

  test('sidebar flags retryable failed and interrupted workspaces as needs you', async ({ page }) => {
    const s = structuredClone(seed)
    const failed = { ...s.loopById['LOOP-2218'], state: 'failed', group: 'needs', statusLabel: 'Failed' }
    s.loops = s.loops.map((l) => l.id === 'LOOP-2218' ? failed : l)
    s.loopById['LOOP-2218'] = failed
    const cut = {
      id: 'LOOP-cut', projectId: 'payments', group: 'needs', state: 'paused', phase: 'interrupted',
      goal: 'Interrupted recover', short: 'Interrupted recover', archived: false, isMain: false, diff: { add: 0, del: 0 },
    }
    s.loops.push(cut)
    s.loopById[cut.id] = cut
    await installBridge(page, s)
    await gotoApp(page)
    await expect(page.locator('.rail-stat.needs')).toContainText('2 need you')
    await expect(page.locator('.rail .tloop[data-workspace-id="LOOP-2218"] .ct')).toHaveAttribute('title', 'Needs you')
    await expect(page.locator('.rail .tloop[data-workspace-id="LOOP-cut"] .ct')).toHaveAttribute('title', 'Needs you')
  })

  test('collapsed group does not badge a hidden PR chat host waiting on the user', async ({ page }) => {
    const s = structuredClone(seed)
    s.projectGroups = [{ id: 'g-sites', name: 'Sites', open: false, createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z' }]
    s.projects = s.projects.map((p) => p.id === 'marketing' ? { ...p, groupId: 'g-sites' } : p)
    const host = {
      id: 'ws-pr-host', projectId: 'marketing', hostPrId: 'pr-host', type: 'solo',
      goal: 'PR chat', short: 'PR chat', archived: false, isMain: false, state: 'paused',
      diff: { add: 0, del: 0 },
    }
    s.loops.push(host)
    s.loopById[host.id] = host
    await installBridge(page, s, {
      resolvers: {
        ListPendingQuestions: () => [{
          id: 'q-host', workspaceId: 'ws-pr-host', agentCode: 'MA', question: 'Ship it?', askedAgo: '1m',
        }],
      },
    })
    await gotoApp(page)
    const group = page.locator('.tgroup[data-group-id="g-sites"]')
    await expect(group).toHaveClass(/collapsed/)
    await expect(group.locator('.tgroup-h .ct')).toHaveCount(0)
    await group.locator('.tgroup-h').click()
    await expect(group).not.toHaveClass(/collapsed/)
    await expect(group.locator('.tproj .ct')).toHaveCount(0)
    await expect(page.locator('.tloop[data-workspace-id="ws-pr-host"]')).toHaveCount(0)
  })

  test('restores the last open workspace after a restart', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('.tloop.on')).toContainText(working.short)

    await reloadApp(page)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('.tloop.on')).toContainText(working.short)
  })

  test('parks the previous workspace shell outside #v-detail', async ({ page }) => {
    await gotoApp(page)
    const idle = new Set(['pr_open', 'merged', 'paused', 'escalated', 'failed', 'stopped'])
    const a = seed.loops.find((l) => !l.archived && !l.isMain && idle.has(l.state))
    const b = seed.loops.find((l) => !l.archived && !l.isMain && idle.has(l.state) && l.id !== a?.id)
    if (!a || !b) throw new Error('need two idle seeded workspaces')
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await expect(page.locator('#v-detail .obs')).toHaveCount(1)
    await openWorkspaceByGoal(page, b.goal)
    await expect(page.locator('#v-detail .obs')).toHaveCount(1)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', b.id)
    await expect(page.locator('#wsv-park .wsv-shell')).toHaveCount(1)
    await expect(page.locator('#wsv-park .wsv-shell')).toHaveAttribute('data-workspace-id', a.id)
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', a.id)
    await expect(page.locator('#wsv-park .wsv-shell')).toHaveAttribute('data-workspace-id', b.id)
    await expect(page.locator('#v-detail .obs')).toHaveCount(1)

    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('#v-detail')).not.toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs')).toHaveCount(1)
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', a.id)
  })

  test('stays on briefing after a restart if that was the last view', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)

    await reloadApp(page)
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('.tloop.on')).toHaveCount(0)
  })

  test('Inbox and PR dashboard do not keep the last workspace row selected', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('.tloop.on')).toContainText(working.short)

    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect(page.locator('.nav.inbox-nav')).toHaveClass(/on/)
    await expect(page.locator('.tloop.on')).toHaveCount(0)

    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('.tloop.on')).toContainText(working.short)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await expect(page.locator('.nav', { hasText: 'Pull requests' })).toHaveClass(/on/)
    await expect(page.locator('.tloop.on')).toHaveCount(0)
  })

  test('sidebar shows live PR count', async ({ page }) => {
    await gotoApp(page)
    const inboxBadge = page.locator('.nav.inbox-nav .ct')
    const prBadge = page.locator('.nav', { hasText: 'Pull requests' }).locator('.ct')
    // PR count badge is data-driven (was hardcoded "3" before).
    await expect(prBadge).toHaveText(String(seed.prs.length))

    const [inboxChip, prChip] = await Promise.all([
      inboxBadge.evaluate((el) => {
        const s = getComputedStyle(el)
        return { radius: s.borderRadius, pad: s.paddingLeft, weight: s.fontWeight }
      }),
      prBadge.evaluate((el) => {
        const s = getComputedStyle(el)
        return {
          radius: s.borderRadius,
          pad: s.paddingLeft,
          weight: s.fontWeight,
          bg: s.backgroundColor,
        }
      }),
    ])
    expect(prChip.radius).toBe(inboxChip.radius)
    expect(prChip.pad).toBe(inboxChip.pad)
    expect(prChip.weight).toBe(inboxChip.weight)
    expect(prChip.bg).not.toBe('rgba(0, 0, 0, 0)')
  })

  test('sidebar usage meter opens a popover with quota bars', async ({ page }) => {
    const usage = {
      fetchedAt: Date.now(),
      stale: false,
      providers: [
        {
          id: 'claude-code',
          name: 'Claude',
          email: 'dev@example.com',
          source: 'Claude Code login',
          sourceId: 'claude_login',
          windows: [
            { id: 'five_hour', label: '5-hour', usedPct: 12 },
            { id: 'seven_day', label: '7-day', usedPct: 34 },
            { id: 'seven_day_fable', label: 'Fable (7-day)', usedPct: 41 },
          ],
        },
        {
          id: 'codex',
          name: 'Codex',
          email: 'codex@example.com',
          source: 'ChatGPT login',
          sourceId: 'chatgpt',
          windows: [
            { id: 'five_hour', label: '5-hour', usedPct: 100 },
            { id: 'seven_day', label: '7-day', usedPct: 16 },
            { id: 'billing', label: 'Monthly', usedPct: 100, detail: '500 of 500 credits' },
          ],
        },
        {
          id: 'cursor',
          name: 'Cursor',
          error: 'No Cursor usage session',
          hint: 'Connect in Settings.',
        },
      ],
    }
    await installBridge(page, seed, {
      resolvers: { GetProviderUsage: () => usage, RefreshProviderUsage: () => usage },
    })
    await gotoApp(page)
    await expect(page.locator('.rail-foot .use-chip')).toHaveCount(2)
    await page.locator('.rail-foot .use-btn').click()
    const pop = page.locator('.use-pop')
    await expect(pop).toBeVisible()
    await expect(pop.locator('.use-prov-name', { hasText: 'Claude' })).toBeVisible()
    await expect(pop.getByText('Fable (7-day)')).toBeVisible()
    await expect(pop.getByText('34.0%')).toBeVisible()
    await expect(pop.getByText('41.0%')).toBeVisible()
    await expect(pop.locator('.use-prov-name', { hasText: 'Codex' })).toBeVisible()
    await expect(pop.getByText('Monthly')).toBeVisible()
    await expect(pop.getByText('500 of 500 credits')).toBeVisible()
    await expect(pop.getByText('Connect Cursor in Settings')).toBeVisible()
    await expect(pop.getByText('dev@example.com')).toBeVisible()
  })

  test('sidebar uses a PR icon only on workspaces that have a pull request', async ({ page }) => {
    await gotoApp(page)
    // Skip in-flight loops — the amber running dot replaces the glyph.
    const idle = new Set(['pr_open', 'merged', 'paused', 'escalated', 'failed', 'stopped'])
    const withPR = seed.prs.find((p) => {
      const loop = seed.loops.find((l) => l.id === p.workspaceId)
      return !!loop && !loop.archived && idle.has(loop.state)
    })
    const withoutPR = seed.loops.find(
      (l) => !l.archived && !l.isMain && idle.has(l.state) && !seed.prs.some((p) => p.workspaceId === l.id),
    )
    expect(withPR, 'seed should include an idle workspace-linked PR').toBeTruthy()
    expect(withoutPR, 'seed should include an idle workspace without a PR').toBeTruthy()

    await expect(page.locator(`.rail .tloop[data-workspace-id="${withPR.workspaceId}"] svg.br`)).toHaveClass(
      /pr-(open|merged|closed)/,
    )
    await expect(page.locator(`.rail .tloop[data-workspace-id="${withoutPR.id}"] svg.br`)).not.toHaveClass(
      /pr-(open|merged|closed)/,
    )
  })

  test('sidebar uses the merged PR icon on a merged workspace', async ({ page }) => {
    const s = structuredClone(seed)
    const ws = s.loops.find((l: { id: string }) => l.id === 'LOOP-2229')
    const byId = s.loopById['LOOP-2229']
    for (const row of [ws, byId]) {
      row.state = 'merged'
      row.type = 'agent'
      row.runStarted = false
    }
    const leftover = {
      id: '4', workspaceId: 'LOOP-2229', group: 'you', number: 4, source: 'local',
      branch: 'recur/pg16', base: 'main', title: 'Migrate analytics to Postgres 16',
      state: 'In review', diff: { add: 1, del: 0 }, fileCount: 1, checksOk: true, when: 'now',
      commits: [], files: [], comments: [],
    }
    const rec = {
      id: 'gh-91', workspaceId: 'LOOP-2229', group: 'merged', number: 91, source: 'gh',
      branch: 'recur/pg16', base: 'main', title: 'Migrate analytics to Postgres 16',
      state: 'Merged', diff: { add: 1, del: 0 }, fileCount: 1, checksOk: true, when: 'now',
      commits: [], files: [], comments: [],
    }
    s.prs.push(leftover, rec)
    s.prById['4'] = leftover
    s.prById['gh-91'] = rec
    await installBridge(page, s)
    await gotoApp(page)
    const icon = page.locator('.rail .tloop[data-workspace-id="LOOP-2229"] svg.br')
    await expect(icon).toHaveClass(/pr-merged/)
    // git-merge has three commit dots; git-pull-request has two.
    await expect(icon.locator('circle')).toHaveCount(3)
  })

  test('sidebar workspace hover shows View PR and Remove', async ({ page }) => {
    await gotoApp(page)
    const row = page.locator('.rail .tloop[data-workspace-id="LOOP-2224"]')
    await expect(row.locator('.tloop-acts')).toBeHidden()
    await row.hover()
    await expect(row.locator('.tloop-acts')).toBeVisible()
    await expect(row.locator('.lact-pr')).toHaveAttribute('aria-label', 'View PR #479')
    await expect(row.locator('.lact-rm')).toHaveAttribute('aria-label', 'Remove')
    await row.locator('.lact-pr').click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(page.locator('#v-prdetail .prh-t .num')).toContainText('#479')
  })

  test('sidebar hover Remove archives a settled workspace after confirm', async ({ page }) => {
    await gotoApp(page)
    const row = page.locator('.rail .tloop[data-workspace-id="LOOP-2224"]')
    await row.hover()
    const rm = row.locator('.lact-rm')
    await rm.click()
    await expect(rm).toHaveClass(/armed/)
    // Confirm fill must win while the pointer is still on the icon (`:hover`
    // is more specific than `.armed` unless hover excludes that class).
    await expect(rm).toHaveCSS('color', 'rgb(10, 12, 16)')
    // Click dismisses the "Remove" tip; sitting still still shows confirm copy.
    await expect(page.locator('.ui-tip.ready')).toContainText('Confirm remove')
    await rm.click()
    await expect(page.locator('.rail .tloop[data-workspace-id="LOOP-2224"]')).toHaveCount(0)
  })

  test('sidebar hover Remove stays disabled on a running workspace', async ({ page }) => {
    await gotoApp(page)
    const row = page.locator('.rail .tloop[data-workspace-id="LOOP-2229"]')
    await row.hover()
    const rm = row.locator('.lact-rm')
    await expect(rm).toBeVisible()
    await expect(rm).toBeDisabled()
    await expect(row.locator('.lact-pr')).toHaveCount(0)
    await rm.click({ force: true })
    await expect(rm).not.toHaveClass(/armed/)
    await expect(row).toBeVisible()
  })

  test('sidebar child tree line meets the parent icon', async ({ page }) => {
    const s = structuredClone(seed)
    const child = s.loops.find((l: { id: string }) => l.id === 'LOOP-2224')
    const childById = s.loopById['LOOP-2224']
    for (const row of [child, childById]) {
      row.parentId = 'LOOP-2229'
    }
    await installBridge(page, s)
    await gotoApp(page)

    const childRow = page.locator('.rail .tloop[data-workspace-id="LOOP-2224"]')
    await expect(childRow).toHaveClass(/tchild/)

    const geo = await page.evaluate(() => {
      const parent = document.querySelector('.rail .tloop[data-workspace-id="LOOP-2229"]') as HTMLElement
      const child = document.querySelector('.rail .tloop[data-workspace-id="LOOP-2224"]') as HTMLElement
      const parentIcon = parent.querySelector('.sd, .br') as HTMLElement
      const childIcon = child.querySelector('.sd, .br') as HTMLElement
      const hit = child.querySelector('.tloop-hit') as HTMLElement
      const parentBox = parent.getBoundingClientRect()
      const childBox = child.getBoundingClientRect()
      const stem = getComputedStyle(parent, '::after')
      const rail = getComputedStyle(child, '::after')
      const tick = getComputedStyle(hit, '::before')
      return {
        iconBottom: parentIcon.getBoundingClientRect().bottom,
        stemTop: parentBox.top + parseFloat(stem.top),
        stemBottom: parentBox.bottom - parseFloat(stem.bottom),
        railTop: childBox.top + parseFloat(rail.top),
        stemLeft: parentBox.left + parseFloat(stem.left),
        railLeft: childBox.left + parseFloat(rail.left),
        tickRight: hit.getBoundingClientRect().left + parseFloat(tick.left) + parseFloat(tick.width),
        childIconLeft: childIcon.getBoundingClientRect().left,
      }
    })
    expect(geo.stemTop).toBeCloseTo(geo.iconBottom, 0)
    expect(geo.stemBottom).toBeCloseTo(geo.railTop, 0)
    expect(geo.stemLeft).toBeCloseTo(geo.railLeft, 0)
    expect(geo.tickRight).toBeCloseTo(geo.childIconLeft, 0)
  })

  test('working copy row has no hover actions', async ({ page }) => {
    await gotoApp(page)
    const row = page.locator('.rail .tproj[data-project-id="storefront"] button.tloop.tmain')
    await row.hover()
    await expect(row.locator('.tloop-acts')).toHaveCount(0)
  })

  test('collapsed sidebar flies out from the left edge', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.tb-rail').click()
    await expect(page.locator('.body')).toHaveClass(/rail-collapsed/)
    await expect(page.locator('.rail')).not.toHaveClass(/is-peek/)
    await expect(page.locator('.rail-hotzone')).toBeVisible()
    // Docked content column still fills the window — the rail is an overlay.
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)

    const body = page.locator('.body')
    const box = await body.boundingBox()
    expect(box).toBeTruthy()
    await page.mouse.move(box!.x + 4, box!.y + 80)
    await expect(page.locator('.rail')).toHaveClass(/is-peek/)
    await expect(page.locator('.rail .nav.inbox-nav')).toBeVisible()

    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('.rail')).toHaveClass(/is-peek/)

    await page.locator('.main').hover({ position: { x: 420, y: 120 } })
    await expect(page.locator('.rail')).not.toHaveClass(/is-peek/)
  })

  test('collapsed sidebar flyout dismisses with Escape without leaving the view', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await page.locator('.tb-rail').click()
    const body = page.locator('.body')
    const box = await body.boundingBox()
    expect(box).toBeTruthy()
    await page.mouse.move(box!.x + 4, box!.y + 80)
    await expect(page.locator('.rail')).toHaveClass(/is-peek/)

    await page.keyboard.press('Escape')
    await expect(page.locator('.rail')).not.toHaveClass(/is-peek/)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
  })

  test('sidebar overlay scrollbar stacks above sticky tree headers', async ({ page }) => {
    await gotoApp(page)
    const tree = page.locator('.rail-tree')
    await expect(tree).toBeVisible()
    // Force overflow so the custom thumb is meaningful; stacking still applies either way.
    await tree.evaluate((el) => {
      el.scrollTop = el.scrollHeight
    })
    const layers = await page.evaluate(() => {
      const z = (sel: string) => {
        const el = document.querySelector(sel)
        if (!el) return null
        const raw = getComputedStyle(el).zIndex
        return raw === 'auto' ? 0 : Number(raw)
      }
      return { sb: z('.rail-sb'), group: z('.tgroup-h'), proj: z('.tproj-h') }
    })
    expect(layers.sb).not.toBeNull()
    expect(layers.proj).not.toBeNull()
    expect(layers.sb!).toBeGreaterThan(layers.proj!)
    if (layers.group != null) expect(layers.sb!).toBeGreaterThan(layers.group)
  })

  test('project tree expand/collapse follows open and toggles', async ({ page }) => {
    await gotoApp(page)
    const openProj = seed.projects.find((p) => p.open)!
    const closedProj = seed.projects.find((p) => !p.open)!
    const openNode = page.locator('.tproj', { has: page.locator('.pn', { hasText: openProj.name }) })
    const closedNode = page.locator('.tproj', { has: page.locator('.pn', { hasText: closedProj.name }) })

    await expect(openNode).not.toHaveClass(/collapsed/)
    await expect(closedNode).toHaveClass(/collapsed/)

    // Hover reveals the caret in the icon slot (name/add stopPropagation for their own actions).
    const header = openNode.locator('.tproj-h')
    await header.hover()
    await header.locator('.car').click()
    await expect(openNode).toHaveClass(/collapsed/)
    await header.hover()
    await header.locator('.car').click()
    await expect(openNode).not.toHaveClass(/collapsed/)
  })

  test('right-click hides a project and the eye toggle reveals it', async ({ page }) => {
    await gotoApp(page)
    const proj = seed.projects[0]
    const node = page.locator(`.tproj[data-project-id="${proj.id}"]`)
    const toggle = page.locator('.sec .sa button[aria-pressed]')
    await expect(toggle).toHaveCount(0)

    await node.locator('.tproj-h').click({ button: 'right' })
    await page.locator('.rail-move-menu').getByRole('menuitem', { name: 'Hide project' }).click()
    await expect(node).toHaveCount(0)

    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-pressed', 'true')
    await expect(node).toHaveClass(/is-hidden/)

    await node.locator('.tproj-h').click({ button: 'right' })
    await page.locator('.rail-move-menu').getByRole('menuitem', { name: 'Unhide project' }).click()
    await expect(node).not.toHaveClass(/is-hidden/)
    await toggle.click()
    await expect(toggle).toHaveCount(0)
    await expect(node).toBeVisible()
  })

  test('boots with saved hidden projects and show-hidden setting', async ({ page }) => {
    const s = structuredClone(seed)
    const [shown, hidden] = s.projects
    s.projects = s.projects.map((p) => (p.id === hidden.id ? { ...p, hidden: true } : p))
    s.settings = { ...s.settings, showHiddenProjects: true }
    await installBridge(page, s)
    await gotoApp(page)
    const toggle = page.locator('.sec .sa button[aria-pressed]')
    const node = page.locator(`.tproj[data-project-id="${hidden.id}"]`)
    await expect(toggle).toHaveAttribute('aria-pressed', 'true')
    await expect(node).toHaveClass(/is-hidden/)
    await expect(page.locator(`.tproj[data-project-id="${shown.id}"]`)).not.toHaveClass(/is-hidden/)

    await node.locator('.tproj-h').hover()
    await node.getByRole('button', { name: `Unhide ${hidden.name}`, exact: true }).click()
    await expect(node).not.toHaveClass(/is-hidden/)
    await toggle.click()
    await expect(toggle).toHaveCount(0)
    await expect(node).toBeVisible()
  })

  test('project settings toggle hides the project from the sidebar', async ({ page }) => {
    await gotoApp(page)
    const proj = seed.projects[0]
    const node = page.locator(`.tproj[data-project-id="${proj.id}"]`)
    await node.locator('.tproj-h').hover()
    await node.getByRole('button', { name: `Project settings for ${proj.name}`, exact: true }).click()
    const tog = page.getByRole('button', { name: 'toggle hide from sidebar' })
    await tog.click()
    await expect(tog).toHaveAttribute('aria-pressed', 'true')
    await expect(node).toHaveCount(0)
    await tog.click()
    await expect(node).toBeVisible()
  })

  test('dragging a project onto a group highlights the whole group', async ({ page }) => {
    const s = structuredClone(seed)
    s.projectGroups = [
      { id: 'g-apps', name: 'Apps', open: true, createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z' },
      { id: 'g-sites', name: 'Sites', open: true, createdAt: '2026-01-01T00:00:00Z', updatedAt: '2026-01-01T00:00:00Z' },
    ]
    s.projects = s.projects.map((p) => {
      if (p.id === 'payments') return { ...p, groupId: 'g-apps' }
      if (p.id === 'marketing') return { ...p, groupId: 'g-sites' }
      return p
    })
    await installBridge(page, s)
    await gotoApp(page)

    const group = page.locator('.tgroup[data-group-id="g-apps"]')
    const other = page.locator('.tgroup[data-group-id="g-sites"]')
    const source = page.locator('.tproj[data-project-id="storefront"] .tproj-h')
    await expect(group).toBeVisible()
    await expect(source).toBeVisible()

    const from = await source.boundingBox()
    const to = await group.locator('.tgroup-h').boundingBox()
    expect(from && to).toBeTruthy()
    await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2)
    await page.mouse.down()
    await page.mouse.move(to!.x + to!.width / 2, to!.y + to!.height / 2, { steps: 12 })

    await expect(group).toHaveClass(/drop-target/)
    await expect(other).not.toHaveClass(/drop-target/)
    const slot = group.locator('.tproj-ph')
    await expect(slot).toBeVisible()

    const look = await group.evaluate((el) => {
      const ring = getComputedStyle(el, '::after')
      const overlay = getComputedStyle(el, '::before')
      const header = el.querySelector('.tgroup-h')
      const proj = el.querySelector('.tproj-h')
      const ph = el.querySelector('.tproj-ph')
      if (!header || !proj || !ph) return null
      const phCss = getComputedStyle(ph)
      return {
        ringShadow: ring.boxShadow,
        ringLeft: ring.left,
        ringRight: ring.right,
        overlayContent: overlay.content,
        headerBg: getComputedStyle(header).backgroundColor,
        projBg: getComputedStyle(proj).backgroundColor,
        slotBg: phCss.backgroundColor,
        slotOutline: phCss.outlineStyle,
        headerCoverLeft: getComputedStyle(header, '::before').left,
      }
    })
    expect(look).not.toBeNull()
    expect(look!.ringShadow).not.toBe('none')
    expect(look!.ringLeft).toBe('9px')
    expect(look!.ringRight).toBe('9px')
    expect(look!.overlayContent === 'none' || look!.overlayContent === 'normal').toBeTruthy()
    expect(look!.headerBg).toBe(look!.slotBg)
    expect(look!.projBg).toBe(look!.slotBg)
    expect(look!.slotOutline).toBe('dashed')
    expect(look!.headerCoverLeft).toBe('0px')

    await page.mouse.up()
  })

  test('opens a working loop into the unified workspace view', async ({ page }) => {
    await gotoApp(page)
    // A working loop routes to the unified detail view (escalated loops do too —
    // the question is an inline Loop-chat card, not a separate screen).
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    // One view: the Observatory; the chat dock leads with the Loop tab.
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await expect(page.locator('#v-detail .obs-left .wsv-chat-tab', { hasText: 'Loop' })).toBeVisible()
  })

  test('opens an escalated loop into Loop chat with an inline question', async ({ page }) => {
    await installBridge(page, seed, {
      resolvers: {
        GetEscalation: () => ({
          ...seed.escByLoop['LOOP-2218'],
          context: [
            'The ticket does not define over-limit behavior.',
            '',
            '1. Reject with `429`',
            '2. Queue and delay',
            '3. Token bucket with refill',
          ].join('\n'),
        }),
      },
    })
    await gotoApp(page)
    const paused = seed.loops.find((l) => l.state === 'escalated')!
    await openWorkspaceByGoal(page, paused.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-escalation')).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-left .wsv-chat-tab', { hasText: 'Loop' })).toBeVisible()
    const card = page.locator('#v-detail .perm.inline')
    await expect(card.locator('.perm-kicker')).toHaveText('Maestro needs a decision')
    await expect(card.locator('.ask-q')).toContainText('rate limit')
    await expect(card.locator('.perm-sum li')).toHaveCount(3)
    await expect(card.locator('.perm-sum code')).toHaveText('429')
    await card.locator('.ask-opt', { hasText: 'Token bucket' }).click()
    await expect(page.locator('#v-detail .perm.inline')).toHaveCount(0)
    const me = page.locator('#v-detail .chat-row.me')
    await expect(me.first()).toBeVisible()
    await expect(me.first()).toContainText(paused.goal)
    await expect(me.last()).toBeVisible()
    await expect(me.last()).toContainText('Token bucket')
  })

  test('plan-approval notes option waits for the text box', async ({ page }) => {
    await installBridge(page, seed, {
      resolvers: {
        GetEscalation: () => ({
          id: 'esc-plan',
          workspaceId: seed.loops.find((l) => l.state === 'escalated')!.id,
          question: 'Review the plan before I start coding.',
          context: '1. Create go.mod\n2. Add the game loop',
          status: 'open',
          options: [
            {
              text: 'Approve and proceed',
              tag: 'recommended',
              desc: 'Start coding to this plan as written.',
            },
            {
              text: 'Proceed with my notes',
              tag: 'notes',
              desc: 'Add guidance in the text box — I fold it into the plan and start.',
            },
          ],
        }),
      },
    })
    await gotoApp(page)
    const paused = seed.loops.find((l) => l.state === 'escalated')!
    await openWorkspaceByGoal(page, paused.goal)
    const card = page.locator('#v-detail .perm.inline')
    await expect(card.locator('.ask-q')).toContainText('Review the plan')
    await card.locator('.ask-opt', { hasText: 'Proceed with my notes' }).click()
    await expect(card).toBeVisible()
    await expect(card.locator('.ask-opt', { hasText: 'Proceed with my notes' })).toHaveClass(/sel/)
    await expect(card.locator('.perm-btn.go')).toBeDisabled()
    await card.locator('.ask-notes').fill('use ncurses for the board')
    await card.locator('.perm-btn.go').click()
    await expect(page.locator('#v-detail .perm.inline')).toHaveCount(0)
    const me = page.locator('#v-detail .chat-row.me')
    await expect(me.first()).toBeVisible()
    await expect(me.first()).toContainText(paused.goal)
    await expect(me.last()).toBeVisible()
    await expect(me.last()).toContainText('use ncurses')
  })

  test('go-to palette opens on ⌘K and searches', async ({ page }) => {
    await gotoApp(page)
    // Titlebar has no search field — ⌘K / Navigate → Go to… are the openers.
    await expect(page.locator('.titlebar .cmd')).toHaveCount(0)
    await page.keyboard.press('Meta+k')
    // Shortcuts / Run Command share .palette — scope to the go-to palette.
    const pal = page.locator('.palette:not(.shortcuts-panel):not(.cmd-palette)')
    await expect(pal).toBeVisible()
    await pal.locator('.pal-in input').fill('pull')
    await expect(pal.locator('.pal-row').first()).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.locator('.palette-bg').first()).not.toHaveClass(/open/)
  })

  test('go-to palette scrolls selection into view with arrow keys', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+k')
    const pal = page.locator('.palette:not(.shortcuts-panel):not(.cmd-palette)')
    await expect(pal).toBeVisible()
    const list = pal.locator('.pal-list')
    const rows = pal.locator('.pal-row')
    const count = await rows.count()
    expect(count).toBeGreaterThan(8)

    const overflows = await list.evaluate((el) => el.scrollHeight > el.clientHeight + 1)
    expect(overflows).toBe(true)

    for (let i = 0; i < count - 1; i++) {
      await page.keyboard.press('ArrowDown')
    }

    const sel = pal.locator('.pal-row.sel')
    await expect(sel).toHaveCount(1)
    const inListView = await page.evaluate(() => {
      const listEl = document.querySelector(
        '.palette:not(.shortcuts-panel):not(.cmd-palette) .pal-list',
      )
      const selEl = listEl?.querySelector('.pal-row.sel')
      if (!listEl || !selEl) return false
      const lr = listEl.getBoundingClientRect()
      const sr = selEl.getBoundingClientRect()
      return sr.top >= lr.top - 1 && sr.bottom <= lr.bottom + 1
    })
    expect(inListView).toBe(true)
    expect(await list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0)
  })

  test('keyboard shortcuts opens on ⌘⇧/ and filters', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+Shift+/')
    const panel = page.locator('.shortcuts-panel')
    await expect(panel).toBeVisible()
    await expect(panel.getByText('Go to (workspaces / PRs)')).toBeVisible()
    await expect(panel.getByText('Run command')).toBeVisible()
    // Bound bundle commands appear under their category group.
    await expect(panel.getByText('Uppercase')).toBeVisible()
    await panel.locator('.pal-in input').fill('terminal')
    await expect(panel.locator('.sc-row').first()).toBeVisible()
    await expect(panel.getByText('Toggle terminal')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.locator('.palette-bg.open')).toHaveCount(0)
  })

  test('⌥⌘↑ / ⌥⌘↓ cycles workspaces in sidebar order', async ({ page }) => {
    await gotoApp(page)
    const rows = page.locator('.rail .tproj:not(.collapsed) .tloop:not(.tmain)')
    const n = await rows.count()
    expect(n).toBeGreaterThan(1)
    await rows.nth(0).click()
    await expect(rows.nth(0)).toHaveClass(/on/)
    await page.keyboard.press('Meta+Alt+ArrowDown')
    await expect(rows.nth(1)).toHaveClass(/on/)
    await page.keyboard.press('Meta+Alt+ArrowUp')
    await expect(rows.nth(0)).toHaveClass(/on/)
  })

  test('⌥⌘↑ / ⌥⌘↓ scrolls the selected workspace into the sidebar tree', async ({ page }) => {
    await gotoApp(page)
    const tree = page.locator('.rail-tree')
    await tree.evaluate((el) => { el.style.maxHeight = '96px' })
    const storefront = page.locator('.tproj', { has: page.locator('.pn', { hasText: 'storefront' }) })
    const rows = storefront.locator('.tloop:not(.tmain)')
    const n = await rows.count()
    expect(n).toBeGreaterThan(2)
    await rows.nth(0).click()
    for (let i = 0; i < n - 1; i++) await page.keyboard.press('Meta+Alt+ArrowDown')
    const last = rows.nth(n - 1)
    await expect(last).toHaveClass(/on/)
    const visible = await page.evaluate(() => {
      const port = document.querySelector('.rail-tree') as HTMLElement | null
      const row = port?.querySelector('.tloop.on') as HTMLElement | null
      if (!port || !row) return false
      const tr = port.getBoundingClientRect()
      const rr = row.getBoundingClientRect()
      const overlap = Math.min(rr.bottom, tr.bottom) - Math.max(rr.top, tr.top)
      return overlap >= rr.height * 0.8
    })
    expect(visible).toBe(true)
  })

  test('opening a workspace from inbox scrolls it into the sidebar tree', async ({ page }) => {
    await gotoApp(page)
    const tree = page.locator('.rail-tree')
    await tree.evaluate((el) => { el.style.maxHeight = '96px' })
    await page.locator('.wbrief-row', { hasText: 'Add Apple & Google sign-in' }).click()
    const row = page.locator('.rail .tloop:not(.tmain)', { hasText: 'Add Apple & Google sign-in' })
    await expect(row).toHaveClass(/on/)
    const visible = await page.evaluate(() => {
      const port = document.querySelector('.rail-tree') as HTMLElement | null
      const on = port?.querySelector('.tloop.on') as HTMLElement | null
      if (!port || !on) return false
      const tr = port.getBoundingClientRect()
      const rr = on.getBoundingClientRect()
      const overlap = Math.min(rr.bottom, tr.bottom) - Math.max(rr.top, tr.top)
      return overlap >= rr.height * 0.8
    })
    expect(visible).toBe(true)
  })

  test('⌥⌘↑ / ⌥⌘↓ peeks a collapsed project and re-collapses unless you dwell', async ({ page }) => {
    await gotoApp(page)
    const storefront = page.locator('.tproj', { has: page.locator('.pn', { hasText: 'storefront' }) })
    const payments = page.locator('.tproj', { has: page.locator('.pn', { hasText: 'payments-api' }) })
    await storefront.locator('.tproj-h').hover()
    await storefront.locator('.car').click()
    await expect(storefront).toHaveClass(/collapsed/)

    // Working copy sits above the first payments task in cycle order. Open it
    // so one ArrowUp lands in storefront (pass-through keys stay back-to-back).
    await payments.locator('button.tloop.tmain').click()
    await expect(payments.locator('button.tloop.tmain')).toHaveClass(/on/)

    await page.keyboard.press('Meta+Alt+ArrowUp')
    await page.keyboard.press('Meta+Alt+ArrowDown')
    await expect(payments.locator('button.tloop.tmain')).toHaveClass(/on/)
    await expect(storefront).toHaveClass(/collapsed/)

    await page.keyboard.press('Meta+Alt+ArrowUp')
    await expect(storefront).not.toHaveClass(/collapsed/)
    await page.waitForTimeout(SCAN_COMMIT_MS + 200)
    await page.keyboard.press('Meta+Alt+ArrowDown')
    await expect(storefront).not.toHaveClass(/collapsed/)
  })

  test('go-to palette search index matches a loop by projectId and a PR by number', async ({ page }) => {
    await gotoApp(page)
    const loop = seed.loops[0]
    const pr = seed.prs[0]

    await page.keyboard.press('Meta+k')
    const pal = page.locator('.palette:not(.shortcuts-panel):not(.cmd-palette)')
    await pal.locator('.pal-in input').fill(loop.projectId)
    await expect(pal.locator('.pal-row', { hasText: loop.goal })).toBeVisible()

    await pal.locator('.pal-in input').fill(`#${pr.number}`)
    await expect(pal.locator('.pal-row', { hasText: pr.title })).toBeVisible()
  })

  test('open-in palette opens from the titlebar and ⌘O on a workspace', async ({ page }) => {
    await gotoApp(page)
    await page.keyboard.press('Meta+o')
    const openIn = page.locator('.open-in-palette:not(.run-palette):not(.stop-palette)')
    await expect(openIn).toHaveCount(1)
    await expect(page.locator('.palette-bg.open .open-in-palette:not(.run-palette):not(.stop-palette)')).toHaveCount(0)

    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const btn = page.locator('.titlebar .tb-open-in')
    await expect(btn).toBeVisible()

    await btn.click()
    const pal = page.locator('.open-in-palette:not(.run-palette):not(.stop-palette)')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-row', { hasText: 'Finder' })).toBeVisible()
    await expect(pal.locator('.pal-row', { hasText: 'Cursor' })).toBeVisible()
    await expect(pal.locator('.pal-row', { hasText: 'Copy path' })).toBeVisible()

    await page.keyboard.press('Escape')
    await expect(page.locator('.palette-bg.open .open-in-palette')).toHaveCount(0)

    await page.keyboard.press('Meta+o')
    await expect(pal).toBeVisible()
    await pal.locator('.pal-row', { hasText: 'Finder' }).click()
    await expect(page.locator('.palette-bg.open .open-in-palette')).toHaveCount(0)

    await page.locator('.titlebar .tb-copy-path').click()
    await expect(page.locator('.toast.info', { hasText: 'Copied path' })).toBeVisible()
  })

  test('open-in palette pins a default app to the top of the list', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    await page.locator('.titlebar .tb-open-in').click()
    const pal = page.getByRole('dialog', { name: 'Open in' })
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-row').first()).toContainText('Finder')

    const cursor = pal.locator('.pal-row', { hasText: 'Cursor' })
    await cursor.hover()
    await cursor.locator('.open-in-pin').click()
    await expect(pal.locator('.pal-row').first()).toContainText('Cursor')
    await expect(pal.locator('.pal-row').first()).toContainText('default')
    await expect(pal.locator('.pal-row').last()).toContainText('Copy path')

    await page.keyboard.press('Escape')
    await page.keyboard.press('Meta+o')
    await expect(pal.locator('.pal-row').first()).toContainText('Cursor')
    await expect(pal.locator('.pal-row').first()).toContainText('default')
  })
})
