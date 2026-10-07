import { test, expect, gotoApp, reloadApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'
import type { Page } from '@playwright/test'

// Terminal dock e2e. The bound TerminalService is mocked (no PTY, no Go), but the
// real xterm.js instance mounts and the real attach/stream/ack wiring runs — which
// is exactly the seam the Go tests can't reach.
//
// What's asserted here is the contract between the dock and the backend:
//   - opening adopts existing sessions instead of spawning duplicates
//   - streamed output is acked, or the Go side would stall the PTY on purpose
//   - hiding/unmounting parks the xterm widget (Go stays attached); only an
//     explicit close kills the shell
// Pixel output isn't asserted — with the WebGL renderer the screen is a canvas.

type Call = { name: string; args: any[] }

/** Demo seed has no project.toml overlay; specs that exercise Run set commands here. */
function seedWithRuns(...runs: { name: string; cmd: string }[]) {
  const data = structuredClone(seed)
  const proj = data.projects.find((p: { id: string }) => p.id === 'storefront')
  if (!proj) throw new Error('seed is missing storefront')
  proj.runCommands = runs.length
    ? runs
    : [{ name: 'dev', cmd: 'pnpm dev --port $RECUR_PORT' }]
  return data
}

/** Records every terminal bound call and answers it the way Go would. */
function terminalResolvers(page: Page, calls: Call[], opts: { existing?: any[]; restored?: any[] } = {}) {
  const record = (name: string, result: unknown = null) => (args: any[]) => {
    calls.push({ name, args })
    return result
  }
  const session = (id: string, workspaceId: string) => ({
    id,
    workspaceId,
    cwd: '/tmp/wt',
    shell: '/bin/zsh',
    title: 'zsh',
    createdAt: '2026-01-01T00:00:00Z',
  })
  let n = 0
  let roster = [...(opts.existing ?? [])]
  const chromeByWs: Record<string, { workspaceId: string; open: boolean; maximized: boolean; editorHidden: boolean; beside: boolean; besidePct: number }> = {}
  return {
    ListTerminals: (args: any[]) => {
      calls.push({ name: 'ListTerminals', args })
      const ws = args[0]
      return roster.filter((s) => s.workspaceId === ws)
    },
    RestoreWorkspaceTerminals: (args: any[]) => {
      calls.push({ name: 'RestoreWorkspaceTerminals', args })
      if (roster.length) return roster.filter((s) => s.workspaceId === args[0])
      const restored = [...(opts.restored ?? [])]
      roster = [...roster, ...restored]
      return restored
    },
    ListTerminalDockChrome: () => {
      calls.push({ name: 'ListTerminalDockChrome', args: [] })
      return Object.values(chromeByWs)
    },
    SetTerminalDockChrome: (args: any[]) => {
      calls.push({ name: 'SetTerminalDockChrome', args })
      const ws = String(args[0] ?? '')
      const c = args[1] ?? {}
      if (!ws) return null
      if (!c.open && !c.beside && !(Number(c.besidePct) > 0)) {
        delete chromeByWs[ws]
        return null
      }
      chromeByWs[ws] = {
        workspaceId: ws,
        open: !!c.open,
        maximized: !!c.maximized,
        editorHidden: !!c.editorHidden,
        beside: !!c.beside,
        besidePct: Number(c.besidePct) || 0,
      }
      return null
    },
    OpenTerminal: (args: any[]) => {
      calls.push({ name: 'OpenTerminal', args })
      const s = session(`sess-${++n}`, args[0])
      roster = [...roster, s]
      return s
    },
    // Attach answers with a replay buffer, base64 of raw VT, as Go does.
    AttachTerminal: (args: any[]) => {
      calls.push({ name: 'AttachTerminal', args })
      return btoa('replayed-scrollback\r\n')
    },
    DetachTerminal: record('DetachTerminal'),
    WriteTerminal: (args: any[]) => {
      calls.push({ name: 'WriteTerminal', args })
      const id = args[0]
      void page.evaluate(({ id }) => {
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'term.busy',
          data: { id, workspaceId: 'ws' },
        })
      }, { id })
      return null
    },
    AckTerminal: record('AckTerminal'),
    ResizeTerminal: record('ResizeTerminal'),
    ClearTerminal: record('ClearTerminal'),
    CloseTerminal: (args: any[]) => {
      calls.push({ name: 'CloseTerminal', args })
      const id = args[0]
      roster = roster.filter((s) => s.id !== id)
      return null
    },
    ReorderTerminals: (args: any[]) => {
      calls.push({ name: 'ReorderTerminals', args })
      const ws = args[0]
      const ids = [...((args[1] ?? []) as string[])]
      const mine = roster.filter((s) => s.workspaceId === ws)
      const by = new Map(mine.map((s) => [s.id, s]))
      if (ids.length !== mine.length || ids.some((id) => !by.has(id))) {
        throw new Error('ReorderTerminals: not a permutation')
      }
      const others = roster.filter((s) => s.workspaceId !== ws)
      roster = [...others, ...ids.map((id) => by.get(id)!)]
      return null
    },
    InterruptTerminal: record('InterruptTerminal'),
  }
}

/** Pushes a term.data event the way the Go bus → Wails emitter would. */
async function pushOutput(page: Page, id: string, text: string) {
  await page.evaluate(
    ({ id, text }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'term.data',
        data: { id, data: btoa(text) },
      })
    },
    { id, text },
  )
}

