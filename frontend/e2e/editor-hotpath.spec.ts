// Regression cover for the editor hot path (see the perf pass that introduced
// them). These pin the behaviours that the optimisations put at risk:
//
//   - grammars are imported on demand, so highlighting has to actually arrive
//     after the chunk loads rather than at mount;
//   - the buffer text reaches the parent on a debounce, so "modified" state and
//     Cmd-S must both read through the synchronous path, not the debounced one;
//   - the live-refresh poll overwrites a clean buffer from disk, and the dirty
//     flag is the only thing keeping it off an unsaved edit.
import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge, BridgeError } from './helpers/mock-bridge'
import { codeContent, codeEditor, diffContent, splitContent } from './helpers/editor'
import type { Page } from '@playwright/test'

const MAIN = 'package main\n\nfunc main() {}\n'

async function pushLoopEvent(page: Page, loopId: string) {
  // The Observatory's live refresh keys off the worktree watcher's signal.
  await page.evaluate((loopId) => {
    ;(window as any)._wails.dispatchWailsEvent({
      name: 'workspace.fs_changed',
      data: { workspaceId: loopId },
    })
  }, loopId)
}

async function openLoopCode(page: Page) {
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  return working
}

/** Changed files open in Diff mode; force the code surface. */
async function openFile(page: Page, name: string) {
  await page.locator('#v-detail .obs-files .obs-node', { hasText: name }).click()
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await code.count()) {
    // Code stays disabled until ReadFile hydrates; clicking early is a no-op
    // and the test would keep looking at Diff (no TextMate token spans).
    await expect(code).toBeEnabled()
    await code.click()
    await expect(code).toHaveClass(/on/)
  }
  await expect(codeEditor(page)).toBeVisible()
}

test('grammar loads lazily and highlights; typing marks dirty synchronously; save clears it', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('func main() {}')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)

  const highlighted = page.locator('#v-detail .cm-content span[class]')
  await expect(highlighted.first()).toBeVisible()
  expect(await highlighted.count()).toBeGreaterThan(0)

  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type(' // edit')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).toContainText('Modified')

  await page.keyboard.press('ControlOrMeta+s')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
  await expect(highlighted.first()).toBeVisible()
})

test('reverting a local edit clears the unsaved marker', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('X')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).toContainText('Modified')

  await page.keyboard.press('Backspace')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).not.toContainText('Modified')
  await expect(page.locator('#v-detail .cm-content')).not.toContainText('X')
})

test('CRLF on disk does not mark the buffer unsaved', async ({ page }) => {
  const crlf = 'package main\r\n\r\nfunc main() {}\r\n'
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: (a) => ({
        path: a[2] || 'src/main.go', language: 'go', content: crlf,
        html: '', lines: [], size: crlf.length, writable: !!a[1],
      }),
    },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('func main() {}')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).not.toContainText('Modified')
})

test('files explorer shows the same unsaved dot as the editor tab', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')
  const row = page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' })
  await expect(row.locator('[aria-label="unsaved"]')).toHaveCount(0)

  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type(' // edit')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  await expect(row.locator('[aria-label="unsaved"]')).toBeVisible()

  await page.keyboard.press('ControlOrMeta+s')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  await expect(row.locator('[aria-label="unsaved"]')).toHaveCount(0)
})

test('save writes the live buffer even when typed inside the debounce window', async ({ page }) => {
  const writes: string[] = []
  await installBridge(page, structuredClone(seed), {
    resolvers: { WriteFile: (a) => { writes.push(a[3] as string); return null } },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('X')
  await page.keyboard.press('ControlOrMeta+s')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  expect(writes.length).toBe(1)
  expect(writes[0]).toContain('X')
})

test('external on-disk change refreshes a clean buffer', async ({ page }) => {
  let content = MAIN
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: (a) => ({
        path: a[2] || 'src/main.go', language: 'go', content,
        html: '', lines: [], size: content.length, writable: !!a[1],
      }),
    },
  })
  const working = await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('func main() {}')

  content = 'package main\n\nfunc main() { AGENT_EDIT }\n'
  await pushLoopEvent(page, working.id)
  await expect(page.locator('#v-detail .cm-content')).toContainText('AGENT_EDIT')
})

