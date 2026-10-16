import { test, expect, gotoApp, reloadApp, seed, openWorkspaceByGoal, sidebarWorkspace } from './helpers/fixtures'
import { BridgeError, installBridge, mockChatAttention, simulateChatTurnIdle } from './helpers/mock-bridge'
import { codeContent, codeEditor, diffContent } from './helpers/editor'
import type { Page } from '@playwright/test'

// Simulates a Go-side loop.event push over the real Wails event bus (not the
// **/wails/runtime call route, which only answers bound method CALLS). The
// Wails runtime exposes window._wails.dispatchWailsEvent for exactly this —
// it's how the generated client dispatches an incoming server event to every
// Events.On('loop.event', …) listener, which is what Observatory's onFeed /
// scheduleLiveRefresh (src/components/Observatory.svelte) subscribes through.
async function pushLoopEvent(page: Page, loopId: string, text: string) {
  // The Observatory's live refresh keys off the worktree watcher's signal; the
  // feed event rides along for surfaces that render it.
  await page.evaluate(
    ({ loopId, text }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'loop.event',
        data: { loopId, event: { id: `ev-${Date.now()}`, agentCode: 'CO', name: 'Coder', text, time: 'now' } },
      })
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.fs_changed',
        data: { workspaceId: loopId },
      })
    },
    { loopId, text },
  )
}

async function openFocusedAgentChat(page: Page) {
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeVisible()
  await expect.poll(() => {
    const a = mockChatAttention(page)
    return a?.focusWs === working.id ? a.focusCode : ''
  }).toBeTruthy()
  return { working, code: mockChatAttention(page)!.focusCode }
}

function workspaceUnseen(page: Page, goal: string) {
  return sidebarWorkspace(page, goal).locator('.ct')
}

