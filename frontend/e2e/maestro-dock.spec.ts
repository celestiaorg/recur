import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

// The chat dock (unified surface): the Observatory's
// left panel hosts every chat tab — a workspace with an engaged run leads with
// the pinned Loop (MA) tab. There is no separate Maestro tab/view anymore; the
// dock IS the chat surface, toggleable like any Observatory panel.

async function openWorking(page: import('@playwright/test').Page) {
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  return working
}

test.describe('chat dock', () => {
  test('the dock opens with the Loop tab, sends a message, and toggles closed', async ({ page }) => {
    // The send path resyncs from the durable transcript after each turn, so the
    // mock must serve what was sent (an empty transcript would clobber the
    // optimistic rows).
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        // Idle roster: the seeded 'directing' MA would lock the composer.
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: 'On it — steering the loop (mock reply).' })
          return 'On it — steering the loop (mock reply).'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    await openWorking(page)

    // Open by default; the run's chat leads with the pinned Loop (MA) tab.
    const dock = page.locator('#v-detail .obs-left')
    await expect(dock).toBeVisible()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'Loop' })).toBeVisible()
    await expect(dock.locator('.chat-in .chat-composer')).toBeVisible()

    // Editor and dock coexist: dock sits left of the editor surface.
    await page.locator('#v-detail .obs-files .obs-node', { hasText: 'main.go' }).click()
    const codeSeg = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeSeg.count()) await codeSeg.click()
    await expect(page.locator('#v-detail .cm-editor')).toBeVisible()
    const leftBox = await dock.boundingBox()
    const editorBox = await page.locator('#v-detail .cm-editor').boundingBox()
    expect(leftBox && editorBox && leftBox.x < editorBox.x).toBeTruthy()

    // Send through the dock: the message renders and the reply lands.
    await dock.locator('.chat-in .chat-composer').fill('steer from dock')
    await dock.locator('.chat-in .send').click()
    await expect(dock.locator('.chat-row.me', { hasText: 'steer from dock' })).toBeVisible()

    // Toggle closed and back open from the left toolbar.
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Hide left panel"]').click()
    await expect(page.locator('#v-detail .obs-left')).toHaveCount(0)
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Show left panel"]').click()
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()
    // The transcript survives the toggle.
    await expect(page.locator('#v-detail .obs-left .chat-row.me', { hasText: 'steer from dock' })).toBeVisible()
  })

  test('hiding the left panel still works after visiting the linked PR', async ({ page }) => {
    await installBridge(page, structuredClone(seed), { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    const linked = seed.loops.find((l: { id: string }) => l.id === 'LOOP-2231')!
    await openWorkspaceByGoal(page, linked.goal)
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()

    await page.locator('.titlebar button', { hasText: 'View PR #482' }).click()
    await expect(page.locator('#prd-agent .obs-left')).toBeVisible()

    await page.locator('.titlebar button', { hasText: 'View workspace' }).click()
    await expect(page.locator('#v-detail')).toHaveClass(/on/)
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()

    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Hide left panel"]').click()
    await expect(page.locator('#v-detail .obs-left')).toHaveCount(0)
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Show left panel"]').click()
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()
  })

  test('the chat panel can open on the right when configured', async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem(
        'recur.codePanelSides',
        JSON.stringify({
          maestro: 'right',
          files: 'left',
          outline: 'right',
          search: 'right',
          refs: 'right',
        }),
      )
    })
    await gotoApp(page)
    await openWorking(page)

    // The chat toggle lives in the right toolbar; Files stays on the left.
    await expect(page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Agent"]')).toBeVisible()
    await expect(page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label^="Agent"]')).toHaveCount(0)

    await page.locator('#v-detail .obs-tb-right .obs-pane-toggle[aria-label^="Agent"]').click()
    await expect(page.locator('#v-detail .obs-side .chat-in .chat-composer')).toBeVisible()

    // Open a file on the left; the chat pane sits right of the editor.
    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Files"]').click()
    await page.locator('#v-detail .obs-left .obs-files .obs-node', { hasText: 'main.go' }).click()
    const codeSeg = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
    if (await codeSeg.count()) await codeSeg.click()
    await expect(page.locator('#v-detail .cm-editor')).toBeVisible()
    const sideBox = await page.locator('#v-detail .obs-side').boundingBox()
    const editorBox = await page.locator('#v-detail .cm-editor').boundingBox()
    expect(sideBox && editorBox && sideBox.x > editorBox.x).toBeTruthy()
  })
})