test('Diff rehydrates when a deleted file comes back on disk', async ({ page }) => {
  let status: Record<string, string> = { 'src/gone.go': 'd' }
  let content: string | null = null
  const base = 'package main\n\nfunc main() {}\n'
  const restored = 'package main\n\nfunc main() { RESTORED }\n'
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) => {
        const dir = (a[2] as string) || ''
        if (!dir) return [{ name: 'src', path: 'src', dir: true }]
        if (dir === 'src') {
          return content == null ? [] : [{ name: 'gone.go', path: 'src/gone.go', dir: false }]
        }
        return []
      },
      ListChangedFiles: () => ['src/gone.go'],
      GitStatus: () => ({ ...status }),
      WorkingStatus: () => ({ ...status }),
      FileAtBase: () => base,
      ReadFile: (a) => {
        if (content == null) return new BridgeError('src/gone.go: no such file or directory')
        const p = String(a[2] || 'src/gone.go')
        return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
      },
    },
  })
  const working = await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'gone.go' }).click()
  await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
  await expect(diffContent(page)).toContainText('func main() {}')

  content = restored
  status = { 'src/gone.go': 'm' }
  await pushLoopEvent(page, working.id)
  await expect(diffContent(page)).toContainText('RESTORED')
})

// Changed files open in Diff (MergeEditor) by default. That surface used to
// ignore parent `doc` updates after mount, so agent writes stayed invisible
// until the user hit Reload / re-opened the tab.
test('external on-disk change refreshes a clean Diff buffer', async ({ page }) => {
  let content = MAIN
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: (a) => ({
        path: a[2] || 'src/main.go', language: 'go', content,
        html: '', lines: [], size: content.length, writable: !!a[1],
      }),
      FileAtBase: () => MAIN,
    },
  })
  const working = await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
  await expect(page.locator('#v-detail .obs-view-seg button.on', { hasText: 'Diff' })).toBeVisible()
  await expect(diffContent(page)).toContainText('func main() {}')

  content = 'package main\n\nfunc main() { AGENT_EDIT }\n'
  await pushLoopEvent(page, working.id)
  await expect(diffContent(page)).toContainText('AGENT_EDIT')
})

test('external on-disk change must NOT clobber unsaved local edits', async ({ page }) => {
  let content = MAIN
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: (a) => ({
        path: a[2] || 'src/main.go', language: 'go', content,
        html: '', lines: [], size: content.length, writable: !!a[1],
      }),
    },
  })
  const working = await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('func main() {}')

  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('LOCAL')
  content = 'package main\n\nfunc main() { AGENT_EDIT }\n'
  await pushLoopEvent(page, working.id)
  await page.waitForTimeout(1200)
  await expect(page.locator('#v-detail .cm-content')).toContainText('LOCAL')
  await expect(page.locator('#v-detail .cm-content')).not.toContainText('AGENT_EDIT')
})

test('tab switch restores the saved buffer', async ({ page }) => {
  const files: Record<string, string> = {
    'src/main.go': MAIN,
    'src/other.go': 'package other\n',
  }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) => {
        const path = (a[2] as string) || ''
        if (!path) return [{ name: 'src', path: 'src', dir: true }]
        if (path === 'src') {
          return [
            { name: 'main.go', path: 'src/main.go', dir: false },
            { name: 'other.go', path: 'src/other.go', dir: false },
          ]
        }
        return []
      },
      ListChangedFiles: () => ['src/main.go', 'src/other.go'],
      GitStatus: () => ({ 'src/main.go': 'm', 'src/other.go': 'm' }),
      ReadFile: (a) => {
        const p = (a[2] as string) || 'src/main.go'
        const content = files[p] ?? MAIN
        return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
      },
      // Persist writes so re-opening a tab reads back what was saved, the way
      // the real backend does.
      WriteFile: (a) => { files[a[2] as string] = a[3] as string; return null },
    },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('func main() {}')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('KEEPME')
  await expect(page.locator('#v-detail .cm-content')).toContainText('KEEPME')
  await page.keyboard.press('ControlOrMeta+s')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)

  await openFile(page, 'other.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('package other')

  await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).click()
  const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await codeBtn.count()) await codeBtn.click()
  await expect(page.locator('#v-detail .cm-content')).toContainText('KEEPME')
})

