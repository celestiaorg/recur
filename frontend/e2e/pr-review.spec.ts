import { test, expect, gotoApp, seed, openWorkspaceByGoal, sidebarWorkspace } from './helpers/fixtures'
import { installBridge, mockChatAttention, simulateChatTurnIdle, BridgeError, type Seed } from './helpers/mock-bridge'

// The richest seeded PR (commits + files + diffs + thread). Used for the review flow.
const RICH_PR = '482'

function patchRichPR(over: Record<string, unknown>): Seed {
  const s = structuredClone(seed)
  s.prById[RICH_PR] = { ...s.prById[RICH_PR], ...over }
  const i = s.prs.findIndex((p) => p.id === RICH_PR)
  if (i >= 0) s.prs[i] = { ...s.prs[i], ...over }
  return s
}

function addIdlePRWorkspace(s: Seed, prId: string) {
  const ws = {
    id: `WS-IDLE-${prId}`,
    projectId: 'storefront',
    type: 'solo',
    goal: 'Idle PR workspace',
    short: 'Idle PR workspace',
    statusLabel: 'Idle',
    phase: 'editing',
    worktreePath: '/tmp/from-pr',
    baseBranch: 'main',
    hostPrId: '',
  }
  s.loops = [ws, ...s.loops]
  s.loopById[ws.id] = ws
  const pr = s.prById[prId]
  if (pr) {
    pr.workspaceId = ws.id
    s.prById[prId] = pr
    s.prs = s.prs.map((p) => (p.id === prId ? { ...p, workspaceId: ws.id } : p))
  }
  return ws
}

function addHostPR(s: Seed, id: string, number: number, title: string) {
  const pr = {
    id,
    projectId: 'storefront',
    workspaceId: '',
    group: 'open',
    number,
    source: 'gh',
    title,
    branch: `feat/${id}`,
    base: 'main',
    state: 'In review',
    diff: { add: 1, del: 0 },
    fileCount: 1,
    checksOk: true,
    when: '1h',
    comments: [],
    commits: [],
    files: [],
    checks: [],
  }
  const host = {
    id: `WS-HOST-${id}`,
    projectId: 'storefront',
    type: 'solo',
    hostPrId: id,
    goal: title,
    short: title,
    statusLabel: 'Review',
    phase: 'editing',
    worktreePath: '/tmp/repo',
    baseBranch: 'main',
    archived: false,
  }
  s.prs = [...s.prs, pr]
  s.prById[id] = pr
  s.loops = [host, ...s.loops]
  s.loopById[host.id] = host
  return host
}

