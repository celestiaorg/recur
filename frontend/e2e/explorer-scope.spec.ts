// Explorer scope behaviour.
//
// The Changed view is derived client-side from the changed-path list rather than
// discovered by walking the tree: every path is already complete, so the nested
// structure needs no directory listings at all. It used to await one depth-1
// ListTree per ancestor directory — and each of those recomputed the repo's git
// status — so opening the view cost dozens of serial round trips. These tests
// pin the call count, and pin that an explicit scope choice is remembered.
import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge, BridgeError } from './helpers/mock-bridge'
import type { Page } from '@playwright/test'

const CHANGED: string[] = []
for (let d = 0; d < 80; d++) {
  for (let f = 0; f < 5; f++) CHANGED.push(`internal/pkg${d % 20}/sub${d}/file${f}.go`)
}

function childrenOf(dir: string): unknown[] {
  const kids = new Map<string, boolean>()
  for (const p of CHANGED) {
    if (dir === '') { kids.set(p.split('/')[0], p.split('/').length > 1); continue }
    if (!p.startsWith(dir + '/')) continue
    const rest = p.slice(dir.length + 1).split('/')
    kids.set(rest[0], rest.length > 1)
  }
  return [...kids].map(([name, isDir]) => ({ name, path: dir ? `${dir}/${name}` : name, dir: isDir }))
}

async function openLoopCode(page: Page) {
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs-scope')).toBeVisible()
}

test('Changed scope renders without listing directories', async ({ page }) => {
  const listed: string[] = []
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) => { listed.push((a[2] as string) || ''); return childrenOf((a[2] as string) || '') },
      ListTrees: (a) => {
        const dirs = (a[2] as string[]) ?? []
        for (const d of dirs) listed.push(d)
        return Object.fromEntries(dirs.map((d) => [d, childrenOf(d)]))
      },
      ListChangedFiles: () => CHANGED,
      GitStatus: () => Object.fromEntries(CHANGED.map((p) => [p, 'm'])),
      WorkingStatus: () => Object.fromEntries(CHANGED.map((p) => [p, 'm'])),
    },
  })
  await gotoApp(page)
  await openLoopCode(page)
  await expect(page.locator('#v-detail .obs-files .obs-node').first()).toBeVisible()
  await page.waitForTimeout(1500)

  // 400 changed files over 80 directories: every one of them is on screen...
  const rows = await page.locator('#v-detail .obs-files .obs-node').count()
  expect(rows).toBeGreaterThan(CHANGED.length)
  // ...and the only directory we asked the backend about is the repo root.
  expect(listed.filter((d) => d !== '')).toEqual([])
})

test('explorer folders stick while scrolling', async ({ page }) => {
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListChangedFiles: () => CHANGED,
      GitStatus: () => Object.fromEntries(CHANGED.map((p) => [p, 'm'])),
      WorkingStatus: () => Object.fromEntries(CHANGED.map((p) => [p, 'm'])),
    },
  })
  await gotoApp(page)
  await openLoopCode(page)
  const files = page.locator('#v-detail .obs-files')
  await expect(files.locator('.obs-node').first()).toBeVisible()
  const folder = files.locator('.obs-node.dir').first()
  await expect(folder).toHaveCSS('position', 'sticky')
  const name = (await folder.locator('.label').innerText()).trim()
  expect(name.length).toBeGreaterThan(0)
  await folder.click()
  await expect(folder).not.toHaveClass(/open/)
  await expect(folder.locator('.label')).toHaveText(name)
  await expect(folder).toHaveCSS('position', 'static')
  await folder.click()
  await expect(folder).toHaveClass(/open/)
  await files.evaluate((el) => { el.scrollTop = 240 })
  const stuck = await folder.evaluate((el) => {
    const pane = el.closest('.obs-files')!
    return Math.abs(el.getBoundingClientRect().top - pane.getBoundingClientRect().top)
  })
  expect(stuck).toBeLessThan(2)
  await expect(folder).toHaveClass(/is-stuck/)
  await expect(files.locator('.obs-node.dir.fade-under')).toHaveCount(1)
  const layers = await files.evaluate((el) => {
    const z = (node: Element | null) => {
      if (!node) return null
      const raw = getComputedStyle(node).zIndex
      return raw === 'auto' ? 0 : Number(raw)
    }
    const pane = el.closest('.obs-files-panel')
    return { sb: z(pane?.querySelector('.overlay-sb') ?? null), dir: z(el.querySelector('.obs-node.dir.is-stuck')) }
  })
  expect(layers.sb).toBeGreaterThan(layers.dir ?? 0)
})