test('unsaved edits survive a tab switch without discarding', async ({ page }) => {
  const files: Record<string, string> = {
    'src/main.go': MAIN,
    'src/other.go': 'package other\n',
  }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) => {
        const path = (a[2] as string) || ''
        if (!path) return [{ name: 'src', path: 'src', dir: true }]
        if (path === 'src') {
          return [
            { name: 'main.go', path: 'src/main.go', dir: false },
            { name: 'other.go', path: 'src/other.go', dir: false },
          ]
        }
        return []
      },
      ListChangedFiles: () => ['src/main.go', 'src/other.go'],
      GitStatus: () => ({ 'src/main.go': 'm', 'src/other.go': 'm' }),
      ReadFile: (a) => {
        const p = (a[2] as string) || 'src/main.go'
        const content = files[p] ?? MAIN
        return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
      },
      WriteFile: (a) => { files[a[2] as string] = a[3] as string; return null },
    },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })

  await openFile(page, 'other.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('package other')
  await expect(page.locator('#v-detail .cm-content')).not.toContainText('KEEPME')
  await expect(page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).locator('.dot')).toBeVisible()

  await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).click()
  const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await codeBtn.count()) await codeBtn.click()
  await expect(page.locator('#v-detail .cm-content')).toContainText('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible()
})

test('unsaved edits survive an app restart', async ({ page }) => {
  const files: Record<string, string> = {
    'src/main.go': MAIN,
    'src/other.go': 'package other\n',
  }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a) => {
        const path = (a[2] as string) || ''
        if (!path) return [{ name: 'src', path: 'src', dir: true }]
        if (path === 'src') {
          return [
            { name: 'main.go', path: 'src/main.go', dir: false },
            { name: 'other.go', path: 'src/other.go', dir: false },
          ]
        }
        return []
      },
      ListChangedFiles: () => ['src/main.go', 'src/other.go'],
      GitStatus: () => ({ 'src/main.go': 'm', 'src/other.go': 'm' }),
      ReadFile: (a) => {
        const p = (a[2] as string) || 'src/main.go'
        const content = files[p] ?? MAIN
        return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
      },
      WriteFile: (a) => { files[a[2] as string] = a[3] as string; return null },
    },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  // Sessions persist Go-side over a debounced RPC now — give the patch time
  // to land in the mock backend before tearing the page down.
  await page.waitForTimeout(600)

  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await code.count()) {
    await expect(code).toBeEnabled()
    await code.click()
    await expect(code).toHaveClass(/on/)
  }
  await expect(page.locator('#v-detail .cm-content')).toContainText('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible()
})

const LONG = Array.from({ length: 80 }, (_, i) => `// line ${i + 1}`).join('\n') + '\n'

function goFileResolvers(files: Record<string, string>) {
  return {
    ListTree: (a: unknown[]) => {
      const path = (a[2] as string) || ''
      if (!path) return [{ name: 'src', path: 'src', dir: true }]
      if (path === 'src') {
        return Object.keys(files)
          .filter((p) => p.startsWith('src/'))
          .map((p) => ({ name: p.slice(4), path: p, dir: false }))
      }
      return []
    },
    ListChangedFiles: () => Object.keys(files),
    GitStatus: () => Object.fromEntries(Object.keys(files).map((p) => [p, 'm'])),
    ReadFile: (a: unknown[]) => {
      const p = (a[2] as string) || 'src/main.go'
      const content = files[p] ?? ''
      return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
    },
    FileAtBase: (a: unknown[]) => files[String(a[2] || '')] ?? '',
  }
}