test.describe('PR review surface', () => {
  test.beforeEach(async ({ page }) => {
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
  })

  test('PR list filters by lifecycle and status', async ({ page }) => {
    const s = structuredClone(seed)
    s.prs = [
      ...s.prs,
      {
        id: 'gh-storefront-90',
        projectId: 'storefront',
        workspaceId: '',
        group: 'open',
        number: 90,
        source: 'gh',
        title: 'WIP checkout polish',
        branch: 'feat/wip',
        base: 'main',
        state: 'In review',
        draft: true,
        diff: { add: 3, del: 0 },
        fileCount: 1,
        checksOk: true,
        when: '4h',
        comments: [],
        commits: [],
        files: [],
        checks: [],
      },
      {
        id: 'gh-storefront-91',
        projectId: 'storefront',
        workspaceId: '',
        group: 'open',
        number: 91,
        source: 'gh',
        title: 'Abandoned experiment',
        branch: 'feat/abandoned',
        base: 'main',
        state: 'Closed',
        diff: { add: 1, del: 8 },
        fileCount: 1,
        checksOk: true,
        when: '3d',
        comments: [],
        commits: [],
        files: [],
        checks: [],
      },
      {
        id: 'gh-storefront-92',
        projectId: 'storefront',
        workspaceId: '',
        group: 'you',
        number: 92,
        source: 'gh',
        title: 'CI is red',
        branch: 'feat/ci-red',
        base: 'main',
        state: 'In review',
        diff: { add: 2, del: 0 },
        fileCount: 1,
        checksOk: false,
        hasChecks: true,
        when: '5h',
        attentionReasons: ['checks_failing'],
        comments: [],
        commits: [],
        files: [],
        checks: [],
      },
    ]
    s.prById['gh-storefront-90'] = s.prs[s.prs.length - 3]
    s.prById['gh-storefront-91'] = s.prs[s.prs.length - 2]
    s.prById['gh-storefront-92'] = s.prs[s.prs.length - 1]
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()

    const counts = page.locator('.titlebar .tb-sub')
    await expect(counts).toContainText('need you')
    await expect(counts).toContainText('open')
    await expect(counts).toContainText('merged')
    await expect(counts).not.toContainText('in the loop')

    const seg = page.locator('#v-pr .briefing-filters .seg')
    await expect(seg.getByRole('button', { name: 'Open' })).toHaveClass(/on/)
    await expect(seg.getByRole('button', { name: 'Needs you' })).toHaveCount(0)
    await expect(seg.getByRole('button', { name: 'Loop' })).toHaveCount(0)

    // Default Open is live non-drafts — merged, closed, and drafts stay hidden.
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#479' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#92' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#90' })).toHaveCount(0)
    await expect(page.locator('#v-pr .prrow', { hasText: '#91' })).toHaveCount(0)
    await expect(page.locator('#v-pr .prrow', { hasText: '#471' })).toHaveCount(0)

    await seg.getByRole('button', { name: 'Draft' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: '#90' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })).toHaveCount(0)

    await seg.getByRole('button', { name: 'Merged' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: '#471' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })).toHaveCount(0)

    await seg.getByRole('button', { name: 'Closed' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: '#91' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#471' })).toHaveCount(0)

    await seg.getByRole('button', { name: 'All' }).click()
    await expect(page.locator('#v-pr .prrow')).toHaveCount(s.prs.length)

    await seg.getByRole('button', { name: 'Open' }).click()
    const status = page.locator('#v-pr .briefing-filters .sel-btn[aria-label="Filter by status"]')
    await expect(status).toContainText('Any status')
    await status.click()
    const menu = page.locator('body > .sel-menu')
    await expect(menu).toBeVisible()
    await menu.getByRole('option', { name: 'Needs your review' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#479' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#92' })).toHaveCount(0)

    await menu.getByRole('option', { name: 'Needs your review' }).click()
    await menu.getByRole('option', { name: 'Checks failing' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: '#92' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })).toHaveCount(0)
  })

  test('PR list sorts within groups', async ({ page }) => {
    const s = structuredClone(seed)
    const row = (over: Record<string, unknown>) => ({
      projectId: 'storefront',
      workspaceId: '',
      group: 'open',
      source: 'gh',
      branch: 'feat/x',
      base: 'main',
      state: 'Open',
      diff: { add: 1, del: 0 },
      fileCount: 1,
      checksOk: true,
      comments: [],
      commits: [],
      files: [],
      checks: [],
      ...over,
    })
    s.prs = [
      row({ id: 'gh-10', number: 10, title: 'Zebra late', when: 'now', createdAt: '2026-08-28T12:00:00Z', updatedAt: '2026-08-26T12:00:00Z' }),
      row({ id: 'gh-20', number: 20, title: 'Alpha recent', when: '2d', createdAt: '2026-08-01T12:00:00Z', updatedAt: '2026-08-28T11:00:00Z' }),
    ]
    s.prById = { 'gh-10': s.prs[0], 'gh-20': s.prs[1] }
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()

    const titles = page.locator('#v-pr .prrow .pr-t-text')
    await expect(titles).toHaveText(['Alpha recent', 'Zebra late'])

    const sort = page.locator('#v-pr .briefing-filters .sel-btn[aria-label="Sort pull requests"]')
    await expect(sort).toContainText('Recently updated')
    await expect(sort.locator('.sel-ico svg')).toBeVisible()
    await sort.click()
    const menu = page.locator('body > .sel-menu')
    await expect(menu).toBeVisible()
    await menu.getByRole('option', { name: 'Newest' }).click()
    await expect(titles).toHaveText(['Zebra late', 'Alpha recent'])

    await sort.click()
    await expect(menu).toBeVisible()
    await menu.getByRole('option', { name: /Title A/ }).click()
    await expect(titles).toHaveText(['Alpha recent', 'Zebra late'])

    await sort.click()
    await expect(menu).toBeVisible()
    await menu.getByRole('option', { name: 'Least recently updated' }).click()
    await expect(titles).toHaveText(['Zebra late', 'Alpha recent'])

    await sort.click()
    await expect(menu).toBeVisible()
    await menu.getByRole('option', { name: 'Oldest' }).click()
    await expect(titles).toHaveText(['Alpha recent', 'Zebra late'])
  })

  test('PR list titlebar keeps counts at the minimum window width', async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 720 })
    const sub = page.locator('.titlebar .tb-sub')
    await expect(sub).toBeVisible()
    await expect(sub).toHaveText(/need you/)
    const sort = page.locator('#v-pr .briefing-filters .sel-btn[aria-label="Sort pull requests"]')
    await expect(sort.locator('.sel-ico svg')).toBeVisible()
    const clipped = await page.evaluate(() => {
      const left = document.querySelector('.titlebar .tb-left')
      const counts = document.querySelector('.titlebar .tb-sub')
      if (!left || !counts) return true
      const l = left.getBoundingClientRect()
      const c = counts.getBoundingClientRect()
      return c.right > l.right + 1
    })
    expect(clipped).toBe(false)
  })

  test('PR list filter triggers stay in the pane with multiple selections', async ({ page }) => {
    await page.setViewportSize({ width: 1100, height: 720 })
    const project = page.locator('#v-pr .briefing-filters .sel-btn[aria-label="Filter by project"]')
    const status = page.locator('#v-pr .briefing-filters .sel-btn[aria-label="Filter by status"]')
    const hug = await status.evaluate((el) => {
      const root = el.closest('.pr-status-filter')
      return {
        width: el.getBoundingClientRect().width,
        max: root ? parseFloat(getComputedStyle(root).maxWidth) : 0,
      }
    })
    expect(hug.max).toBeGreaterThan(0)
    expect(hug.width).toBeLessThan(hug.max - 1)
    await project.click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'payments-api' }).click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'storefront' }).click()
    await page.keyboard.press('Escape')
    await status.click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Needs your review' }).click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Changes requested' }).click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'Assigned to you' }).click()
    await page.keyboard.press('Escape')
    expect(await status.locator('.sel-val').evaluate((el) => el.scrollWidth > el.clientWidth + 1)).toBe(true)
    const overflow = await page.evaluate(() => {
      const pane = document.querySelector('#v-pr .briefing-filters')
      const seg = document.querySelector('#v-pr .briefing-filters .seg')
      if (!pane || !seg) return true
      return seg.getBoundingClientRect().right > pane.getBoundingClientRect().right + 1
    })
    expect(overflow).toBe(false)
  })

  test('PR list meta columns align across rows', async ({ page }) => {
    // Same-row people/why columns only apply on a wide list.
    await page.setViewportSize({ width: 1800, height: 900 })
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    const rows = page.locator('#v-pr .prrow')
    await expect(rows.first()).toBeVisible()
    expect(await rows.count()).toBeGreaterThan(1)
    const cols = await rows.evaluateAll((els) =>
      els.map((el) => {
        const box = (sel: string) => Math.round((el.querySelector(sel) as HTMLElement).getBoundingClientRect().left)
        return { people: box('.pr-people'), why: box('.pr-why'), diff: box('.pr-diff'), files: box('.pr-files'), ci: box('.pr-ci'), checks: box('.pr-checks'), when: box('.pr-when') }
      }),
    )
    const first = cols[0]
    for (const row of cols) {
      expect(row).toEqual(first)
    }
    expect(first.checks).toBeLessThan(first.diff)
  })

  test('PR list stacks participant avatars and names them on hover', async ({ page }) => {
    const crowd = Array.from({ length: 8 }, (_, i) => ({ login: `user${i}` }))
    const s = structuredClone(seed)
    const i = s.prs.findIndex((p) => p.id === '479')
    if (i >= 0) s.prs[i] = { ...s.prs[i], participants: crowd }
    s.prById['479'] = { ...s.prById['479'], participants: crowd }
    await installBridge(page, s)
    await gotoApp(page)
    await page.setViewportSize({ width: 1800, height: 900 })
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()

    const oauth = page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })
    await expect(oauth.locator('.pr-faces .pr-face')).toHaveCount(5)
    const peopleBox = await oauth.locator('.pr-people').boundingBox()
    const title = await oauth.locator('.pr-t').boundingBox()
    const why = await oauth.locator('.pr-why').boundingBox()
    expect(peopleBox).toBeTruthy()
    expect(title).toBeTruthy()
    expect(why).toBeTruthy()
    expect(peopleBox!.x).toBeGreaterThan(title!.x)
    expect(peopleBox!.x).toBeLessThan(why!.x)
    await oauth.locator('.pr-people').hover()
    const oauthPop = oauth.locator('.pr-people-pop')
    await expect(oauthPop).toBeVisible()
    await expect(oauthPop).toContainText('@loopbot')
    await expect(oauthPop).toContainText('@avery')
    await expect(oauthPop).toContainText('Review requested')
    await expect(oauthPop).toContainText('@sam')
    await expect(oauthPop).toContainText('Approved')
    await expect(oauthPop).toContainText('PR Responder')
    await expect(oauthPop).toContainText('Coder')

    const crowded = page.locator('#v-pr .prrow', { hasText: '#479' })
    await expect(crowded.locator('.pr-faces .pr-face')).toHaveCount(6)
    await expect(crowded.locator('.pr-face-more')).toHaveText('+3')
    await crowded.locator('.pr-people').hover()
    const crowdPop = crowded.locator('.pr-people-pop')
    await expect(crowdPop).toBeVisible()
    await expect(crowdPop).toContainText('@user0')
    await expect(crowdPop).toContainText('@user7')
  })

  test('sticky group headers stay above hovered PR row chrome', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    const header = page.locator('#v-pr .briefing-h').first()
    const row = page.locator('#v-pr .prrow').first()
    await expect(header).toBeVisible()
    await expect(row).toBeVisible()
    await page.locator('#v-pr .briefing-scroll').evaluate((el) => {
      if (el.scrollHeight <= el.clientHeight + 1) el.style.paddingBottom = '2000px'
      el.scrollTop = 20
    })
    const headerBox = await header.boundingBox()
    const box = await row.boundingBox()
    expect(headerBox).toBeTruthy()
    expect(box).toBeTruthy()
    expect(box!.y).toBeLessThan(headerBox!.y + headerBox!.height)
    expect(box!.y + box!.height).toBeGreaterThan(headerBox!.y + headerBox!.height)
    await page.mouse.move(box!.x + box!.width - 24, box!.y + box!.height - 6)
    const people = row.locator('.pr-people')
    const peopleBox = await people.boundingBox()
    if (peopleBox && peopleBox.y + 4 >= headerBox!.y + headerBox!.height) await people.hover()
    const top = await header.evaluate((h) => {
      const r = h.getBoundingClientRect()
      const el = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)
      return {
        header: !!el?.closest('.briefing-h'),
        row: !!el?.closest('.prrow') && !el?.closest('.briefing-h'),
        pop: !!el?.closest('.pr-people-pop'),
      }
    })
    expect(top).toEqual({ header: true, row: false, pop: false })
  })

  test('PR list prefixes titles with a source icon and number instead of a chip', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()

    const gh = page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })
    await expect(gh.locator('.srcbadge')).toHaveCount(0)
    await expect(gh.locator('.pr-num')).toHaveText(`#${RICH_PR}`)
    await expect(gh.locator('.pr-src')).toHaveAttribute('title', 'GitHub PR')
    const ghTitle = gh.locator('.pr-t-text')
    const ghNum = gh.locator('.pr-num')
    const ghTitleBox = await ghTitle.boundingBox()
    const ghNumBox = await ghNum.boundingBox()
    expect(ghTitleBox).toBeTruthy()
    expect(ghNumBox).toBeTruthy()
    expect(ghNumBox!.x).toBeLessThan(ghTitleBox!.x)

    const local = page.locator('#v-pr .prrow', { hasText: '#479' })
    await expect(local.locator('.pr-num')).toHaveText('#479')
    await expect(local.locator('.pr-src')).toHaveAttribute('title', 'Local PR')
    await expect(local.locator('.pr-t')).not.toContainText('GitHub')
    await expect(local.locator('.pr-sub')).not.toContainText('Local')
    await expect(local.locator('.pr-out')).toHaveCount(0)

    const ghOut = gh.locator('a.pr-out')
    await expect(ghOut).toHaveAttribute('href', 'https://github.com/acme/app/pull/482')
    await expect(ghOut).toHaveAttribute('title', 'Open in GitHub')
    await page.evaluate(() => {
      ;(window as any).__opened = []
      window.open = (url?: string | URL, target?: string, feat?: string) => {
        ;(window as any).__opened.push({ url: String(url), target, feat })
        return null
      }
    })
    await ghOut.click()
    await expect.poll(() => page.evaluate(() => (window as any).__opened)).toEqual([
      { url: 'https://github.com/acme/app/pull/482', target: '_blank', feat: 'noopener,noreferrer' },
    ])
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await expect(page.locator('#v-prdetail')).not.toHaveClass(/on/)

    await gh.locator('.pr-t-text').click()
    await expect(page.locator('#v-prdetail .prh-t .pr-src')).toHaveAttribute('title', 'GitHub PR')
    await expect(page.locator('#v-prdetail .prh-t .num')).toContainText(`#${RICH_PR}`)
    const headerPeople = page.locator('#v-prdetail .prh-side .pr-people')
    await expect(headerPeople.locator('.pr-faces .pr-face')).toHaveCount(5)
    await headerPeople.hover()
    await expect(headerPeople.locator('.pr-people-pop')).toContainText('Approved')
    await expect(headerPeople.locator('.pr-people-pop')).toContainText('Review requested')
    const pane = page.locator('#v-prdetail .prpane.on')
    await pane.evaluate((el) => {
      if (el.scrollHeight <= el.clientHeight + 1) el.style.paddingBottom = '2000px'
      el.scrollTop = 80
    })
    await expect(pane).toHaveClass(/fade-top/)
    await headerPeople.hover()
    const pop = headerPeople.locator('.pr-people-pop')
    await expect(pop).toBeVisible()
    expect(await pop.evaluate((el) => !!el.closest('.fade-top'))).toBe(false)
    expect(await pop.evaluate((el) => getComputedStyle(el).opacity)).toBe('1')
  })

  test('GitHub PR detail skeletons the thread until GetPR hydrates it', async ({ page }) => {
    const s = structuredClone(seed)
    const full = structuredClone(s.prById[RICH_PR])
    const summary = { ...full, comments: [], files: [], commits: [], checks: [] }
    const i = s.prs.findIndex((p) => p.id === RICH_PR)
    if (i >= 0) s.prs[i] = summary
    s.prById[RICH_PR] = summary
    let release!: () => void
    const gate = new Promise<void>((r) => { release = r })
    await installBridge(page, s, {
      resolvers: {
        GetPR: async () => {
          await gate
          return full
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(page.locator('#v-prdetail .prh-title')).toContainText(full.title)
    await expect(page.locator('#v-prdetail .prpane.on .pr-skel')).toBeVisible()
    await expect(page.locator('#v-prdetail .cmt.desc .cn')).toHaveCount(0)
    await expect(page.getByText('someone')).toHaveCount(0)
    await expect(page.getByText('No description provided.')).toHaveCount(0)
    release()
    await expect(page.locator('#v-prdetail .cmt.desc .cn')).toHaveText('loopbot')
    await expect(page.locator('#v-prdetail .prpane.on .pr-skel')).toHaveCount(0)
  })

  test('local PR description is the first conversation card', async ({ page }) => {
    const s = structuredClone(seed)
    const local = {
      ...s.prById['479'],
      summary: [{ kind: 'p', html: 'This loop implemented: <b>hidden when a description exists</b>.' }],
      focus: { title: 'Where to focus your review', body: 'The bulk of the change is in <code>e2e.spec.ts</code>.' },
      verify: [{ html: 'Opened checkout', passed: true }],
      comments: [{
        id: 'gh-description',
        kind: 'description',
        author: 'You',
        bodyMd: 'Stabilize the flaky checkout tests.\n\n## Changes\n\n- 1 commit across 2 files',
        time: 'now',
      }],
    }
    s.prById['479'] = local
    const i = s.prs.findIndex((p) => p.id === '479')
    if (i >= 0) s.prs[i] = local
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#479' }).click()
    const desc = page.locator('#v-prdetail .cmt.desc')
    await expect(desc).toContainText('Stabilize the flaky checkout tests.')
    await expect(page.getByText('hidden when a description exists')).toHaveCount(0)
    const descBox = await desc.boundingBox()
    const verifyBox = await page.locator('#v-prdetail .card', { hasText: 'Verified in browser' }).boundingBox()
    expect(descBox).toBeTruthy()
    expect(verifyBox).toBeTruthy()
    expect(descBox!.y).toBeLessThan(verifyBox!.y)
  })

  test('renders GitHub description and comment images via /gh-image', async ({ page }) => {
    const asset = 'https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
    const comments = ((seed.prById[RICH_PR] as { comments?: { id?: string }[] }).comments ?? []).map((c) => {
      if (c.id === 'gh-description') return { ...c, bodyMd: `<img alt="Screenshot" src="${asset}" />` }
      if (c.id === 'c1') return { ...c, bodyMd: `![inline](${asset})` }
      return c
    })
    await installBridge(page, patchRichPR({ comments }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    const imgs = page.locator('#v-prdetail .cm-md img')
    await expect(imgs).toHaveCount(2)
    await expect(imgs.first()).toHaveAttribute('src', /\/gh-image\?url=/)
    await expect(imgs.last()).toHaveAttribute('src', /\/gh-image\?url=/)
    const lb = page.locator('.chat-att-lb')
    const assertLightboxOnBody = async () => {
      await expect(lb).toBeVisible()
      expect(await lb.evaluate((el) => el.parentElement === document.body)).toBe(true)
      expect(await lb.evaluate((el) => !!el.closest('.prpane'))).toBe(false)
      const box = await lb.evaluate((el) => {
        const r = el.getBoundingClientRect()
        return { w: r.width, h: r.height }
      })
      const vp = await page.evaluate(() => ({ w: window.innerWidth, h: window.innerHeight }))
      expect(box.w).toBe(vp.w)
      expect(box.h).toBe(vp.h)
    }
    await imgs.first().click()
    await expect(lb.locator('.chat-att-lb-img')).toHaveAttribute('src', /\/gh-image\?url=/)
    await assertLightboxOnBody()
    await page.keyboard.press('Escape')
    await expect(lb).toHaveCount(0)
    await imgs.last().click()
    await assertLightboxOnBody()
    await page.keyboard.press('Escape')
    await expect(lb).toHaveCount(0)
  })

  test('PR list drops avatars under the title with attention copy on a narrow pane', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    const row = page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })
    await expect(row.locator('.pr-why')).toHaveText('Needs your review')
    const title = await row.locator('.pr-t').boundingBox()
    const people = await row.locator('.pr-people').boundingBox()
    const why = await row.locator('.pr-why').boundingBox()
    expect(title).toBeTruthy()
    expect(people).toBeTruthy()
    expect(why).toBeTruthy()
    expect(people!.y).toBeGreaterThan(title!.y + title!.height - 2)
    expect(Math.abs(people!.y - why!.y)).toBeLessThan(8)
    expect(people!.x).toBeLessThan(why!.x)
    const checks = await row.locator('.pr-checks').boundingBox()
    const diff = await row.locator('.pr-diff').boundingBox()
    expect(checks).toBeTruthy()
    expect(diff).toBeTruthy()
    expect(checks!.x).toBeLessThan(diff!.x)
  })

  test('PR list and detail show why a GitHub PR needs you', async ({ page }) => {
    const s = structuredClone(seed)
    s.prs = [
      ...s.prs,
      {
        id: 'gh-storefront-88',
        projectId: 'storefront',
        workspaceId: '',
        group: 'you',
        number: 88,
        source: 'gh',
        title: 'Please look at this',
        branch: 'feat/review-me',
        base: 'main',
        state: 'Review required',
        diff: { add: 4, del: 1 },
        fileCount: 2,
        checksOk: true,
        when: '1h',
        reviewRequested: true,
        attentionReasons: ['review_requested'],
        comments: [],
        commits: [],
        files: [],
        checks: [],
      },
      {
        id: 'gh-storefront-87',
        projectId: 'storefront',
        workspaceId: '',
        group: 'you',
        number: 87,
        source: 'gh',
        title: 'My failing PR',
        branch: 'feat/ci',
        base: 'main',
        state: 'In review',
        diff: { add: 2, del: 0 },
        fileCount: 1,
        checksOk: false,
        hasChecks: true,
        when: '3h',
        authorLogin: 'avery',
        viewerLogin: 'avery',
        reviewRequested: false,
        attentionReasons: ['checks_failing'],
        comments: [],
        commits: [],
        files: [],
        checks: [],
      },
    ]
    s.prById['gh-storefront-88'] = s.prs[s.prs.length - 2]
    s.prById['gh-storefront-87'] = s.prs[s.prs.length - 1]
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()

    const requested = page.locator('#v-pr .prrow', { hasText: '#88' })
    await expect(requested.locator('.pr-why')).toHaveText('Needs your review')
    await expect(page.locator('#v-pr .prrow', { hasText: '#87' }).locator('.pr-why')).toHaveText('')
    await expect(page.locator('#v-pr .prrow', { hasText: '#87' }).locator('.pr-ci-mark.fail')).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).locator('.pr-why')).toHaveText('Needs your review')
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).locator('.pr-ci-mark.ok')).toBeVisible()

    await requested.click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(page.locator('#v-prdetail .pr-attn-pill')).toHaveText('Needs your review')
    await expect(page.locator('#v-prdetail .pr-attn')).toHaveCount(0)
  })

  test('project filter uses the app select', async ({ page }) => {
    const picker = page.locator('#v-pr .briefing-filters .sel-btn[aria-label="Filter by project"]')
    await expect(picker).toBeVisible()
    await expect(picker).toContainText('All projects')
    await expect(page.locator('#v-pr select')).toHaveCount(0)
    await picker.click()
    const menu = page.locator('body > .sel-menu')
    await expect(menu.locator('.sel-opt', { hasText: 'storefront' })).toBeVisible()
    await expect(menu.locator('.sel-opt', { hasText: 'payments-api' })).toBeVisible()
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'payments-api' }).click()
    await expect(menu).toBeVisible()
    await expect(page.locator('body > .sel-menu .sel-opt', { hasText: 'payments-api' })).toHaveClass(/on/)
    await page.locator('body > .sel-menu .sel-opt', { hasText: 'storefront' }).click()
    await expect(page.locator('body > .sel-menu .sel-opt.on')).toHaveCount(2)
    await page.keyboard.press('Escape')
    await expect(picker).toContainText('payments-api')
    await expect(picker).toContainText('storefront')
  })

  test('opens a PR, shows the diff, and opens the review modal from Files', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    // Files-changed tab renders a real, line-numbered diff.
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await expect(page.locator('#v-prdetail .df-head').first()).toBeVisible()
    expect(await page.locator('#v-prdetail .hline').count()).toBeGreaterThan(0)

    await page.locator('.titlebar .review-actions button', { hasText: 'Approve' }).click()
    await expect(page.locator('#v-pr-review .pr-rev-modal')).toBeVisible()
    await expect(page.locator('#v-pr-review .nt-pickers .sel-btn')).toContainText('Approve')
    await expect(page.locator('#v-pr-review .nt-go')).toContainText('Approve')
  })

  test('open in workspace from a PR file diff', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await expect(page.locator('#v-prdetail .df-open').first()).toBeVisible()
    await page.locator('#v-prdetail .df-open').first().click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('oauth.ts')
  })

  test('Files changed is a sidebar TOC plus a scrolling stack', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await expect(page.locator('#v-prdetail .filelist')).toBeVisible()
    await expect(page.locator('#v-prdetail button[aria-label="Tree"]')).toHaveAttribute('aria-pressed', 'true')
    await expect(page.locator('#v-prdetail .diffpane')).toHaveCount(0)
    await expect(page.locator('#v-prdetail .cfile').first()).toBeVisible()
    expect(await page.locator('#v-prdetail .diffscroll .cfile .df-head').count()).toBeGreaterThan(1)

    const row = page.locator('#v-prdetail .filelist .fl-row:not(.dir)').first()
    const firstPath = ((await row.getAttribute('title')) ?? '').trim()
    await row.click()
    await expect(row).toHaveClass(/on/)
    if (firstPath) {
      await expect(page.locator('#v-prdetail .cfile[data-path="' + firstPath + '"]')).toBeInViewport()
    }

    await page.locator('#v-prdetail button[aria-label="Hide file list"]').click()
    await expect(page.locator('#v-prdetail .filelist')).toHaveCount(0)
    await expect(page.locator('#v-prdetail .cfile').first()).toBeVisible()

    await page.locator('#v-prdetail button[aria-label="Show file list"]').click()
    await expect(page.locator('#v-prdetail .filelist')).toBeVisible()
  })

  test('collapsing a files-changed card shrinks it to the header', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    const card = page.locator('#v-prdetail .diffscroll .cfile').first()
    await expect(card.locator('.hline').first()).toBeVisible()
    const expanded = await card.boundingBox()
    expect(expanded?.height ?? 0).toBeGreaterThan(80)

    await card.locator('.df-chev').click()
    await expect(card.locator('.df-head')).toHaveClass(/collapsed/)
    await expect(card.locator('.hline')).toHaveCount(0)
    const collapsed = await card.boundingBox()
    expect(collapsed?.height ?? 0).toBeLessThan(64)
  })

  test('files-changed findings render as inline cards and filter the TOC', async ({ page }) => {
    const finding = {
      id: 'f-race',
      file: 'src/hooks/useOAuthSession.ts',
      line: 2,
      message: 'token refresh can race across two tabs',
      status: 'open',
      severity: 'high',
    }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListPRFindings: () => [finding],
        ListFindings: () => [finding],
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    const toc = page.locator('#v-prdetail .filelist .fl-row', { hasText: 'useOAuthSession.ts' })
    await expect(toc.locator('.obs-finding-flag')).toBeVisible()
    expect(await page.locator('#v-prdetail .filelist .fl-row:not(.dir)').count()).toBeGreaterThan(1)

    await toc.click()
    const card = page.locator('#v-prdetail .cfile[data-path="src/hooks/useOAuthSession.ts"]')
    await expect(card.locator('.icmt-finding-card')).toContainText('token refresh can race')
    await expect(card.locator('.flag')).toHaveCount(0)
    await expect(card.locator('.finding-note-btn', { hasText: 'Open in Findings' })).toBeVisible()
    await card.locator('.finding-note-btn', { hasText: 'Open in Findings' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .findings-panel .finding-title')).toContainText('token refresh can race')

    await page.locator('.titlebar button', { hasText: 'View PR #482' }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await page.locator('#v-prdetail .fl-filter button[aria-label="Show files with open findings"]').click()
    await expect(page.locator('#v-prdetail .filelist .fl-row:not(.dir)')).toHaveCount(1)
    await expect(toc).toBeVisible()
    await expect(page.locator('#v-prdetail .fl-filter button[aria-pressed="true"]')).toBeVisible()
  })

  test('github-only findings post as pending review comments', async ({ page }) => {
    const findings = [
      {
        id: 'f-race',
        file: 'src/hooks/useOAuthSession.ts',
        line: 2,
        message: 'token refresh can race across two tabs',
        status: 'open',
        severity: 'high',
      },
      {
        id: 'f-abort',
        file: 'src/hooks/useOAuthSession.ts',
        line: 8,
        message: 'refresh ignores abort',
        status: 'open',
        severity: 'medium',
      },
    ]
    await installBridge(page, patchRichPR({ workspaceId: '' }), {
      resolvers: {
        ListPRFindings: () => findings.filter((f) => f.status === 'open'),
        SetFindingStatus: (a: unknown[]) => {
          const id = String(a[1] ?? '')
          const status = String(a[2] ?? '')
          const f = findings.find((x) => x.id === id)
          if (f) f.status = status
          return { ...(f ?? { id }), status }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()

    const card = page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })
    await expect(card).toBeVisible()
    await expect(card.getByRole('button', { name: 'Post all' })).toBeVisible()
    await expect(card.locator('.finding-note-btn', { hasText: 'Open in Findings' })).toHaveCount(0)

    await card.locator('.finding-row-actions', { hasText: 'token refresh' }).getByRole('button', { name: 'Post as GitHub comment' }).click()
    await expect(page.locator('.titlebar .pending-chip', { hasText: '1 pending' })).toBeVisible()
    await expect(card.locator('.finding-row', { hasText: 'token refresh' })).toHaveCount(0)
    await expect(card.locator('.finding-row', { hasText: 'refresh ignores abort' })).toBeVisible()

    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await expect(page.locator('#v-prdetail .fl-head').getByRole('button', { name: 'Post all' })).toBeVisible()
    const file = page.locator('#v-prdetail .cfile[data-path="src/hooks/useOAuthSession.ts"]')
    await page.locator('#v-prdetail .filelist .fl-row', { hasText: 'useOAuthSession.ts' }).click()
    await expect(file.locator('.icmt-pending-body', { hasText: 'token refresh can race' })).toBeVisible()
    await file.locator('.icmt-pending-card', { hasText: 'token refresh' }).getByRole('button', { name: 'Remove pending comment' }).click()
    await expect(file.locator('.icmt-pending-body', { hasText: 'token refresh can race' })).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()
    await expect(card.locator('.finding-row', { hasText: 'token refresh' })).toBeVisible()

    await card.getByRole('button', { name: 'Post all' }).click()
    await expect(page.locator('.titlebar .pending-chip', { hasText: '2 pending' })).toBeVisible()
    await expect(page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await page.locator('#v-prdetail .filelist .fl-row', { hasText: 'useOAuthSession.ts' }).click()
    await expect(file.locator('.icmt-pending-body', { hasText: 'token refresh can race' })).toBeVisible()
    await expect(file.locator('.icmt-pending-body', { hasText: 'refresh ignores abort' })).toBeVisible()
  })

  test('a matching submitted comment does not hide or auto-resolve a finding', async ({ page }) => {
    const finding = {
      id: 'f-race',
      workspaceId: 'WS-1',
      file: 'src/hooks/useOAuthSession.ts',
      line: 2,
      message: 'token refresh can race across two tabs',
      status: 'open',
      severity: 'high',
    }
    let settled = 0
    await installBridge(page, patchRichPR({
      comments: [
        ...((seed.prById[RICH_PR] as { comments?: object[] }).comments ?? []),
        {
          id: 'gh-inline-pre',
          code: 'AR',
          author: 'Avery Rourke',
          authorLogin: 'avery',
          time: '1m',
          bodyMd: finding.message,
          parentId: 'gh-review-99',
          path: finding.file,
          line: finding.line,
          side: 'RIGHT',
        },
      ],
    }), {
      resolvers: {
        ListPRFindings: () => [finding],
        SetFindingStatus: () => {
          settled++
          return { ...finding, status: 'resolved' }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    const card = page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })
    await expect(card).toBeVisible()
    await expect(card.locator('.finding-row', { hasText: 'token refresh' })).toBeVisible()
    expect(settled).toBe(0)
  })

  test('a persisted finding draft resolves after submit', async ({ page }) => {
    const finding = {
      id: 'f-race',
      workspaceId: 'LOOP-2231',
      file: 'src/hooks/useOAuthSession.ts',
      line: 2,
      message: 'token refresh can race across two tabs',
      status: 'open',
      severity: 'high',
    }
    let settled = 0
    await installBridge(page, patchRichPR({
      pendingComments: [{
        id: 'pend-finding-race',
        file: finding.file,
        line: finding.line,
        side: 'RIGHT',
        body: finding.message,
        findingId: finding.id,
      }],
    }), {
      resolvers: {
        ListPRFindings: () => finding.status === 'open' ? [finding] : [],
        SetFindingStatus: () => {
          settled++
          finding.status = 'resolved'
          return { ...finding, status: 'resolved' }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('.titlebar .pending-chip', { hasText: '1 pending' })).toBeVisible()
    await expect(page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })).toHaveCount(0)

    await page.locator('.titlebar .pending-chip').click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal).toBeVisible()
    await modal.locator('.nt-go', { hasText: 'Approve' }).click()
    await expect(modal).toHaveCount(0)
    await expect.poll(() => settled).toBe(1)
    await expect(page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })).toHaveCount(0)
  })

  test('deleting a finding draft does not resolve from an older matching comment', async ({ page }) => {
    const finding = {
      id: 'f-race',
      workspaceId: 'LOOP-2231',
      file: 'src/hooks/useOAuthSession.ts',
      line: 2,
      message: 'token refresh can race across two tabs',
      status: 'open',
      severity: 'high',
    }
    let settled = 0
    await installBridge(page, patchRichPR({
      comments: [
        ...((seed.prById[RICH_PR] as { comments?: object[] }).comments ?? []),
        {
          id: 'gh-inline-pre',
          code: 'AR',
          author: 'Avery Rourke',
          authorLogin: 'avery',
          time: '1m',
          bodyMd: finding.message,
          parentId: 'gh-review-99',
          path: finding.file,
          line: finding.line,
          side: 'RIGHT',
        },
      ],
    }), {
      resolvers: {
        ListPRFindings: () => [finding],
        SetFindingStatus: () => {
          settled++
          return { ...finding, status: 'resolved' }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    const card = page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })
    await card.locator('.finding-row-actions', { hasText: 'token refresh' }).getByRole('button', { name: 'Post as GitHub comment' }).click()
    await expect(page.locator('.titlebar .pending-chip', { hasText: '1 pending' })).toBeVisible()

    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    const file = page.locator('#v-prdetail .cfile[data-path="src/hooks/useOAuthSession.ts"]')
    await page.locator('#v-prdetail .filelist .fl-row', { hasText: 'useOAuthSession.ts' }).click()
    await file.locator('.icmt-pending-card', { hasText: 'token refresh' }).getByRole('button', { name: 'Remove pending comment' }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()
    await expect(card.locator('.finding-row', { hasText: 'token refresh' })).toBeVisible()
    await expect(card.getByRole('button', { name: 'Post as GitHub comment' })).toBeVisible()
    expect(settled).toBe(0)
  })

  test('a failed settle offers resolve without posting again', async ({ page }) => {
    const finding = {
      id: 'f-race',
      workspaceId: 'LOOP-2231',
      file: 'src/hooks/useOAuthSession.ts',
      line: 2,
      message: 'token refresh can race across two tabs',
      status: 'open',
      severity: 'high',
    }
    let settled = 0
    await installBridge(page, patchRichPR({
      pendingComments: [{
        id: 'pend-finding-race',
        file: finding.file,
        line: finding.line,
        side: 'RIGHT',
        body: finding.message,
        findingId: finding.id,
      }],
    }), {
      resolvers: {
        ListPRFindings: () => finding.status === 'open' ? [finding] : [],
        SetFindingStatus: () => {
          settled++
          if (settled === 1) return new BridgeError('nope')
          finding.status = 'resolved'
          return { ...finding, status: 'resolved' }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('.titlebar .pending-chip', { hasText: '1 pending' })).toBeVisible()

    await page.locator('.titlebar .pending-chip').click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal).toBeVisible()
    await modal.locator('.nt-go', { hasText: 'Approve' }).click()
    await expect(modal).toHaveCount(0)
    await expect.poll(() => settled).toBe(1)

    const card = page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })
    await expect(card.locator('.finding-row', { hasText: 'token refresh' })).toBeVisible()
    await expect(card.getByRole('button', { name: 'Post as GitHub comment' })).toHaveCount(0)
    await card.getByRole('button', { name: 'Resolve finding' }).click()
    await expect.poll(() => settled).toBe(2)
    await expect(page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })).toHaveCount(0)
  })

  test('a posted finding still settles after the PR merges', async ({ page }) => {
    const finding = {
      id: 'f-race',
      workspaceId: 'LOOP-2231',
      file: 'src/hooks/useOAuthSession.ts',
      line: 2,
      message: 'token refresh can race across two tabs',
      status: 'open',
      severity: 'high',
    }
    let settled = 0
    await installBridge(page, patchRichPR({
      state: 'Merged',
      group: 'merged',
      pendingComments: [{
        id: 'pend-seed',
        file: finding.file,
        line: 8,
        side: 'RIGHT',
        body: 'seed',
        findingId: finding.id,
      }],
      comments: [
        ...((seed.prById[RICH_PR] as { comments?: object[] }).comments ?? []),
        {
          id: 'gh-posted',
          code: 'AR',
          author: 'Avery Rourke',
          authorLogin: 'avery',
          time: '1m',
          bodyMd: finding.message,
          parentId: 'gh-review-99',
          path: finding.file,
          line: finding.line,
          side: 'RIGHT',
        },
      ],
    }), {
      resolvers: {
        ListPRFindings: () => finding.status === 'open' ? [finding] : [],
        SetFindingStatus: () => {
          settled++
          finding.status = 'resolved'
          return { ...finding, status: 'resolved' }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect.poll(() => settled).toBe(1)
    await expect(page.locator('#v-prdetail .card', { hasText: 'Reviewer findings' })).toHaveCount(0)
  })

  test('flags files with unresolved line comments and jumps to them', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    const toc = page.locator('#v-prdetail .filelist .fl-row', { hasText: 'oauth.ts' }).last()
    await expect(toc.locator('.obs-comment-flag')).toBeVisible()
    expect(await page.locator('#v-prdetail .filelist .fl-row:not(.dir)').count()).toBeGreaterThan(1)

    await page.locator('#v-prdetail .fl-filter button[aria-label="Show files with unresolved comments"]').click()
    await expect(page.locator('#v-prdetail .filelist .fl-row:not(.dir)')).toHaveCount(1)
    await expect(toc).toBeVisible()

    await toc.locator('.obs-comment-flag').click()
    const card = page.locator('#v-prdetail .cfile[data-path="src/auth/oauth.ts"]')
    await expect(card.locator('.hline.noted')).toHaveCount(2)
    await expect(card.locator('.hline.note-end')).toHaveCount(1)
    await expect(card.locator('.ln-note')).toHaveCount(0)
    await expect(card.locator('.hline.pulse')).toHaveCount(0)
    const thread = card.locator('.icmt-thread[data-note-line="12"]', { hasText: 'hmm' })
    await expect(thread).toBeVisible()
    const pane = page.locator('#v-prdetail .diffscroll')
    const paneBox = await pane.boundingBox()
    const threadBox = await thread.boundingBox()
    expect(paneBox && threadBox).toBeTruthy()
    expect(threadBox!.y).toBeGreaterThanOrEqual(paneBox!.y - 24)
    expect(threadBox!.y).toBeLessThan(paneBox!.y + paneBox!.height)
  })

  test('files-changed filters do not carry over to another PR', async ({ page }) => {
    const s = structuredClone(seed)
    s.prById['479'] = { ...s.prById['479'], files: s.prById[RICH_PR].files, fileCount: 3 }
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    await page.locator('#v-prdetail .fl-filter button[aria-label="Show files with unresolved comments"]').click()
    await expect(page.locator('#v-prdetail .filelist .fl-row:not(.dir)')).toHaveCount(1)
    await expect(page.locator('#v-prdetail .fl-filter button[aria-pressed="true"]')).toBeVisible()

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#479' }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    await expect(page.locator('#v-prdetail .empty', { hasText: 'No files match' })).toHaveCount(0)
    await expect(page.locator('#v-prdetail .filelist .fl-row:not(.dir)')).toHaveCount(3)
    await expect(page.locator('#v-prdetail .fl-filter button[aria-pressed="true"]')).toHaveCount(0)
  })

  test('commit detail shows the per-commit diff', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Commits' }).click()
    await expect(page.locator('#v-prdetail .commit .cm-t').first()).toHaveText('Add tests for Apple + Google providers')
    await expect(page.locator('#v-prdetail .commit .cm-t').last()).toHaveText('Add Apple & Google OAuth providers')
    await expect(page.locator('#v-prdetail .commit').first()).toBeVisible()
    await page.locator('#v-prdetail .commit').first().click()
    await expect(page.locator('#v-prdetail .commit-hd')).toBeVisible()
    await expect(page.locator('#v-prdetail .cfile .df-head').first()).toBeVisible()
    expect(await page.locator('#v-prdetail .cfile .hline').count()).toBeGreaterThan(0)
  })

  test('commit detail loads a lazy per-commit diff', async ({ page }) => {
    const original = structuredClone(seed.prById[RICH_PR].commits) as { hash: string; sha?: string; files: unknown[] }[]
    const byHash = Object.fromEntries(original.map((c) => [c.hash, c]))
    const stripped = original.map((c) => ({ ...c, files: [] }))
    await installBridge(page, patchRichPR({ commits: stripped }), {
      resolvers: {
        GetCommitDiff: (a) => {
          const ref = String(a[1] ?? '')
          return byHash[ref] ?? Object.values(byHash).find((c) => c.hash === ref || c.sha === ref) ?? {}
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Commits' }).click()
    await page.locator('#v-prdetail .commit').first().click()
    await expect(page.locator('#v-prdetail .commit-hd')).toBeVisible()
    await expect(page.locator('#v-prdetail .cfile .df-head').first()).toBeVisible()
    expect(await page.locator('#v-prdetail .cfile .hline').count()).toBeGreaterThan(0)
  })

  test('Tab inserts the selected emoji in the review modal', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('.titlebar .review-actions button', { hasText: 'Approve' }).click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal).toBeVisible()
    const ta = modal.locator('textarea')
    await ta.click()
    await ta.pressSequentially(':')
    await expect(page.locator('.emoji-suggest')).toBeVisible()
    await page.keyboard.press('Tab')
    await expect(ta).toHaveValue('👍')
    await expect(ta).toBeFocused()
    await expect(modal.locator('.nt-go')).not.toHaveClass(/kb-focus/)
    await expect(page.locator('.emoji-suggest')).toHaveCount(0)
  })

  test('Tab inserts a GitHub user mention in the review modal', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('.titlebar .review-actions button', { hasText: 'Approve' }).click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal).toBeVisible()
    const ta = modal.locator('textarea')
    await ta.click()
    await ta.pressSequentially('@')
    const picker = page.locator('.user-mention')
    await expect(picker).toBeVisible()
    await expect(picker).toContainText('loopbot')
    await expect(picker).toContainText('avery')
    await page.keyboard.press('Tab')
    await expect(ta).toHaveValue('@avery ')
    await expect(ta).toBeFocused()
    await expect(modal.locator('.nt-go')).not.toHaveClass(/kb-focus/)
    await expect(picker).toHaveCount(0)
  })

  test('Tab inserts a mentionable GitHub user from repository search', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        SearchPRMentionableUsers: () => [{ login: 'octocat', name: 'The Octocat' }],
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('.titlebar .review-actions button', { hasText: 'Approve' }).click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal).toBeVisible()
    const ta = modal.locator('textarea')
    await ta.click()
    await ta.pressSequentially('@octo')
    const picker = page.locator('.user-mention')
    await expect(picker).toBeVisible()
    await expect(picker).toContainText('octocat')
    await page.keyboard.press('Tab')
    await expect(ta).toHaveValue('@octocat ')
    await expect(ta).toBeFocused()
    await expect(picker).toHaveCount(0)
  })

  test('request changes from the titlebar split on any tab', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    await page.locator('.titlebar .review-chev').click()
    await page.locator('.titlebar .merge-menu-item', { hasText: 'Request changes' }).click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal).toBeVisible()
    await expect(modal.locator('.nt-pickers .sel-btn')).toContainText('Request changes')
    await modal.locator('textarea').fill('Please add tests for the secrets init path.')
    await modal.locator('.nt-go', { hasText: 'Request changes' }).click()
    await expect(modal).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()
    await expect(page.locator('#v-prdetail .thread .cmt', { hasText: 'Please add tests for the secrets init path.' })).toBeVisible()
  })

  test('shows a posted line note on the files diff and replies', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    const thread = page.locator('#v-prdetail .icmt-thread', { hasText: 'hmm' })
    await expect(thread).toBeVisible()
    const note = thread.locator('.cmt-kid-note', { hasText: 'hmm' }).first()
    await expect(note.locator('.cmt-act[aria-label="More"]')).toBeVisible()
    await note.locator('.cmt-act[aria-label="More"]').click()
    await expect(page.getByRole('menuitem', { name: 'Quote reply' })).toBeVisible()
    await expect(page.getByRole('menuitem', { name: 'Edit' })).toBeVisible()
    await expect(page.getByRole('menuitem', { name: 'Delete' })).toBeVisible()
    await page.getByRole('menuitem', { name: 'Quote reply' }).click()
    await expect(thread.locator('.finding-note-input')).toHaveValue(/> hmm/)
    await thread.locator('.finding-note-btn', { hasText: 'Cancel' }).click()
    await thread.locator('.cmt-reply-ph').click()
    await thread.locator('.finding-note-input').fill('diff reply from e2e')
    await thread.locator('.finding-note-btn.primary', { hasText: 'Reply' }).click()
    await expect(thread.locator('.cmt-kid-note', { hasText: 'diff reply from e2e' })).toBeVisible()
    await expect(thread.locator('.cmt-resolve', { hasText: 'Resolve conversation' })).toBeVisible()
    await thread.locator('.cmt-resolve', { hasText: 'Resolve conversation' }).click()
    await expect(thread.locator('.cmt-resolved-bar', { hasText: 'Resolved' })).toBeVisible()
    await thread.locator('.cmt-resolve', { hasText: 'Unresolve' }).click()
    await expect(thread.locator('.cmt-resolve', { hasText: 'Resolve conversation' })).toBeVisible()
  })

  test('leaves an inline comment anchored to a diff line', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    // The "+" on a diff line opens an inline composer (PRD headline: "leave an inline comment").
    await page.locator('#v-prdetail .addc').first().click({ force: true })
    const composer = page.locator('#v-prdetail .icmt textarea')
    await expect(composer).toBeVisible()

    const body = 'E2E: verify the secrets client initialises before providers.'
    await composer.fill(body)
    await page.locator('#v-prdetail .icmt .finding-note-btn.primary').click()

    await expect(page.locator('#v-prdetail .icmt-pending', { hasText: body })).toBeVisible()
    await expect(page.locator('#v-prdetail .icmt-pending', { hasText: body })).toHaveCount(1)
    await expect(page.locator('.titlebar .pending-chip', { hasText: 'pending' })).toBeVisible()

    await page.locator('.titlebar .pending-chip').click()
    const pending = page.locator('#v-pr-review .pr-rev-pending', { hasText: body })
    await expect(pending).toBeVisible()
    await expect(pending).toHaveCount(1)

    await page.locator('#v-pr-review .nt-x').click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()
    await expect(page.locator('#v-prdetail .composer-pending')).toHaveCount(0)
    await expect(page.locator('#v-prdetail .composer .nt-go', { hasText: 'Comment' })).toBeVisible()
  })

  test('drag on the comment control selects a multiline line comment', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    const file = page.locator('#v-prdetail .cfile[data-path="src/auth/oauth.ts"]')
    const start = file.locator('.hline[data-cmt-side="right"][data-cmt-line="13"] .addc')
    const end = file.locator('.hline[data-cmt-side="right"][data-cmt-line="15"] .addc')
    await start.dragTo(end, { force: true })

    const composer = file.locator('.icmt textarea')
    await expect(composer).toBeVisible()
    await expect(composer).toHaveAttribute('placeholder', /lines 13/)
    await expect(file.locator('.hline[data-cmt-line="13"]')).toHaveClass(/drafting/)
    await expect(file.locator('.hline[data-cmt-line="15"]')).toHaveClass(/draft-end/)
    await composer.fill('E2E: this span should stay together')
    await file.locator('.icmt .finding-note-btn.primary').click()

    await expect(file.locator('.icmt-pending', { hasText: 'this span should stay together' })).toBeVisible()
    await expect(file.locator('.hline[data-cmt-line="13"]')).toHaveClass(/noted/)
    await expect(file.locator('.hline[data-cmt-line="15"]')).toHaveClass(/note-end/)
    await page.locator('.titlebar .pending-chip').click()
    await expect(page.locator('#v-pr-review .pr-rev-chip-loc')).toHaveText('src/auth/oauth.ts:13-15')
  })

  test('long inline code in a PR description and comment does not widen the pane', async ({ page }) => {
    const token = "localStorage.setItem('eden-onboarding-storage',JSON.stringify({state:{hasCompletedOnboarding:true,hasSkippedOnboarding:false,hasStartedOnboarding:true,currentStep:0,onboardingVersion:\"v1\"},version:0}))"
    const s = patchRichPR({})
    const comments = (s.prById[RICH_PR].comments ?? []).map((c: { id?: string; kind?: string }) => {
      if (c.kind === 'description') {
        return { ...c, bodyMd: `1. Bypass onboarding: \`${token}\`\n2. Navigate to \`/mint\`.` }
      }
      if (c.id === 'c1') {
        return { ...c, bodyHtml: '', bodyMd: `Please run \`${token}\` locally.` }
      }
      return c
    })
    s.prById[RICH_PR] = { ...s.prById[RICH_PR], comments }
    const i = s.prs.findIndex((p) => p.id === RICH_PR)
    if (i >= 0) s.prs[i] = { ...s.prs[i], comments }
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    const descCode = page.locator('#v-prdetail .cmt.desc code').first()
    await expect(descCode).toContainText('hasCompletedOnboarding')
    await expect(page.locator('#v-prdetail .thread .cmt code', { hasText: 'hasCompletedOnboarding' })).toBeVisible()

    const pane = page.locator('#v-prdetail .prpane.on')
    expect(await pane.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThan(2)

    const wrapBox = await page.locator('#v-prdetail .prpane.on .conv-wrap').boundingBox()
    const codeBox = await descCode.boundingBox()
    expect(wrapBox).toBeTruthy()
    expect(codeBox).toBeTruthy()
    expect(codeBox!.width).toBeLessThanOrEqual(wrapBox!.width + 1)
  })

  test('edits the GitHub PR title and an owned comment, and quote-replies', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    await page.locator('#v-prdetail .prh-t .cmt-act[aria-label="Edit title"]').click()
    await page.locator('#v-prdetail .prh-title-input').fill('OAuth secrets — retitled')
    await page.locator('#v-prdetail .prh-t .finding-note-btn.primary').click()
    await expect(page.locator('#v-prdetail .prh-t')).toContainText('OAuth secrets — retitled')

    await page.locator('#v-prdetail .cmt.desc .cmt-act[aria-label="More"]').click()
    await page.getByRole('menuitem', { name: 'Edit' }).click()
    const descBox = page.locator('#v-prdetail .cmt.desc .finding-note-input')
    await descBox.fill('Retitled description for checkout OAuth.')
    await descBox.press('Meta+Enter')
    await expect(page.locator('#v-prdetail .cmt.desc')).toContainText('Retitled description for checkout OAuth.')

    const mine = page.locator('#v-prdetail .thread .cmt', { hasText: 'Avery Rourke' }).first()
    await mine.locator('.cmt-act[aria-label="More"]').click()
    await page.getByRole('menuitem', { name: 'Edit' }).click()
    const commentBox = mine.locator('.finding-note-input')
    await commentBox.fill('Edited: confirm secrets.init() still runs first.')
    await commentBox.press('Meta+Enter')
    await expect(mine).toContainText('Edited: confirm secrets.init() still runs first.')

    await mine.locator('.cmt-act[aria-label="More"]').click()
    await page.getByRole('menuitem', { name: 'Quote reply' }).click()
    await expect(page.locator('#v-prdetail .composer textarea')).toHaveValue(/> Edited: confirm secrets.init/)

    const other = page.locator('#v-prdetail .thread .cmt', { hasText: 'Maestro' }).first()
    await other.locator('.cmt-act[aria-label="More"]').click()
    await expect(page.getByRole('menuitem', { name: 'Delete' })).toBeVisible()
    await page.getByRole('menuitem', { name: 'Quote reply' }).click()
    await expect(page.locator('#v-prdetail .composer textarea')).toHaveValue(/> Got the review notes/)

    await mine.locator('.cmt-act[aria-label="More"]').click()
    await expect(page.getByRole('menuitem', { name: 'Delete' })).toBeVisible()
    await page.getByRole('menuitem', { name: 'Delete' }).click()
    await expect(page.locator('#v-prdetail .thread .cmt', { hasText: 'Edited: confirm secrets.init' })).toHaveCount(0)

    const review = page.locator('#v-prdetail .thread > .cmt', { hasText: 'testing again' })
    await review.locator('.cc > .ch .cmt-act[aria-label="More"]').click()
    await expect(page.getByRole('menuitem', { name: 'Edit' })).toBeVisible()
    await expect(page.getByRole('menuitem', { name: 'Delete' })).toHaveCount(0)
    await review.locator('.cc > .ch .cmt-act[aria-label="More"]').click()

    await page.locator('#v-prdetail .cmt.desc .cmt-act[aria-label="More"]').click()
    await expect(page.getByRole('menuitem', { name: 'Open in GitHub' })).toBeVisible()
    await page.getByRole('menuitem', { name: 'Copy link' }).click()
    await expect(page.locator('.toast.info', { hasText: 'Copied link' })).toBeVisible()
  })

  test('nests line comments under the review that submitted them', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const review = page.locator('#v-prdetail .thread > .cmt', { hasText: 'testing again' })
    await expect(review).toBeVisible()
    await expect(review.locator('.cmt-kid', { hasText: 'hmm' })).toBeVisible()
    await expect(review.locator('.cmt-ctx-path')).toHaveText('src/auth/oauth.ts:11-12')
    await expect(review.locator('.cmt-ctx-diff', { hasText: 'OAUTH_CLIENT_SECRET' })).toBeVisible()
    await expect(review.locator('.cmt-ctx-diff', { hasText: 'function register' })).toHaveCount(0)
    await expect(review.locator('.cmt-ctx-diff .hline.target')).toContainText('secrets.get')
    await review.locator('.cmt-ctx-fill').click()
    await expect(review.locator('.cmt-ctx-diff')).toHaveCount(0)
    await review.locator('.cmt-ctx-toggle').click()
    await expect(review.locator('.cmt-ctx-diff', { hasText: 'OAUTH_CLIENT_SECRET' })).toBeVisible()
    await expect(review.locator('.cmt-reply-ph', { hasText: 'Reply' })).toBeVisible()
    await expect(review.locator('.cmt-resolve', { hasText: 'Resolve conversation' })).toBeVisible()
    await review.locator('.cmt-resolve', { hasText: 'Resolve conversation' }).click()
    await expect(review.locator('.cmt-resolved-bar', { hasText: 'Resolved' })).toBeVisible()
    await expect(page.locator('#v-prdetail .thread > .cmt', { hasText: /^hmm$/ })).toHaveCount(0)
    await review.locator('.cmt-resolve', { hasText: 'Unresolve' }).click()
    await expect(review.locator('.cmt-reply-ph', { hasText: 'Reply' })).toBeVisible()
  })

  test('replies to and deletes a line note in conversation', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const kid = page.locator('#v-prdetail .thread > .cmt', { hasText: 'testing again' }).locator('.cmt-kid')
    await kid.locator('.cmt-reply-ph').click()
    await kid.locator('.finding-note-input').fill('thread reply from e2e')
    await kid.locator('.finding-note-btn.primary', { hasText: 'Reply' }).click()
    await expect(kid.locator('.cmt-kid-note', { hasText: 'thread reply from e2e' })).toBeVisible()

    await kid.locator('.cmt-kid-note', { hasText: 'thread reply from e2e' }).locator('.cmt-act[aria-label="More"]').click()
    await page.getByRole('menuitem', { name: 'Quote reply' }).click()
    await expect(kid.locator('.finding-note-input')).toHaveValue(/> thread reply from e2e/)
    await kid.locator('.finding-note-btn', { hasText: 'Cancel' }).click()

    await kid.locator('.cmt-kid-note', { hasText: 'hmm' }).locator('.cmt-act[aria-label="More"]').click()
    await page.getByRole('menuitem', { name: 'Delete' }).click()
    await expect(kid.locator('.cmt-kid-note', { hasText: 'hmm' })).toHaveCount(0)
  })

  test('closes the reply composer before a slow GitHub write returns', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ReplyPRComment: async (a) => {
          await new Promise((r) => setTimeout(r, 1500))
          const body = String(a[2] ?? '')
          return {
            id: 'gh-inline-slow-reply',
            code: 'YO',
            author: 'You',
            time: 'now',
            bodyHtml: '',
            bodyMd: body,
            verb: 'commented',
            kind: 'comment',
            inReplyToId: String(a[1] ?? ''),
          }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const kid = page.locator('#v-prdetail .thread > .cmt', { hasText: 'testing again' }).locator('.cmt-kid')
    await kid.locator('.cmt-reply-ph').click()
    await kid.locator('.finding-note-input').fill('slow thread reply')
    await kid.locator('.finding-note-btn.primary', { hasText: 'Reply' }).click()
    await expect(kid.locator('.cmt-reply-ph')).toBeVisible({ timeout: 400 })
    await expect(kid.locator('.cmt-kid-note', { hasText: 'slow thread reply' })).toBeVisible({ timeout: 400 })
    await expect(kid.getByText('Saving…')).toHaveCount(0)
  })

  test('submits a review with pending line notes as one conversation unit', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()

    await page.locator('#v-prdetail .addc').first().click({ force: true })
    const composer = page.locator('#v-prdetail .icmt textarea')
    await expect(composer).toBeVisible()
    await composer.fill('group me under the review')
    await page.locator('#v-prdetail .icmt .finding-note-btn.primary').click()

    await page.locator('.titlebar .review-actions button', { hasText: 'Approve' }).click()
    const modal = page.locator('#v-pr-review .pr-rev-modal')
    await expect(modal.locator('.pr-rev-pending', { hasText: 'group me under the review' })).toBeVisible()
    await modal.locator('textarea').fill('review wrap')
    await modal.locator('.nt-go', { hasText: /Approve/ }).click()
    await expect(modal).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()
    const unit = page.locator('#v-prdetail .thread > .cmt', { hasText: 'review wrap' })
    await expect(unit).toBeVisible()
    await expect(unit.locator('.cmt-kid', { hasText: 'group me under the review' })).toBeVisible()
    await expect(page.locator('#v-prdetail .thread > .cmt', { hasText: /^group me under the review$/ })).toHaveCount(0)
  })

  test('shows and toggles reactions on conversation items', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('#v-prdetail .cmt.desc .rxn-pill[aria-label*="hooray"]')).toBeVisible()

    const note = page.locator('#v-prdetail .thread .cmt', { hasText: 'Left a comment on' })
    await expect(note.locator('.rxn-pill[aria-label*="thumbs up"]')).toContainText('2')
    const heart = note.locator('.rxn-pill[aria-label*="heart"]')
    await expect(heart).toHaveClass(/mine/)
    await heart.click()
    await expect(note.locator('.rxn-pill[aria-label*="heart"]')).toHaveCount(0)

    await note.locator('.rxn-add').click()
    await page.getByRole('menuitem', { name: 'React with rocket' }).click()
    await expect(note.locator('.rxn-pill[aria-label*="rocket"]')).toBeVisible()

    const kid = page.locator('#v-prdetail .thread > .cmt', { hasText: 'testing again' }).locator('.cmt-kid')
    await expect(kid.locator('.rxn-pill[aria-label*="eyes"]')).toBeVisible()

    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    const thread = page.locator('#v-prdetail .icmt-thread', { hasText: 'hmm' })
    await expect(thread.locator('.rxn-pill[aria-label*="eyes"]')).toBeVisible()
    await thread.locator('.rxn-add').first().click()
    await page.getByRole('menuitem', { name: 'React with thumbs up' }).click()
    await expect(thread.locator('.rxn-pill[aria-label*="thumbs up"]')).toBeVisible()
  })

  test('review events stand out and have no reaction picker', async ({ page }) => {
    const s = patchRichPR({})
    const comments = [
      ...(s.prById[RICH_PR].comments ?? []),
      { id: 'gh-event-fp', kind: 'action', author: 'jcstein', bodyMd: 'force-pushed the branch', time: '29d' },
      { id: 'gh-review-action-1', kind: 'action', author: 'Ferret-san', verb: 'approved', bodyMd: 'approved', time: '28d' },
      { id: 'gh-review-empty', kind: 'comment', author: 'ggrimes', verb: 'approved', time: '1mo' },
    ]
    s.prById[RICH_PR] = { ...s.prById[RICH_PR], comments }
    const i = s.prs.findIndex((p) => p.id === RICH_PR)
    if (i >= 0) s.prs[i] = { ...s.prs[i], comments }
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const push = page.locator('#v-prdetail .tl-event', { hasText: 'force-pushed the branch' })
    await expect(push).toBeVisible()
    await expect(push.locator('.rxn-add')).toHaveCount(0)

    const ok = page.locator('#v-prdetail .tl-event.review-ok', { hasText: 'approved these changes' })
    await expect(ok).toBeVisible()
    await expect(ok.locator('.tl-ico.review-ok')).toBeVisible()
    await expect(ok.locator('.rxn-add')).toHaveCount(0)

    const card = page.locator('#v-prdetail .thread > .cmt.review-ok', { hasText: 'ggrimes' })
    await expect(card).toContainText('approved these changes')
    await expect(card.locator('.cmt-review-badge.ok')).toBeVisible()
    await expect(card.locator('.rxn-add')).toHaveCount(0)
  })

  test('PR list labels a draft as Draft, not In review', async ({ page }) => {
    await installBridge(page, patchRichPR({ draft: true, mergeStateStatus: 'DRAFT' }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    const row = page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` })
    await expect(row.locator('.pr-checks')).toHaveText('Draft')
    await expect(row).not.toContainText('In review')
    const openBucket = page.locator('#v-pr .briefing-bucket', { has: page.locator('.briefing-h', { hasText: /^Open/ }) })
    const youBucket = page.locator('#v-pr .briefing-bucket', { has: page.locator('.briefing-h', { hasText: 'Needs your attention' }) })
    await expect(openBucket.locator('.prrow', { hasText: `#${RICH_PR}` })).toBeVisible()
    await expect(youBucket.locator('.prrow', { hasText: `#${RICH_PR}` })).toHaveCount(0)
  })

  test('converts a GitHub PR to a draft and back', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const convert = page.locator('.titlebar .review-actions button', { hasText: 'Convert to draft' })
    await expect(convert).toBeVisible()
    await convert.click()
    await expect(page.locator('#v-prdetail .statepill', { hasText: 'Draft' })).toBeVisible()
    await expect(page.locator('#v-prdetail .statepill', { hasText: 'In review' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Ready for review' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Approve' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toHaveCount(0)

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).locator('.pr-checks')).toHaveText('Draft')
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Ready for review' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Approve' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toHaveCount(0)

    await page.locator('.titlebar .review-actions button', { hasText: 'Ready for review' }).click()
    await expect(page.locator('#v-prdetail .statepill', { hasText: 'Draft' })).toHaveCount(0)
    await expect(page.locator('#v-prdetail .statepill', { hasText: 'In review' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Convert to draft' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toBeVisible()
  })

  test('hides Approve on your own GitHub PR but still offers Merge', async ({ page }) => {
    await installBridge(page, patchRichPR({
      authorLogin: 'avery', viewerLogin: 'avery', viewerCanWrite: true,
      mergeStateStatus: 'CLEAN', draft: false,
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Approve' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Request changes' })).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toBeVisible()
  })

  test('a behind GitHub PR offers Update with main instead of Merge', async ({ page }) => {
    await installBridge(page, patchRichPR({ mergeStateStatus: 'BEHIND', behindBase: 2, viewerCanWrite: true, viewerLogin: 'josh' }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('.titlebar .review-actions .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Update with main' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Merge' })).toHaveCount(0)
    await expect(page.locator('.rework', { hasText: 'Out of date' })).toHaveCount(0)
  })

  test('the PR Update split can switch from merge to rebase onto the base', async ({ page }) => {
    await installBridge(page, patchRichPR({ mergeStateStatus: 'BEHIND', behindBase: 2, viewerCanWrite: true, viewerLogin: 'josh' }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await page.locator('.titlebar .review-actions .update-split button[aria-label="Choose update method"]').click()
    const rebase = page.locator('.titlebar .review-actions .merge-menu-item', { hasText: 'Rebase' })
    await expect(rebase).toBeVisible()
    const hit = await rebase.evaluate((el) => {
      const r = el.getBoundingClientRect()
      const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)
      return { item: !!top?.closest('.merge-menu-item'), prh: !!top?.closest('.prh') }
    })
    expect(hit).toEqual({ item: true, prh: false })
    await rebase.click()
    await expect(page.locator('.titlebar .review-actions .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Rebase onto main' })).toBeVisible()
  })

  test('a behind draft still offers Update with main next to Ready for review', async ({ page }) => {
    await installBridge(page, patchRichPR({
      draft: true, mergeStateStatus: 'DRAFT', behindBase: 2,
      viewerCanWrite: true, viewerLogin: 'josh',
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Ready for review' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Update with main' })).toBeVisible()
  })

  test('a GitHub PR that is behind locally still offers Merge when GitHub says CLEAN', async ({ page }) => {
    await installBridge(page, patchRichPR({
      mergeStateStatus: 'CLEAN', behindBase: 2, viewerCanWrite: true, viewerLogin: 'josh',
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('.titlebar .review-actions .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Update with main' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Merge' })).toBeVisible()
  })

  test('a behind GitHub PR waiting for review still offers Update with main', async ({ page }) => {
    await installBridge(page, patchRichPR({
      mergeStateStatus: 'BLOCKED', behindBase: 2, checksOk: true,
      reviewDecision: 'REVIEW_REQUIRED', viewerCanWrite: true, viewerLogin: 'josh',
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('.titlebar .review-actions .update-split .tbtn.solid:not(.update-chev)', { hasText: 'Update with main' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Merge' })).toHaveCount(0)
  })

  test('a repo that allows squash and rebase hides merge-commit', async ({ page }) => {
    await installBridge(page, patchRichPR({
      allowedMergeMethods: ['squash', 'rebase'], viewerCanWrite: true, viewerLogin: 'josh',
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await page.locator('.titlebar .review-actions .rbtn.merge-chev').click()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Squash and merge' })).toBeVisible()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: 'Rebase and merge' })).toBeVisible()
    await expect(page.locator('.titlebar .merge-menu-item', { hasText: /^Merge$/ })).toHaveCount(0)
  })

  test('confirming merge paints merged before GitHub returns', async ({ page }) => {
    let release!: (pr: unknown) => void
    const held = new Promise((resolve) => { release = resolve })
    const s = patchRichPR({
      viewerCanWrite: true, viewerLogin: 'josh', mergeStateStatus: 'CLEAN', draft: false,
    })
    await installBridge(page, s, {
      resolvers: {
        MergePR: () => held,
        GetPR: (a, seed) => seed.prById[a[0]] ?? null,
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const merge = page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Merge' })
    await merge.click()
    await merge.click()

    await expect(page.locator('#v-prdetail .pr-settled', { hasText: 'successfully merged' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Commits' }).click()
    await expect(page.locator('#v-prdetail .statepill', { hasText: 'Merged' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toHaveCount(0)

    const row = s.prById[RICH_PR]
    release({ ...row, state: 'In review', group: 'you' })
    await expect(page.locator('#v-prdetail .statepill', { hasText: 'Merged' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toHaveCount(0)
  })

  test('a failed merge restores the button and keeps updates from the round-trip', async ({ page }) => {
    let release!: (value: unknown) => void
    const held = new Promise((resolve) => { release = resolve })
    let merging = false
    const s = patchRichPR({
      viewerCanWrite: true, viewerLogin: 'josh', mergeStateStatus: 'CLEAN', draft: false,
    })
    await installBridge(page, s, {
      resolvers: {
        MergePR: () => { merging = true; return held },
        GetPR: (a, seed) => {
          const pr = seed.prById[a[0]]
          if (!pr) return null
          if (!merging) return pr
          const comments = [...(pr.comments ?? [])]
          if (!comments.some((c: { id?: string }) => c.id === 'during-merge')) {
            comments.push({
              id: 'during-merge',
              kind: 'comment',
              author: 'sam',
              bodyMd: 'landed while merging',
              bodyHtml: 'landed while merging',
              time: 'now',
            })
          }
          return { ...pr, comments }
        },
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const merge = page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Merge' })
    await merge.click()
    await merge.click()
    await expect(page.locator('#v-prdetail .pr-settled', { hasText: 'successfully merged' })).toBeVisible()

    await page.locator('#v-prdetail .prtab', { hasText: 'Commits' }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Conversation' }).click()
    await expect(page.locator('#v-prdetail .cmt', { hasText: 'landed while merging' })).toBeVisible()

    release(new BridgeError('protected branch'))
    await expect(page.locator('.toast.error')).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge')).toBeVisible()
    await expect(page.locator('#v-prdetail .pr-settled')).toHaveCount(0)
    await expect(page.locator('#v-prdetail .cmt', { hasText: 'landed while merging' })).toBeVisible()
  })

  test('a repo that only allows squash hides the other merge methods', async ({ page }) => {
    await installBridge(page, patchRichPR({
      allowedMergeMethods: ['squash'], viewerCanWrite: true, viewerLogin: 'josh',
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    await expect(page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Squash and merge' })).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge-chev')).toHaveCount(0)
    await expect(page.locator('.titlebar .merge-menu-item')).toHaveCount(0)
  })

  test('a blocked GitHub PR with failing checks offers Fix checks instead of Merge', async ({ page }) => {
    const sent: string[] = []
    const displays: string[] = []
    const s = patchRichPR({
      mergeStateStatus: 'BLOCKED', checksOk: false, reviewDecision: 'APPROVED', viewerCanWrite: true, viewerLogin: 'josh',
      checks: [
        { name: 'lint', passed: false, status: 'fail' },
        { name: 'build', passed: true, status: 'pass' },
      ],
    })
    addIdlePRWorkspace(s, RICH_PR)
    await installBridge(page, s, {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: unknown[]) => {
          sent.push(String(a[2] ?? ''))
          displays.push(String(a[7] ?? ''))
          return 'On it.'
        },
        WorkspaceChatItems: () => [],
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const fixChecks = page.locator('.titlebar .review-actions button', { hasText: 'Fix checks' })
    await expect(fixChecks).toBeVisible()
    await expect(page.locator('.titlebar .review-actions .rbtn.merge', { hasText: 'Merge' })).toHaveCount(0)
    await fixChecks.click()
    await expect(page.locator('#v-prdetail .prtab', { hasText: 'Checks' })).toHaveClass(/on/)
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toContain('gh run view --log-failed')
    expect(displays[0]).toBe('Fix failing GitHub checks')
  })

  test('failed check rows offer Fix which focuses that check', async ({ page }) => {
    const sent: string[] = []
    const displays: string[] = []
    const s = patchRichPR({
      checksOk: false,
      checks: [
        { name: 'lint', passed: false, status: 'fail' },
        { name: 'build', passed: true, status: 'pass' },
      ],
    })
    addIdlePRWorkspace(s, RICH_PR)
    await installBridge(page, s, {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: unknown[]) => {
          sent.push(String(a[2] ?? ''))
          displays.push(String(a[7] ?? ''))
          return 'On it.'
        },
        WorkspaceChatItems: () => [],
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Checks' }).click()

    const row = page.locator('#v-prdetail .prpane.on .checkrow', { hasText: 'lint' })
    await expect(row.locator('button.checks-fix', { hasText: 'Fix' })).toBeVisible()
    await expect(page.locator('#v-prdetail .prpane.on .checks-h button.checks-fix', { hasText: 'Fix checks' })).toBeVisible()
    await row.locator('button.checks-fix', { hasText: 'Fix' }).click()
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toContain('failing GitHub check lint')
    expect(displays[0]).toBe('Fix failing check: lint')
  })

  test('in-progress check duration keeps counting', async ({ page }) => {
    await page.clock.install({ time: new Date('2026-08-21T12:00:45.000Z') })
    await page.clock.pauseAt(new Date('2026-08-21T12:00:45.000Z'))
    const existing = seed.prById[RICH_PR].checks ?? []
    await installBridge(page, patchRichPR({
      checks: [
        { name: 'build', detail: '45s', passed: false, status: 'pending', startedAt: '2026-08-21T12:00:00Z' },
        ...existing,
      ],
      checksOk: false,
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await page.locator('#v-prdetail .prtab', { hasText: 'Checks' }).click()

    const row = page.locator('#v-prdetail .prpane.on .checkrow', { hasText: 'build' })
    await expect(row.locator('.cd')).toHaveText('45s')
    await expect(row.locator('a.cn')).toHaveAttribute('href', 'https://github.com/acme/app/pull/482/checks')
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-link')).toHaveAttribute('href', 'https://github.com/acme/app/pull/482/checks')
    await page.clock.fastForward(5000)
    await expect(row.locator('.cd')).toHaveText('50s')
  })

  test('Checks tab and card header show fail, pending, and pass', async ({ page }) => {
    await expect(page.locator('#v-pr .prrow').first()).toBeVisible()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail .prtab .prtab-check')).toHaveAttribute('data-kind', 'pass')
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-ico')).toHaveAttribute('data-kind', 'pass')
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-link')).toHaveText('All checks passed')

    await installBridge(page, patchRichPR({
      checksOk: false,
      checks: [
        { name: 'lint', passed: false, status: 'fail' },
        { name: 'build', passed: false, status: 'pending' },
      ],
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail .prtab .prtab-check')).toHaveAttribute('data-kind', 'fail')
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-ico')).toHaveAttribute('data-kind', 'fail')
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-link')).toHaveText('Some checks failed')

    await installBridge(page, patchRichPR({
      checksOk: false,
      checks: [{ name: 'build', passed: false, status: 'pending' }],
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail .prtab .prtab-check')).toHaveAttribute('data-kind', 'pending')
    await expect(page.locator('#v-prdetail .prtab .prtab-check')).toHaveClass(/spin/)
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-ico')).toHaveAttribute('data-kind', 'pending')
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-ico')).toHaveClass(/spin/)
    await expect(page.locator('#v-prdetail .prpane.on .checks-h-link')).toHaveText('Checks in progress')
    await expect(page.locator('#v-prdetail .prpane.on .checkrow .ci-wrap')).toHaveClass(/spin/)
  })

  test('conversation collapses successful checks; the Checks tab leaves them open', async ({ page }) => {
    await installBridge(page, patchRichPR({
      checksOk: false,
      checks: [
        { name: 'lint', passed: false, status: 'fail' },
        { name: 'test', passed: true, status: 'pass' },
        { name: 'deploy', passed: false, status: 'skipped' },
      ],
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    const conv = page.locator('#v-prdetail .prpane.on')
    await expect(conv.locator('.checkrow', { hasText: 'lint' })).toBeVisible()
    await expect(conv.locator('.checks-group-h', { hasText: 'successful' })).toHaveClass(/collapsed/)
    await expect(conv.locator('.checks-group-h', { hasText: 'skipped' })).toHaveClass(/collapsed/)
    await expect(conv.locator('.checkrow', { hasText: 'test' })).toHaveCount(0)
    await expect(conv.locator('.checkrow', { hasText: 'deploy' })).toHaveCount(0)
    await page.locator('#v-prdetail .prtab', { hasText: 'Checks' }).click()
    const tab = page.locator('#v-prdetail .prpane.on')
    await expect(tab.locator('.checks-group-h', { hasText: 'successful' })).not.toHaveClass(/collapsed/)
    await expect(tab.locator('.checks-group-h', { hasText: 'skipped' })).not.toHaveClass(/collapsed/)
    await expect(tab.locator('.checkrow', { hasText: 'test' })).toBeVisible()
    await expect(tab.locator('.checkrow', { hasText: 'deploy' })).toBeVisible()
  })

  test('conversation commit SHAs stay aligned when some rows have no checks', async ({ page }) => {
    const existing = seed.prById[RICH_PR].comments ?? []
    await installBridge(page, patchRichPR({
      comments: [
        {
          id: 'gh-commit-nocheck',
          kind: 'action',
          author: 'loopbot',
          bodyMd: 'Upgrade bridge-sdk version to 0.3.0',
          time: '95d',
          commitSha: 'f8dc8a4aaaaaaa',
          verified: true,
          url: 'https://github.com/acme/app/commit/f8dc8a4aaaaaaa',
        },
        {
          id: 'gh-commit-pending',
          kind: 'action',
          author: 'loopbot',
          bodyMd: 'Merge branch main',
          time: '1d',
          commitSha: '162ae9daaaaaaa',
          verified: true,
          checkStatus: 'pending',
          url: 'https://github.com/acme/app/commit/162ae9daaaaaaa',
        },
        ...existing,
      ],
    }))
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const rows = page.locator('#v-prdetail .tl-commit')
    await expect(rows).toHaveCount(2)
    await expect(rows.locator('.tl-ck')).toHaveCount(2)
    await expect(rows.nth(0).locator('.tl-ck svg')).toHaveCount(0)
    await expect(rows.nth(1).locator('.tl-ck svg')).toHaveCount(1)

    const xs = await rows.locator('.tl-sha').evaluateAll((els) => els.map((el) => el.getBoundingClientRect().x))
    expect(Math.abs(xs[0] - xs[1])).toBeLessThan(1)

    const family = await rows.locator('.tl-sha').first().evaluate((el) => getComputedStyle(el).fontFamily)
    expect(family.toLowerCase()).toMatch(/mono|jetbrains|menlo/)
  })

  test('posts a conversation comment without submitting a review', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()

    const box = page.locator('#v-prdetail .composer textarea')
    await box.fill('Just a note on the approach — not a formal review.')
    await expect(page.locator('#v-prdetail .composer .seg')).toHaveCount(0)
    await page.locator('#v-prdetail .composer .nt-go', { hasText: 'Comment' }).click()
    await expect(page.locator('#v-prdetail .cmt', { hasText: 'Just a note on the approach' })).toBeVisible()
    await expect(page.locator('#v-prdetail .composer .nt-go', { hasText: 'Comment' })).toBeVisible()
  })

  test('merged PRs hide commenting and conversation checks', async ({ page }) => {
    const s = structuredClone(seed)
    const merged = {
      ...s.prById['471'],
      viewerCanWrite: true,
      viewerLogin: 'avery',
      authorLogin: 'avery',
      comments: [
        { id: 'gh-description', kind: 'description', author: 'avery', bodyMd: 'Adds idempotency keys.', time: '1d' },
        { id: 'gh-event-merged', kind: 'action', author: 'mycodecrafting', bodyMd: 'merged commit a3f73e2 into main', commitSha: 'a3f73e2abcdef', time: '5m' },
        { id: 'gh-event-deleted', kind: 'action', author: 'mycodecrafting', bodyMd: 'deleted the recur/idempotency branch', time: '5m' },
      ],
      checks: [{ name: 'test', detail: '12s', passed: true, status: 'pass' }],
      files: s.prById[RICH_PR].files,
    }
    s.prById['471'] = merged
    const i = s.prs.findIndex((p: { id: string }) => p.id === '471')
    if (i >= 0) s.prs[i] = merged
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#471' }).click()

    const conv = page.locator('#v-prdetail .prpane.on')
    await expect(page.locator('#v-prdetail .pr-settled', { hasText: 'successfully merged' })).toBeVisible()
    await expect(conv.locator('.composer')).toHaveCount(0)
    await expect(conv.locator('.checks-box-body')).toHaveCount(0)
    await expect(conv.locator('.tl-event', { hasText: 'merged commit a3f73e2 into main' })).toBeVisible()
    await expect(conv.locator('.tl-event a.tl-sha', { hasText: 'a3f73e2' })).toHaveAttribute('href', 'https://github.com/acme/app/commit/a3f73e2abcdef')
    await expect(conv.locator('.tl-event', { hasText: 'deleted the recur/idempotency branch' })).toBeVisible()
    await expect(conv.locator('.tl-sub')).toHaveCount(0)
    await expect(page.locator('.titlebar .review-actions button', { hasText: 'Approve' })).toHaveCount(0)
    await expect(page.locator('#v-prdetail .prh-t .cmt-act[aria-label="Edit title"]')).toHaveCount(0)
    await page.locator('#v-prdetail .cmt.desc .cmt-act[aria-label="More"]').click()
    await expect(page.getByRole('menuitem', { name: 'Edit' })).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await expect(page.locator('#v-prdetail .df-head').first()).toBeVisible()
    await expect(page.locator('#v-prdetail .prpane.on .addc')).toHaveCount(0)

    await page.locator('#v-prdetail .prtab', { hasText: 'Checks' }).click()
    await expect(page.locator('#v-prdetail .prpane.on .checkrow', { hasText: 'test' })).toBeVisible()
  })

  test('tab panes fade at the top only after scroll', async ({ page }) => {
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    async function expectFadeOnScroll(scroller: ReturnType<typeof page.locator>) {
      await expect(scroller).toBeVisible()
      await scroller.evaluate((el) => {
        if (el.scrollHeight <= el.clientHeight + 1) el.style.paddingBottom = '2000px'
        el.scrollTop = 0
      })
      await expect(scroller).not.toHaveClass(/fade-top/)
      await scroller.evaluate((el) => { el.scrollTop = 80 })
      await expect(scroller).toHaveClass(/fade-top/)
      await scroller.evaluate((el) => { el.scrollTop = 0 })
      await expect(scroller).not.toHaveClass(/fade-top/)
    }

    await expectFadeOnScroll(page.locator('#v-prdetail .prpane.on'))

    await page.locator('#v-prdetail .prtab', { hasText: 'Commits' }).click()
    await expectFadeOnScroll(page.locator('#v-prdetail .prpane.on'))

    await page.locator('#v-prdetail .prtab', { hasText: 'Checks' }).click()
    await expectFadeOnScroll(page.locator('#v-prdetail .prpane.on'))

    await page.locator('#v-prdetail .prtab', { hasText: 'Files changed' }).click()
    await expect(page.locator('#v-prdetail .fl-body')).toBeVisible()
    await expect(page.locator('#v-prdetail .diffscroll')).toBeVisible()
  })
})

test.describe('PR agent chat dock', () => {
  test('shows the linked workspace chat with the same tabs', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    const linked = seed.loops.find((l: { id: string }) => l.id === seed.prById[RICH_PR].workspaceId)!
    await openWorkspaceByGoal(page, linked.goal)
    const wsDock = page.locator('#v-detail .obs-left')
    await expect(wsDock.locator('.chat-in .chat-composer')).toBeVisible()
    const wsTabs = await wsDock.locator('.wsv-chat-tab').allTextContents()
    expect(wsTabs.length).toBeGreaterThan(0)

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    const prDock = page.locator('#prd-agent .obs-left')
    await expect(prDock.locator('.chat-in .chat-composer')).toBeVisible()
    await expect(prDock.locator('.wsv-chat-tab')).toHaveText(wsTabs)
    await expect(page.locator('#v-prdetail .prtab', { hasText: 'Conversation' })).toBeVisible()

    const header = await page.locator('#v-prdetail .prh').boundingBox()
    const split = await page.locator('#v-prdetail .prd-split').boundingBox()
    const agentTabs = await prDock.locator('.obs-side-head').boundingBox()
    const prTabs = await page.locator('#v-prdetail .prtabs').boundingBox()
    expect(header && split && Math.abs(header.width - split.width) < 2).toBeTruthy()
    expect(agentTabs && prTabs && Math.abs(agentTabs.y - prTabs.y) < 2).toBeTruthy()
    expect(agentTabs && prTabs && agentTabs.x + agentTabs.width <= prTabs.x + 2).toBeTruthy()
  })

  test('selected agent chat tab stays in view when opening a PR', async ({ page }) => {
    const s = structuredClone(seed)
    const host = addHostPR(s, 'gh-tab-scroll', 810, 'Tab scroll host')
    const extra = Array.from({ length: 8 }, (_, i) => ({
      code: `PA${i + 2}`,
      title: `Chat ${i + 2}`,
      updatedAt: 0,
    }))
    ;(s as any).chatsByLoop = { [host.id]: extra }
    await installBridge(page, s, {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#810' }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    const strip = page.locator('#prd-agent .wsv-chat-tabs .tab-strip-scroll')
    await expect.poll(() => strip.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(60)
    await page.locator('#prd-agent .wsv-chat-tab').last().click()
    await expect(page.locator('#prd-agent .wsv-chat-tab.on')).toHaveAttribute('data-tab-key', 'PA9')

    const chipOverlap = (el: HTMLElement) => {
      const chip = el.querySelector<HTMLElement>('.tab-chip.on')
      if (!chip) return 0
      const sr = el.getBoundingClientRect()
      const cr = chip.getBoundingClientRect()
      return Math.min(cr.right, sr.right) - Math.max(cr.left, sr.left)
    }
    await expect.poll(() => strip.evaluate(chipOverlap)).toBeGreaterThan(24)

    await page.locator('#prd-agent .wsv-chat-tab').first().click()
    await expect(page.locator('#prd-agent .wsv-chat-tab.on')).toHaveAttribute('data-tab-key', 'PA')
    await expect.poll(() => strip.evaluate(chipOverlap)).toBeGreaterThan(24)
    await page.locator('#v-prdetail').getByRole('button', { name: 'Hide left panel' }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeHidden()
    await page.locator('#v-prdetail').getByRole('button', { name: 'Show left panel' }).click()
    await expect(page.locator('#prd-agent .wsv-chat-tab.on')).toHaveAttribute('data-tab-key', 'PA')
    await expect.poll(() => strip.evaluate(chipOverlap)).toBeGreaterThan(24)

    await page.locator('#prd-agent .wsv-chat-tab').last().click()
    await expect(page.locator('#prd-agent .wsv-chat-tab.on')).toHaveAttribute('data-tab-key', 'PA9')
    await expect.poll(() => strip.evaluate(chipOverlap)).toBeGreaterThan(24)

    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#810' }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(page.locator('#prd-agent .wsv-chat-tab.on')).toHaveAttribute('data-tab-key', 'PA9')
    await expect.poll(() => strip.evaluate(chipOverlap)).toBeGreaterThan(24)

    await page.locator('#v-prdetail').getByRole('button', { name: 'Hide left panel' }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeHidden()
    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#810' }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await page.locator('#v-prdetail').getByRole('button', { name: 'Show left panel' }).click()
    await expect(page.locator('#prd-agent .wsv-chat-tab.on')).toHaveAttribute('data-tab-key', 'PA9')
    await expect.poll(() => strip.evaluate(chipOverlap)).toBeGreaterThan(24)
  })

  test('a message sent on the PR is visible after View workspace', async ({ page }) => {
    const transcript: { role: string; kind: string; text: string }[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: unknown[]) => {
          const text = String(a[2] ?? '')
          transcript.push({ role: 'user', kind: 'text', text })
          transcript.push({ role: 'assistant', kind: 'text', text: `reply to: ${text}` })
          return `reply to: ${text}`
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const linked = seed.loops.find((l: { id: string }) => l.id === seed.prById[RICH_PR].workspaceId)!
    await openWorkspaceByGoal(page, linked.goal)
    const wsDock = page.locator('#v-detail .obs-left')
    await expect(wsDock.locator('.chat-in .chat-composer')).toBeVisible()
    await wsDock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await wsDock.locator('.chat-in .chat-composer').fill('from the workspace')
    await wsDock.locator('.chat-in .send:not(.stop)').click()
    await expect(wsDock.locator('.chat-row.me', { hasText: 'from the workspace' })).toBeVisible()

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    const prDock = page.locator('#prd-agent .obs-left')
    await expect(prDock.locator('.chat-row.me', { hasText: 'from the workspace' })).toBeVisible()
    await prDock.locator('.chat-in .chat-composer').fill('from the PR')
    await prDock.locator('.chat-in .send:not(.stop)').click()
    await expect(prDock.locator('.chat-row.me', { hasText: 'from the PR' })).toBeVisible()

    await page.locator('.titlebar button', { hasText: 'View workspace' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(wsDock.locator('.chat-row.me', { hasText: 'from the workspace' })).toBeVisible()
    await expect(wsDock.locator('.chat-row.me', { hasText: 'from the PR' })).toBeVisible()
  })

  test('nav and list show when a host agent is running with no workspace', async ({ page }) => {
    const prId = 'gh-host-run'
    const s = structuredClone(seed)
    s.resource = { ...s.resource, needsYou: 0 }
    const host = addHostPR(s, prId, 808, 'Imported checkout review')
    const hostId = host.id
    for (const l of s.loops) {
      if (l.id === hostId) continue
      l.state = 'paused'
      l.phase = 'idle'
      if (l.group === 'needs') l.group = 'ready'
    }
    await installBridge(page, s, {
      resolvers: {
        ListLoopAgents: (a: unknown[]) => {
          const id = String(a[0] ?? '')
          if (id !== hostId) return []
          return [{ id: `${hostId}-PA`, workspaceId: hostId, code: 'PA', status: 'working' }]
        },
      },
    })
    await gotoApp(page)

    const prNav = page.locator('.nav', { hasText: 'Pull requests' })
    await expect(prNav.locator('.sd.pulse')).toBeVisible()
    await expect(prNav.locator('svg')).toHaveCount(0)
    await expect(page.locator(`.tloop[data-workspace-id="${hostId}"]`)).toHaveCount(0)
    await expect(page.locator('.rail-stat.run')).toHaveText('1 PR agent running')

    await prNav.click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    const row = page.locator('#v-pr .prrow', { hasText: '#808' })
    await expect(row.locator('.sd.pulse')).toBeVisible()

    await row.click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(page.locator('#prd-agent .obs-left')).toBeVisible()
    await page.locator('#v-prdetail').getByRole('button', { name: 'Hide left panel' }).click()
    const showDock = page.locator('#v-prdetail').getByRole('button', { name: 'Show left panel. Agent running' })
    await expect(showDock).toBeVisible()
    await expect(showDock).toHaveClass(/running/)

    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-PA`, status: 'idle' },
      })
    }, { workspaceId: hostId })
    await expect(prNav.locator('.sd.pulse')).toHaveCount(0)
    await expect(prNav.locator('svg')).toHaveCount(1)
    await expect(page.locator('#v-prdetail').getByRole('button', { name: 'Show left panel' })).not.toHaveClass(/running/)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: '#808' }).locator('.sd.pulse')).toHaveCount(0)
  })

  test('shows the agent dock when the PR has no workspace', async ({ page }) => {
    await installBridge(page, patchRichPR({ workspaceId: '' }), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr')).toHaveClass(/on/)
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    const prDock = page.locator('#prd-agent .obs-left')
    await expect(prDock).toBeVisible()
    await expect(prDock.locator('.chat-in .chat-composer')).toBeVisible()
    await expect(page.locator('#v-prdetail .prtab', { hasText: 'Conversation' })).toBeVisible()
    await expect(page.locator('.titlebar button', { hasText: 'View workspace' })).toHaveCount(0)
    await expect(page.locator('.titlebar button', { hasText: 'Create workspace' })).toBeVisible()
    await page.locator('.titlebar button', { hasText: 'Create workspace' }).click()
    await expect(page.locator('.titlebar button', { hasText: 'View workspace' })).toBeVisible()
    await expect(prDock.locator('.chat-in .chat-composer')).toBeVisible()
  })

  test('PR chat survives creating a workspace', async ({ page }) => {
    const transcript: { role: string; kind: string; text: string }[] = []
    await installBridge(page, patchRichPR({ workspaceId: '' }), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: unknown[]) => {
          const text = String(a[2] ?? '')
          transcript.push({ role: 'user', kind: 'text', text })
          transcript.push({ role: 'assistant', kind: 'text', text: `reply to: ${text}` })
          return `reply to: ${text}`
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    const prDock = page.locator('#prd-agent .obs-left')
    await expect(prDock.locator('.chat-in .chat-composer')).toBeVisible()
    await prDock.locator('.chat-in .chat-composer').fill('from the imported PR')
    await prDock.locator('.chat-in .send:not(.stop)').click()
    await expect(prDock.locator('.chat-row.me', { hasText: 'from the imported PR' })).toBeVisible()

    await page.locator('.titlebar button', { hasText: 'Create workspace' }).click()
    await expect(page.locator('.titlebar button', { hasText: 'View workspace' })).toBeVisible()
    await expect(prDock.locator('.chat-row.me', { hasText: 'from the imported PR' })).toBeVisible()

    await page.locator('.titlebar button', { hasText: 'View workspace' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    const wsDock = page.locator('#v-detail .obs-left')
    await expect(wsDock.locator('.chat-row.me', { hasText: 'from the imported PR' })).toBeVisible()
  })

  test('Open filter includes other open PRs and needs-you group', async ({ page }) => {
    const s = patchRichPR({})
    s.prs = [
      ...s.prs,
      {
        id: 'gh-storefront-99',
        projectId: 'storefront',
        workspaceId: '',
        group: 'open',
        number: 99,
        source: 'gh',
        title: 'Docs typo',
        branch: 'docs/typo',
        base: 'main',
        state: 'Open',
        diff: { add: 1, del: 0 },
        fileCount: 1,
        checksOk: true,
        when: '3h',
        comments: [],
        commits: [],
        files: [],
        checks: [],
      },
    ]
    s.prById['gh-storefront-99'] = s.prs[s.prs.length - 1]
    await installBridge(page, s)
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await expect(page.locator('#v-pr .briefing-h', { hasText: 'Needs your attention' })).toBeVisible()
    await expect(page.locator('#v-pr .prrow', { hasText: '#99' })).toBeVisible()
    await expect(page.locator('#v-pr .briefing-h', { hasText: 'Open' })).toBeVisible()
  })

  test('the chat panel can be hidden and shown like the observatory', async ({ page }) => {
    await installBridge(page, structuredClone(seed), { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    const linked = seed.loops.find((l: { id: string }) => l.id === seed.prById[RICH_PR].workspaceId)!
    await openWorkspaceByGoal(page, linked.goal)
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    const split = page.locator('#v-prdetail .prd-split')
    const prDock = page.locator('#prd-agent .obs-left')
    const hideBtn = page.locator('#v-prdetail').getByRole('button', { name: 'Hide left panel' })
    const showBtn = page.locator('#v-prdetail').getByRole('button', { name: 'Show left panel' })

    async function expectToggleAtFarLeft(btn: typeof hideBtn) {
      const splitBox = await split.boundingBox()
      const btnBox = await btn.boundingBox()
      expect(splitBox && btnBox && btnBox.x - splitBox.x < 16).toBeTruthy()
    }

    await expect(prDock).toBeVisible()
    await expect(hideBtn).toBeVisible()
    await expectToggleAtFarLeft(hideBtn)

    await hideBtn.click()
    await expect(prDock).toBeHidden()
    await expect(page.locator('#v-prdetail .prtab', { hasText: 'Conversation' })).toBeVisible()
    await expect(page.locator('#v-prdetail .prtab', { hasText: 'Conversation' })).toHaveClass(/lit/)
    await expect(showBtn).toBeVisible()
    await expect(showBtn).toBeFocused()
    await expectToggleAtFarLeft(showBtn)

    // Hiding the PR dock must not collapse the workspace observatory chat.
    await page.locator('.titlebar button', { hasText: 'View workspace' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()

    await page.locator('.titlebar button', { hasText: 'View PR #482' }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(prDock).toBeHidden()

    await showBtn.click()
    await expect(prDock).toBeVisible()
    await expect(hideBtn).toBeVisible()
    await expect(hideBtn).toBeFocused()
    await expectToggleAtFarLeft(hideBtn)

    await page.keyboard.press('Meta+Alt+b')
    await expect(prDock).toBeHidden()
    await expect(showBtn).toBeFocused()
    await page.keyboard.press('Meta+Alt+b')
    await expect(prDock).toBeVisible()
    await expect(hideBtn).toBeFocused()
    await expectToggleAtFarLeft(hideBtn)

    const comment = page.locator('#v-prdetail textarea.finding-note-input')
    await comment.click()
    await expect(comment).toBeFocused()
    await page.keyboard.press('Meta+Alt+b')
    await expect(prDock).toBeHidden()
    await expect(comment).toBeFocused()
    await page.keyboard.press('Meta+Alt+b')
    await expect(prDock).toBeVisible()
    await expect(comment).toBeFocused()
  })

  test('a finish while the PR dock is hidden raises unseen', async ({ page }) => {
    await installBridge(page, structuredClone(seed), { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    const linked = seed.loops.find((l: { id: string }) => l.id === seed.prById[RICH_PR].workspaceId)!
    await openWorkspaceByGoal(page, linked.goal)
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeVisible()
    await expect.poll(() => {
      const a = mockChatAttention(page)
      return a?.focusWs === linked.id ? a.focusCode : ''
    }).toBeTruthy()
    const code = mockChatAttention(page)!.focusCode

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeVisible()
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(false)

    await page.locator('#v-prdetail').getByRole('button', { name: 'Hide left panel' }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeHidden()
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)

    await simulateChatTurnIdle(page, linked.id, code)
    await expect(sidebarWorkspace(page, linked.goal).locator('.ct')).toHaveText('1')
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await expect(page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).locator('.ct')).toHaveText('1')
  })

  test('a finish on a host PR with no workspace lands in inbox', async ({ page }) => {
    const prId = 'gh-host-inbox'
    const s = structuredClone(seed)
    const host = addHostPR(s, prId, 808, 'Imported checkout review')
    const hostId = host.id
    await installBridge(page, s, { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#808' }).click()
    await expect(page.locator('#prd-agent .obs-left .chat-in .chat-composer')).toBeVisible()
    await expect.poll(() => mockChatAttention(page)?.focusWs).toBe(hostId)
    const code = mockChatAttention(page)!.focusCode

    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)

    await simulateChatTurnIdle(page, hostId, code)
    // The host workspace has no Overview card, so its notification lands in
    // the Elsewhere section of the unified Briefing list.
    const row = page.locator('#v-briefing .briefing-elsewhere .ibrow', { hasText: 'finished a reply' })
    await expect(row).toBeVisible()
    await expect(row).toContainText('#808')
    await expect(page.locator(`.tloop[data-workspace-id="${hostId}"]`)).toHaveCount(0)
    const prNav = page.locator('.nav', { hasText: 'Pull requests' })
    await expect(prNav.locator('.ct.unseen')).toHaveText('1')

    await prNav.click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await expect(prNav.locator('.ct.unseen')).toHaveText('1')
    await expect(page.locator('#v-pr .prrow', { hasText: '#808' }).locator('.ct')).toHaveText('1')

    await page.locator('.nav.inbox-nav').click()
    await row.click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)
    await expect(page.locator('#prd-agent .obs-left')).toBeVisible()
    await expect.poll(() => mockChatAttention(page)?.focusWs).toBe(hostId)
    await expect(page.locator('#v-briefing .briefing-elsewhere .ibrow', { hasText: 'finished a reply' })).toHaveCount(0)
    await expect(prNav.locator('.ct.unseen')).toHaveCount(0)
  })

  test('PR nav unread is the sum of hidden host chats and counts down as they are read', async ({ page }) => {
    const s = structuredClone(seed)
    const hostA = addHostPR(s, 'gh-host-a', 808, 'First imported')
    const hostB = addHostPR(s, 'gh-host-b', 809, 'Second imported')
    await installBridge(page, s, { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: '#808' }).click()
    await expect(page.locator('#prd-agent .obs-left .chat-in .chat-composer')).toBeVisible()
    await expect.poll(() => mockChatAttention(page)?.focusWs).toBe(hostA.id)
    const codeA = mockChatAttention(page)!.focusCode

    await page.locator('.nav.inbox-nav').click()
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)
    await simulateChatTurnIdle(page, hostA.id, codeA)
    await simulateChatTurnIdle(page, hostB.id, 'PA')

    const prNav = page.locator('.nav', { hasText: 'Pull requests' })
    await expect(prNav.locator('.ct.unseen')).toHaveText('2')

    await prNav.click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await expect(prNav.locator('.ct.unseen')).toHaveText('2')
    await expect(page.locator('#v-pr .prrow', { hasText: '#808' }).locator('.ct')).toHaveText('1')
    await expect(page.locator('#v-pr .prrow', { hasText: '#809' }).locator('.ct')).toHaveText('1')

    await page.locator('#v-pr .prrow', { hasText: '#808' }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeVisible()
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(false)
    await expect(prNav.locator('.ct.unseen')).toHaveText('1')

    await prNav.click()
    await expect(page.locator('#v-pr .prrow', { hasText: '#808' }).locator('.ct')).toHaveCount(0)
    await expect(page.locator('#v-pr .prrow', { hasText: '#809' }).locator('.ct')).toHaveText('1')
    await expect(prNav.locator('.ct.unseen')).toHaveText('1')

    await page.locator('#v-pr .prrow', { hasText: '#809' }).click()
    await expect.poll(() => mockChatAttention(page)?.focusWs).toBe(hostB.id)
    await expect(prNav.locator('.ct.unseen')).toHaveCount(0)
  })

  test('focusing the agent dock lights chat tabs and un-lits the PR tab', async ({ page }) => {
    await installBridge(page, structuredClone(seed), { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    const linked = seed.loops.find((l: { id: string }) => l.id === seed.prById[RICH_PR].workspaceId)!
    await openWorkspaceByGoal(page, linked.goal)
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()

    await page.locator('.nav', { hasText: 'Pull requests' }).click()
    await page.locator('#v-pr .briefing-filters .seg button', { hasText: 'All' }).click()
    await page.locator('#v-pr .prrow', { hasText: `#${RICH_PR}` }).click()
    await expect(page.locator('#v-prdetail')).toHaveClass(/on/)

    const conv = page.locator('#v-prdetail .prtab', { hasText: 'Conversation' })
    const chatTab = page.locator('#prd-agent .wsv-chat-tab.on')
    const composer = page.locator('#prd-agent .chat-in .chat-composer')
    await expect(composer).toBeVisible()
    await expect(conv).toHaveClass(/on/)
    await expect(conv).toHaveClass(/lit/)
    await expect(chatTab).not.toHaveClass(/lit/)

    await composer.click()
    await expect(chatTab).toHaveClass(/lit/)
    await expect(conv).not.toHaveClass(/lit/)

    await conv.click()
    await expect(conv).toHaveClass(/lit/)
    await expect(chatTab).not.toHaveClass(/lit/)

    await chatTab.click()
    await expect(chatTab).toHaveClass(/lit/)
    await expect(conv).not.toHaveClass(/lit/)

    await page.locator('#v-prdetail').getByRole('button', { name: 'Hide left panel' }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeHidden()
    await expect(conv).toHaveClass(/lit/)
  })
})