test('expanded folder does not hide the next folder name', async ({ page }) => {
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListChangedFiles: () => ['alpha/a.go', 'beta/b.go'],
      GitStatus: () => ({ 'alpha/a.go': 'm', 'beta/b.go': 'm' }),
      WorkingStatus: () => ({ 'alpha/a.go': 'm', 'beta/b.go': 'm' }),
    },
  })
  await gotoApp(page)
  await openLoopCode(page)
  const files = page.locator('#v-detail .obs-files')
  await expect(files.locator('.obs-node.dir', { hasText: 'alpha' })).toHaveClass(/open/)
  await expect(files.locator('.obs-node.dir', { hasText: 'beta' }).locator('.label')).toHaveText('beta')
})

test('an explicit scope choice is remembered per loop', async ({ page }) => {
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListChangedFiles: () => ['src/main.go'],
      GitStatus: () => ({ 'src/main.go': 'm' }),
    },
  })
  await gotoApp(page)
  await openLoopCode(page)

  const scope = (name: string) =>
    page.locator('#v-detail .obs-scope button', { hasText: name })

  await scope('All files').click()
  await expect(scope('All files')).toHaveAttribute('aria-pressed', 'true')

  await page.locator('.tb-back').click()
  await openLoopCode(page)
  await expect(scope('All files')).toHaveAttribute('aria-pressed', 'true')

  await scope('Changed').click()
  await page.locator('.tb-back').click()
  await openLoopCode(page)
  await expect(scope('Changed')).toHaveAttribute('aria-pressed', 'true')
})

// Switching project/worktree used to leave the previous repo's tree, git status
// and changed-file list in place until the new reads landed — and because
// scopePaths unions two of those, there was a window showing both repos' files
// at once. The data is now cleared up front, the reads go out in parallel, and a
// skeleton covers the gap.
test('switching worktree never shows the previous one\'s files', async ({ page }) => {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
  const [A, B] = seed.loops.filter((l) => l.group === 'active')
  const filesFor = (loopId: string) =>
    loopId === B.id ? ['pkg/beta_only.go'] : ['pkg/alpha_only.go']

  await installBridge(page, structuredClone(seed), {
    resolvers: {
      // Stand in for a slow worktree, comfortably past SKELETON_DELAY_MS.
      ListTree: async (a) => {
        await sleep(900)
        return ((a[2] as string) || '') === '' ? [{ name: 'pkg', path: 'pkg', dir: true }] : []
      },
      ListChangedFiles: async (a) => { await sleep(900); return filesFor(a[0] as string) },
      GitStatus: async (a) => {
        await sleep(900)
        return Object.fromEntries(filesFor(a[1] as string).map((p) => [p, 'm']))
      },
      WorkingStatus: async (a) =>
        Object.fromEntries(filesFor(a[1] as string).map((p) => [p, 'm'])),
    },
  })
  await gotoApp(page)
  const openCode = async (goal: string) => {
    await openWorkspaceByGoal(page, goal)
  }
  await openCode(A.goal)
  await expect(page.locator('#v-detail .obs-files')).toContainText('alpha_only.go')

  await page.locator('.tb-back').click()
  await openCode(B.goal)
  // Slow backend, well past SKELETON_DELAY_MS: placeholder rows appear rather
  // than the old repo's contents.
  await expect(page.locator('#v-detail .obs-skel-row').first()).toBeVisible()

  const sawStale = await page.evaluate(async () => {
    let stale = false
    const t0 = performance.now()
    while (performance.now() - t0 < 2000) {
      const txt = document.querySelector('#v-detail .obs-files')?.textContent ?? ''
      if (txt.includes('alpha_only')) stale = true
      await new Promise((r) => setTimeout(r, 50))
    }
    return stale
  })
  expect(sawStale).toBe(false)
  await expect(page.locator('#v-detail .obs-files')).toContainText('beta_only.go')
})