async function restartWorkspace(page: Page) {
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
}

test('caret and scroll survive an app restart', async ({ page }) => {
  const files = { 'src/main.go': LONG }
  await installBridge(page, structuredClone(seed), { resolvers: goFileResolvers(files) })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await codeContent(page).click()
  await page.keyboard.press('ControlOrMeta+ArrowDown')
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).toContainText(/Ln 8[01]/)
  const scroller = page.locator('#v-detail .obs-surface:not(.off) .cm-scroller')
  await expect.poll(async () => scroller.evaluate((el) => el.scrollTop)).toBeGreaterThan(0)
  await page.waitForTimeout(600)

  await restartWorkspace(page)
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  await expect(code).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).toContainText(/Ln 8[01]/)
  await expect.poll(async () => scroller.evaluate((el) => el.scrollTop)).toBeGreaterThan(0)
})

test('Code chip survives an app restart', async ({ page }) => {
  const files = { 'src/main.go': MAIN }
  await installBridge(page, structuredClone(seed), { resolvers: goFileResolvers(files) })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })).toHaveClass(/on/)
  await page.waitForTimeout(600)

  await restartWorkspace(page)
  await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })).toHaveClass(/on/)
  await expect(codeEditor(page)).toBeVisible()
})

test('Diff chip survives an app restart', async ({ page }) => {
  const files = { 'src/main.go': MAIN }
  await installBridge(page, structuredClone(seed), { resolvers: goFileResolvers(files) })
  await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
  const diff = page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })
  await expect(diff).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
  await page.waitForTimeout(600)

  await restartWorkspace(page)
  await expect(diff).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
})

test('Diff chip survives restart when git status lags the session', async ({ page }) => {
  const files = { 'src/main.go': MAIN }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ...goFileResolvers(files),
      GitStatus: async () => {
        await new Promise((r) => setTimeout(r, 500))
        return { 'src/main.go': 'm' }
      },
    },
  })
  await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
  const diff = page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })
  await expect(diff).toHaveClass(/on/)
  await page.waitForTimeout(600)

  await restartWorkspace(page)
  // Session restore must not flip to Code while GitStatus is still in flight,
  // nor after it lands.
  await expect(diff).toHaveClass(/on/)
  await page.waitForTimeout(700)
  await expect(diff).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
})

test('Preview chip survives an app restart', async ({ page }) => {
  const files = { 'README.md': '# Hello\n\nWorld\n' }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a: unknown[]) => {
        const path = (a[2] as string) || ''
        if (!path) return [{ name: 'README.md', path: 'README.md', dir: false }]
        return []
      },
      ListChangedFiles: () => ['README.md'],
      GitStatus: () => ({ 'README.md': 'm' }),
      ReadFile: (a: unknown[]) => {
        const path = String(a[2] || 'README.md')
        return {
          path, language: 'markdown', content: files[path] ?? '', size: (files[path] ?? '').length, writable: !!a[1],
        }
      },
      FileAtBase: () => '# Hello\n',
    },
  })
  await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'README.md' }).click()
  const preview = page.locator('#v-detail .obs-view-seg button', { hasText: 'Preview' })
  await expect(preview).toBeVisible()
  await preview.click()
  await expect(preview).toHaveClass(/on/)
  await expect(page.locator('#v-detail .file-preview')).toBeVisible()
  await page.waitForTimeout(600)

  await restartWorkspace(page)
  await expect(preview).toHaveClass(/on/)
  await expect(page.locator('#v-detail .file-preview')).toBeVisible()
})

test('quick open filters the file index', async ({ page }) => {
  await openLoopCode(page)
  await page.locator('#v-detail .obs').first().click()
  await page.keyboard.press('ControlOrMeta+p')
  const qo = page.locator('.qo')
  await expect(qo).toBeVisible()
  await page.keyboard.type('main')
  await expect(qo.locator('.qo-row').first()).toContainText('src/main.go')
  await page.keyboard.type('zzzzz')
  await expect(qo.locator('.qo-empty')).toBeVisible()
})