async function pushIdle(page: Page, id: string) {
  await page.evaluate(
    ({ id }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'term.idle',
        data: { id, workspaceId: 'ws' },
      })
    },
    { id },
  )
}

async function pushExit(page: Page, id: string, exitCode: number) {
  await page.evaluate(
    ({ id, exitCode }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'term.exit',
        data: { id, workspaceId: 'ws', exitCode },
      })
    },
    { id, exitCode },
  )
}

/**
 * Opens a workspace — the unified view mounts the Observatory directly (there is
 * no separate Code tab since the unified-view restructure), and it is the only
 * Observatory mount that carries a workspaceId, so it is the only one with a
 * terminal dock.
 */
async function gotoObservatory(page: Page) {
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail')).toHaveClass(/on/)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
}

const dock = '#v-detail .term-dock'

async function expectDockFillsObservatory(page: Page) {
  const obs = page.locator('#v-detail .obs')
  await expect(obs).toHaveClass(/term-max/)
  await expect.poll(async () => {
    const d = await page.locator(dock).boundingBox()
    const o = await obs.boundingBox()
    if (!d || !o) return 999
    return Math.abs(d.height - o.height) + Math.abs(d.width - o.width)
      + Math.abs(d.y - o.y) + Math.abs(d.x - o.x)
  }).toBeLessThan(8)
  // Frost is a sibling layer. backdrop-filter on .term-dock itself (an ancestor
  // of the WebGL canvas) tiles the expanded screen with opaque black in WKWebView.
  await expect(page.locator(dock)).toHaveCSS('backdrop-filter', 'none')
  await expect(page.locator(`${dock} .term-dock-frost`)).toHaveCSS('backdrop-filter', /blur\(/)
}

test.describe('terminal dock', () => {
  test('toggle opens a shell, streams output, and acks it', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeVisible()
    await expect(page.locator(`${dock} .xterm`)).toBeVisible()

    // Nothing was running, so the dock spawns one shell and attaches to it.
    expect(calls.filter((c) => c.name === 'OpenTerminal')).toHaveLength(1)
    await expect.poll(() => calls.some((c) => c.name === 'AttachTerminal')).toBe(true)

    // The PTY is opened at the dock's measured geometry rather than a blind 80x24,
    // so the first prompt doesn't reflow.
    const [, cols, rows] = calls.find((c) => c.name === 'OpenTerminal')!.args
    expect(cols).toBeGreaterThanOrEqual(20)
    expect(rows).toBeGreaterThan(3)

    // Enough output to cross the 32KB ack threshold: without an ack the Go side
    // would deliberately stall this session's PTY.
    await pushOutput(page, 'sess-1', 'x'.repeat(40 * 1024))
    await expect
      .poll(() => calls.filter((c) => c.name === 'AckTerminal').length, { timeout: 5000 })
      .toBeGreaterThan(0)
    const acked = calls.find((c) => c.name === 'AckTerminal')!
    expect(acked.args[0]).toBe('sess-1')
    expect(acked.args[1]).toBeGreaterThanOrEqual(32 * 1024)
  })

  test('dropping an explorer file or folder onto the terminal inserts the path', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-pane`)).toBeVisible()

    const pane = page.locator(`${dock} .term-pane`).first()
    const working = seed.loops.find((l) => l.group === 'active')!
    const root = `/mock/worktrees/${working.id}`
    for (const path of ['src/main.go', 'src']) {
      const node = page.locator(`#v-detail .obs-files .obs-node[data-tree-path="${path}"]`)
      await expect(node).toBeVisible()
      const dt = await page.evaluateHandle((p: string) => {
        const d = new DataTransfer()
        d.setData('application/x-recur-workspace-file', JSON.stringify([p]))
        return d
      }, path)
      await node.dispatchEvent('dragstart', { dataTransfer: dt })
      await pane.dispatchEvent('dragover', { dataTransfer: dt })
      await pane.dispatchEvent('drop', { dataTransfer: dt })
      await node.dispatchEvent('dragend', { dataTransfer: dt })
      const abs = `${root}/${path}`
      await expect.poll(() =>
        calls.filter((c) => c.name === 'WriteTerminal' && c.args[1] === abs).length,
      ).toBe(1)
    }
  })

  test('reopening adopts the running shell instead of spawning another', async ({ page }) => {
    const calls: Call[] = []
    const existing = [{
      id: 'sess-live',
      workspaceId: seed.loops.find((l) => l.group === 'active')!.id,
      cwd: '/tmp/wt',
      shell: '/bin/zsh',
      title: 'npm run dev',
      createdAt: '2026-01-01T00:00:00Z',
    }]
    await installBridge(page, structuredClone(seed), { resolvers: terminalResolvers(page, calls, { existing }) })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveText(/npm run dev/)
    expect(calls.filter((c) => c.name === 'OpenTerminal')).toHaveLength(0)

    // Hiding the dock parks the xterm widget — Detach would force Attach+replay
    // on the next open. Close still kills the shell.
    await page.locator(`${dock} .term-act[aria-label="Hide terminal"]`).click()
    await expect(page.locator(dock)).toBeHidden()
    await expect.poll(() => calls.some((c) => c.name === 'CloseTerminal')).toBe(false)
    expect(calls.filter((c) => c.name === 'DetachTerminal')).toHaveLength(0)
  })

  test('restart restore respawns saved tabs instead of a blank shell', async ({ page }) => {
    const calls: Call[] = []
    const ws = seed.loops.find((l) => l.group === 'active')!.id
    const restored = [
      {
        id: 'sess-a', workspaceId: ws, cwd: '/tmp/wt', shell: '/bin/zsh',
        title: 'server', createdAt: '2026-01-01T00:00:00Z',
      },
      {
        id: 'sess-b', workspaceId: ws, cwd: '/tmp/other', shell: '/bin/zsh',
        title: 'zsh', createdAt: '2026-01-01T00:00:01Z', active: true,
      },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls, { restored }), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(2)
    await expect(page.locator(`${dock} .term-tab`).first()).toHaveText(/server/)
    await expect(page.locator(`${dock} .term-tab`).nth(1)).toHaveClass(/on/)
    expect(calls.filter((c) => c.name === 'OpenTerminal')).toHaveLength(0)
    expect(calls.filter((c) => c.name === 'RestoreWorkspaceTerminals')).toHaveLength(1)
  })

  test('tabs open, switch, and close', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)

    await page.locator(`${dock} .term-act[aria-label="New Tab"]`).click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(2)
    await expect(page.locator(`${dock} .term-tab`).nth(1)).toHaveClass(/on/)

    // Both panes stay mounted so a background shell keeps a live screen.
    await expect(page.locator(`${dock} .xterm`)).toHaveCount(2)

    // Click the label, not the whole chip: the × is opacity 0 until hover/active
    // and sits in the chip hit box. Scroll the strip so the first tab isn't
    // under the trailing actions (the previous evaluate(el.click()) skipped
    // Playwright's hit-target check).
    await page.locator(`${dock} .tab-strip-scroll`).evaluate((el: HTMLElement) => { el.scrollLeft = 0 })
    await page.locator(`${dock} .term-tab`).first().locator('.tab-chip-body').click()
    await expect(page.locator(`${dock} .term-tab`).first()).toHaveClass(/on/)

    await page.locator(`${dock} .term-tab`).first().locator('.tab-chip-x').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)
    await expect.poll(() => calls.some((c) => c.name === 'CloseTerminal')).toBe(true)

    // Closing the last tab leaves nothing to show, so the drawer closes with it.
    await page.locator(`${dock} .term-tab`).first().locator('.tab-chip-x').click()
    await expect(page.locator(dock)).toBeHidden()
  })

  test('dragging a terminal tab reorders the strip and survives remount', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)
    await page.locator(`${dock} .term-act[aria-label="New Tab"]`).click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(2)
    await expect(page.locator(`${dock} .term-tab`).first()).toHaveAttribute('data-tab-key', 'sess-1')

    const first = page.locator(`${dock} .term-tab`).first()
    const second = page.locator(`${dock} .term-tab`).nth(1)
    const from = await second.boundingBox()
    const to = await first.boundingBox()
    expect(from && to).toBeTruthy()
    await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2)
    await page.mouse.down()
    await page.mouse.move(to!.x + 8, to!.y + to!.height / 2, { steps: 12 })
    await page.mouse.up()

    await expect(page.locator(`${dock} .term-tab`).first()).toHaveAttribute('data-tab-key', 'sess-2')
    await expect.poll(() => {
      const hit = [...calls].reverse().find((c) => c.name === 'ReorderTerminals')
      return (hit?.args[1] as string[] | undefined)?.[0]
    }).toBe('sess-2')

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeHidden()
    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`).first()).toHaveAttribute('data-tab-key', 'sess-2')
    await expect(page.locator(`${dock} .term-tab`).nth(1)).toHaveAttribute('data-tab-key', 'sess-1')
  })

  // ⌘T / ⌘W only when the dock has focus — same as every other Mac terminal, and
  // without stealing those chords from the editor / window chrome.
  // e2e injects Env.Info OS=darwin, so CmdOrCtrl is ⌘ (Meta). CDP dispatches
  // Meta without Chromium eating ⌘W as "close tab".
  test('⌘T and ⌘W open and close tabs while the dock is focused', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)

    const xterm = page.locator(`${dock} .xterm`).first()
    await xterm.click()
    await expect(xterm).toHaveClass(/focus/)

    await page.keyboard.press('Meta+t')
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(2)
    await expect(page.locator(`${dock} .term-tab`).nth(1)).toHaveClass(/on/)

    await page.keyboard.press('Meta+w')
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)
    await expect.poll(() => calls.filter((c) => c.name === 'CloseTerminal')).toHaveLength(1)

    // Focus elsewhere: the chords must not touch the dock.
    await page.locator('#v-detail .obs-toolbar').click()
    await page.keyboard.press('Meta+t')
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)
  })

  test('⌘F opens the terminal find bar while the dock is focused', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .xterm`)).toBeVisible()
    await page.locator(`${dock} .xterm`).first().click()
    await expect(page.locator(`${dock} .xterm`).first()).toHaveClass(/focus/)

    await page.keyboard.press('Meta+f')
    await expect(page.locator(`${dock} .cm-recur-search`)).toBeVisible()
  })

  test('⌥⌘←/→ rotates through tabs while the dock is focused', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await page.locator(`${dock} .term-act[aria-label="New Tab"]`).click()
    await page.locator(`${dock} .term-act[aria-label="New Tab"]`).click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(3)
    await expect(page.locator(`${dock} .term-tab`).nth(2)).toHaveClass(/on/)

    const xterm = page.locator(`${dock} .xterm`).nth(2)
    await xterm.click()
    await expect(xterm).toHaveClass(/focus/)

    // ⌥⌘← / ⌥⌘→. e2e injects Env.Info OS=darwin, so CmdOrCtrl is ⌘ (Meta).
    await page.keyboard.press('Meta+Alt+ArrowLeft')
    await expect(page.locator(`${dock} .term-tab`).nth(1)).toHaveClass(/on/)
    await page.keyboard.press('Meta+Alt+ArrowLeft')
    await expect(page.locator(`${dock} .term-tab`).nth(0)).toHaveClass(/on/)
    // Wraps.
    await page.keyboard.press('Meta+Alt+ArrowLeft')
    await expect(page.locator(`${dock} .term-tab`).nth(2)).toHaveClass(/on/)
    await page.keyboard.press('Meta+Alt+ArrowRight')
    await expect(page.locator(`${dock} .term-tab`).nth(0)).toHaveClass(/on/)

    await page.locator('#v-detail .obs-toolbar').click()
    await page.keyboard.press('Meta+Alt+ArrowRight')
    await expect(page.locator(`${dock} .term-tab`).nth(0)).toHaveClass(/on/)
  })

  // Amber underline marks the focused panel's active tab; an active tab in a
  // background panel keeps the thick rule in the standard border colour.
  test('active tab underline tracks which panel has focus', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.keyboard.press('Meta+p') // Quick Open is a chord now
    const qo = page.locator('.qo')
    await expect(qo).toBeVisible()
    await qo.locator('input').fill('main.go')
    await expect(qo.locator('.qo-row').first()).toContainText('main.go')
    await page.keyboard.press('Enter')
    const filechip = page.locator('#v-detail .obs-filechip.on')
    await expect(filechip).toContainText('main.go')
    await expect(page.locator('#v-detail .cm-editor')).toBeVisible()
    await expect(filechip).toHaveClass(/lit/)

    await page.locator('#v-detail .obs-term-toggle').click()
    await page.locator(`${dock} .xterm`).first().click()
    await expect(page.locator(`${dock} .term-tab.on`)).toHaveClass(/lit/)
    await expect(filechip).not.toHaveClass(/lit/)

    await page.locator('#v-detail .cm-content').click()
    await expect(filechip).toHaveClass(/lit/)
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)

    // File explorer is part of the editor workbench — keep file tabs lit.
    await page.locator('#v-detail .obs-pane-toggle[aria-label="Files"]').click()
    await page.locator('#v-detail .obs-files').focus()
    await expect(filechip).toHaveClass(/lit/)
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)
  })

  test('⌘1 / ⌘2 / ⌘3 focus agent, editor, and terminal', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.keyboard.press('Meta+3')
    await expect(page.locator(dock)).toBeVisible()
    await expect.poll(async () => {
      return page.locator(`${dock} .xterm.focus, ${dock}:focus-within`).count()
    }).toBeGreaterThan(0)
    await expect(page.locator(`${dock} .term-tab.on`)).toHaveClass(/lit/)

    await page.keyboard.press('Meta+2')
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)

    await page.keyboard.press('Meta+1')
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeFocused()
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)

    // From files (non-agent) left panel, ⌘1 switches back to the agent.
    await page.locator('#v-detail .obs-pane-toggle[aria-label="Files"]').click()
    await expect(page.locator('#v-detail .obs-files')).toBeVisible()
    await page.keyboard.press('Meta+1')
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeFocused()
  })

  test('an exited shell keeps its tab so the final output stays readable', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(`${dock} .term-tab`)).toHaveCount(1)

    await pushExit(page, 'sess-1', 127)
    await expect(page.locator(`${dock} .term-tab`)).toHaveText(/exited \(127\)/)
    await expect(page.locator(`${dock} .term-tab.exited`)).toBeVisible()
  })

  test('⌃⇧` maximizes the dock over the whole Observatory and back', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    const toolbar = page.locator('#v-detail .obs-toolbar')
    const sidePanel = page.locator('#v-detail .obs-side')
    await page.keyboard.press('Control+`')
    await expect(page.locator(dock)).toBeVisible()
    await expect(page.locator(`${dock} .term-dock-frost`)).toBeHidden()
    const normal = (await page.locator(dock).boundingBox())!.height
    const whole = (await obs.boundingBox())!.height
    await expect(toolbar).toBeVisible()

    await page.keyboard.press('Control+Shift+`')
    await expectDockFillsObservatory(page)
    // Chrome stays in the tree (blurred under the overlay) so restore can shrink
    // back into the same slot — coverage is the dock matching the Observatory box.
    await expect(toolbar).toBeVisible()
    await expect(sidePanel).toBeVisible()
    const maxed = (await page.locator(dock).boundingBox())!.height
    expect(maxed).toBeGreaterThan(normal)
    expect(Math.abs(maxed - whole)).toBeLessThan(3)
    await expect(page.locator(`${dock} .term-act[aria-label="Restore terminal size"]`)).toBeVisible()

    await page.keyboard.press('Control+Shift+`')
    await expect(obs).not.toHaveClass(/term-max/)
    await expect(toolbar).toBeVisible()
    await expect(sidePanel).toBeVisible()
    await expect.poll(async () => {
      const h = (await page.locator(dock).boundingBox())?.height
      return h == null ? 999 : Math.abs(h - normal)
    }).toBeLessThan(3)
  })

  test('⌃⇧` maximizes and restores while the dock is beside the editor', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    await page.locator('#v-detail .obs-term-beside-toggle').click()
    await expect(page.locator('#v-detail .obs-main')).toHaveClass(/term-beside/)
    await expect(page.locator(dock)).toBeVisible()
    const beside = await page.locator(dock).boundingBox()
    expect(beside).toBeTruthy()

    await page.keyboard.press('Control+Shift+`')
    await expectDockFillsObservatory(page)

    await page.keyboard.press('Control+Shift+`')
    await expect(obs).not.toHaveClass(/term-max/)
    await expect(page.locator('#v-detail .obs-main')).toHaveClass(/term-beside/)
    await expect.poll(async () => {
      const box = await page.locator(dock).boundingBox()
      if (!box || !beside) return 999
      return Math.abs(box.height - beside.height) + Math.abs(box.width - beside.width)
    }).toBeLessThan(8)
  })

  // Hide-editor is lighter than maximize: the terminal fills the middle panel while
  // the toolbar (and its Show editor control) and side docks stay put.
  test('hide editor expands the terminal in the middle panel', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    const toolbar = page.locator('#v-detail .obs-toolbar')
    const sidePanel = page.locator('#v-detail .obs-side')
    const editors = page.locator('#v-detail .obs-editors')
    const fileTabs = page.locator('#v-detail .obs-tb-center .tab-strip-scroll')
    const editorToggle = page.locator('#v-detail .obs-editor-toggle')

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeVisible()
    const withEditor = (await page.locator(dock).boundingBox())!.height

    await editorToggle.click()
    await expect(obs).toHaveClass(/editor-hidden/)
    await expect(editors).toBeHidden()
    await expect(fileTabs).toBeHidden()
    await expect(toolbar).toBeVisible()
    await expect(sidePanel).toBeVisible()
    await expect(editorToggle).toBeVisible()
    await expect(obs).not.toHaveClass(/term-max/)
    const filled = (await page.locator(dock).boundingBox())!.height
    expect(filled).toBeGreaterThan(withEditor)
    await expect(editorToggle).toHaveAttribute('aria-label', 'Show editor')

    await editorToggle.click()
    await expect(obs).not.toHaveClass(/editor-hidden/)
    await expect(editors).toBeVisible()
    await expect(fileTabs).toBeVisible()
    expect(Math.abs((await page.locator(dock).boundingBox())!.height - withEditor)).toBeLessThan(3)
  })

  test('hiding the editor opens the terminal when it was closed', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await expect(page.locator(dock)).toBeHidden()
    await page.locator('#v-detail .obs-editor-toggle').click()
    await expect(page.locator(dock)).toBeVisible()
    await expect(page.locator('#v-detail .obs')).toHaveClass(/editor-hidden/)
  })

  test('⌥⌘E toggles editor visibility', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    await page.keyboard.press('Meta+Alt+E')
    await expect(obs).toHaveClass(/editor-hidden/)
    await expect(page.locator(dock)).toBeVisible()

    await page.keyboard.press('Meta+Alt+E')
    await expect(obs).not.toHaveClass(/editor-hidden/)
  })

  test('hiding the terminal clears editor-hidden', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    await page.locator('#v-detail .obs-editor-toggle').click()
    await expect(obs).toHaveClass(/editor-hidden/)
    await expect(page.locator(dock)).toBeVisible()

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeHidden()
    await expect(obs).not.toHaveClass(/editor-hidden/)
    await expect(page.locator('#v-detail .obs-editors')).toBeVisible()
  })

  // Maximize is transient: it must not survive a hide, or reopening the drawer
  // later ambushes you with a terminal covering the editor.
  test('hiding the dock clears maximize', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    // From closed, the maximize binding opens the dock rather than arming an
    // invisible state.
    await page.keyboard.press('Control+Shift+`')
    await expect(page.locator(dock)).toBeVisible()
    await expect(obs).toHaveClass(/term-max/)

    await page.keyboard.press('Control+`')
    await expect(page.locator(dock)).toBeHidden()
    await page.keyboard.press('Control+`')
    await expect(page.locator(dock)).toBeVisible()
    await expect(obs).not.toHaveClass(/term-max/)
  })

  // A terminal belongs to the workspace it was opened in, so its drawer does too.
  // A single global flag meant opening a shell in one workspace silently opened —
  // and maximized — the dock in every other one.
  test('dock visibility and maximize are per workspace', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoApp(page)

    const active = seed.loops.filter((l) => l.group === 'active')
    const obs = page.locator('#v-detail .obs')
    const open = async (goal: string) => {
      await openWorkspaceByGoal(page, goal)
      await expect(obs).toBeVisible()
    }
    const backToList = async () => {
      await page.locator('.nav.inbox-nav').click()
      await expect(page.locator('#v-briefing')).toHaveClass(/on/)
    }

    await open(active[0].goal)
    await page.keyboard.press('Control+Shift+`')
    await expect(page.locator(dock)).toBeVisible()
    await expect(obs).toHaveClass(/term-max/)

    await backToList()
    await open(active[1].goal)
    await expect(page.locator(dock)).toBeHidden()
    await expect(obs).not.toHaveClass(/term-max/)

    // Opening the second workspace's dock must not inherit the first's maximize.
    await page.keyboard.press('Control+`')
    await expect(page.locator(dock)).toBeVisible()
    await expect(obs).not.toHaveClass(/term-max/)

    await backToList()
    await open(active[0].goal)
    await expect(obs).toHaveClass(/term-max/)
  })

  // Hot shells stay mounted when switching away. Autofocusing a remounted
  // xterm used to steal keys; the live shell must restore the last chrome
  // (editor / agent / terminal) without tearing the dock down.
  test('switching workspaces restores the last focused chrome, not the terminal', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoApp(page)

    const active = seed.loops.filter((l) => l.group === 'active')
    const obs = page.locator('#v-detail .obs')
    const filechip = page.locator('#v-detail .obs-filechip.on')
    const open = async (goal: string) => {
      await openWorkspaceByGoal(page, goal)
      await expect(obs).toBeVisible()
    }

    await open(active[0].goal)
    await page.keyboard.press('Meta+p')
    const qo = page.locator('.qo')
    await expect(qo).toBeVisible()
    await qo.locator('input').fill('main.go')
    await expect(qo.locator('.qo-row').first()).toContainText('main.go')
    await page.keyboard.press('Enter')
    await expect(filechip).toContainText('main.go')
    await expect(page.locator('#v-detail .cm-editor')).toBeVisible()

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeVisible()
    await expect(page.locator(`${dock} .xterm`)).toBeVisible()
    await page.locator('#v-detail .cm-content').click()
    await expect(filechip).toHaveClass(/lit/)
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)

    await open(active[1].goal)
    await expect(obs).toBeVisible()
    const attaches = () => calls.filter((c) => c.name === 'AttachTerminal').length
    const attachesBeforeReturn = attaches()
    await open(active[0].goal)
    await expect(page.locator('#v-detail .cm-editor')).toBeVisible()
    await expect(filechip).toContainText('main.go')
    await expect(page.locator(dock)).toBeVisible()
    // Keep-alive: the dock never unmounted, so AttachTerminal is not called again.
    await expect.poll(() => attaches()).toBe(attachesBeforeReturn)
    await expect(page.locator(`${dock} .term-tab.on`)).toBeVisible()
    await page.evaluate(() => new Promise<void>((r) => {
      requestAnimationFrame(() => requestAnimationFrame(() => r()))
    }))
    await expect(filechip).toHaveClass(/lit/)
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)
    await expect(page.locator(`${dock}:focus-within`)).toHaveCount(0)

    await page.locator(`${dock} .xterm`).first().click()
    await expect(page.locator(`${dock} .term-tab.on`)).toHaveClass(/lit/)
    await expect(filechip).not.toHaveClass(/lit/)

    await open(active[1].goal)
    await open(active[0].goal)
    await expect(page.locator(dock)).toBeVisible()
    await expect.poll(async () => {
      return page.locator(`${dock} .xterm.focus, ${dock}:focus-within`).count()
    }).toBeGreaterThan(0)
    await expect(page.locator(`${dock} .term-tab.on`)).toHaveClass(/lit/)
    await expect(filechip).not.toHaveClass(/lit/)

    await page.keyboard.press('Meta+1')
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeFocused()
    await expect(page.locator(`${dock} .term-tab.on`)).not.toHaveClass(/lit/)

    await open(active[1].goal)
    const attachesBeforeAgent = attaches()
    await open(active[0].goal)
    await expect.poll(() => attaches()).toBe(attachesBeforeAgent)
    await expect(page.locator(`${dock} .term-tab.on`)).toBeVisible()
    await page.evaluate(() => new Promise<void>((r) => {
      requestAnimationFrame(() => requestAnimationFrame(() => r()))
    }))
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeFocused()
    await expect(page.locator(`${dock}:focus-within`)).toHaveCount(0)
  })

  // Regression: xterm names its root `terminal xterm` and adds `focus` while the
  // terminal has keyboard focus. The app's own `.focus` class is an amber card
  // (border + radius + padding + margin + display:flex), so focusing the terminal
  // wrapped it in a panel AND re-laid-out xterm's viewport/screen as flex items.
  // Any future app-level class with one of those generic names would do it again.
  test('focusing the terminal does not let app styles reshape xterm', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    const xterm = page.locator(`${dock} .xterm`)
    await expect(xterm).toBeVisible()
    await xterm.click()
    await expect(xterm).toHaveClass(/focus/)

    const box = await xterm.evaluate((el) => {
      const cs = getComputedStyle(el)
      return {
        display: cs.display,
        border: cs.borderWidth,
        radius: cs.borderRadius,
        padding: cs.padding,
        margin: cs.margin,
      }
    })
    expect(box).toEqual({
      display: 'block',
      border: '0px',
      radius: '0px',
      padding: '0px',
      margin: '0px',
    })
  })

  test('the dock survives a reload and stays out of the way when never opened', async ({ page }) => {
    const calls: Call[] = []
    // Idle roster: the seeded 'directing' MA would lock the composer (busy gate).
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    // Never opened: no PTY is spawned for someone who doesn't use the terminal.
    await expect(page.locator(dock)).toBeHidden()
    expect(calls.filter((c) => c.name === 'OpenTerminal')).toHaveLength(0)

    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeVisible()

    await reloadApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator(dock)).toBeVisible()
  })

  test('maximize survives a reload', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    const obs = page.locator('#v-detail .obs')
    await page.keyboard.press('Control+Shift+`')
    await expect(page.locator(dock)).toBeVisible()
    await expect(obs).toHaveClass(/term-max/)
    await expect.poll(() => calls.some((c) => c.name === 'SetTerminalDockChrome' && c.args[1]?.maximized)).toBe(true)
    expect(await page.evaluate(() => localStorage.getItem('recur.terminalDockMaximized'))).toBeNull()

    await reloadApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator(dock)).toBeVisible()
    await expect(obs).toHaveClass(/term-max/)
    await expect(page.locator('#v-detail .obs-toolbar')).toHaveAttribute('inert', '')
  })

  test('beside survives a reload via dock chrome, not localStorage', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-beside-toggle').click()
    await expect(page.locator('#v-detail .obs-main')).toHaveClass(/term-beside/)
    await expect.poll(() => calls.some((c) => c.name === 'SetTerminalDockChrome' && c.args[1]?.beside)).toBe(true)
    expect(await page.evaluate(() => localStorage.getItem('recur.terminalBeside'))).toBeNull()
    expect(await page.evaluate(() => localStorage.getItem('recur.terminalBesidePct'))).toBeNull()

    await reloadApp(page)
    const working = seed.loops.find((l) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs-main')).toHaveClass(/term-beside/)
    await expect(page.locator(dock)).toBeVisible()
  })

  const titlebarDev = '.titlebar .tb-right button'

  test('Run types into a live tab; Stop interrupts; another Run reuses it', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await expect(page.locator(titlebarDev, { hasText: 'Run dev' })).toBeVisible()
    await page.locator('#v-detail .obs-term-toggle').click()
    await expect(page.locator(dock)).toBeVisible()
    await expect(page.locator(`${dock} .term-act[aria-label="Run dev"]`)).toBeVisible()

    await page.locator(`${dock} .term-act[aria-label="Run dev"]`).click()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()
    await expect(page.locator(`${dock} .term-act[aria-label="Stop dev"]`)).toBeVisible()
    await expect.poll(() => calls.filter((c) => c.name === 'WriteTerminal').length).toBeGreaterThan(0)
    const written = calls.filter((c) => c.name === 'WriteTerminal').at(-1)!
    expect(written.args[1]).toMatch(/pnpm dev/)
    expect(written.args[1]).not.toMatch(/exec/)

    const opensAfterRun = calls.filter((c) => c.name === 'OpenTerminal').length

    await page.locator(`${dock} .term-act[aria-label="Stop dev"]`).click()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toHaveCount(0)
    await expect(page.locator(`${dock} .term-act[aria-label="Run dev"]`)).toBeVisible()
    await expect.poll(() => calls.some((c) => c.name === 'InterruptTerminal')).toBe(true)
    await expect(page.locator(`${dock} .term-tab`, { hasText: 'exited' })).toHaveCount(0)

    await page.locator(`${dock} .term-act[aria-label="Run dev"]`).click()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()
    await expect.poll(() => calls.filter((c) => c.name === 'WriteTerminal' && /pnpm dev/.test(String(c.args[1]))).length).toBe(2)
    expect(calls.filter((c) => c.name === 'OpenTerminal')).toHaveLength(opensAfterRun)
  })

  test('titlebar Stop dev interrupts the process even if the dock is hidden', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator(titlebarDev, { hasText: 'Run dev' }).click()
    await expect(page.locator(dock)).toBeVisible()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()
    await expect(page.locator(`${dock} .term-act[aria-label="Stop dev"]`)).toBeVisible()

    await page.locator(`${dock} .term-act[aria-label="Hide terminal"]`).click()
    await expect(page.locator(dock)).toBeHidden()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()

    const closesBefore = calls.filter((c) => c.name === 'CloseTerminal').length
    await page.locator(titlebarDev, { hasText: 'Stop dev' }).click()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toHaveCount(0)
    await expect(page.locator(dock)).toBeHidden()
    await expect.poll(() => calls.some((c) => c.name === 'InterruptTerminal')).toBe(true)
    expect(calls.filter((c) => c.name === 'CloseTerminal')).toHaveLength(closesBefore)
  })

  test('an idle run returns the controls to Run without killing the tab', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await page.locator(`${dock} .term-act[aria-label="Run dev"]`).click()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()

    const n = calls.filter((c) => c.name === 'OpenTerminal').length
    await expect.poll(async () => {
      await pushIdle(page, `sess-${n}`)
      return page.locator(titlebarDev, { hasText: 'Stop dev' }).count()
    }).toBe(0)
    await expect(page.locator(`${dock} .term-act[aria-label="Run dev"]`)).toBeVisible()
    await expect(page.locator(`${dock} .term-tab`, { hasText: 'exited' })).toHaveCount(0)
  })

  test('an exited run session returns the controls to Run', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.locator('#v-detail .obs-term-toggle').click()
    await page.locator(`${dock} .term-act[aria-label="Run dev"]`).click()
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()

    const n = calls.filter((c) => c.name === 'OpenTerminal').length
    await pushExit(page, `sess-${n}`, 0)
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toHaveCount(0)
    await expect(page.locator(`${dock} .term-act[aria-label="Run dev"]`)).toBeVisible()
  })

  test('⌘R opens the run palette; Enter runs the selected command', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await expect(page.locator(titlebarDev, { hasText: 'Run dev' })).toBeVisible()
    await expect(page.locator(titlebarDev).locator('.kbd')).toHaveCount(0)

    await page.keyboard.press('ControlOrMeta+r')
    const pal = page.locator('.run-palette')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await expect(pal.locator('.pal-row').first()).toContainText('dev')
    await expect(pal.locator('.pal-row').first()).toHaveClass(/sel/)
    await expect(pal.locator('.pal-foot')).toContainText('navigate')

    await page.keyboard.press('Enter')
    await expect(page.locator('.palette-bg.open .run-palette')).toHaveCount(0)
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()
    await expect(page.locator(dock)).toBeVisible()
    await expect.poll(() => calls.filter((c) => c.name === 'WriteTerminal').length).toBeGreaterThan(0)
  })

  test('⌘R palette filters to matching commands as you type', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(
      { name: 'dev', cmd: 'wails3 task dev' },
      { name: 'test:go', cmd: 'wails3 task test:go' },
      { name: 'test:frontend', cmd: 'wails3 task test:frontend' },
      { name: 'check', cmd: 'wails3 task check' },
      { name: 'tidy', cmd: 'wails3 task common:go:mod:tidy' },
    ), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.keyboard.press('ControlOrMeta+r')
    const pal = page.locator('.run-palette')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await expect(pal.locator('.pal-row')).toHaveCount(5)

    await pal.locator('.pal-in input').pressSequentially('tid')
    await expect(pal.locator('.pal-in input')).toHaveValue('tid')
    await expect(pal.locator('.pal-row')).toHaveCount(1)
    await expect(pal.locator('.pal-row')).toContainText('tidy')
    await expect(pal.locator('.pal-row')).toHaveClass(/sel/)
    await expect(pal.locator('.pal-row', { hasText: 'dev' })).toHaveCount(0)

    await page.keyboard.press('Escape')
    await page.keyboard.press('ControlOrMeta+r')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await expect(pal.locator('.pal-row')).toHaveCount(5)
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('ArrowDown')
    await expect(pal.locator('.pal-row').nth(3)).toContainText('check')
    await expect(pal.locator('.pal-row').nth(3)).toHaveClass(/sel/)

    await pal.locator('.pal-in input').pressSequentially('test')
    await expect(pal.locator('.pal-row')).toHaveCount(2)
    await expect(pal.locator('.pal-row').first()).toContainText('test:go')
    await expect(pal.locator('.pal-row').first()).toHaveClass(/sel/)
    await expect(pal.locator('.pal-row').nth(1)).toContainText('test:frontend')
    await expect(pal.locator('.pal-row').nth(1)).not.toHaveClass(/sel/)
  })

  test('⌘R palette 1–9 and Escape match Open in', async ({ page }) => {
    const calls: Call[] = []
    await installBridge(page, seedWithRuns(), {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.keyboard.press('ControlOrMeta+r')
    const pal = page.locator('.run-palette')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(page.locator('.palette-bg.open .run-palette')).toHaveCount(0)

    await page.keyboard.press('ControlOrMeta+r')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await page.keyboard.press('1')
    await expect(page.locator('.palette-bg.open .run-palette')).toHaveCount(0)
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()
  })

  test('⌘⇧R opens the stop palette when several commands are running', async ({ page }) => {
    const calls: Call[] = []
    const data = seedWithRuns(
      { name: 'dev', cmd: 'pnpm dev --port $RECUR_PORT' },
      { name: 'test', cmd: 'go test ./...' },
    )
    await installBridge(page, data, {
      resolvers: { ...terminalResolvers(page, calls), ListLoopAgents: () => [] },
    })
    await gotoObservatory(page)

    await page.keyboard.press('ControlOrMeta+r')
    await expect(page.locator('.run-palette')).toBeVisible()
    await expect(page.locator('.run-palette .pal-in input')).toBeFocused()
    await page.keyboard.press('1')
    await expect(page.locator(titlebarDev, { hasText: 'Stop dev' })).toBeVisible()

    await page.keyboard.press('ControlOrMeta+r')
    await expect(page.locator('.run-palette')).toBeVisible()
    await expect(page.locator('.run-palette .pal-in input')).toBeFocused()
    await page.keyboard.press('2')
    await expect(page.locator('.titlebar .tb-right .tbtn', { hasText: /^Stop$/ })).toBeVisible()

    await page.keyboard.press('ControlOrMeta+Shift+r')
    const pal = page.locator('.stop-palette')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await expect(pal.locator('.pal-row')).toHaveCount(3)
    await expect(pal.locator('.pal-row').nth(0)).toContainText('Stop dev')
    await expect(pal.locator('.pal-row').nth(1)).toContainText('Stop test')
    await expect(pal.locator('.pal-row').nth(2)).toContainText('Stop all')
    await expect(pal.locator('.pal-foot')).toContainText('navigate')

    await page.keyboard.press('Escape')
    await expect(page.locator('.palette-bg.open .stop-palette')).toHaveCount(0)

    await page.keyboard.press('ControlOrMeta+Shift+r')
    await expect(pal).toBeVisible()
    await expect(pal.locator('.pal-in input')).toBeFocused()
    await page.keyboard.press('1')
    await expect(page.locator('.palette-bg.open .stop-palette')).toHaveCount(0)
    await expect.poll(() => calls.filter((c) => c.name === 'InterruptTerminal').length).toBeGreaterThan(0)
  })
})