test.describe('code observatory', () => {
  test('project setup has no Code observatory tab', async ({ page }) => {
    // Project-level Observatory was removed — the working-copy workspace is the
    // code surface. Setup is config-only (D-working-copy-observatory).
    await gotoApp(page)
    const proj = seed.projects.find((p) => p.open) ?? seed.projects[0]
    const projRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: proj.name }) })
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.tproj-h').hover()
    await projRow.locator('.pset').click()
    await expect(page.locator('#v-project')).toHaveClass(/on/)
    await expect(page.locator('#v-project .obs')).toHaveCount(0)
    await expect(page.locator('#v-project .set-shell')).toBeVisible()
    await expect(page.locator('#v-project .set-head h2')).toHaveText('General')
    await expect(page.locator('#v-project .set-head-sub')).toContainText(proj.name)
  })

  test('loop detail mounts the observatory', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    // One unified view: the Observatory is the detail body; loops live in chat.
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    // The chat dock leads with the Loop (MA) tab on the shared tab strip.
    await expect(page.locator('#v-detail .obs-left .wsv-chat-tab', { hasText: 'Loop' })).toBeVisible()
    await expect(page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Agent"] .obs-pane-badge')).toHaveCount(0)
  })

  test('agent chat icon shows an unread badge from workspace attention', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeVisible()
    const agentBtn = page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Agent"]')
    await expect(agentBtn.locator('.obs-pane-badge')).toHaveCount(0)
    await expect.poll(() => {
      const a = mockChatAttention(page)
      return a?.focusWs === working.id ? a.focusCode : ''
    }).toBeTruthy()
    const code = mockChatAttention(page)!.focusCode

    // Active tab finishing while Agent is on screen must not raise a badge.
    await simulateChatTurnIdle(page, working.id, code)
    await expect(agentBtn.locator('.obs-pane-badge')).toHaveCount(0)

    // Unread lands while looking at another left panel — the header icon is how you notice.
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Findings"]').click()
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)
    await simulateChatTurnIdle(page, working.id, code)

    await expect(agentBtn.locator('.obs-pane-badge')).toHaveText('1')
    await expect(agentBtn).toHaveAttribute('title', 'Agent (1 unread)')
    await expect(agentBtn).toHaveAttribute('aria-label', 'Agent (1 unread)')
  })

  test('agent chat unread badges when the observatory is off-screen', async ({ page }) => {
    const { working, code } = await openFocusedAgentChat(page)
    const agentBtn = page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Agent"]')
    const obs = page.locator('#v-detail .obs')

    // Terminal maximize hides .obs-left outright — a finish must still raise unread.
    await page.keyboard.press('Control+Shift+`')
    await expect(obs).toHaveClass(/term-max/)
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)
    await simulateChatTurnIdle(page, working.id, code)
    await expect(workspaceUnseen(page, working.goal)).toHaveText('1')

    await page.keyboard.press('Control+Shift+`')
    await expect(obs).not.toHaveClass(/term-max/)
    await expect(agentBtn.locator('.obs-pane-badge')).toHaveCount(0)
  })

  test('agent chat unread badges when navigating to Briefing', async ({ page }) => {
    const { working, code } = await openFocusedAgentChat(page)

    // Views stay mounted (visibility:hidden on #v-detail, not display:none); leaving detail must occlude the transcript.
    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)
    await simulateChatTurnIdle(page, working.id, code)
    await expect(workspaceUnseen(page, working.goal)).toHaveText('1')

    await openWorkspaceByGoal(page, working.goal)
    const agentBtn = page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Agent"]')
    await expect(agentBtn.locator('.obs-pane-badge')).toHaveCount(0)
  })

  test('unread chat appears in inbox and View opens the tab', async ({ page }) => {
    const { working, code } = await openFocusedAgentChat(page)
    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-new').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)
    await expect.poll(() => mockChatAttention(page)?.focusCode).toBe('PA2')
    // Leave PA2 in the background so the inbox target must switch tabs.
    // Select the original tab by identity — nth(0) is Loop when the workspace
    // has a run, so it must not be used as a PA fallback.
    if (code === 'MA') {
      await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    } else {
      await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).first().click()
    }
    await expect.poll(() => mockChatAttention(page)?.focusCode).toBe(code)

    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    await expect.poll(() => mockChatAttention(page)?.occluded).toBe(true)
    await simulateChatTurnIdle(page, working.id, 'PA2')

    const row = page.locator('#briefing-needs .wbrief-row', { hasText: working.short || working.goal })
    await expect(row).toBeVisible()
    await expect(row.locator('.wbrief-unread')).toBeVisible()
    await row.click()

    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect.poll(() => mockChatAttention(page)?.focusCode).toBe('PA2')
    await expect(page.locator('#v-briefing .wbrief-row', { hasText: working.short || working.goal }).locator('.wbrief-unread')).toHaveCount(0)
    await expect(workspaceUnseen(page, working.goal)).toHaveCount(0)
  })

  test('loop Code tab opens CodeMirror editor for a changed file', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    // Changed files open in Diff mode; the editor assertions below are about
    // the CODE surface, so switch explicitly.
    const codeSeg = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeSeg.count()) await codeSeg.click()
    await expect(codeEditor(page)).toBeVisible()
    await expect(page.locator('#v-detail .obs-tb-center .obs-filechip')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs:not(.busy)')).toBeVisible()
    await expect(page.locator('#v-detail .obs-side-title').first()).toHaveText('Files')

    // Active-line gutter fill comes from the editor theme (--cm-active-line),
    // not app-chrome amber. Must be visibly tinted (not CM's near-invisible
    // dark default that matches the gutter background). The left inset bar is
    // the only app-theme accent on the active line.
    await codeContent(page).click()
    const activeGutter = codeEditor(page).locator('.cm-lineNumbers .cm-gutterElement.cm-activeLineGutter').first()
    await expect(activeGutter).toBeVisible()
    const { bg, bar } = await activeGutter.evaluate((ln) => {
      const cs = getComputedStyle(ln)
      const root = ln.closest('.cm-editor')
      const bar = root
        ? [...root.querySelectorAll('.cm-gutters-before .cm-gutterElement.cm-activeLineGutter')]
          .map((el) => getComputedStyle(el).boxShadow)
          .find((s) => /inset/.test(s)) ?? ''
        : cs.boxShadow
      return { bg: cs.backgroundColor, bar }
    })
    expect(bg).toMatch(/rgba?\(/)
    const m = bg.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)/)
    expect(m).toBeTruthy()
    const a = Number(m![4] ?? '1')
    expect(a).toBeGreaterThan(0.03)
    expect(a).toBeLessThan(1)
    // Charcoal active-line wash is neutral white, not clay/amber (diff-red look).
    const [r, g, b] = [Number(m![1]), Number(m![2]), Number(m![3])]
    expect(Math.abs(r - g)).toBeLessThan(8)
    expect(Math.abs(g - b)).toBeLessThan(8)
    expect(bar).toMatch(/inset/)
  })

  test('save keeps the file tab selected and syntax highlighting', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    const codeSeg = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeSeg.count()) {
      await expect(codeSeg).toBeEnabled()
      await codeSeg.click()
    }
    await expect(codeEditor(page)).toBeVisible()

    const highlighted = codeContent(page).locator('span[class*="cm-tm-"]')
    await expect(highlighted.first()).toBeVisible()
    const before = await highlighted.count()
    expect(before).toBeGreaterThan(0)

    await codeContent(page).click()
    await page.keyboard.type(' // edit')
    await expect(page.locator('#v-detail .obs-filechip.on .dot')).toBeVisible()
    await page.keyboard.press('ControlOrMeta+s')

    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-filechip.on .dot')).toHaveCount(0)
    await expect(highlighted.first()).toBeVisible()
    expect(await highlighted.count()).toBeGreaterThan(0)
  })

  test('opening a folder in the explorer keeps highlighting and breadcrumbs', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    // Changed files open in Diff; the editor chrome under test is the Code surface.
    const codeSeg = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeSeg.count()) await codeSeg.click()
    await expect(codeEditor(page)).toBeVisible()

    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-crumbbar .crumb.file')).toHaveText('main.go')
    await expect(codeContent(page)).toContainText('package main')

    await page.locator('#v-detail .obs-files .obs-node.dir', { hasText: 'src' }).click()

    await expect(page.locator('#v-detail .obs-files .obs-node.dir', { hasText: 'src' })).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-crumbbar .crumb.file')).toHaveText('main.go')
    await expect(codeContent(page)).toContainText('package main')
    await expect(page.locator('#v-detail .obs-editor-pane .obs-empty')).toHaveCount(0)
  })

  test('double-clicking a file or folder in the explorer starts a rename', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const input = page.locator('#v-detail .obs-files .obs-rename')
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).dblclick()
    await expect(input).toBeVisible()
    await expect(input).toHaveValue('main.go')
    await expect(input).toHaveAttribute('aria-label', 'Rename')
    await page.keyboard.press('Escape')
    await expect(input).toHaveCount(0)
    await page.locator('#v-detail .obs-files .obs-node.dir', { hasText: 'src' }).dblclick()
    await expect(input).toBeVisible()
    await expect(input).toHaveValue('src')
  })

  test('dragging an explorer row onto a folder moves it there', async ({ page }) => {
    const renames: string[][] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => {
          const path = String(a[2] || '')
          if (!path) return [
            { name: 'notes.txt', path: 'notes.txt', dir: false },
            { name: 'src', path: 'src', dir: true },
          ]
          if (path === 'src') return [{ name: 'main.go', path: 'src/main.go', dir: false }]
          return []
        },
        Rename: (a) => {
          renames.push([String(a[2]), String(a[3])])
          return null
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-scope button', { hasText: 'All files' }).click()
    const files = page.locator('#v-detail .obs-files')
    await files.locator('.obs-node', { hasText: 'notes.txt' }).dragTo(files.locator('.obs-node.dir', { hasText: 'src' }))
    await expect.poll(() => renames).toEqual([['notes.txt', 'src/notes.txt']])
    await files.locator('.obs-node.dir', { hasText: 'src' }).dragTo(files.locator('.obs-node', { hasText: 'notes.txt' }))
    await expect.poll(() => renames.length).toBe(1)
  })

  test('double-click rename stays focused after a slow All-scope file open', async ({ page }) => {
    const body = 'hello notes\n'
    let outlineDone = false
    const delay = () => new Promise((r) => setTimeout(r, 250))
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => {
          const path = String(a[2] || '')
          if (!path) return [
            { name: 'notes.txt', path: 'notes.txt', dir: false },
            { name: 'src', path: 'src', dir: true },
          ]
          if (path === 'src') return [{ name: 'main.go', path: 'src/main.go', dir: false }]
          return []
        },
        ReadFile: async (a) => {
          await delay()
          const path = String(a[2] || 'notes.txt')
          const content = path === 'notes.txt' ? body : 'package main\n\nfunc main() {}\n'
          return { path, language: path.endsWith('.txt') ? 'text' : 'go', content, size: content.length, writable: !!a[1] }
        },
        Outline: async () => {
          await delay()
          outlineDone = true
          return []
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-scope button', { hasText: 'All files' }).click()
    const row = page.locator('#v-detail .obs-files .obs-node', { hasText: 'notes.txt' })
    await expect(row).toBeVisible()
    const input = page.locator('#v-detail .obs-files .obs-rename')
    await row.dblclick()
    await expect(input).toBeVisible()
    await expect(input).toHaveValue('notes.txt')
    await expect.poll(() => outlineDone).toBe(true)
    await expect(codeContent(page)).toContainText('hello notes')
    await page.evaluate(() => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))))
    await expect(input).toBeVisible()
    await expect(input).toBeFocused()
  })

  test('changed-files list updates live as the coding agent edits, without navigation', async ({ page }) => {
    let changed = ['src/main.go']
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListChangedFiles: () => changed,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    await expect(page.locator('#v-detail .obs-scope .cnt')).toHaveText('1')
    await expect(page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' })).toBeVisible()

    // Simulate the coding agent creating a new file: the backend's next
    // ListChangedFiles poll (triggered by the feed event below) picks it up.
    changed = ['src/main.go', 'src/new_feature.go']
    await pushLoopEvent(page, working.id, 'Write: <code>src/new_feature.go</code>')

    // No click, no reload — the tree/changed-files panel is expected to
    // reflect the new file on its own, in near-real-time.
    await expect(page.locator('#v-detail .obs-scope .cnt')).toHaveText('2')
    await expect(
      page.locator('#v-detail .obs-files .obs-node', { hasText: 'new_feature.go' }),
    ).toBeVisible()
  })

  test('an already-open file re-reads its content live when the coding agent edits it on disk', async ({
    page,
  }) => {
    let content = 'package main\n\nfunc main() {}\n'
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ReadFile: (a) => ({
          path: a[2] || 'src/main.go',
          language: 'go',
          content,
          html: content,
          lines: content.split('\n'),
          size: content.length,
          writable: !!a[1],
        }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    const codeSeg2 = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeSeg2.count()) await codeSeg2.click()
    await expect(codeEditor(page)).toBeVisible()
    await expect(codeContent(page)).toContainText('func main() {}')

    // Simulate the coding agent overwriting the open file on disk mid-loop.
    content = 'package main\n\nfunc main() {\n\tprintln("patched by agent")\n}\n'
    await pushLoopEvent(page, working.id, 'Edit: <code>src/main.go</code>')

    // The pane the user is already looking at should pick up the change on
    // its own — no re-click, no reopen.
    await expect(codeContent(page)).toContainText('patched by agent')
    // And the tab strip must not have been disturbed by the refresh.
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
  })

  test('⌘⇧V cycles the editor view chip through Diff and Code', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    const diff = page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })
    const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    await expect(diff).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-diff')).toBeVisible()

    await page.keyboard.press('ControlOrMeta+Shift+v')
    await expect(code).toHaveClass(/on/)
    await expect(codeEditor(page)).toBeVisible()

    await page.keyboard.press('ControlOrMeta+Shift+v')
    await expect(diff).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
  })

  test('binary images skip Code; modified ones offer Preview and a visual Diff', async ({ page }) => {
    const png = Buffer.from([
      0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
      0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
      0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00,
      0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
      0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00,
      0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ])
    await page.route('**/workspace-file?*', async (route) => {
      const url = new URL(route.request().url())
      const at = url.searchParams.get('at')
      const body = at === 'base' ? png : Buffer.from(png)
      body[body.length - 1] = at === 'base' ? png[png.length - 1]! : (png[png.length - 1]! ^ 0xff)
      await route.fulfill({ status: 200, contentType: 'image/png', body })
    })
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => {
          const path = (a[2] as string) || ''
          if (!path) return [{ name: 'src', path: 'src', dir: true }]
          if (path === 'src') {
            return [
              { name: 'main.go', path: 'src/main.go', dir: false },
              { name: 'shot.png', path: 'src/shot.png', dir: false, status: 'u' },
              { name: 'icon.png', path: 'src/icon.png', dir: false, status: 'm' },
              { name: 'gone.png', path: 'src/gone.png', dir: false, status: 'd' },
            ]
          }
          return []
        },
        ListChangedFiles: () => ['src/main.go', 'src/shot.png', 'src/icon.png', 'src/gone.png'],
        GitStatus: () => ({ 'src/main.go': 'm', 'src/shot.png': 'u', 'src/icon.png': 'm', 'src/gone.png': 'd' }),
        WorkingStatus: () => ({ 'src/main.go': 'm', 'src/shot.png': 'u', 'src/icon.png': 'm', 'src/gone.png': 'd' }),
        ReadFile: (a) => {
          const path = String(a[2] || '')
          if (path.endsWith('.png')) {
            return { path, language: '', content: '', size: png.length, writable: !!a[1], mime: 'image/png' }
          }
          return {
            path, language: 'go', content: 'package main\n\nfunc main() {}\n', size: 32, writable: !!a[1],
          }
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const findBtn = page.locator('#v-detail button[aria-label="Find"]')
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'shot.png' }).click()
    await expect(page.locator('#v-detail .obs-image img')).toHaveCount(1)
    await expect(page.locator('#v-detail .obs-view-seg')).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-diff')).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-image.compare')).toHaveCount(0)
    await expect(findBtn).toBeDisabled()
    await expect(page.locator('#v-detail button[aria-label="Full screen"]')).toBeVisible()
    await page.locator('#v-detail .obs-image img').click()
    const lb = page.locator('.chat-att-lb')
    await expect(lb.locator('img')).toHaveAttribute('src', /path=src%2Fshot\.png/)
    await page.keyboard.press('Escape')
    await expect(lb).toHaveCount(0)

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'icon.png' }).click()
    const seg = page.locator('#v-detail .obs-view-seg')
    const preview = seg.locator('button', { hasText: 'Preview' })
    const diff = seg.locator('button', { hasText: 'Diff' })
    await expect(preview).toBeVisible()
    await expect(diff).toHaveClass(/on/)
    await expect(seg.locator('button', { hasText: 'Code' })).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-image.compare img')).toHaveCount(2)
    await expect(page.locator('#v-detail .obs-image img').first()).toHaveAttribute('src', /at=base/)
    await expect(page.locator('#v-detail .obs-diff')).toHaveCount(0)
    await expect(findBtn).toBeDisabled()

    await preview.click()
    await expect(preview).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-image.compare')).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-image img')).toHaveCount(1)
    await expect(page.locator('#v-detail .obs-image img').first()).not.toHaveAttribute('src', /at=base/)

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'gone.png' }).click()
    await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Preview' })).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-image img')).toHaveCount(1)
    await expect(page.locator('#v-detail .obs-image img').first()).toHaveAttribute('src', /at=base/)
    await expect(findBtn).toBeDisabled()
  })

  test('videos play in Preview and zoom into the lightbox', async ({ page }) => {
    await page.route('**/workspace-file?*', (route) => route.fulfill({ status: 200, contentType: 'video/mp4', body: '' }))
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => (a[2] ? [] : [{ name: 'clip.mp4', path: 'clip.mp4', dir: false, status: 'u' }]),
        ListChangedFiles: () => ['clip.mp4'],
        GitStatus: () => ({ 'clip.mp4': 'u' }),
        WorkingStatus: () => ({ 'clip.mp4': 'u' }),
        ReadFile: (a) => ({ path: String(a[2]), language: '', content: '', size: 2048, writable: !!a[1], mime: 'video/mp4' }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'clip.mp4' }).click()
    await expect(page.locator('#v-detail .obs-image video')).toHaveAttribute('src', /path=clip\.mp4/)
    await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })).toHaveCount(0)
    await expect(page.locator('#v-detail button[aria-label="Full screen"]')).toBeVisible()

    await page.locator('#v-detail button[aria-label="Zoom video"]').click()
    const lb = page.locator('.chat-att-lb')
    await expect(lb.locator('video')).toHaveAttribute('src', /path=clip\.mp4/)
    await expect(lb.locator('button[aria-label="Copy image"]')).toHaveCount(0)
    await page.keyboard.press('Escape')
    await expect(lb).toHaveCount(0)
  })

  test('⌘⇧V cycles Code, Diff, and Preview when the chip offers all three', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => {
          const path = (a[2] as string) || ''
          if (!path) {
            return [
              { name: 'README.md', path: 'README.md', dir: false },
              { name: 'src', path: 'src', dir: true },
            ]
          }
          if (path === 'src') return [{ name: 'main.go', path: 'src/main.go', dir: false }]
          return []
        },
        ListChangedFiles: () => ['src/main.go', 'README.md'],
        GitStatus: () => ({ 'src/main.go': 'm', 'README.md': 'm' }),
        WorkingStatus: () => ({ 'src/main.go': 'm', 'README.md': 'm' }),
        ReadFile: (a) => {
          const path = String(a[2] || 'src/main.go')
          if (path.endsWith('.md')) {
            return {
              path, language: 'markdown', content: '# Hello\n\nWorld\n', size: 15, writable: !!a[1],
            }
          }
          return {
            path, language: 'go', content: 'package main\n\nfunc main() {}\n', size: 32, writable: !!a[1],
          }
        },
        FileAtBase: (a) => String(a[2] || '').endsWith('.md') ? '# Hello\n' : 'package main\n\nfunc main() {}\n',
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'README.md' }).click()
    const seg = page.locator('#v-detail .obs-view-seg')
    const diff = seg.locator('button', { hasText: 'Diff' })
    const preview = seg.locator('button', { hasText: 'Preview' })
    const code = seg.locator('button', { hasText: 'Code' })
    await expect(preview).toBeVisible()
    await expect(diff).toHaveClass(/on/)

    await page.keyboard.press('ControlOrMeta+Shift+v')
    await expect(preview).toHaveClass(/on/)
    await expect(page.locator('#v-detail .file-preview')).toBeVisible()

    await page.keyboard.press('ControlOrMeta+Shift+v')
    await expect(code).toHaveClass(/on/)
    await expect(codeEditor(page)).toBeVisible()

    await page.keyboard.press('ControlOrMeta+Shift+v')
    await expect(diff).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
  })

  test('switching back to a workspace keeps file-tab and editor scroll', async ({ page }) => {
    const [A, B] = seed.loops.filter((l) => l.group === 'active')
    const files = ['src/main.go']
    for (let i = 0; i < 12; i++) {
      files.push(`src/long-file-name-${String(i).padStart(2, '0')}.go`)
    }
    const longBody = ['package main', '', 'func main() {']
    for (let i = 0; i < 120; i++) longBody.push(`\tprintln("line ${i}")`)
    longBody.push('}', '')
    const main = longBody.join('\n')

    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => {
          const path = (a[2] as string) || ''
          if (!path) return [{ name: 'src', path: 'src', dir: true }]
          if (path === 'src') {
            return files.map((p) => ({ name: p.slice(4), path: p, dir: false }))
          }
          return []
        },
        ListChangedFiles: () => files,
        GitStatus: () => Object.fromEntries(files.map((p) => [p, 'm'])),
        ReadFile: (a) => {
          const path = (a[2] as string) || 'src/main.go'
          const content = path === 'src/main.go' ? main : `package p\n`
          return { path, language: 'go', content, size: content.length, writable: !!a[1] }
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, A.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    const codeBtn = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeBtn.count()) {
      await expect(codeBtn).toBeEnabled()
      await codeBtn.click()
      await expect(codeBtn).toHaveClass(/on/)
    }
    await expect(codeEditor(page)).toBeVisible()
    for (const name of files.slice(1)) {
      await page.locator('#v-detail .obs-files .obs-node', { hasText: name.slice(4) }).click()
    }
    await page.locator('#v-detail .obs-tb-center .obs-filechip', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    if (await codeBtn.count()) {
      await expect(codeBtn).toBeEnabled()
      await codeBtn.click()
      await expect(codeBtn).toHaveClass(/on/)
    }
    await expect(codeEditor(page)).toBeVisible()

    const strip = page.locator('#v-detail .obs-tb-center .tab-strip-scroll')
    await expect.poll(() => strip.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(60)
    await expect.poll(() => strip.evaluate((el) => {
      const chip = el.querySelector<HTMLElement>('.tab-chip.on')
      if (!chip) return 0
      const sr = el.getBoundingClientRect()
      const cr = chip.getBoundingClientRect()
      return Math.min(cr.right, sr.right) - Math.max(cr.left, sr.left)
    })).toBeGreaterThan(24)
    const tabScroll = await strip.evaluate((el) => {
      el.scrollLeft = Math.min(120, Math.max(40, el.scrollWidth - el.clientWidth))
      return el.scrollLeft
    })
    expect(tabScroll).toBeGreaterThan(0)

    const scroller = page.locator('#v-detail .obs-editor-pane:not(.split-side) .obs-surface:not(.off) .cm-scroller')
    await scroller.hover()
    for (let i = 0; i < 10; i++) await page.mouse.wheel(0, 240)
    // The last wheel tick is still in CodeMirror's input queue; read after it
    // lands so keep-alive restore is compared to the offset the user saw.
    let prev = -1
    await expect.poll(async () => {
      const top = await scroller.evaluate((el) => el.scrollTop)
      const stable = top > 80 && top === prev
      prev = top
      return stable
    }).toBe(true)
    const editorScroll = prev
    expect(editorScroll).toBeGreaterThan(80)

    await openWorkspaceByGoal(page, B.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await openWorkspaceByGoal(page, A.goal)
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')

    await expect.poll(() => strip.evaluate((el) => el.scrollLeft)).toBe(tabScroll)
    // Wheel offset vs CodeMirror's parked scroll snapshot still lands ~3
    // lines off in this harness. 64px fails a jump-to-caret (editorScroll > 80).
    await expect.poll(async () => {
      const top = await scroller.evaluate((el) => el.scrollTop)
      return Math.abs(top - editorScroll)
    }).toBeLessThan(64)

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'long-file-name-11.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('long-file-name-11')
    await expect.poll(() => strip.evaluate((el) => {
      const chip = el.querySelector<HTMLElement>('.tab-chip.on')
      if (!chip) return 0
      const sr = el.getBoundingClientRect()
      const cr = chip.getBoundingClientRect()
      return Math.min(cr.right, sr.right) - Math.max(cr.left, sr.left)
    })).toBeGreaterThan(24)
  })

  test('opening an off-screen file tab scrolls it into view', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const files = ['src/main.go']
    for (let i = 0; i < 12; i++) {
      files.push(`src/long-file-name-${String(i).padStart(2, '0')}.go`)
    }

    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListTree: (a) => {
          const path = (a[2] as string) || ''
          if (!path) return [{ name: 'src', path: 'src', dir: true }]
          if (path === 'src') {
            return files.map((p) => ({ name: p.slice(4), path: p, dir: false }))
          }
          return []
        },
        ListChangedFiles: () => files,
        GitStatus: () => Object.fromEntries(files.map((p) => [p, 'm'])),
        ReadFile: (a) => {
          const path = (a[2] as string) || 'src/main.go'
          const content = path === 'src/main.go' ? 'package main\n' : 'package p\n'
          return { path, language: 'go', content, size: content.length, writable: !!a[1] }
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    for (const name of files.slice(1)) {
      await page.locator('#v-detail .obs-files .obs-node', { hasText: name.slice(4) }).click()
    }
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('long-file-name-11')

    const strip = page.locator('#v-detail .obs-tb-center .tab-strip-scroll')
    await expect.poll(() => strip.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(60)
    const mainOffscreen = await strip.evaluate((el) => {
      const chip = [...el.querySelectorAll<HTMLElement>('.obs-filechip')]
        .find((c) => (c.textContent || '').includes('main.go'))
      if (!chip) return false
      const sr = el.getBoundingClientRect()
      const cr = chip.getBoundingClientRect()
      return Math.min(cr.right, sr.right) - Math.max(cr.left, sr.left) < 8
    })
    expect(mainOffscreen).toBe(true)

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect.poll(() => strip.evaluate((el) => {
      const chip = el.querySelector<HTMLElement>('.tab-chip.on')
      if (!chip) return 0
      const sr = el.getBoundingClientRect()
      const cr = chip.getBoundingClientRect()
      return Math.min(cr.right, sr.right) - Math.max(cr.left, sr.left)
    })).toBeGreaterThan(24)
  })

  test('Changed-filter open collapses unchanged lines on first Diff paint', async ({ page }) => {
    // collapseUnchanged needs a stretch of ≥6 lines (minSize) beyond the 3-line
    // margin. FileAtBase is instant; ReadFile is delayed so the old race —
    // mount merge with an empty current doc, then swap the real buffer in —
    // would paint every hunk expanded until Code → Diff remounted.
    const preamble = Array.from({ length: 20 }, (_, i) => `const a${i} = ${i}`)
    const tail = Array.from({ length: 20 }, (_, i) => `const b${i} = ${i}`)
    const base = ['package main', '', ...preamble, 'func main() { println("old") }', ...tail, ''].join('\n')
    const current = ['package main', '', ...preamble, 'func main() { println("new") }', ...tail, ''].join('\n')
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListChangedFiles: () => ['src/main.go'],
        GitStatus: () => ({ 'src/main.go': 'm' }),
        WorkingStatus: () => ({ 'src/main.go': 'm' }),
        FileAtBase: () => base,
        ReadFile: async (a) => {
          await new Promise((r) => setTimeout(r, 250))
          const path = String(a[2] || 'src/main.go')
          return { path, language: 'go', content: current, size: current.length, writable: !!a[1] }
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs-scope button', { hasText: 'Changed' })).toHaveAttribute('aria-pressed', 'true')
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    const diff = page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })
    await expect(diff).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-diff')).toBeVisible()
    await expect(diffContent(page)).toContainText('println("new")')
    await expect(page.locator('#v-detail .obs-diff .cm-collapsedLines').first()).toBeVisible()
    await expect(page.locator('#v-detail .obs-diff .cm-line', { hasText: 'const a10 = 10' })).toHaveCount(0)
  })

  test('agent edit while Diff is open recollapses unchanged lines', async ({ page }) => {
    const preamble = Array.from({ length: 20 }, (_, i) => `const a${i} = ${i}`)
    const tail = Array.from({ length: 20 }, (_, i) => `const b${i} = ${i}`)
    const hunk = (mid: string) =>
      ['package main', '', ...preamble, `func main() { println("${mid}") }`, ...tail, ''].join('\n')
    const base = hunk('old')
    let current = hunk('new')
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListChangedFiles: () => ['src/main.go'],
        GitStatus: () => ({ 'src/main.go': 'm' }),
        WorkingStatus: () => ({ 'src/main.go': 'm' }),
        FileAtBase: () => base,
        ReadFile: (a) => {
          const path = String(a[2] || 'src/main.go')
          return { path, language: 'go', content: current, size: current.length, writable: !!a[1] }
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-view-seg button', { hasText: 'Diff' })).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-diff .cm-collapsedLines').first()).toBeVisible()

    current = hunk('newer')
    await pushLoopEvent(page, working.id, 'agent edit')
    await expect(diffContent(page)).toContainText('println("newer")')
    await expect(page.locator('#v-detail .obs-diff .cm-collapsedLines').first()).toBeVisible()
    await expect(page.locator('#v-detail .obs-diff .cm-line', { hasText: 'const a10 = 10' })).toHaveCount(0)
  })

  test('artifacts pane badges unseen creates and present_artifact opens the pane', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const artifactBody = '# Badge test\n\nPresented body.\n'
    await page.route(`**/workspaces/${working.id}/artifacts/art-unseen-1.md`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/markdown; charset=utf-8', body: artifactBody })
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    const artifactsBtn = page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Artifacts"]')
    await expect(artifactsBtn).toBeVisible()
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveCount(0)

    const artifact = {
      id: 'art-unseen-1',
      title: 'Badge test',
      kind: 'md',
      path: `workspaces/${working.id}/artifacts/art-unseen-1.md`,
    }
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'artifact.created',
          data: { workspaceId, artifact },
        })
      },
      { workspaceId: working.id, artifact },
    )
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveText('1')
    await expect(artifactsBtn).toHaveAttribute('title', 'Artifacts (1 new)')

    // list_artifacts (and other tool results) update the workspaces store and
    // re-pass an equal workspaceId — that must not clear the unseen badge.
    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.updated',
        data: { workspaceId, phase: 'working', iteration: 1 },
      })
    }, { workspaceId: working.id })
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveText('1')

    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'artifact.presented',
          data: { workspaceId, artifact },
        })
      },
      { workspaceId: working.id, artifact },
    )
    await expect(artifactsBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(page.locator('#v-detail .art-panel')).toBeVisible()
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveCount(0)
    const chip = page.locator('#v-detail .obs-filechip.on')
    await expect(chip).toContainText('Badge test')
    await expect(chip).toHaveAttribute('title', /scratch:\/\/artifact-art-unseen-1\.md/)
  })

  test('artifact full screen fills the window and Esc restores it', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const artifact = {
      id: 'art-full-1',
      title: 'Full screen',
      kind: 'md',
      path: `workspaces/${working.id}/artifacts/art-full-1.md`,
    }
    await page.route(`**/workspaces/${working.id}/artifacts/art-full-1.md`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/markdown; charset=utf-8', body: '# Full screen\n' })
    })
    await page.setViewportSize({ width: 1800, height: 900 })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-crumb-actions button[aria-label="Reload"]')).toBeVisible()
    await expect(page.locator('#v-detail .obs-crumb-actions button[aria-label="Full screen"]')).toHaveCount(0)
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'artifact.presented', data: { workspaceId, artifact } })
      },
      { workspaceId: working.id, artifact },
    )
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('Full screen')

    const main = page.locator('#v-detail .obs-main')
    const preview = page.locator('#v-detail .obs-view-seg button', { hasText: 'Preview' })
    await page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' }).click()
    await expect(preview).not.toHaveClass(/on/)
    await page.locator('#v-detail .obs-crumb-actions button[aria-label="Full screen"]').click()
    await expect(main).toHaveClass(/fullscreen/)
    await expect(preview).toHaveClass(/on/)
    expect(await main.boundingBox()).toEqual({ x: 0, y: 0, width: 1800, height: 900 })
    await expect(page.locator('#v-detail .obs-crumbbar')).toBeHidden()
    await expect(page.locator('#v-detail .obs-statusbar')).toBeHidden()
    const titlebarHit = await page.evaluate(() => !!document.elementFromPoint(900, 20)?.closest('.titlebar'))
    expect(titlebarHit).toBe(false)

    const exit = page.locator('#v-detail .obs-fs-exit button[aria-label="Exit full screen"]')
    await expect(exit).toHaveCSS('opacity', '0', { timeout: 5000 })
    await page.mouse.move(900, 450)
    await page.mouse.move(910, 460)
    await expect(exit).toHaveCSS('opacity', '1')
    await exit.click()
    await expect(main).not.toHaveClass(/fullscreen/)

    await page.locator('#v-detail .obs-crumb-actions button[aria-label="Full screen"]').click()
    await expect(main).toHaveClass(/fullscreen/)
    await expect.poll(() => page.evaluate(() => !!document.activeElement?.closest('.obs-editor-pane'))).toBe(true)
    await page.keyboard.press('Escape')
    await expect(main).not.toHaveClass(/fullscreen/)
    await expect(page.locator('#v-detail')).toHaveClass(/\bon\b/)
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('Full screen')

    const html = { id: 'art-full-html', title: 'Full html', kind: 'html', path: `workspaces/${working.id}/artifacts/art-full-html.html` }
    await page.route(`**/workspaces/${working.id}/artifacts/art-full-html.html`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/html', body: '<p>Full html</p>' })
    })
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'artifact.presented', data: { workspaceId, artifact } })
      },
      { workspaceId: working.id, artifact: html },
    )
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('Full html')
    await page.locator('#v-detail .obs-crumb-actions button[aria-label="Full screen"]').click()
    await expect(main).toHaveClass(/fullscreen/)
    await expect.poll(() => page.evaluate(() => document.activeElement?.tagName)).toBe('IFRAME')
    await page.keyboard.press('Escape')
    await expect(main).not.toHaveClass(/fullscreen/)
    await expect(page.locator('#v-detail')).toHaveClass(/\bon\b/)

    await page.locator('#v-detail .obs-crumb-actions button[aria-label="Full screen"]').click()
    await expect(main).toHaveClass(/fullscreen/)
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({ name: 'menu.action', data: 'settings' })
    })
    await expect(page.locator('#v-settings')).toHaveClass(/\bon\b/)
    await expect(main).not.toHaveClass(/fullscreen/)
  })

  test('large html artifact keeps its body after its workspace shell is evicted', async ({ page }) => {
    const [A, B, C] = seed.loops.filter((l) => l.group === 'active')
    const artifact = { id: 'art-big-html', title: 'Big html', kind: 'html', path: `workspaces/${A.id}/artifacts/art-big-html.html` }
    const body = '<p>Big artifact</p><!--' + 'x'.repeat(1_200_000) + '-->'
    await page.route(`**/workspaces/${A.id}/artifacts/art-big-html.html`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/html', body })
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, A.goal)
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'artifact.presented', data: { workspaceId, artifact } })
      },
      { workspaceId: A.id, artifact },
    )
    const iframe = page.locator('#v-detail .file-preview-html iframe')
    await expect(iframe).toHaveAttribute('srcdoc', /Big artifact/)
    // Two other workspaces push A out of the hot shells, so it remounts from the keep-alive snapshot.
    await openWorkspaceByGoal(page, B.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await openWorkspaceByGoal(page, C.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await openWorkspaceByGoal(page, A.goal)
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('Big html')
    await expect(iframe).toHaveAttribute('srcdoc', /Big artifact/)
  })

  test('clicking an artifact leaves Review changes and opens it in the editor', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const artifact = {
      id: 'art-from-review-1',
      title: 'From review',
      kind: 'md',
      path: `workspaces/${working.id}/artifacts/art-from-review-1.md`,
    }
    await page.route(`**/workspaces/${working.id}/artifacts/art-from-review-1.md`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/markdown; charset=utf-8', body: '# From review\n' })
    })
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListArtifacts: () => [artifact] },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    await page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Artifacts"]').click()
    await expect(page.locator('#v-detail .art-panel .art-row', { hasText: 'From review' })).toBeVisible()

    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeVisible()
    await expect(page.locator('#v-detail .obs-editors')).toBeHidden()

    await page.locator('#v-detail .art-panel .art-row', { hasText: 'From review' }).click()
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeHidden()
    await expect(page.locator('#v-detail .obs-editors')).toBeVisible()
    const chip = page.locator('#v-detail .obs-filechip.on')
    await expect(chip).toContainText('From review')
    await expect(chip).toHaveAttribute('title', /scratch:\/\/artifact-art-from-review-1\.md/)
  })

  test('refocusing the editor on an artifact tab does not outline it from disk', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const artifact = {
      id: 'art-outline-1',
      title: 'No outline',
      kind: 'md',
      path: `workspaces/${working.id}/artifacts/art-outline-1.md`,
    }
    await page.route(`**/workspaces/${working.id}/artifacts/art-outline-1.md`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/markdown; charset=utf-8', body: '# No outline\n' })
    })
    let scratchOutlines = 0
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        Outline: (a) => {
          if (!String(a[2]).startsWith('scratch://')) return []
          scratchOutlines += 1
          return new BridgeError('no such file')
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'artifact.presented', data: { workspaceId, artifact } })
      },
      { workspaceId: working.id, artifact },
    )
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('No outline')

    await page.keyboard.press('Meta+1')
    await page.keyboard.press('Meta+2')
    await expect.poll(() => page.evaluate(() => !!document.activeElement?.closest('#v-detail .obs-main'))).toBe(true)
    await expect(page.locator('.toast.error')).toHaveCount(0)
    expect(scratchOutlines).toBe(0)
  })

  test('Reload on an artifact tab refetches it from the artifact store', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const artifact = {
      id: 'art-reload-1',
      title: 'Reload me',
      kind: 'md',
      path: `workspaces/${working.id}/artifacts/art-reload-1.md`,
    }
    let body = '# Version one\n'
    await page.route(`**/workspaces/${working.id}/artifacts/art-reload-1.md`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/markdown; charset=utf-8', body })
    })
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListArtifacts: () => [artifact] },
    })
    await page.setViewportSize({ width: 1800, height: 900 })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'artifact.presented', data: { workspaceId, artifact } })
      },
      { workspaceId: working.id, artifact },
    )
    const surface = page.locator('#v-detail .obs-surface:not(.off)')
    await expect(surface).toContainText('Version one')

    body = '# Version two\n'
    await page.locator('#v-detail .obs-crumb-actions button[aria-label="Reload"]').click()
    await expect(surface).toContainText('Version two')
    await expect(surface).not.toContainText('Version one')
  })

  test('artifact rename via context menu and hover trash delete', async ({ page }) => {
    const working = seed.loops.find((l) => l.group === 'active')!
    const artifact = {
      id: 'art-rename-1',
      title: 'Old name',
      kind: 'md',
      path: `workspaces/${working.id}/artifacts/art-rename-1.md`,
    }
    await page.route(`**/workspaces/${working.id}/artifacts/art-rename-1.md`, async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/markdown; charset=utf-8', body: '# Body\n' })
    })
    let deleted = false
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListArtifacts: () => (deleted ? [] : [artifact]),
        RenameArtifact: (a) => Object.assign(artifact, { title: String(a[2]) }),
        DeleteArtifact: () => { deleted = true; return null },
      },
    })
    await page.setViewportSize({ width: 1800, height: 900 })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({ name: 'artifact.presented', data: { workspaceId, artifact } })
      },
      { workspaceId: working.id, artifact },
    )
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('Old name')

    await page.locator('.art-row', { hasText: 'Old name' }).click({ button: 'right' })
    const menu = page.locator('.obs-ctx[role="menu"]')
    await expect(menu.getByRole('menuitem')).toHaveText(['Rename…', 'Reveal in Finder', 'Delete'])
    await menu.getByRole('menuitem', { name: 'Rename…' }).click()
    const input = page.locator('.art-rename')
    await input.fill('New name')
    await input.press('Enter')
    await expect(page.locator('.art-row', { hasText: 'New name' })).toBeVisible()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('New name')

    const row = page.locator('.art-item', { hasText: 'New name' })
    await row.hover()
    await row.getByRole('button', { name: 'Delete New name' }).click()
    await row.getByRole('button', { name: 'Confirm delete New name' }).click()
    await expect(row).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-filechip', { hasText: 'New name' })).toHaveCount(0)
  })

  test('artifacts badge increments while Observatory is off-screen', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const artifactsBtn = page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Artifacts"]')
    await expect(artifactsBtn).toBeVisible()
    // Select Artifacts first — otherwise artifactsPanelFocused is already false
    // (rightKind defaults to files) and the test would pass without the
    // $view === 'detail' / !termOverlaying gates.
    await artifactsBtn.click()
    await expect(artifactsBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveCount(0)

    await page.locator('.nav.inbox-nav').click()
    await expect(page.locator('#v-briefing')).toHaveClass(/on/)

    await page.evaluate(
      ({ workspaceId }) => {
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'artifact.created',
          data: {
            workspaceId,
            artifact: {
              id: 'art-inbox-1',
              title: 'Off-screen',
              kind: 'md',
              path: `workspaces/${workspaceId}/artifacts/art-inbox-1.md`,
            },
          },
        })
      },
      { workspaceId: working.id },
    )

    // Assert while still on Inbox. Returning would focus the pane and clear.
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveText('1')
  })

  test('present_artifact on a background workspace badges that workspace', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.filter((l) => l.group === 'active')
    const here = working[0]
    const other = working.find((l) => l.id !== here.id)
    if (!here || !other) throw new Error('need two active workspaces')
    await openWorkspaceByGoal(page, here.goal)

    const artifact = {
      id: 'art-bg-1',
      title: 'Background',
      kind: 'md',
      path: `workspaces/${other.id}/artifacts/art-bg-1.md`,
    }
    await page.evaluate(
      ({ workspaceId, artifact }) => {
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'artifact.created',
          data: { workspaceId, artifact },
        })
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'artifact.presented',
          data: { workspaceId, artifact },
        })
      },
      { workspaceId: other.id, artifact },
    )

    await openWorkspaceByGoal(page, other.goal)
    const artifactsBtn = page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Artifacts"]')
    await expect(artifactsBtn.locator('.obs-pane-badge')).toHaveText('1')
  })

  test('dragging a file tab reorders the strip and survives reload', async ({ page }) => {
    const MAIN = 'package main\n\nfunc main() {}\n'
    const OTHER = 'package other\n'
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
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
        ListChangedFiles: () => ['src/main.go', 'src/other.go'],
        GitStatus: () => ({ 'src/main.go': 'm', 'src/other.go': 'm' }),
        ReadFile: (a: unknown[]) => {
          const p = (a[2] as string) || 'src/main.go'
          const content = p.endsWith('other.go') ? OTHER : MAIN
          return { path: p, language: 'go', content, html: '', lines: [], size: content.length, writable: !!a[1] }
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const filesBtn = page.locator('#v-detail button[aria-label="Files"]')
    if (await filesBtn.getAttribute('aria-pressed') !== 'true') {
      await filesBtn.click()
    }
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'other.go' }).click()
    const strip = page.locator('#v-detail .obs-tb-center')
    await expect(strip.locator('.obs-filechip')).toHaveCount(2)
    await strip.locator('.tab-strip-scroll').evaluate((el: HTMLElement) => { el.scrollLeft = 0 })

    const first = strip.locator('.obs-filechip').first()
    const last = strip.locator('.obs-filechip').nth(1)
    const firstKey = await first.getAttribute('data-tab-key')
    const lastKey = await last.getAttribute('data-tab-key')
    expect(lastKey && firstKey && lastKey !== firstKey).toBeTruthy()
    const from = await first.boundingBox()
    const to = await last.boundingBox()
    expect(from && to).toBeTruthy()
    await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2)
    await page.mouse.down()
    await page.mouse.move(to!.x + to!.width - 4, to!.y + to!.height / 2, { steps: 12 })
    await page.mouse.up()

    await expect(strip.locator('.obs-filechip').first()).toHaveAttribute('data-tab-key', lastKey!)
    await expect(strip.locator('.obs-filechip').nth(1)).toHaveAttribute('data-tab-key', firstKey!)

    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    const stripAgain = page.locator('#v-detail .obs-tb-center')
    await expect(stripAgain.locator('.obs-filechip').first()).toHaveAttribute('data-tab-key', lastKey!)
    await expect(stripAgain.locator('.obs-filechip').nth(1)).toHaveAttribute('data-tab-key', firstKey!)
  })

  test('branch review shows files changed and commits without a PR', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')

    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeVisible()
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip')).toHaveCount(2)
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip').first()).toContainText('Changes')
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip').nth(1)).toContainText('Commits')
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip').nth(1).locator('.tab-chip-n')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] button[aria-label="Tree"]')).toHaveAttribute('aria-pressed', 'true')
    await expect(page.locator('#v-detail [data-review="changes"] .fl-row:not(.dir) .fp')).toHaveText('main.go')
    await expect(page.locator('#v-detail [data-review="changes"] .diffpane')).toHaveCount(0)
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')).toBeVisible()
    await expect(page.locator('#v-detail .obs-filechip')).toHaveCount(0)

    const toc = page.locator('#v-detail [data-review="changes"] .fl-row:not(.dir)', { hasText: 'main.go' })
    await toc.click()
    await expect(toc).toHaveClass(/on/)
    const stacked = page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')
    await expect(stacked).toBeInViewport()

    await page.locator('#v-detail [data-review="changes"] button[aria-label="Hide file list"]').click()
    await expect(page.locator('#v-detail [data-review="changes"] .filelist')).toHaveCount(0)
    await expect(stacked).toBeVisible()
    await page.locator('#v-detail [data-review="changes"] button[aria-label="Show file list"]').click()
    await expect(page.locator('#v-detail [data-review="changes"] .filelist')).toBeVisible()

    await page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' }).click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="commits"] .commit[data-working] .cm-t')).toHaveText('Uncommitted changes')
    const realCommits = page.locator('#v-detail [data-review="commits"] .commit:not([data-working])')
    await expect(realCommits.locator('.cm-t').first()).toHaveText('Add feature')
    await expect(realCommits.locator('.cm-t').last()).toHaveText('Older work')
    await expect(page.locator('#v-detail .obs-filechip')).toHaveCount(0)
    await realCommits.first().click()
    const changesChip = page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Changes' })
    const commitsChip = page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' })
    await expect(commitsChip).toHaveAttribute('aria-selected', 'true')
    await expect(changesChip).toHaveAttribute('aria-selected', 'false')
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .commit-hd .cm-t')).toHaveText('Add feature')
    await expect(page.locator('#v-detail [data-review="changes"] .commit-hd .chash')).toHaveText('a3f91c2')
    await expect(page.locator('#v-detail [data-review="changes"] .diff-file')).toHaveCount(1)

    await changesChip.click()
    await expect(changesChip).toHaveAttribute('aria-selected', 'true')
    await expect(page.locator('#v-detail [data-review="changes"] .commit-hd')).toHaveCount(0)
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toHaveCount(0)
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')).toBeVisible()

    await commitsChip.click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await page.locator('#v-detail [data-review="commits"] .commit:not([data-working])').first().click()
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toBeVisible()
    await page.locator('#v-detail [data-review="changes"] .cback').click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await expect(commitsChip).toHaveAttribute('aria-selected', 'true')
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toHaveCount(0)

    await changesChip.click()
    await page.locator('#v-detail [data-review="changes"] .df-open').click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-reviewchip')).toHaveCount(0)
    await expect(page.locator('#v-detail .obs-tb-center.file-tabs-muted')).toHaveCount(0)
  })

  test('commits tab uncommitted row is vs HEAD, not vs base', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        WorkingStatus: () => ({ 'src/scratch.ts': 'u' }),
        ListChangedFiles: () => ['src/main.go', 'src/scratch.ts'],
        GitStatus: () => ({ 'src/main.go': 'm', 'src/scratch.ts': 'u' }),
        DiffVsBase: () => ({
          files: [
          {
            path: 'src/main.go',
            status: 'm',
            diff: { add: 1, del: 0 },
            lines: [
              { old: '', new: '', type: 'hunk', codeHtml: '@@ -1,1 +1,2 @@' },
              { old: '1', new: '1', type: 'ctx', codeHtml: 'package main' },
              { old: '', new: '2', type: 'add', codeHtml: '// edited' },
            ],
          },
          ],
        }),
        DiffWorking: () => ({
          files: [
          {
            path: 'src/scratch.ts',
            status: 'a',
            diff: { add: 1, del: 0 },
            lines: [
              { old: '', new: '', type: 'hunk', codeHtml: '@@ -0,0 +1,1 @@' },
              { old: '', new: '1', type: 'add', codeHtml: 'export {}' },
            ],
          },
          ],
        }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/scratch.ts"]')).toHaveCount(0)

    const commitsChip = page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' })
    await expect(commitsChip).toHaveClass(/dirty/)
    await commitsChip.click()
    await page.locator('#v-detail [data-review="commits"] .commit[data-working]').click()
    await expect(page.locator('#v-detail [data-review="changes"] .working-hd .cm-t')).toHaveText('Uncommitted changes')
    await expect(page.locator('#v-detail [data-review="changes"] .fl-head-main .obs-review-vs')).toHaveText('vs HEAD')
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/scratch.ts"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')).toHaveCount(0)
    const scratchLine = page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/scratch.ts"] .hline.add').first()
    await scratchLine.hover()
    await expect(scratchLine.locator('.addc')).toBeVisible()

    await page.locator('#v-detail [data-review="changes"] .cback').click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()

    await page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Changes' }).click()
    await expect(page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .working-hd')).toHaveCount(0)
  })

  test('clean worktree hides the uncommitted commits row', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: { WorkingStatus: () => ({}) },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' }).click()
    await expect(page.locator('#v-detail [data-review="commits"] .commit[data-working]')).toHaveCount(0)
    await expect(page.locator('#v-detail [data-review="commits"] .commit .cm-t').first()).toHaveText('Add feature')
  })

  test('opt+cmd+r toggles review from the editor, not while Files is focused', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()

    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    // Changed files open in Diff. The editable CodeMirror side is contenteditable,
    // which used to make the window handler treat ⌥⌘R as typing and no-op.
    const editor = page.locator('#v-detail .cm-content[contenteditable="true"]')
    await expect(editor).toBeVisible()
    await editor.click()
    const reviewBtn = page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]')
    await page.keyboard.press('Meta+Alt+r')
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeVisible()
    await page.keyboard.press('Meta+Alt+r')
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'false')
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeHidden()

    await page.locator('#v-detail .obs-files').focus()
    await page.keyboard.press('Meta+Alt+r')
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'false')
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeHidden()
  })

  test('Review changes is remembered on hold, warm remount, and cold reload', async ({ page }) => {
    const active = seed.loops.filter((l) => l.group === 'active' && !l.archived && !l.isMain)
    const pool = active.length >= 3 ? active : seed.loops.filter((l) => !l.archived && !l.isMain)
    const [a, b, c] = pool
    if (!a || !b || !c) throw new Error('need three seeded workspaces')
    const reviewBtn = page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]')
    const reviewPane = page.locator('#v-detail [data-review="changes"]')

    await gotoApp(page)
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()
    await reviewBtn.click()
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(reviewPane).toBeVisible()

    await openWorkspaceByGoal(page, b.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', b.id)
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'false')
    await expect(reviewPane).not.toBeVisible()

    await openWorkspaceByGoal(page, a.goal)
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(reviewPane).toBeVisible()

    await openWorkspaceByGoal(page, b.goal)
    await openWorkspaceByGoal(page, c.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', c.id)
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', a.id)
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(reviewPane).toBeVisible()

    await reloadApp(page)
    await openWorkspaceByGoal(page, a.goal)
    await expect(reviewBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(reviewPane).toBeVisible()
  })

  test('left/right pane kind and visibility are remembered on hold, warm remount, and cold reload', async ({ page }) => {
    const active = seed.loops.filter((l) => l.group === 'active' && !l.archived && !l.isMain)
    const pool = active.length >= 3 ? active : seed.loops.filter((l) => !l.archived && !l.isMain)
    const [a, b, c] = pool
    if (!a || !b || !c) throw new Error('need three seeded workspaces')

    const findingsBtn = page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Findings"]')
    const scmBtn = page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Source Control"]')
    const hideLeft = page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Hide left panel"]')
    const showLeft = page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Show left panel"]')
    const hideRight = page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label="Hide right panel"]')
    const showRight = page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label="Show right panel"]')
    const leftPane = page.locator('#v-detail .obs-left')
    const rightPane = page.locator('#v-detail .obs-side')

    await gotoApp(page)
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail .obs')).toBeVisible()

    await findingsBtn.click()
    await scmBtn.click()
    await expect(findingsBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(scmBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(rightPane.locator('.obs-side-title')).toHaveText('Source Control')

    await hideLeft.click()
    await hideRight.click()
    await expect(leftPane).toHaveCount(0)
    await expect(rightPane).toHaveCount(0)

    await openWorkspaceByGoal(page, b.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', b.id)
    await expect(leftPane).toBeVisible()
    await expect(rightPane).toBeVisible()
    await expect(findingsBtn).toHaveAttribute('aria-pressed', 'false')
    await expect(scmBtn).toHaveAttribute('aria-pressed', 'false')

    // Hold: A is still mounted in the park.
    await openWorkspaceByGoal(page, a.goal)
    await expect(leftPane).toHaveCount(0)
    await expect(rightPane).toHaveCount(0)

    await openWorkspaceByGoal(page, b.goal)
    await openWorkspaceByGoal(page, c.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', c.id)
    await openWorkspaceByGoal(page, a.goal)
    await expect(page.locator('#v-detail .wsv-shell')).toHaveAttribute('data-workspace-id', a.id)
    await expect(leftPane).toHaveCount(0)
    await expect(rightPane).toHaveCount(0)

    await showLeft.click()
    await showRight.click()
    await expect(findingsBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(scmBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(rightPane.locator('.obs-side-title')).toHaveText('Source Control')

    await hideLeft.click()
    await hideRight.click()

    await reloadApp(page)
    await openWorkspaceByGoal(page, a.goal)
    await expect(leftPane).toHaveCount(0)
    await expect(rightPane).toHaveCount(0)
    await showLeft.click()
    await showRight.click()
    await expect(findingsBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(scmBtn).toHaveAttribute('aria-pressed', 'true')
    await expect(rightPane.locator('.obs-side-title')).toHaveText('Source Control')
  })

  test('working copy review includes the commits tab', async ({ page }) => {
    await gotoApp(page)
    await page.locator('.rail button.tmain').first().click()
    await expect(page.locator('#v-detail .obs')).toBeVisible()

    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeVisible()
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip')).toHaveCount(2)
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip').first()).toContainText('Changes')
    await expect(page.locator('#v-detail .obs-tb-center .obs-reviewchip').nth(1)).toContainText('Commits')

    await page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' }).click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="commits"] .commit[data-working] .cm-t')).toHaveText('Uncommitted changes')
    const realCommits = page.locator('#v-detail [data-review="commits"] .commit:not([data-working])')
    await expect(realCommits.locator('.cm-t').first()).toHaveText('Add feature')
    await expect(realCommits.locator('.cm-t').last()).toHaveText('Older work')
    await expect(page.locator('#v-detail [data-review="commits"] .hint')).toContainText('Recent commits on this branch')

    await realCommits.first().click()
    const commitsChip = page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' })
    await expect(commitsChip).toHaveAttribute('aria-selected', 'true')
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .commit-hd .cm-t')).toHaveText('Add feature')
  })

  test('commit review stays open when files are edited', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await page.locator('#v-detail .obs-tb-center .obs-reviewchip', { hasText: 'Commits' }).click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await page.locator('#v-detail [data-review="commits"] .commit:not([data-working])').first().click()
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .commit-hd .cm-t')).toHaveText('Add feature')

    // Same-string workspaceId re-pass (store refresh + live fs watch) must
    // not clear the commit scope back to the list.
    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.updated',
        data: { workspaceId, phase: 'working', iteration: 1 },
      })
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.fs_changed',
        data: { workspaceId },
      })
    }, { workspaceId: working.id })
    await expect(page.locator('#v-detail [data-review="changes"] .cback')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .commit-hd .cm-t')).toHaveText('Add feature')
    await expect(page.locator('#v-detail [data-review="commits"]')).not.toBeVisible()

    await page.locator('#v-detail [data-review="changes"] .cback').click()
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.updated',
        data: { workspaceId, phase: 'working', iteration: 2 },
      })
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'workspace.fs_changed',
        data: { workspaceId },
      })
    }, { workspaceId: working.id })
    await expect(page.locator('#v-detail [data-review="commits"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="commits"] .commit:not([data-working]) .cm-t').first()).toHaveText('Add feature')
    await expect(page.locator('#v-detail [data-review="commits"] .empty')).toHaveCount(0)
  })

  test('collapsing a files-changed card shrinks it to the header', async ({ page }) => {
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    const card = page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')
    await expect(card.locator('.hline').first()).toBeVisible()
    const expanded = await card.boundingBox()
    expect(expanded?.height ?? 0).toBeGreaterThan(80)

    await card.locator('.df-chev').click()
    await expect(card.locator('.df-head')).toHaveClass(/collapsed/)
    await expect(card.locator('.hline')).toHaveCount(0)
    const collapsed = await card.boundingBox()
    expect(collapsed?.height ?? 0).toBeLessThan(64)
  })

  test('review line note becomes a finding', async ({ page }) => {
    const notes: Record<string, unknown>[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListFindings: () => notes,
        ListChangedFiles: () => ['src/main.go', 'src/util.go'],
        DiffVsBase: () => ({
          files: [
          {
            path: 'src/main.go',
            status: 'm',
            diff: { add: 1, del: 1 },
            lines: [
              { old: '', new: '', type: 'hunk', codeHtml: '@@ -1,2 +1,2 @@' },
              { old: '1', new: '1', type: 'ctx', codeHtml: 'package main' },
              { old: '2', new: '', type: 'del', codeHtml: '// gone' },
              { old: '', new: '2', type: 'add', codeHtml: '// edited' },
            ],
          },
          {
            path: 'src/util.go',
            status: 'a',
            diff: { add: 1, del: 0 },
            lines: [
              { old: '', new: '', type: 'hunk', codeHtml: '@@ -0,0 +1,1 @@' },
              { old: '', new: '1', type: 'add', codeHtml: 'package util' },
            ],
          },
          ],
        }),
        CreateFinding: (a) => {
          const f = {
            id: 'f-user-1',
            workspaceId: String(a[0] || ''),
            file: String(a[1] || ''),
            line: Number(a[2] || 0),
            endLine: Number(a[4] || 0),
            message: String(a[3] || ''),
            status: 'open',
            severity: 'info',
            source: 'user',
            iteration: 1,
          }
          notes.push(f)
          return f
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeVisible()
    await expect(page.locator('#v-detail [data-review="changes"] .hline.del .addc')).toHaveCount(0)
    const addLine = page.locator('#v-detail [data-review="changes"] .hline.add').first()
    await addLine.hover()
    await addLine.locator('.addc').click()
    await page.locator('#v-detail [data-review="changes"] .finding-note-input').fill('check the timeout')
    await page.locator('#v-detail [data-review="changes"] .finding-note-btn.primary').click()
    await expect(page.locator('#v-detail [data-review="changes"] .icmt-finding-card')).toContainText('check the timeout')
    await expect(page.locator('#v-detail [data-review="changes"] .fl-row:not(.dir) .obs-finding-flag')).toBeVisible()
    expect(await page.locator('#v-detail [data-review="changes"] .fl-row:not(.dir)').count()).toBe(2)
    await page.locator('#v-detail [data-review="changes"] .fl-filter button[aria-label="Show files with open findings"]').click()
    await expect(page.locator('#v-detail [data-review="changes"] .fl-row:not(.dir)')).toHaveCount(1)
    await expect(page.locator('#v-detail [data-review="changes"] .fl-row', { hasText: 'main.go' })).toBeVisible()
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Findings"]').click()
    await expect(page.locator('#v-detail .findings-panel .finding-title')).toContainText('check the timeout')
    await expect(page.locator('#v-detail .findings-panel .finding-loc')).toHaveText('main.go:2')
    await expect(page.locator('#v-detail .findings-panel .finding-loc')).toHaveAttribute('title', 'src/main.go')
    await page.locator('#v-detail .findings-panel .finding-row-md').click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
  })

  test('drag on the comment control selects a multiline finding', async ({ page }) => {
    const notes: Record<string, unknown>[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListFindings: () => notes,
        ListChangedFiles: () => ['src/main.go'],
        DiffVsBase: () => ({
          files: [
            {
              path: 'src/main.go',
              status: 'm',
              diff: { add: 3, del: 0 },
              lines: [
                { old: '', new: '', type: 'hunk', codeHtml: '@@ -1,1 +1,4 @@' },
                { old: '1', new: '1', type: 'ctx', codeHtml: 'package main' },
                { old: '', new: '2', type: 'add', codeHtml: 'func a() {}' },
                { old: '', new: '3', type: 'add', codeHtml: 'func b() {}' },
                { old: '', new: '4', type: 'add', codeHtml: 'func c() {}' },
              ],
            },
          ],
        }),
        CreateFinding: (a) => {
          const f = {
            id: 'f-user-span',
            workspaceId: String(a[0] || ''),
            file: String(a[1] || ''),
            line: Number(a[2] || 0),
            message: String(a[3] || ''),
            endLine: Number(a[4] || 0),
            status: 'open',
            severity: 'info',
            source: 'user',
            iteration: 1,
          }
          notes.push(f)
          return f
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    await expect(page.locator('#v-detail [data-review="changes"]')).toBeVisible()

    const file = page.locator('#v-detail [data-review="changes"] .cfile[data-path="src/main.go"]')
    const start = file.locator('.hline[data-cmt-side="right"][data-cmt-line="2"] .addc')
    const end = file.locator('.hline[data-cmt-side="right"][data-cmt-line="4"] .addc')
    await start.dragTo(end, { force: true })

    const composer = file.locator('.icmt textarea')
    await expect(composer).toBeVisible()
    await expect(composer).toHaveAttribute('placeholder', /lines 2/)
    await expect(file.locator('.hline[data-cmt-line="2"]')).toHaveClass(/drafting/)
    await expect(file.locator('.hline[data-cmt-line="4"]')).toHaveClass(/draft-end/)
    await composer.fill('this span should stay together')
    await file.locator('.icmt .finding-note-btn.primary').click()

    await expect(file.locator('.icmt-finding-card', { hasText: 'this span should stay together' })).toBeVisible()
    await expect(file.locator('.icmt-finding-card')).toContainText('lines 2–4')
    await expect(file.locator('.hline[data-cmt-line="2"]')).toHaveClass(/noted/)
    await expect(file.locator('.hline[data-cmt-line="3"]')).toHaveClass(/noted/)
    await expect(file.locator('.hline[data-cmt-line="4"]')).toHaveClass(/note-end/)
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Findings"]').click()
    await expect(page.locator('#v-detail .findings-panel .finding-loc')).toHaveText('main.go:2–4')
  })

  test('review changes survives scrolling a large mixed-size diff', async ({ page }) => {
    test.setTimeout(90_000)
    await page.setViewportSize({ width: 1800, height: 1000 })
    // Hydration swaps change card heights, which changes what is in view. The
    // spy effect must not track that state or the swap loops until Svelte
    // throws effect_update_depth_exceeded and the workspace fails to render.
    const files = Array.from({ length: 300 }, (_, i) => {
      const n = i % 11 === 0 ? 400 : 2 + (i * 7) % 15
      const lines: { old: string; new: string; type: string; codeHtml: string }[] = [
        { old: '', new: '', type: 'hunk', codeHtml: `@@ -1,${n} +1,${n} @@` },
      ]
      for (let k = 1; k <= n; k++) {
        const type = k % 3 === 0 ? 'add' : k % 5 === 0 ? 'del' : 'ctx'
        const width = (k + i) % 4 === 0 ? 40 : 6
        lines.push({ old: type === 'add' ? '' : String(k), new: type === 'del' ? '' : String(k), type, codeHtml: `l${k} f${i} ${'word '.repeat(width)}` })
      }
      return { path: `src/pkg${i % 9}/file${i}.ts`, status: 'm', diff: { add: Math.floor(n / 3), del: Math.floor(n / 5) }, lines }
    })
    const paths = files.map((f) => f.path)
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(e.message))
    page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()) })
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListChangedFiles: () => paths,
        GitStatus: () => Object.fromEntries(paths.map((p) => [p, 'm'])),
        DiffVsBase: () => ({ files }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await page.locator('#v-detail .obs-pane-toggle[aria-label="Hide left panel"]').first().click()
    await page.locator('#v-detail .obs-pane-toggle[aria-label="Hide right panel"]').first().click()
    await page.locator('#v-detail .obs-tb-center .obs-pane-toggle[aria-label="Review changes"]').click()
    const pane = page.locator('#v-detail [data-review="changes"] .diffscroll')
    await expect(page.locator('#v-detail [data-review="changes"] .cfile')).toHaveCount(files.length)

    await pane.hover()
    for (let i = 0; i < 120; i++) {
      await page.mouse.wheel(0, 300)
      await page.waitForTimeout(16)
    }
    for (const frac of [0.3, 0.8, 0.95, 0.4, 0.99, 0.2]) {
      await pane.evaluate((el, f) => { el.scrollTop = (el.scrollHeight - el.clientHeight) * f }, frac)
      await page.waitForTimeout(250)
    }
    for (let i = 0; i < 120; i++) {
      await page.mouse.wheel(0, i % 4 === 0 ? -500 : 300)
      await page.waitForTimeout(16)
    }

    await expect(pane).toBeVisible()
    expect(errors.filter((e) => e.includes('effect_update_depth_exceeded'))).toEqual([])
    await expect(page.locator('#v-detail [data-review="changes"] .cfile:not(.stub)')).not.toHaveCount(0)
  })
})