const TWO_FILES: Record<string, string> = {
  'src/main.go': MAIN,
  'src/other.go': 'package other\n',
}

function twoFileResolvers(files: Record<string, string>) {
  return {
    ListTree: (a: unknown[]) => {
      const path = (a[2] as string) || ''
      if (!path) return [{ name: 'src', path: 'src', dir: true }]
      if (path === 'src') {
        return [
          { name: 'main.go', path: 'src/main.go', dir: false },
          { name: 'other.go', path: 'src/other.go', dir: false },
        ]
      }
      return []
    },
    ListChangedFiles: () => Object.keys(files),
    GitStatus: () => Object.fromEntries(Object.keys(files).map((p) => [p, 'm'])),
    ReadFile: (a: unknown[]) => {
      const p = (a[2] as string) || 'src/main.go'
      const content = files[p] ?? MAIN
      return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
    },
    WriteFile: (a: unknown[]) => { files[a[2] as string] = a[3] as string; return null },
  }
}

test('quick open hydrates the picked file without a read-failed toast', async ({ page }) => {
  const files = { ...TWO_FILES }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ...twoFileResolvers(files),
      ListFiles: () => Object.keys(files),
    },
  })
  await openLoopCode(page)
  await page.locator('#v-detail .obs').first().click()
  // Opening a file must not steal the side panel (Search stays selected).
  await page.locator('#v-detail button[aria-label="Search"]').click()
  await expect(page.locator('#v-detail button[aria-label="Search"]')).toHaveAttribute('aria-pressed', 'true')
  await page.keyboard.press('ControlOrMeta+p')
  const qo = page.locator('.qo')
  await expect(qo).toBeVisible()
  await qo.locator('input').fill('other')
  await expect(qo.locator('.qo-row').first()).toContainText('src/other.go')
  await page.keyboard.press('Enter')
  await expect(qo).toHaveCount(0)
  await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('other.go')
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  await expect(code).toBeVisible()
  await expect(code).toBeEnabled()
  await code.click()
  await expect(page.locator('#v-detail .cm-content')).toContainText('package other')
  await expect(page.locator('#v-detail button[aria-label="Search"]')).toHaveAttribute('aria-pressed', 'true')
  await expect(page.locator('#v-detail button[aria-label="Outline"]')).toHaveAttribute('aria-pressed', 'false')
  await expect(page.locator('.toast.error', { hasText: 'Read failed' })).toHaveCount(0)
})