test('a fast worktree switch shows no skeleton at all', async ({ page }) => {
  const [A, B] = seed.loops.filter((l) => l.group === 'active')
  const filesFor = (loopId: string) =>
    loopId === B.id ? ['pkg/beta_only.go'] : ['pkg/alpha_only.go']
  // No artificial latency: the load lands inside SKELETON_DELAY_MS, so the
  // placeholder must never be drawn — a skeleton visible for a frame or two is
  // the flicker this delay exists to prevent.
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) =>
        ((a[2] as string) || '') === '' ? [{ name: 'pkg', path: 'pkg', dir: true }] : [],
      ListChangedFiles: (a) => filesFor(a[0] as string),
      GitStatus: (a) => Object.fromEntries(filesFor(a[1] as string).map((p) => [p, 'm'])),
      WorkingStatus: (a) => Object.fromEntries(filesFor(a[1] as string).map((p) => [p, 'm'])),
    },
  })
  await gotoApp(page)
  const openCode = async (goal: string) => {
    await openWorkspaceByGoal(page, goal)
  }
  await openCode(A.goal)
  await expect(page.locator('#v-detail .obs-files')).toContainText('alpha_only.go')

  await page.locator('.tb-back').click()
  await openWorkspaceByGoal(page, B.goal)
  const watch = page.evaluate(async () => {
    let sawSkeleton = false
    let sawStale = false
    const t0 = performance.now()
    while (performance.now() - t0 < 1200) {
      if (document.querySelector('#v-detail .obs-skel-row')) sawSkeleton = true
      const txt = document.querySelector('#v-detail .obs-files')?.textContent ?? ''
      if (txt.includes('alpha_only')) sawStale = true
      await new Promise((r) => requestAnimationFrame(() => r(null)))
    }
    return { sawSkeleton, sawStale }
  })
  await expect(page.locator('#v-detail .obs-files')).toContainText('beta_only.go')
  const { sawSkeleton, sawStale } = await watch
  expect(sawSkeleton).toBe(false)
  expect(sawStale).toBe(false)
})

// Changed lists can include git submodule paths (directories). Clicking one used
// to ReadFile + Outline and toast a raw EISDIR RuntimeError JSON blob.
test('clicking a changed submodule directory opens Diff without error toasts', async ({ page }) => {
  const SUB = 'packages/contracts/lib/openzeppelin-contracts-upgradeable'
  let outlineCalls = 0
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListChangedFiles: () => [SUB],
      GitStatus: () => ({ [SUB]: 'm' }),
      WorkingStatus: () => ({ [SUB]: 'm' }),
      ReadFile: (a) => {
        if (a[2] === SUB) return new BridgeError(`${SUB} is a directory`)
        return {
          path: a[2] || 'src/main.go',
          language: 'go',
          content: 'package main\n',
          size: 14,
          writable: !!a[1],
        }
      },
      Outline: (a) => {
        outlineCalls += 1
        if (a[2] === SUB) return new BridgeError('should not outline a directory')
        return []
      },
      FileAtBase: (a) => (a[2] === SUB ? 'Subproject commit abcdef1\n' : ''),
    },
  })
  await gotoApp(page)
  await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'openzeppelin-contracts-upgradeable' }).click()

  await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
  await expect(page.locator('.toast.error')).toHaveCount(0)
  expect(outlineCalls).toBe(0)
})

test('gitignored entries are muted; tracked dotfiles are not', async ({ page }) => {
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) => {
        if ((a[2] as string) || '') return []
        return [
          { name: '.gitignore', path: '.gitignore', dir: false },
          { name: 'TODO.md', path: 'TODO.md', dir: false, ignored: true },
          { name: 'dist', path: 'dist', dir: true, ignored: true },
          { name: 'src', path: 'src', dir: true },
        ]
      },
    },
  })
  await gotoApp(page)
  await openLoopCode(page)
  await page.locator('#v-detail .obs-scope button', { hasText: 'All files' }).click()

  const row = (name: string) =>
    page.locator('#v-detail .obs-files .obs-node', { hasText: name })

  await expect(row('TODO.md')).toHaveClass(/ignored/)
  await expect(row('dist')).toHaveClass(/ignored/)
  await expect(row('.gitignore')).toBeVisible()
  await expect(row('.gitignore')).not.toHaveClass(/ignored/)
  await expect(row('src')).toBeVisible()
  await expect(row('src')).not.toHaveClass(/ignored/)
})