test('quick open scrolls the explorer to the selected file when Files is showing', async ({ page }) => {
  const files: Record<string, string> = { 'src/main.go': MAIN }
  for (let i = 0; i < 40; i++) files[`src/f${String(i).padStart(2, '0')}.go`] = `package f${i}\n`
  const names = Object.keys(files)
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListTree: (a: unknown[]) => {
        const path = (a[2] as string) || ''
        if (!path) return [{ name: 'src', path: 'src', dir: true }]
        if (path === 'src') return names.map((p) => ({ name: p.slice(4), path: p, dir: false }))
        return []
      },
      ListFiles: () => names,
      ListChangedFiles: () => names,
      GitStatus: () => Object.fromEntries(names.map((p) => [p, 'm'])),
      ReadFile: (a: unknown[]) => {
        const p = (a[2] as string) || 'src/main.go'
        const content = files[p] ?? MAIN
        return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
      },
    },
  })
  await openLoopCode(page)
  await page.locator('#v-detail button[aria-label="Files"]').click()
  await expect(page.locator('#v-detail button[aria-label="Files"]')).toHaveAttribute('aria-pressed', 'true')
  const pane = page.locator('#v-detail .obs-files')
  const last = 'src/f39.go'
  await expect(pane.locator(`.obs-node[data-tree-path="${last}"]`)).toBeAttached()
  await pane.evaluate((el) => { el.scrollTop = 0 })
  await expect.poll(async () => pane.evaluate((el, path) => {
    const row = el.querySelector<HTMLElement>(`.obs-node[data-tree-path="${path}"]`)
    if (!row) return false
    const pr = el.getBoundingClientRect()
    const rr = row.getBoundingClientRect()
    return rr.bottom > pr.bottom + 1
  }, last)).toBe(true)

  await page.locator('#v-detail .obs').first().click()
  await page.keyboard.press('ControlOrMeta+p')
  const qo = page.locator('.qo')
  await expect(qo).toBeVisible()
  await qo.locator('input').fill('f39')
  await expect(qo.locator('.qo-row').first()).toContainText(last)
  await page.keyboard.press('Enter')
  await expect(qo).toHaveCount(0)
  await expect(pane.locator('.obs-node.on')).toHaveAttribute('data-tree-path', last)
  await expect.poll(async () => pane.evaluate((el, path) => {
    const row = el.querySelector<HTMLElement>(`.obs-node[data-tree-path="${path}"]`)
    if (!row) return false
    const pr = el.getBoundingClientRect()
    const rr = row.getBoundingClientRect()
    return rr.top >= pr.top - 1 && rr.bottom <= pr.bottom + 1
  }, last)).toBe(true)

  // Re-picking the already-open file must still scroll the explorer.
  await pane.evaluate((el) => { el.scrollTop = 0 })
  await expect.poll(async () => pane.evaluate((el, path) => {
    const row = el.querySelector<HTMLElement>(`.obs-node[data-tree-path="${path}"]`)
    if (!row) return false
    const pr = el.getBoundingClientRect()
    const rr = row.getBoundingClientRect()
    return rr.bottom > pr.bottom + 1
  }, last)).toBe(true)
  await page.keyboard.press('ControlOrMeta+p')
  await expect(qo).toBeVisible()
  await qo.locator('input').fill('f39')
  await expect(qo.locator('.qo-row').first()).toContainText(last)
  await page.keyboard.press('Enter')
  await expect(qo).toHaveCount(0)
  await expect(pane.locator('.obs-node.on')).toHaveAttribute('data-tree-path', last)
  await expect.poll(async () => pane.evaluate((el, path) => {
    const row = el.querySelector<HTMLElement>(`.obs-node[data-tree-path="${path}"]`)
    if (!row) return false
    const pr = el.getBoundingClientRect()
    const rr = row.getBoundingClientRect()
    return rr.top >= pr.top - 1 && rr.bottom <= pr.bottom + 1
  }, last)).toBe(true)
})

test('a single keystroke reaches Diff after the emit debounce', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await codeContent(page).click()
  await page.keyboard.type('X')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  await page.waitForTimeout(200)
  const diff = page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })
  await expect(diff).toBeEnabled()
  await diff.click()
  await expect(diffContent(page)).toContainText('X')
})

test('reverting a restored stash clears the unsaved marker', async ({ page }) => {
  const files = { ...TWO_FILES }
  await installBridge(page, structuredClone(seed), { resolvers: twoFileResolvers(files) })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('X')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })

  await openFile(page, 'other.go')
  await expect(page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).locator('.dot')).toBeVisible()

  await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).click()
  const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await codeBtn.count()) await codeBtn.click()
  await expect(page.locator('#v-detail .cm-content')).toContainText('X')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.press('Backspace')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).not.toContainText('Modified')
})

test('closing the last tab does not restore it after restart', async ({ page }) => {
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('func main() {}')
  await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).locator('.tab-chip-x').click()
  await expect(page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' })).toHaveCount(0)

  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  await expect(page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' })).toHaveCount(0)
  await expect(page.locator('#v-detail .cm-content')).toHaveCount(0)
})

test('re-clicking a file retries after a failed read', async ({ page }) => {
  let reads = 0
  const recovered = 'package main\n\nfunc main() { RETRY_OK }\n'
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: (a) => {
        reads += 1
        if (reads === 1) return new BridgeError('binary or non-UTF-8 file')
        const p = (a[2] as string) || 'src/main.go'
        return {
          path: p, language: 'go', content: recovered,
          html: '', lines: [], size: recovered.length, writable: !!a[1],
        }
      },
    },
  })
  await openLoopCode(page)
  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
  await expect(page.locator('.toast.error', { hasText: 'Read failed' })).toBeVisible()
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  await expect(code).toBeDisabled()
  // Diff still mounts against the base (empty current) — not a stuck Loading overlay.
  await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
  await expect(page.locator('#v-detail .obs-load-overlay')).toHaveCount(0)

  await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
  await expect(code).toBeEnabled()
  await code.click()
  await expect(page.locator('#v-detail .cm-content')).toContainText('RETRY_OK')
})

test('restoring a stash warns when disk changed while the tab was away', async ({ page }) => {
  const files = { ...TWO_FILES }
  await installBridge(page, structuredClone(seed), { resolvers: twoFileResolvers(files) })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })

  await openFile(page, 'other.go')
  files['src/main.go'] = 'package main\n\nfunc main() { AGENT_EDIT }\n'

  await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).click()
  await expect(page.locator('.toast.info', { hasText: 'changed on disk' })).toBeVisible()
  const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await codeBtn.count()) await codeBtn.click()
  await expect(page.locator('#v-detail .cm-content')).toContainText('KEEPME')
  await expect(page.locator('#v-detail .cm-content')).not.toContainText('AGENT_EDIT')
  await codeContent(page).click()
  for (const _ of 'KEEPME') await page.keyboard.press('Backspace')
  // Buffer is back to stash-time disk text, but disk has moved on.
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible()
  await expect(page.locator('#v-detail .obs-statusbar[aria-label="Editor status"]')).toContainText('Modified')
})

// A restored stash carries only the hash of the disk text it diverged from
// (the full saved copy stays in-session only). This pins the restart-time
// drift check: hash of the freshly read file vs the stored baseHash.
test('restoring a stash after a restart warns when disk moved meanwhile', async ({ page }) => {
  const files = { ...TWO_FILES }
  await installBridge(page, structuredClone(seed), { resolvers: twoFileResolvers(files) })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  // Let the debounced session patch reach the mock backend before "restart".
  await page.waitForTimeout(600)

  files['src/main.go'] = 'package main\n\nfunc main() { AGENT_EDIT }\n'
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('.toast.info', { hasText: 'changed on disk' })).toBeVisible()
  const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await codeBtn.count()) {
    await expect(codeBtn).toBeEnabled()
    await codeBtn.click()
  }
  await expect(page.locator('#v-detail .cm-content')).toContainText('KEEPME')
  await expect(page.locator('#v-detail .cm-content')).not.toContainText('AGENT_EDIT')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible()
})

// Split pane reuses the CodeEditor via setState(); a mount-time dirty=false
// used to drop the restored stash before checkStashDiskMoved's hash landed.
test('opening a restored stash in the split pane warns when disk moved', async ({ page }) => {
  const files = { ...TWO_FILES }
  await installBridge(page, structuredClone(seed), { resolvers: twoFileResolvers(files) })
  await openLoopCode(page)
  await openFile(page, 'other.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('OTHER')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })

  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type('KEEPME')
  await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible({ timeout: 250 })
  await page.waitForTimeout(600)

  files['src/other.go'] = 'package other\n\nfunc Other() { AGENT_EDIT }\n'
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'other.go' })).toBeVisible()
  const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await codeBtn.count()) {
    await expect(codeBtn).toBeEnabled()
    await codeBtn.click()
  }

  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.press('ControlOrMeta+\\')
  await expect(splitContent(page)).toBeVisible()
  await page.locator('#v-detail .obs-editor-pane.split-side').click()
  await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'other.go' }).click()
  await expect(page.locator('.toast.info', { hasText: 'changed on disk' })).toBeVisible()
  await expect(splitContent(page)).toContainText('OTHER')
  await expect(splitContent(page)).not.toContainText('AGENT_EDIT')
})
