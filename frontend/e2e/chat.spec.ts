import { type Locator } from '@playwright/test'
import { test, expect, gotoApp, seed, openWorkspaceByGoal, waitForSavedChatTab } from './helpers/fixtures'
import { installBridge, simulateChatTurnIdle } from './helpers/mock-bridge'
import { codeContent } from './helpers/editor'
import { NEAR_BOTTOM_PX } from '../src/lib/caches/chat-transcript-ui'
import { CHAT_HYDRATE_ALL_BELOW } from '../src/lib/chat-window'

test.describe('Maestro chat auto-scroll', () => {
  test('sending a message scrolls to the bottom', async ({ page }) => {
    // The send path resyncs from the durable transcript after each turn; serve
    // what was sent so the resync cannot clobber the rows mid-assertion.
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        // The seeded MA agent is 'directing', which marks the tab busy (Stop +
        // queue mode) — this test drives immediate sends, so the roster is idle.
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: `reply to: ${a[2]}` })
          return `reply to: ${a[2]}`
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    // The Maestro thread is the Loop (MA) tab in the left chat dock.
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeVisible()

    const scroller = page.locator('#v-detail .chat-scroll')
    const textarea = page.locator('#v-detail .chat-in .chat-composer')
    for (let i = 0; i < 8; i++) {
      await textarea.fill(`message ${i}`)
      await page.locator('#v-detail .chat-in .send:not(.stop)').click()
      await expect(page.locator('#v-detail .chat-row.me', { hasText: `message ${i}` })).toBeVisible()
    }

    await expect.poll(
      () => scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight < 4),
    ).toBe(true)
  })

  test('scrolling away from the bottom keeps position when a reply streams in', async ({ page }) => {
    let resolveReply: (() => void) | undefined
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          return new Promise((resolve) => {
            resolveReply = () => {
              transcript.push({ role: 'assistant', kind: 'text', text: 'a slow reply' })
              resolve('a slow reply')
            }
          })
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    // The Maestro thread is the Loop (MA) tab in the left chat dock.
    await expect(page.locator('#v-detail .obs-left .chat-in .chat-composer')).toBeVisible()

    const scroller = page.locator('#v-detail .chat-scroll')
    const textarea = page.locator('#v-detail .chat-in .chat-composer')
    await textarea.fill('kick off a slow reply')
    await page.locator('#v-detail .chat-in .send:not(.stop)').click()

    // Scroll away from the bottom before the reply lands.
    await scroller.evaluate((el) => { el.scrollTop = 0 })
    const before = await scroller.evaluate((el) => el.scrollTop)

    resolveReply?.()
    await expect(page.locator('#v-detail .chat-canvas', { hasText: 'a slow reply' })).toBeVisible()
    const after = await scroller.evaluate((el) => el.scrollTop)
    expect(after).toBe(before)
  })
})

test.describe('Chat follow-up queue', () => {
  test('all follow-ups queued while a turn is running fold into the next user turn', async ({ page }) => {
    const sent: string[] = []
    const items: Record<string, any[]> = {}
    let resolveFirst: ((value: string) => void) | undefined
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'MA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          sent.push(text)
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text })
          if (sent.length === 1) {
            return new Promise((resolve) => { resolveFirst = resolve })
          }
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: `reply to: ${text}` })
          return `reply to: ${text}`
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const textarea = dock.locator('.chat-in .chat-composer')
    await textarea.fill('first turn')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()

    // Composer stays editable — queue multiple follow-ups while the turn is in flight.
    await expect(textarea).toBeEditable()
    await textarea.fill('fix the tests')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await textarea.fill('and update the docs')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me.queued', { hasText: 'fix the tests' })).toBeVisible()
    await expect(dock.locator('.chat-row.me.queued', { hasText: 'and update the docs' })).toBeVisible()
    expect(sent).toEqual(['first turn'])

    // Drop one while still queued — only the remaining follow-up should flush.
    await dock.locator('.chat-row.me.queued', { hasText: 'fix the tests' })
      .locator('.chat-queued-x').click()
    await expect(dock.locator('.chat-row.me.queued', { hasText: 'fix the tests' })).toHaveCount(0)
    await expect(dock.locator('.chat-row.me.queued', { hasText: 'and update the docs' })).toBeVisible()

    resolveFirst?.('done with first')
    await expect.poll(() => sent.slice()).toEqual(['first turn', 'and update the docs'])
    await expect(dock.locator('.chat-row.me.queued')).toHaveCount(0)
    await expect(dock.locator('.chat-row.me', { hasText: 'and update the docs' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'fix the tests' })).toHaveCount(0)
  })

  for (const queueFollowUps of [false, true]) {
    test(`busy follow-ups: ⏎ ${queueFollowUps ? 'queues' : 'steers'}, ⌘⏎ does the other (queueFollowUps=${queueFollowUps})`, async ({ page }) => {
      const sent: string[] = []
      const items: Record<string, any[]> = {}
      const s = structuredClone(seed)
      s.settings = { ...s.settings, queueFollowUps }
      await installBridge(page, s, {
        resolvers: {
          ListLoopAgents: () => [],
          WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
          SendWorkspaceMessage: (a: any[]) => {
            const text = a[2] as string
            sent.push(text)
            ;(items[a[1]] ??= []).push({ role: 'user', kind: 'text', text })
            return sent.length === 1 ? new Promise(() => {}) : '\u2060inject'
          },
        },
      })
      await gotoApp(page)
      const working = seed.loops.find((l: any) => l.group === 'active')!
      await openWorkspaceByGoal(page, working.goal)

      const dock = page.locator('#v-detail .obs-left')
      await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
      const composer = dock.locator('.chat-in .chat-composer')
      await composer.fill('first turn')
      await composer.press('Enter')
      await expect(dock.locator('.chat-in .send.stop')).toBeVisible()

      const steerKey = queueFollowUps ? 'Meta+Enter' : 'Enter'
      const queueKey = queueFollowUps ? 'Enter' : 'Meta+Enter'
      await composer.fill('steer now')
      await composer.press(steerKey)
      await expect.poll(() => sent.slice()).toEqual(['first turn', 'steer now'])
      await expect(dock.locator('.chat-row.me.queued')).toHaveCount(0)

      await composer.fill('after the turn')
      await composer.press(queueKey)
      await expect(dock.locator('.chat-row.me.queued', { hasText: 'after the turn' })).toBeVisible()
      expect(sent).toEqual(['first turn', 'steer now'])

      await composer.fill('steer past the queue')
      await composer.press(steerKey)
      await expect.poll(() => sent.slice()).toEqual(['first turn', 'steer now', 'steer past the queue'])
      await expect(dock.locator('.chat-row.me.queued', { hasText: 'after the turn' })).toBeVisible()
    })
  }

  test('Stop restores queued follow-ups into the composer', async ({ page }) => {
    const sent: string[] = []
    const items: Record<string, any[]> = {}
    let resolveFirst: ((value: string) => void) | undefined
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'MA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          sent.push(text)
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text })
          if (sent.length === 1) {
            return new Promise((resolve) => { resolveFirst = resolve })
          }
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: `reply to: ${text}` })
          return `reply to: ${text}`
        },
        CancelWorkspaceMessage: (a: any[]) => {
          ;(items[a[1]] ??= []).push({ role: 'assistant', kind: 'text', text: 'Stopped by user' })
          resolveFirst?.('Stopped by user')
          return null
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const textarea = dock.locator('.chat-in .chat-composer')
    await textarea.fill('first turn')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()

    await textarea.fill('fix the tests')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await textarea.fill('and update the docs')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me.queued')).toHaveCount(2)

    await dock.locator('.chat-in .send.stop').click()
    await expect(dock.locator('.chat-stopped', { hasText: 'Stopped by user' })).toBeVisible()
    // Queue clears into the composer — Cancel must not auto-dispatch follow-ups.
    await expect(dock.locator('.chat-row.me.queued')).toHaveCount(0)
    // Contenteditable hydrates each line as a <p>; textContent joins them
    // without the `\n\n` separators the source string used.
    await expect.poll(async () => {
      const texts = await textarea.locator('p').allTextContents()
      return texts.map((t) => t.trim()).filter(Boolean)
    }).toEqual(['fix the tests', 'and update the docs'])
    expect(sent).toEqual(['first turn'])
  })
})

test.describe('Composer undo', () => {
  test('⌘Z undoes one typing burst at a time, including Shift+Enter', async ({ page }) => {
    await installBridge(page, structuredClone(seed), { resolvers: { ListLoopAgents: () => [] } })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const composer = page.locator('#v-detail .obs-left .chat-in .chat-composer')
    const lines = async () => (await composer.locator('p').allTextContents()).map((t) => t.trim())
    await composer.click()
    await page.keyboard.type('hello')
    await page.waitForTimeout(1000)
    await page.keyboard.type(' world')
    await page.keyboard.press('Shift+Enter')
    await page.keyboard.type('next')

    await page.keyboard.press('Meta+z')
    await expect.poll(lines).toEqual(['hello world', ''])
    await page.keyboard.press('Meta+z')
    await expect.poll(lines).toEqual(['hello world'])
    await page.keyboard.press('Meta+z')
    await expect.poll(lines).toEqual(['hello'])
    await page.keyboard.press('Meta+Shift+z')
    await expect.poll(lines).toEqual(['hello world'])
  })
})

test.describe('Fix findings', () => {
  const openFinding = {
    id: 'f-1',
    iteration: 1,
    file: 'src/main.go',
    line: 10,
    severity: 'medium',
    message: 'handle the error',
    status: 'open',
  }

  test('slash /fix-findings shows a short label while sending the full checklist', async ({ page }) => {
    const sent: string[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFindings: (a) => [{ ...openFinding, workspaceId: a[0] }],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push(text)
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: 'On it.' })
          return 'On it.'
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await page.keyboard.type('/fix-findings')
    const row = dock.locator('.chat-mention-row', { hasText: 'fix-findings' })
    await expect(row).toBeVisible()
    // Enter keeps focus on the composer while hydrate clears the /query — that
    // used to leave WebKit’s caret on the root (visually too high).
    await page.keyboard.press('Enter')

    await expect(dock.locator('.chat-row.me', { hasText: 'Fix open reviewer findings' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'mcp__recur__list_findings' })).toHaveCount(0)
    expect(sent).toHaveLength(1)
    expect(sent[0]).toContain('mcp__recur__list_findings')
    expect(sent[0]).toContain('mcp__recur__update_finding')

    await expect(composer).toBeFocused()
    const caretInsideP = await composer.evaluate((el) => {
      const sel = window.getSelection()
      const node = sel?.focusNode
      if (!node) return false
      for (let n: Node | null = node; n && n !== el; n = n.parentNode) {
        if (n.nodeType === Node.ELEMENT_NODE && (n as HTMLElement).tagName === 'P') return true
      }
      return false
    })
    expect(caretInsideP).toBe(true)
  })

  test('slash /review-and-fix opens a Loop tab instead of converting the Agent chat', async ({ page }) => {
    const sent: { text: string; mode: string; display: string; code: string }[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const mode = (a[6] as string) || ''
          const display = (a[7] as string) || text
          sent.push({ text, mode, display, code })
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: 'Loop engaged.' })
          return 'Loop engaged.'
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await page.keyboard.type('/review-and-fix')
    const row = dock.locator('.chat-mention-row', { hasText: 'review-and-fix' })
    await expect(row).toBeVisible()
    await expect(row.locator('.chat-mention-badge')).toHaveText('Loop')
    await page.keyboard.press('Enter')

    await expect(dock.locator('.chat-row.me', { hasText: 'Review & fix' })).toBeVisible()
    await expect(dock.locator('.sel-btn[aria-label="Chat mode"]')).toContainText('Loop')
    // Catalog ListLoops shares the mock name with the workspace alias, so the
    // picker may show the id; the value is what engage stamps.
    await expect(dock.locator('.sel-btn[aria-label="Loop"]')).toHaveAttribute('title', 'recur.loops.converge')
    expect(sent).toHaveLength(1)
    expect(sent[0].mode).toBe('loop')
    expect(sent[0].code).not.toBe('PA')
    expect(sent[0].display).toBe('Review & fix')
    expect(sent[0].text).toContain('fix and re-review until the policy bar passes')
  })

  test('slash /review on a started agent tab opens a new chat', async ({ page }) => {
    const sent: { text: string; display: string; code: string }[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push({ text, display, code })
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: 'On it.' })
          return 'On it.'
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.fill('hello from the agent tab')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()

    await composer.click()
    await page.keyboard.type('/review')
    await dock.locator('.chat-mention-name', { hasText: /^\/review$/ }).click()

    await expect(dock.locator('.chat-row.me', { hasText: 'Please review' })).toBeVisible()
    const review = sent.find((s) => s.text.includes('file findings'))
    expect(review).toBeTruthy()
    expect(review!.code).not.toBe('PA')
    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'Please review' })).toHaveCount(0)
  })

  test('slash /commit on a started agent tab opens a grouped tab and isolates', async ({ page }) => {
    const sent: { text: string; display: string; code: string; fresh: boolean }[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push({ text, display, code, fresh: !!a[8] })
          const reply = `On it. (${sent.length})`
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: reply })
          return reply
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.fill('hello from the agent tab')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()

    await composer.click()
    await page.keyboard.type('/commit')
    await dock.locator('.chat-mention-name', { hasText: /^\/commit$/ }).click()

    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toHaveCount(0)
    expect(sent).toHaveLength(2)
    expect(sent[1].code).not.toBe(sent[0].code)
    expect(sent[1].fresh).toBe(true)

    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()
    await composer.click()
    await page.keyboard.type('/commit-push')
    await dock.locator('.chat-mention-name', { hasText: /^\/commit-push$/ }).click()

    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes and push' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toHaveCount(0)
    expect(sent).toHaveLength(3)
    expect(sent[2].code).toBe(sent[1].code)
    expect(sent[2].fresh).toBe(true)
  })

  test('slash /commit does not claim a New chat with an unsent draft', async ({ page }) => {
    const sent: { text: string; display: string; code: string }[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push({ text, display, code })
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: 'On it.' })
          return 'On it.'
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.fill('unsent draft stays here')
    await expect(composer).toContainText('unsent draft stays here')

    // Invoke from Maestro so the slash is not typed into the draft composer.
    await dock.locator('.wsv-chat-tab').first().click()
    await composer.click()
    await page.keyboard.type('/commit')
    await dock.locator('.chat-mention-name', { hasText: /^\/commit$/ }).click()

    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes' })).toBeVisible()
    expect(sent).toHaveLength(1)
    expect(sent[0].code).not.toBe('PA')

    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(composer).toContainText('unsent draft stays here')
    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes' })).toHaveCount(0)
  })

  test('failed deferred /commit retry sends a fresh grouped action without eating the draft', async ({ page }) => {
    const sent: { text: string; display: string; code: string; fresh: boolean }[] = []
    const items: Record<string, any[]> = {}
    let createCalls = 0
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        CreateWorkspaceChat: (a: any[], s: any) => {
          createCalls++
          if (createCalls === 1) return null
          const by = (s.chatsByLoop ??= {})
          const n = (by[a[0]]?.length ?? 0) + 2
          const tab = { code: `PA${n}`, title: 'New chat', updatedAt: 0 }
          by[a[0]] = [...(by[a[0]] ?? []), tab]
          return tab
        },
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push({ text, display, code, fresh: !!a[8] })
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: `On it. (${sent.length})` })
          return `On it. (${sent.length})`
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.fill('unsent draft stays here')
    await expect(composer).toContainText('unsent draft stays here')

    await dock.locator('.wsv-chat-tab').first().click()
    await composer.click()
    await page.keyboard.type('/commit')
    await dock.locator('.chat-mention-name', { hasText: /^\/commit$/ }).click()

    await expect(page.locator('.toast.error .ta', { hasText: 'Retry' })).toBeVisible()
    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(composer).toContainText('unsent draft stays here')
    expect(sent).toHaveLength(0)

    await page.locator('.toast.error .ta', { hasText: 'Retry' }).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes' })).toBeVisible()
    expect(sent).toHaveLength(1)
    expect(sent[0].code).not.toBe('PA')
    expect(sent[0].fresh).toBe(true)

    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(composer).toContainText('unsent draft stays here')
    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes' })).toHaveCount(0)
  })

  test('slash /commit-push does not reuse an ops tab with an unsent draft', async ({ page }) => {
    const sent: { text: string; display: string; code: string }[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push({ text, display, code })
          const reply = `On it. (${sent.length})`
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: reply })
          return reply
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.fill('hello from the agent tab')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()

    await composer.click()
    await page.keyboard.type('/commit')
    await dock.locator('.chat-mention-name', { hasText: /^\/commit$/ }).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes' })).toBeVisible()
    const opsCode = sent[1].code
    expect(opsCode).not.toBe('PA')

    await composer.click()
    await composer.fill('follow-up stays on the ops tab')
    await expect(composer).toContainText('follow-up stays on the ops tab')

    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()
    await composer.click()
    await page.keyboard.type('/commit-push')
    await dock.locator('.chat-mention-name', { hasText: /^\/commit-push$/ }).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes and push' })).toBeVisible()
    expect(sent).toHaveLength(3)
    expect(sent[2].code).not.toBe(opsCode)
    expect(sent[2].code).not.toBe('PA')

    await dock.locator('.wsv-chat-tab').nth(2).click()
    await expect(composer).toContainText('follow-up stays on the ops tab')
    await expect(dock.locator('.chat-row.me', { hasText: 'Commit all changes and push' })).toHaveCount(0)
  })

  test('slash /review send stays on the routed tab if the operator switches away', async ({ page }) => {
    let release!: () => void
    const gate = new Promise<void>((r) => { release = r })
    const sent: { text: string; display: string; code: string }[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push({ text, display, code })
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: 'On it.' })
          return 'On it.'
        },
        RunPromptAction: async (a: any[]) => {
          if (a[0] === 'recur.chat.review') {
            await gate
            return {
              prompt:
                'Review the changes in this worktree and file findings. Git context is already gathered — do NOT re-run git status/diff/log.',
              display: 'Please review the changes in this worktree and file any real issues',
              freshSession: true,
            }
          }
          return { prompt: 'x', display: 'x' }
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.fill('hello from the agent tab')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()

    await composer.click()
    await page.keyboard.type('/review')
    await dock.locator('.chat-mention-name', { hasText: /^\/review$/ }).click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)

    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'hello from the agent tab' })).toBeVisible()
    release()

    await expect.poll(() => sent.filter((s) => s.text.includes('file findings')).length).toBe(1)
    const review = sent.find((s) => s.text.includes('file findings'))!
    expect(review.code).not.toBe('PA')
    await expect(dock.locator('.chat-row.me', { hasText: 'Please review' })).toHaveCount(0)
  })

  test('findings-panel Fix open findings switches to the Agent dock and tab', async ({ page }) => {
    const sent: string[] = []
    const items: Record<string, any[]> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFindings: (a) => [{ ...openFinding, workspaceId: a[0] }],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          const code = a[1] as string
          const text = a[2] as string
          const display = (a[7] as string) || text
          sent.push(text)
          ;(items[code] ??= []).push({ role: 'user', kind: 'text', text: display })
          ;(items[code] ??= []).push({ role: 'assistant', kind: 'text', text: 'On it.' })
          return 'On it.'
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    // Prime an idle Agent tab (pickers ready) so send() does not no-op.
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.chat-in .chat-composer')).toBeVisible()

    // Leave Agent for Findings — the regression was staying here after Fix.
    await page.locator('#v-detail .obs-pane-toggle[aria-label^="Findings"]').click()
    await expect(dock.locator('.chat-in .chat-composer')).toHaveCount(0)
    await expect(dock.locator('.obs-icon-btn[aria-label="Fix open findings"]')).toBeVisible()
    await dock.locator('.obs-icon-btn[aria-label="Fix open findings"]').click()

    await expect(page.locator('#v-detail .obs-pane-toggle[aria-label^="Agent"]')).toHaveAttribute('aria-pressed', 'true')
    await expect(dock.locator('.chat-in .chat-composer')).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'Fix open reviewer findings' })).toBeVisible()
    expect(sent).toHaveLength(1)
    expect(sent[0]).toContain('mcp__recur__list_findings')
  })
})

test.describe('Chat stop', () => {
  test("Stop interrupts the ACTIVE tab's in-flight turn (unified surface)", async ({ page }) => {
    const cancelled: string[] = []
    const items: Record<string, any[]> = {}
    let resolveReply: ((value: string) => void) | undefined
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        // Idle roster: the seeded 'directing' MA marks the tab busy (Stop + queue).
        ListLoopAgents: () => [],
        // Durable transcript per tab: the post-turn resync is the authority and
        // would clobber the optimistic rows if mocked empty.
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          ;(items[a[1]] ??= []).push({ role: 'user', kind: 'text', text: a[2] })
          return new Promise((resolve) => { resolveReply = resolve })
        },
        CancelWorkspaceMessage: (a: any[]) => {
          cancelled.push(`${a[0]}:${a[1]}`)
          ;(items[a[1]] ??= []).push({ role: 'assistant', kind: 'text', text: 'Stopped by user' })
          resolveReply?.('Stopped by user')
          return null
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    // The Loop (MA) tab leads the dock; send there, then Stop it.
    const dock = page.locator('#v-detail .obs-left')
    const textarea = dock.locator('.chat-in .chat-composer')
    await textarea.fill('please run forever')
    await dock.locator('.chat-in .send:not(.stop)').click()

    const stopBtn = dock.locator('.chat-in .send.stop')
    await expect(stopBtn).toBeVisible()
    await stopBtn.click()

    await expect(dock.locator('.chat-stopped', { hasText: 'Stopped by user' })).toBeVisible()
    // Cancellation is keyed to the ACTIVE tab — workspace + MA, nothing else.
    expect(cancelled).toEqual([`${working.id}:MA`])
    await expect(dock.locator('.chat-in .send.stop')).toHaveCount(0)
    await expect(dock.locator('.chat-in .send')).toBeVisible()
  })

  test('⌘⇧⌫ stops an in-flight turn when the agent panel is focused', async ({ page }) => {
    const cancelled: string[] = []
    const items: Record<string, any[]> = {}
    let resolveReply: ((value: string) => void) | undefined
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          ;(items[a[1]] ??= []).push({ role: 'user', kind: 'text', text: a[2] })
          return new Promise((resolve) => { resolveReply = resolve })
        },
        CancelWorkspaceMessage: (a: any[]) => {
          cancelled.push(`${a[0]}:${a[1]}`)
          ;(items[a[1]] ??= []).push({ role: 'assistant', kind: 'text', text: 'Stopped by user' })
          resolveReply?.('Stopped by user')
          return null
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const textarea = dock.locator('.chat-in .chat-composer')
    await textarea.fill('please run forever')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()

    await textarea.focus()
    await page.keyboard.press('Meta+Shift+Backspace')
    await expect(dock.locator('.chat-stopped', { hasText: 'Stopped by user' })).toBeVisible()
    expect(cancelled).toEqual([`${working.id}:MA`])
  })

  test("Stop cancels ONLY the active tab — a background tab's turn keeps running", async ({ page }) => {
    const cancelled: string[] = []
    const items: Record<string, any[]> = {}
    const resolvers: Record<string, (v: string) => void> = {}
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'PA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          ;(items[a[1]] ??= []).push({ role: 'user', kind: 'text', text: a[2] })
          return new Promise((resolve) => { resolvers[a[1]] = resolve })
        },
        CancelWorkspaceMessage: (a: any[]) => {
          cancelled.push(`${a[0]}:${a[1]}`)
          ;(items[a[1]] ??= []).push({ role: 'assistant', kind: 'text', text: 'Stopped by user' })
          resolvers[a[1]]?.('Stopped by user')
          return null
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')

    // Start a turn on the PA tab…
    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await dock.locator('.chat-in .chat-composer').fill('long PA task')
    await dock.locator('.chat-in .send:not(.stop)').click()

    // …open a SECOND tab, start a turn there, and Stop only that one.
    // Wait for the new idle tab — the prior tab's composer stays editable while
    // busy (follow-up queue), so filling too early types into the wrong tab.
    await dock.locator('.wsv-chat-new').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)
    await expect(dock.locator('.chat-in .send.stop')).toHaveCount(0)
    await dock.locator('.chat-in .chat-composer').fill('long PA2 task')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()
    await dock.locator('.chat-in .send.stop').click()

    await expect(dock.locator('.chat-stopped', { hasText: 'Stopped by user' })).toBeVisible()
    expect(cancelled).toEqual([`${working.id}:PA2`])

    // The PA tab's turn is untouched: its transcript intact, no stopped note.
    // (Titles reset to "New chat" when the mock re-lists tabs — select by
    // position: the strip is [Loop, PA, PA2].)
    await dock.locator('.wsv-chat-tab').nth(1).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'long PA task' })).toBeVisible()
    await expect(dock.locator('.chat-stopped')).toHaveCount(0)
  })
})

test.describe('Chat live turn restore', () => {
  test('leaving and returning mid-turn keeps earlier messages and the elapsed clock', async ({ page }) => {
    const items: Record<string, any[]> = {
      MA: [
        { role: 'user', kind: 'text', text: 'earlier question' },
        { role: 'assistant', kind: 'text', text: 'earlier answer' },
      ],
    }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => items[a[1] || 'MA'] ?? [],
        SendWorkspaceMessage: (a: any[]) => {
          ;(items[a[1]] ??= []).push({ role: 'user', kind: 'text', text: a[2] })
          return new Promise(() => { /* hang — turn stays in flight */ })
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await expect(dock.locator('.chat-row.me', { hasText: 'earlier question' })).toBeVisible()
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()
    await expect.poll(async () => dock.locator('.chat-turn-elapsed').innerText()).not.toBe('0:00')
    const elapsedBefore = await dock.locator('.chat-turn-elapsed').innerText()

    // Findings unmounts AgentChat; Agent remounts it — the live transcript and
    // turn clock must survive that round-trip.
    await page.locator('#v-detail .obs-pane-toggle[aria-label^="Findings"]').click()
    await expect(dock.locator('.chat-in .chat-composer')).toHaveCount(0)
    await page.locator('#v-detail .obs-pane-toggle[aria-label^="Agent"]').click()
    await expect(dock.locator('.chat-row.me', { hasText: 'earlier question' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'keep going' })).toBeVisible()
    await expect(dock.locator('.chat-canvas', { hasText: 'earlier answer' })).toBeVisible()
    const elapsedAfter = await dock.locator('.chat-turn-elapsed').innerText()
    expect(elapsedAfter).not.toBe('0:00')
    const toSec = (s: string) => {
      const [m, sec] = s.split(':').map(Number)
      return m * 60 + sec
    }
    expect(toSec(elapsedAfter)).toBeGreaterThanOrEqual(toSec(elapsedBefore))
  })

  // Regression: optimistic sendingByKey must drop on agent.updated idle even
  // when the Send RPC never resolves (binding timeout / remount).
  test('elapsed clock hides when the engine goes idle while Send is still pending', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [],
        SendWorkspaceMessage: () => new Promise(() => { /* hang — RPC lags past idle */ }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()

    await simulateChatTurnIdle(page, working.id, 'MA')
    await expect(dock.locator('.chat-turn-elapsed')).toHaveCount(0)
    // The seeded walk is still live, so Stop stays; Send leaves queue mode.
    await expect(dock.locator('.chat-in .send:not(.stop)')).toHaveAttribute('aria-label', 'Send message')
  })

  // session.turn_end stamps "completed in" while Send / agentsRunning can still
  // be lit (RPC lag, lost idle). The live clock must not sit under that footer.
  test('elapsed clock hides when turn_end lands while Send is still pending', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [],
        SendWorkspaceMessage: () => new Promise(() => { /* hang past turn_end */ }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'here is the reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 103000 },
      })
    }, { workspaceId: working.id })

    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await expect(dock.locator('.chat-canvas', { hasText: 'here is the reply' })).toBeVisible()
    await expect(dock.locator('.chat-turn-elapsed')).toHaveCount(0)
  })

  test('leftover stream after turn_end does not leak the elapsed clock onto other chats', async ({ page }) => {
    // One AgentChat instance serves every tab. A leftover delta after the
    // footer must not land (that would reopen showTurnElapsed) and must not
    // reuse the same clockOrigin on every chat. Stop/queue stay tied to the
    // engine busy flag — only the clock hides.
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [],
        SendWorkspaceMessage: () => new Promise(() => { /* hang past turn_end */ }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'here is the reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 103000 },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'late leftover chunk', seq: 2, replace: true },
      })
    }, { workspaceId: working.id })

    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await expect(dock.locator('.chat-canvas', { hasText: 'here is the reply' })).toBeVisible()
    await expect(dock.locator('.chat-canvas')).not.toContainText('late leftover chunk')
    await expect(dock.locator('.chat-turn-elapsed')).toHaveCount(0)
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.chat-turn-elapsed')).toHaveCount(0)

    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await expect(dock.locator('.chat-turn-elapsed')).toHaveCount(0)
    await expect(dock.locator('.chat-canvas')).not.toContainText('late leftover chunk')
  })

  test('a new engine turn after lost idle still streams past the previous footer', async ({ page }) => {
    // turn_end leaves the dispatchSend stamp in place when idle never arrives.
    // The next working status must refresh that stamp so live deltas are not
    // treated as leftover of the closed turn.
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [],
        SendWorkspaceMessage: () => new Promise(() => { /* hang past turn_end */ }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'here is the reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 103000 },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'late leftover chunk', seq: 2, replace: true },
      })
    }, { workspaceId: working.id })

    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await expect(dock.locator('.chat-canvas')).not.toContainText('late leftover chunk')

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'second turn after lost idle', seq: 3, replace: true },
      })
    }, { workspaceId: working.id })

    await expect(dock.locator('.chat-canvas', { hasText: 'second turn after lost idle' })).toBeVisible()
    await expect(dock.getByText('late leftover chunk')).toHaveCount(0)
  })

  test('idle reordered past the next turn still lets that turn stream', async ({ page }) => {
    // Turn 2's working can land before turn 1's idle on the Wails bridge.
    // Idle must not clear the refreshed stamp or the live tail is frozen.
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [],
        SendWorkspaceMessage: () => new Promise(() => { /* hang past turn_end */ }),
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'here is the reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 103000 },
      })
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
    }, { workspaceId: working.id })

    await simulateChatTurnIdle(page, working.id, 'MA')

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'second turn after reordered idle', seq: 2, replace: true },
      })
    }, { workspaceId: working.id })

    await expect(dock.locator('.chat-canvas', { hasText: 'second turn after reordered idle' })).toBeVisible()
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
  })

  test('a reordered idle still drops sticky send so a queued follow-up can flush', async ({ page }) => {
    // Turn 2 can abort with no transcript row. The reordered idle must not
    // wipe the live stamp, but it must still clear sendingByKey so the
    // follow-up queue can flush (busy is sendingByKey / agentsRunning, not
    // the leftover stamp).
    const sent: string[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          sent.push(String(a[2] ?? ''))
          if (sent.length === 1) return new Promise(() => { /* hang past turn_end */ })
          return 'ok'
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()
    await dock.locator('.chat-in .chat-composer').fill('follow up next')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me.queued', { hasText: 'follow up next' })).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'here is the reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 103000 },
      })
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
    }, { workspaceId: working.id })

    await simulateChatTurnIdle(page, working.id, 'MA')
    await expect.poll(() => sent.slice()).toEqual(['keep going', 'follow up next'])
    await expect(dock.locator('.chat-row.me.queued')).toHaveCount(0)
  })

  test('a reply landing after idle flushed the queue does not repaint under the follow-up', async ({ page }) => {
    const sent: string[] = []
    const items: any[] = []
    let resolveFirst: ((value: string) => void) | undefined
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => items,
        SendWorkspaceMessage: (a: any[]) => {
          const text = String(a[2] ?? '')
          sent.push(text)
          items.push({ role: 'user', kind: 'text', text })
          if (sent.length === 1) return new Promise((resolve) => { resolveFirst = resolve })
          return new Promise(() => { /* second turn still running */ })
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('first turn')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()
    await dock.locator('.chat-in .chat-composer').fill('queued follow-up')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-row.me.queued', { hasText: 'queued follow-up' })).toBeVisible()

    items.push({ role: 'assistant', kind: 'text', text: 'first reply text' })
    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'first reply text', seq: 1, replace: true },
      })
    }, { workspaceId: working.id })
    await expect(dock.getByText('first reply text', { exact: true })).toHaveCount(1)

    await simulateChatTurnIdle(page, working.id, 'MA')
    await expect.poll(() => sent.slice()).toEqual(['first turn', 'queued follow-up'])
    resolveFirst?.('first reply text')
    await page.waitForTimeout(300)
    await expect(dock.getByText('first reply text', { exact: true })).toHaveCount(1)
  })

  test('an autonomous turn after idle still streams past the previous footer', async ({ page }) => {
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => transcript,
        SendWorkspaceMessage: (a: any[]) => {
          transcript.push({ role: 'user', kind: 'text', text: String(a[2] ?? '') })
          return new Promise(() => { /* hang — first turn stays pending */ })
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('kick off')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'first reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 1200 },
      })
    }, { workspaceId: working.id })
    transcript.push(
      { role: 'assistant', kind: 'text', text: 'first reply' },
      { role: 'assistant', kind: 'turn', durationMs: 1200, ts: Date.now() },
    )
    await simulateChatTurnIdle(page, working.id, 'MA')
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await expect(dock.locator('.chat-turn-elapsed')).toHaveCount(0)

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'planning the next slice', seq: 2, replace: true },
      })
    }, { workspaceId: working.id })

    await expect(dock.locator('.chat-canvas', { hasText: 'planning the next slice' })).toBeVisible()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()
  })

  test('a second idle resyncs a stream that never got turn_end after a footer', async ({ page }) => {
    // After a stamp refresh over a footer, the first idle is the reordered
    // previous turn. A later idle must still resync so a live spinner does
    // not stick when session.turn_end never arrives.
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => transcript,
        SendWorkspaceMessage: (a: any[]) => {
          transcript.push({ role: 'user', kind: 'text', text: String(a[2] ?? '') })
          return new Promise(() => { /* hang — first turn stays pending */ })
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('kick off')
    await dock.locator('.chat-in .send:not(.stop)').click()

    await page.evaluate(({ workspaceId }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'first reply', seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 1200 },
      })
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'working' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'second turn still streaming', seq: 2, replace: true },
      })
    }, { workspaceId: working.id })
    transcript.push(
      { role: 'assistant', kind: 'text', text: 'first reply' },
      { role: 'assistant', kind: 'turn', durationMs: 1200, ts: Date.now() },
      { role: 'assistant', kind: 'text', text: 'second turn still streaming' },
    )

    await expect(dock.locator('.chat-row.assistant.streaming', { hasText: 'second turn still streaming' })).toBeVisible()

    await simulateChatTurnIdle(page, working.id, 'MA')
    await expect(dock.locator('.chat-row.assistant.streaming', { hasText: 'second turn still streaming' })).toBeVisible()

    await simulateChatTurnIdle(page, working.id, 'MA')
    await expect(dock.locator('.chat-row.assistant.streaming')).toHaveCount(0)
    await expect(dock.locator('.chat-canvas', { hasText: 'second turn still streaming' })).toBeVisible()
  })
})

test.describe('Chat code fence copy', () => {
  test('fenced code in a settled assistant reply has a copy control', async ({ page }) => {
    const fence = 'here is a snippet:\n\n```ts\nconst n = 1\n```\n'
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [
          { role: 'assistant', kind: 'text', text: fence },
        ],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const canvas = page.locator('#v-detail .obs-left .chat-canvas')
    const fenceEl = canvas.locator('.md-fence')
    await expect(fenceEl).toBeVisible()
    const copy = fenceEl.locator('.md-copy')
    await expect(copy).toHaveAttribute('aria-label', 'Copy code')
    await fenceEl.hover()
    await expect(copy).toBeVisible()
    await copy.click()
    await expect(copy).toHaveClass(/copied/)
  })

  test('user messages have a copy control', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [
          { role: 'user', kind: 'text', text: 'please copy this prompt' },
          { role: 'assistant', kind: 'text', text: 'ok' },
        ],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const bubble = page.locator('#v-detail .obs-left .chat-row.me .chat-bubble', {
      hasText: 'please copy this prompt',
    })
    await expect(bubble).toBeVisible()
    const copy = bubble.locator('.chat-bubble-copy')
    await expect(copy).toHaveAttribute('aria-label', 'Copy message')
    await bubble.hover()
    await expect(copy).toBeVisible()
    await copy.click()
    await expect(copy).toHaveClass(/copied/)
  })
})

test.describe('Chat agent error retry', () => {
  test('spend-limit error renders a card with Retry that re-sends the last prompt', async ({ page }) => {
    const sent: { text: string; display: string }[] = []
    const transcript: any[] = []
    let fail = true
    const err = "Agent error: You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit"
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          const text = String(a[2] ?? '')
          const display = String(a[7] ?? '')
          sent.push({ text, display })
          if (display !== '\u2060') {
            transcript.push({ role: 'user', kind: 'text', text })
          }
          if (fail) {
            fail = false
            transcript.push({ role: 'assistant', kind: 'text', text: err })
            return err
          }
          transcript.push({ role: 'assistant', kind: 'text', text: 'ok, continuing' })
          return 'ok, continuing'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('ship the dark mode polish')
    await dock.locator('.chat-in .send:not(.stop)').click()

    const card = dock.locator('.chat-error', { hasText: 'Usage limit' })
    await expect(card).toBeVisible()
    await expect(card.locator('.chat-error-body', { hasText: 'individual spend limit' })).toBeVisible()
    await expect(dock.locator('.chat-row.me', { hasText: 'ship the dark mode polish' })).toHaveCount(1)

    await card.locator('.chat-error-retry').click()
    await expect(dock.locator('.chat-canvas', { hasText: 'ok, continuing' })).toBeVisible()
    await expect.poll(() => sent.map((s) => s.display)).toEqual(['', '\u2060'])
    expect(sent[0].text).toBe('ship the dark mode polish')
    expect(sent[1].text).toBe('\u2060')
    await expect(dock.locator('.chat-row.me', { hasText: 'ship the dark mode polish' })).toHaveCount(1)
  })

  test('Retry shows the elapsed clock before the first stream row', async ({ page }) => {
    const err = "Agent error: You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit"
    const transcript: any[] = []
    let first = true
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => transcript,
        SendWorkspaceMessage: (a: any[]) => {
          if (first) {
            first = false
            transcript.push({ role: 'user', kind: 'text', text: String(a[2] ?? '') })
            transcript.push({ role: 'assistant', kind: 'text', text: err })
            return err
          }
          return new Promise(() => { /* hang — clock must show before any delta */ })
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.chat-in .chat-composer').fill('ship the dark mode polish')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-error-retry')).toBeVisible()
    await page.evaluate(({ workspaceId }) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'session.turn_end',
        data: { workspaceId, code: 'MA', durationMs: 1200 },
      })
    }, { workspaceId: working.id })
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()

    await dock.locator('.chat-error-retry').click()
    await expect(dock.locator('.chat-turn-elapsed')).toBeVisible()
  })
})

test.describe('@ file mentions', () => {
  test('typing @ inserts deletable inline chips in the composer sentence', async ({ page }) => {
    const sent: string[] = []
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => ['PROGRESS.md', 'DECISIONS.md', '.gitignore', 'frontend/src/lib/agentchat.ts'],
        ListSkills: () => [
          { name: 'design-audit', description: 'Audit UI polish', scope: 'workspace', userInvocable: true },
          { name: 'land-it', description: 'Ship the PR', scope: 'workspace', userInvocable: true },
        ],
        ListAllSkills: () => [
          { name: 'design-audit', description: 'Audit UI polish', scope: 'workspace', userInvocable: true },
          { name: 'land-it', description: 'Ship the PR', scope: 'workspace', userInvocable: true },
        ],
        SendWorkspaceMessage: (a: any[]) => {
          sent.push(String(a[2] ?? ''))
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: 'ok' })
          return 'ok'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await expect(composer).toBeVisible()

    // Chips sit inside the sentence — not a strip above a bare textarea.
    await composer.click()
    await composer.pressSequentially('how does @', { delay: 15 })
    await expect(dock.locator('.chat-mention')).toBeVisible()
    // ↑/↓ must move the highlight — not snap back to the first row on keyup.
    const rows = dock.locator('.chat-mention-row')
    await expect(rows.first()).toHaveAttribute('aria-selected', 'true')
    await page.keyboard.press('ArrowDown')
    await expect(rows.nth(1)).toHaveAttribute('aria-selected', 'true')
    await expect(rows.first()).toHaveAttribute('aria-selected', 'false')

    // Typing a name.ext prefix must NOT auto-chip — only Enter/Tab/click commits.
    await composer.pressSequentially('PROG', { delay: 15 })
    await expect(composer.locator('.chat-composer-chip')).toHaveCount(0)
    await expect(dock.locator('.chat-mention-row', { hasText: 'PROGRESS.md' })).toBeVisible()
    await page.keyboard.press('Enter')
    await expect(composer.locator('.chat-composer-chip', { hasText: 'PROGRESS.md' })).toBeVisible()
    await expect(composer).toContainText('how does')

    await composer.pressSequentially('compare to @DEC', { delay: 15 })
    await expect(dock.locator('.chat-mention-row', { hasText: 'DECISIONS.md' })).toBeVisible()
    await page.keyboard.press('Enter')
    await expect(composer.locator('.chat-composer-chip', { hasText: 'DECISIONS.md' })).toBeVisible()
    await expect(composer.locator('.chat-composer-chip')).toHaveCount(2)

    // Chip × removes that inline referent from the sentence.
    await composer.locator('.chat-composer-chip', { hasText: 'PROGRESS.md' }).locator('.chat-composer-chip-x').click()
    await expect(composer.locator('.chat-composer-chip', { hasText: 'PROGRESS.md' })).toHaveCount(0)
    await expect(composer.locator('.chat-composer-chip', { hasText: 'DECISIONS.md' })).toBeVisible()

    // Leading-dot files (.gitignore) must chipify too — not stay as bare @text.
    await composer.click()
    await composer.fill('')
    await composer.pressSequentially('check @.git', { delay: 15 })
    await expect(dock.locator('.chat-mention-row', { hasText: '.gitignore' })).toBeVisible()
    await page.keyboard.press('Enter')
    await expect(composer.locator('.chat-composer-chip', { hasText: '.gitignore' })).toBeVisible()

    // Rebuild a clean two-file compare and send — wire text keeps @path referents.
    await composer.click()
    await composer.fill('')
    await composer.pressSequentially('how does @PROG', { delay: 15 })
    await page.keyboard.press('Enter')
    await composer.pressSequentially('compare to @DEC', { delay: 15 })
    await page.keyboard.press('Enter')

    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toBe('how does @PROGRESS.md compare to @DECISIONS.md')
    await expect(dock.locator('.chat-file-inline', { hasText: 'PROGRESS.md' })).toBeVisible()
    await expect(dock.locator('.chat-file-inline', { hasText: 'DECISIONS.md' })).toBeVisible()
  })

  test('mention menu keeps lower-ranked matches past the old top-12 cap', async ({ page }) => {
    // Short paths outrank deep ones on the fuzzy length bonus. With limit=12 the
    // nested target used to vanish even though it clearly matches the query.
    const shortMatches = Array.from({ length: 15 }, (_, i) =>
      `a${String(i).padStart(2, '0')}/match.md`,
    )
    const nested = 'frontend/src/lib/deeply/nested/path/match-util.ts'
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => [...shortMatches, nested],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.pressSequentially('@match', { delay: 15 })
    await expect(dock.locator('.chat-mention')).toBeVisible()
    await expect(dock.locator('.chat-mention-row')).toHaveCount(16)
    await expect(dock.locator('.chat-mention-row', { hasText: 'match-util.ts' })).toBeVisible()
  })

  test('directory hits appear in @ search with a folder icon', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => ['src/lib/a.ts', 'src/app.ts', 'README.md'],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.pressSequentially('@src', { delay: 15 })
    await expect(dock.locator('.chat-mention')).toBeVisible()
    const dirRow = dock.locator('.chat-mention-row').filter({ has: page.locator('.folder-ico') })
    await expect(dirRow.first()).toBeVisible()
    await dirRow.filter({ has: page.locator('.chat-mention-name', { hasText: /^src$/ }) }).click()
    await expect(composer.locator('.chat-composer-chip .folder-ico')).toBeVisible()
    await expect(composer.locator('.chat-composer-chip-name')).toHaveText('src')
    await expect(composer.locator('.chat-composer-chip')).toHaveAttribute('title', 'src')
    await composer.locator('.chat-composer-chip-name').click()
    await expect(page.locator('.toast.error')).toHaveCount(0)
    await expect(composer.locator('.chat-composer-chip-name')).toHaveText('src')
  })

  test('@hyp lists both hyperlane validator scripts', async ({ page }) => {
    // Repro: stale/partial index showed only decode-hyperlane-message.sh.
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => [
          'validators/decode-hyperlane-message.sh',
          'validators/hyperlane_validator2.sh',
          'hypothesis.txt',
        ],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.pressSequentially('@hyp', { delay: 15 })
    await expect(dock.locator('.chat-mention')).toBeVisible()
    await expect(dock.locator('.chat-mention-row', { hasText: 'decode-hyperlane-message.sh' })).toBeVisible()
    await expect(dock.locator('.chat-mention-row', { hasText: 'hyperlane_validator2.sh' })).toBeVisible()
  })

  test('Escape dismisses the mention menu without reopening on keyup', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => ['PROGRESS.md', 'DECISIONS.md'],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.pressSequentially('@PRO', { delay: 15 })
    await expect(dock.locator('.chat-mention')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(dock.locator('.chat-mention')).toHaveCount(0)
    // keyup must not reopen while the same @query is still under the caret
    await page.waitForTimeout(50)
    await expect(dock.locator('.chat-mention')).toHaveCount(0)
    // Typing further clears the escape suppress and shows the menu again
    await composer.pressSequentially('G', { delay: 15 })
    await expect(dock.locator('.chat-mention')).toBeVisible()
  })

  test('transcript file chip opens the editor without stealing the Files panel', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [
          { role: 'user', kind: 'text', text: 'please look at @src/main.go' },
        ],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const files = page.locator('#v-detail .obs-files')
    await expect(files.locator('.obs-node', { hasText: 'main.go' })).toBeVisible()
    await expect(page.locator('#v-detail .obs-side .obs-side-title')).toHaveText('Files')

    await page.locator('#v-detail .obs-left .chat-file-inline', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-side .obs-side-title')).toHaveText('Files')
    await expect(files.locator('.obs-node.on', { hasText: 'main.go' })).toBeVisible()
  })

  test('transcript file chip keeps Source Control open instead of flipping to Outline', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [
          { role: 'user', kind: 'text', text: 'please look at @src/main.go' },
        ],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    await page.locator('#v-detail .obs-tb-right .obs-panel-icons button[aria-label^="Source Control"]').click()
    await expect(page.locator('#v-detail .obs-side .obs-side-title')).toHaveText('Source Control')

    await page.locator('#v-detail .obs-left .chat-file-inline', { hasText: 'main.go' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('main.go')
    await expect(page.locator('#v-detail .obs-side .obs-side-title')).toHaveText('Source Control')
    await expect(page.locator('#v-detail .obs-side .obs-side-title')).not.toHaveText('Outline')
  })

  test('transcript file chip does not select a file that is filtered out of the explorer', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => [
          { role: 'user', kind: 'text', text: 'please look at @PROGRESS.md' },
        ],
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const files = page.locator('#v-detail .obs-files')
    await expect(files.locator('.obs-node', { hasText: 'main.go' })).toBeVisible()
    await files.locator('.obs-node', { hasText: 'main.go' }).click()
    await expect(files.locator('.obs-node.on', { hasText: 'main.go' })).toBeVisible()

    await page.locator('#v-detail .obs-left .chat-file-inline', { hasText: 'PROGRESS.md' }).click()
    await expect(page.locator('#v-detail .obs-filechip.on')).toContainText('PROGRESS.md')
    await expect(page.locator('#v-detail .obs-side .obs-side-title')).toHaveText('Files')
    // Changed-scope listing still has main.go; PROGRESS.md is not in it, so the
    // explorer highlight must stay put rather than expanding/filtering to match.
    await expect(files.locator('.obs-node.on', { hasText: 'main.go' })).toBeVisible()
    await expect(files.locator('.obs-node', { hasText: 'PROGRESS.md' })).toHaveCount(0)
  })

  test('dragging a file from the explorer inserts an @file chip', async ({ page }) => {
    const sent: string[] = []
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          sent.push(String(a[2] ?? ''))
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: 'ok' })
          return 'ok'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const node = page.locator('#v-detail .obs-files .obs-node[data-tree-path="src/main.go"]')
    await expect(node).toBeVisible()

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await expect(composer).toBeVisible()
    await composer.click()
    await composer.pressSequentially('please review ', { delay: 15 })

    // HTML5 DnD: a real DataTransfer carries FILE_DRAG_MIME so the drop
    // highlight and chip insert still work if the 1s idle watchdog cleared
    // module state between these dispatched events (slow CI).
    const dt = await page.evaluateHandle(() => {
      const d = new DataTransfer()
      d.setData('application/x-recur-workspace-file', JSON.stringify(['src/main.go']))
      return d
    })
    await node.dispatchEvent('dragstart', { dataTransfer: dt })
    const chat = dock.locator('.chat')
    await chat.dispatchEvent('dragenter', { dataTransfer: dt })
    await chat.dispatchEvent('dragover', { dataTransfer: dt })
    await expect(chat).toHaveClass(/drop-file/)
    await chat.dispatchEvent('drop', { dataTransfer: dt })
    await node.dispatchEvent('dragend', { dataTransfer: dt })
    await expect(chat).not.toHaveClass(/drop-file/)

    await expect(composer.locator('.chat-composer-chip', { hasText: 'main.go' })).toBeVisible()
    await expect(composer).toContainText('please review')

    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toBe('please review @src/main.go')
    await expect(dock.locator('.chat-file-inline', { hasText: 'main.go' })).toBeVisible()
  })

  test('dragging a folder from the explorer inserts a folder chip', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const node = page.locator('#v-detail .obs-files .obs-node[data-tree-path="src"]')
    await expect(node).toBeVisible()

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await expect(composer).toBeVisible()
    await composer.click()

    const dt = await page.evaluateHandle(() => {
      const d = new DataTransfer()
      d.setData('application/x-recur-workspace-file', JSON.stringify(['src']))
      return d
    })
    await node.dispatchEvent('dragstart', { dataTransfer: dt })
    const chat = dock.locator('.chat')
    await chat.dispatchEvent('dragenter', { dataTransfer: dt })
    await chat.dispatchEvent('dragover', { dataTransfer: dt })
    await chat.dispatchEvent('drop', { dataTransfer: dt })
    await node.dispatchEvent('dragend', { dataTransfer: dt })

    await expect(composer.locator('.chat-composer-chip .folder-ico')).toBeVisible()
    await expect(composer.locator('.chat-composer-chip-name')).toHaveText('src')
  })

  test('dropping a file URI onto chat inserts an @file chip', async ({ page }) => {
    const sent: string[] = []
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          sent.push(String(a[2] ?? ''))
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: 'ok' })
          return 'ok'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await expect(composer).toBeVisible()
    await composer.click()

    const abs = `/mock/worktrees/${working.id}/src/main.go`
    const dt = await page.evaluateHandle((uri: string) => {
      const d = new DataTransfer()
      d.setData('text/uri-list', uri)
      return d
    }, `file://${abs}`)
    await dock.locator('.chat').dispatchEvent('drop', { dataTransfer: dt })

    await expect(composer.locator('.chat-composer-chip', { hasText: 'main.go' })).toBeVisible()
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toBe('@src/main.go')
  })

  test('dropping an external image attaches it like paste', async ({ page }) => {
    const png = Buffer.from(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
      'base64',
    )
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await page.route('**/chat-image**', async (route) => {
      await route.fulfill({ status: 200, contentType: 'image/png', body: png })
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await expect(composer).toBeVisible()
    await composer.click()

    const dropId = await dock.locator('.chat').getAttribute('data-chat-drop')
    await page.evaluate((id: string) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'files.dropped',
        data: {
          files: ['/Users/me/Desktop/shot.png'],
          attributes: { 'data-chat-drop': id },
        },
      })
    }, dropId || '')

    await expect(composer.locator('.chat-composer-att')).toBeVisible()
    await expect(composer.locator('.chat-composer-chip[data-file-path]')).toHaveCount(0)
  })

  test('clicking an external file chip opens it in the editor', async ({ page }) => {
    const csv = 'a,b\n1,2\n'
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await page.route('**/external-file**', async (route) => {
      await route.fulfill({ status: 200, contentType: 'text/plain; charset=utf-8', body: csv })
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await expect(composer).toBeVisible()
    await composer.click()

    const dt = await page.evaluateHandle(() => {
      const d = new DataTransfer()
      d.setData('text/uri-list', 'file:///Users/josh/Downloads/export-111.csv')
      return d
    })
    await dock.locator('.chat').dispatchEvent('drop', { dataTransfer: dt })
    const chip = composer.locator('.chat-composer-chip', { hasText: 'export-111.csv' })
    await expect(chip).toBeVisible()
    await chip.click()

    await expect(codeContent(page)).toContainText('a,b')
    await expect(page.locator('.toast.error', { hasText: 'Read failed' })).toHaveCount(0)
  })
})

test.describe('@terminal mentions', () => {
  test('typing @ lists terminals and inserts a chip', async ({ page }) => {
    const sent: string[] = []
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => ['README.md'],
        ListTerminals: () => [
          { id: 'term-dev-1', title: 'dev', cwd: '/mock', shell: '/bin/zsh' },
        ],
        SendWorkspaceMessage: (a: any[]) => {
          sent.push(String(a[2] ?? ''))
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: 'ok' })
          return 'ok'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.pressSequentially('errors in @term', { delay: 15 })
    await expect(dock.locator('.chat-mention-row', { hasText: 'dev' })).toBeVisible()
    await page.keyboard.press('Enter')
    await expect(composer.locator('.chat-composer-chip', { hasText: 'dev' })).toBeVisible()
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toContain('@terminal:term-dev-1')
  })
})

test.describe('@artifact mentions', () => {
  test('typing @ lists artifacts and inserts a chip', async ({ page }) => {
    const sent: string[] = []
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ListFiles: () => ['README.md'],
        ListArtifacts: () => [
          {
            id: 'art-notes-1',
            title: 'Notes',
            kind: 'md',
            path: 'workspaces/w/artifacts/art-notes-1.md',
            createdAt: '2026-08-24T12:00:00Z',
          },
        ],
        SendWorkspaceMessage: (a: any[]) => {
          sent.push(String(a[2] ?? ''))
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({ role: 'assistant', kind: 'text', text: 'ok' })
          return 'ok'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const composer = dock.locator('.chat-in .chat-composer')
    await composer.click()
    await composer.pressSequentially('review @Notes', { delay: 15 })
    await expect(dock.locator('.chat-mention-row', { hasText: 'Notes' })).toBeVisible()
    await page.keyboard.press('Enter')
    await expect(composer.locator('.chat-composer-chip', { hasText: 'Notes' })).toBeVisible()
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect.poll(() => sent.length).toBe(1)
    expect(sent[0]).toContain('@artifact:art-notes-1')
  })
})

test.describe('Chat tab persistence', () => {
  test('reopening a workspace after reload restores the last active chat tab', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const loopTab = dock.locator('.wsv-chat-tab', { hasText: 'Loop' })
    const pairTab = dock.locator('.wsv-chat-tab', { hasText: 'New chat' })
    await expect(loopTab).toHaveAttribute('aria-selected', 'true')

    await pairTab.click()
    await expect(pairTab).toHaveAttribute('aria-selected', 'true')
    await expect(loopTab).toHaveAttribute('aria-selected', 'false')
    await waitForSavedChatTab(page, working.id, 'PA')

    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    const dockAgain = page.locator('#v-detail .obs-left')
    await expect(dockAgain.locator('.wsv-chat-tab', { hasText: 'New chat' })).toHaveAttribute('aria-selected', 'true')
    await expect(dockAgain.locator('.wsv-chat-tab', { hasText: 'Loop' })).toHaveAttribute('aria-selected', 'false')
  })

  test('a newly opened chat tab stays selected after reload', async ({ page }) => {
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-new').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)
    await expect(dock.locator('.wsv-chat-tab').nth(2)).toHaveAttribute('aria-selected', 'true')
    await waitForSavedChatTab(page, working.id, 'PA2')

    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    const dockAgain = page.locator('#v-detail .obs-left')
    await expect(dockAgain.locator('.wsv-chat-tab')).toHaveCount(3)
    await expect(dockAgain.locator('.wsv-chat-tab').nth(2)).toHaveAttribute('aria-selected', 'true')
  })

  test('dragging a chat tab reorders the strip and survives reload', async ({ page }) => {
    let saved: string[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        ReorderWorkspaceChats: (a, s) => {
          saved = [...((a[1] ?? []) as string[])]
          ;((s as any).chatOrderByLoop ??= {})[a[0]] = [...saved]
          return null
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await dock.locator('.wsv-chat-new').click()
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(3)
    const loop = dock.locator('.wsv-chat-tab[data-tab-key="MA"]')
    const pair = dock.locator('.wsv-chat-tab[data-tab-key="PA"]')
    await expect(dock.locator('.wsv-chat-tab').first()).toHaveAttribute('data-tab-key', 'MA')

    const from = await pair.boundingBox()
    const to = await loop.boundingBox()
    expect(from && to).toBeTruthy()
    await page.mouse.move(from!.x + from!.width / 2, from!.y + from!.height / 2)
    await page.mouse.down()
    await page.mouse.move(to!.x + 8, to!.y + to!.height / 2, { steps: 12 })
    await page.mouse.up()

    await expect(dock.locator('.wsv-chat-tab').first()).toHaveAttribute('data-tab-key', 'PA')
    await expect(dock.locator('.wsv-chat-tab').nth(1)).toHaveAttribute('data-tab-key', 'MA')
    await expect.poll(() => saved[0]).toBe('PA')

    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    const dockAgain = page.locator('#v-detail .obs-left')
    await expect(dockAgain.locator('.wsv-chat-tab').first()).toHaveAttribute('data-tab-key', 'PA')
    await expect(dockAgain.locator('.wsv-chat-tab').nth(1)).toHaveAttribute('data-tab-key', 'MA')
  })

  test('switching workspaces restores agent-tab strip scroll', async ({ page }) => {
    const active = seed.loops.filter((l: any) => l.group === 'active')
    const working = active[0]!
    const other = active.find((l: any) => l.id !== working.id)!
    await installBridge(page, structuredClone(seed), {
      resolvers: { ListLoopAgents: () => [] },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'Loop' })).toBeVisible()
    for (let i = 0; i < 8; i++) {
      await dock.locator('.wsv-chat-new').click()
    }
    await expect(dock.locator('.wsv-chat-tab')).toHaveCount(10)
    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'Loop' })).toHaveAttribute('aria-selected', 'true')

    const strip = dock.locator('.wsv-chat-tabs .tab-strip-scroll')
    await expect.poll(() => strip.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeGreaterThan(60)
    const tabScroll = await strip.evaluate((el) => {
      el.scrollLeft = Math.min(120, Math.max(40, el.scrollWidth - el.clientWidth))
      return el.scrollLeft
    })
    expect(tabScroll).toBeGreaterThan(0)

    await openWorkspaceByGoal(page, other.goal)
    await expect(page.locator('#v-detail .obs-left .wsv-chat-tab')).not.toHaveCount(10)

    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs-left .wsv-chat-tab')).toHaveCount(10)
    await expect.poll(() => strip.evaluate((el) => el.scrollLeft)).toBe(tabScroll)

    await page.locator('#v-detail .obs-left .wsv-chat-tab').last().click()
    await expect.poll(() => strip.evaluate((el) => {
      const chip = el.querySelector<HTMLElement>('.tab-chip.on')
      if (!chip) return 0
      const sr = el.getBoundingClientRect()
      const cr = chip.getBoundingClientRect()
      return Math.min(cr.right, sr.right) - Math.max(cr.left, sr.left)
    })).toBeGreaterThan(24)
  })
})

test.describe('Chat transcript UI state', () => {
  const toolItems = [
    { role: 'user', kind: 'text', text: 'run the check' },
    {
      role: 'assistant', kind: 'tool', tool: 'Bash',
      toolInput: 'python3 -m py_compile tetris.py',
      description: 'Compile check',
      input: '{"command":"python3 -m py_compile tetris.py && echo OK"}',
      result: 'OK',
    },
  ]

  test('expanded tool calls stay expanded after switching chat tabs', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? toolItems : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const toolRow = dock.locator('.tc', { hasText: 'Compile check' })
    await expect(toolRow).toBeVisible()
    await expect(toolRow.locator('.tc-head')).toHaveAttribute('aria-expanded', 'false')
    await toolRow.locator('.tc-head').click()
    await expect(toolRow.locator('.tc-head')).toHaveAttribute('aria-expanded', 'true')
    await expect(toolRow.locator('.tc-pre')).toContainText('python3 -m py_compile tetris.py')

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'New chat' })).toHaveAttribute('aria-selected', 'true')
    await expect(toolRow).toHaveCount(0)

    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect(toolRow.locator('.tc-head')).toHaveAttribute('aria-expanded', 'true')
    await expect(toolRow.locator('.tc-pre')).toContainText('OK')
  })

  test('expanded tool calls stay expanded after switching workspaces', async ({ page }) => {
    const active = seed.loops.filter((l: any) => l.group === 'active')
    const working = active[0]!
    const other = active.find((l: any) => l.id !== working.id)!
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? toolItems : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const toolRow = dock.locator('.tc', { hasText: 'Compile check' })
    await toolRow.locator('.tc-head').click()
    await expect(toolRow.locator('.tc-head')).toHaveAttribute('aria-expanded', 'true')

    await openWorkspaceByGoal(page, other.goal)
    await expect(page.locator('#v-detail .obs-left .tc', { hasText: 'Compile check' })).toHaveCount(0)

    await openWorkspaceByGoal(page, working.goal)
    const again = page.locator('#v-detail .obs-left .tc', { hasText: 'Compile check' })
    await expect(again.locator('.tc-head')).toHaveAttribute('aria-expanded', 'true')
    await expect(again.locator('.tc-pre')).toBeVisible()
  })

  function tallTranscript() {
    const items: any[] = []
    for (let i = 0; i < 20; i++) {
      items.push({ role: 'user', kind: 'text', text: `question ${i}\n${'line '.repeat(6)}` })
      items.push({ role: 'assistant', kind: 'text', text: `answer ${i}\n${'paragraph '.repeat(18)}` })
    }
    return items
  }

  /** Crosses CHAT_HYDRATE_ALL_BELOW so stubs + measured heights are on the path. */
  function windowedTranscript() {
    const items: any[] = []
    const n = Math.ceil((CHAT_HYDRATE_ALL_BELOW + 4) / 2)
    for (let i = 0; i < n; i++) {
      items.push({ role: 'user', kind: 'text', text: `question ${i}\n${'line '.repeat(6)}` })
      items.push({ role: 'assistant', kind: 'text', text: `answer ${i}\n${'paragraph '.repeat(18)}` })
    }
    return items
  }

  /** Programmatic scrollTop can lose to pinBottom until the scroller records unpinned. */
  async function leaveMidScroll(scroller: Locator, top: number) {
    await expect.poll(() => scroller.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeGreaterThan(top + 80)
    await expect.poll(async () => {
      const applied = await scroller.evaluate((el, t) => {
        el.scrollTop = t
        el.dispatchEvent(new Event('scroll'))
        const dist = el.scrollHeight - el.scrollTop - el.clientHeight
        return el.scrollTop >= t - 40 && dist > 80
      }, top)
      if (!applied) return false
      // pinBottom runs on rAF — confirm the unpinned offset survived a frame.
      await scroller.evaluate(() => new Promise((r) => requestAnimationFrame(r)))
      return scroller.evaluate((el, t) => {
        const dist = el.scrollHeight - el.scrollTop - el.clientHeight
        return el.scrollTop >= t - 40 && dist > 80
      }, top)
    }).toBe(true)
    return scroller.evaluate((el) => el.scrollTop)
  }

  async function expectRestoredMidScroll(scroller: Locator, saved: number) {
    await expect.poll(() => scroller.evaluate((el, s) => {
      const dist = el.scrollHeight - el.scrollTop - el.clientHeight
      return el.scrollTop > s - 80 && el.scrollTop < s + 80 && dist > 80
    }, saved)).toBe(true)
  }

  /** Visible row + its offset in the port — stub min-height can keep scrollTop
   *  while shifting which message sits at that offset. */
  async function captureTranscriptView(scroller: Locator) {
    return scroller.evaluate((el) => {
      const port = el.getBoundingClientRect()
      const rows = [...el.querySelectorAll('.chat-row')] as HTMLElement[]
      const hit = rows.find((r) => {
        const b = r.getBoundingClientRect()
        return b.bottom > port.top + 4 && b.top < port.bottom - 4
      })
      return {
        scrollTop: el.scrollTop,
        key: hit?.textContent?.replace(/\s+/g, ' ').trim().slice(0, 48) ?? '',
        offset: hit ? hit.getBoundingClientRect().top - port.top : 0,
      }
    })
  }

  async function expectRestoredTranscriptView(
    scroller: Locator,
    saved: { scrollTop: number; key: string; offset: number },
  ) {
    await expect.poll(async () => {
      const got = await captureTranscriptView(scroller)
      const dist = await scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight)
      if (dist <= 80) return false
      if (!saved.key) return Math.abs(got.scrollTop - saved.scrollTop) < 80
      return got.key === saved.key && Math.abs(got.offset - saved.offset) < 80
    }).toBe(true)
  }

  test('scroll offset is restored after switching chat tabs', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const items = tallTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    const saved = await leaveMidScroll(scroller, 360)

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'New chat' })).toHaveAttribute('aria-selected', 'true')
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toHaveCount(0)

    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expectRestoredMidScroll(scroller, saved)
  })

  test('scroll offset is restored after switching tabs on a windowed transcript', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const items = windowedTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect.poll(() => scroller.locator('.cblk').count()).toBeGreaterThan(CHAT_HYDRATE_ALL_BELOW)
    await expect(scroller.locator('.cblk.stub')).not.toHaveCount(0)
    await leaveMidScroll(scroller, 360)
    const saved = await captureTranscriptView(scroller)

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'New chat' })).toHaveAttribute('aria-selected', 'true')
    await expect(scroller.locator('.cblk')).toHaveCount(0)

    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect.poll(() => scroller.locator('.cblk').count()).toBeGreaterThan(CHAT_HYDRATE_ALL_BELOW)
    await expectRestoredTranscriptView(scroller, saved)
  })

  test('scroll offset is restored after hiding the agent panel on a windowed transcript', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const items = windowedTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect.poll(() => scroller.locator('.cblk').count()).toBeGreaterThan(CHAT_HYDRATE_ALL_BELOW)
    await expect(scroller.locator('.cblk.stub')).not.toHaveCount(0)
    await leaveMidScroll(scroller, 360)
    const saved = await captureTranscriptView(scroller)

    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Hide left panel"]').click()
    await expect(page.locator('#v-detail .obs-left')).toHaveCount(0)

    await page.locator('#v-detail .obs-tb-left .obs-pane-toggle[aria-label="Show left panel"]').click()
    await expect(page.locator('#v-detail .obs-left')).toBeVisible()
    await expect.poll(() => scroller.locator('.cblk').count()).toBeGreaterThan(CHAT_HYDRATE_ALL_BELOW)
    await expectRestoredTranscriptView(scroller, saved)
  })

  test('a small scroll up from the tail is not pulled back on a windowed transcript', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    // Tall rows so unpinning the tail shifts pickHydrated (overscan past the 600px pad).
    const items: any[] = []
    const n = Math.ceil((CHAT_HYDRATE_ALL_BELOW + 4) / 2)
    for (let i = 0; i < n; i++) {
      items.push({ role: 'user', kind: 'text', text: `question ${i}\n${'line\n'.repeat(8)}` })
      items.push({ role: 'assistant', kind: 'text', text: `answer ${i}\n${'paragraph\n'.repeat(16)}` })
    }
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect.poll(() => scroller.locator('.cblk').count()).toBeGreaterThan(CHAT_HYDRATE_ALL_BELOW)
    await expect(scroller.locator('.cblk.stub')).not.toHaveCount(0)
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)

    const stubsBefore = await scroller.evaluate((el) =>
      [...el.querySelectorAll('.cblk')].map((n, i) => (n.classList.contains('stub') ? i : '')).join(),
    )
    await scroller.evaluate((el) => {
      el.scrollTop = Math.max(0, el.scrollTop - 40)
      el.dispatchEvent(new Event('scroll'))
    })
    // applyViewportPin only runs on a hydration swap, via tick().then(rAF).
    await expect.poll(() => scroller.evaluate((el) =>
      [...el.querySelectorAll('.cblk')].map((n, i) => (n.classList.contains('stub') ? i : '')).join(),
    )).not.toBe(stubsBefore)
    await scroller.evaluate(() => new Promise((r) => requestAnimationFrame(r)))
    expect(
      await scroller.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight),
    ).toBeGreaterThan(20)
  })

  test('scroll offset is restored after switching workspaces', async ({ page }) => {
    const active = seed.loops.filter((l: any) => l.group === 'active')
    const working = active[0]!
    const other = active.find((l: any) => l.id !== working.id)!
    const items = tallTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const scroller = page.locator('#v-detail .obs-left .chat-scroll')
    await expect(page.locator('#v-detail .obs-left .chat-row.me', { hasText: 'question 0' })).toBeVisible()
    const saved = await leaveMidScroll(scroller, 360)

    await openWorkspaceByGoal(page, other.goal)
    await expect(page.locator('#v-detail .obs-left .chat-row.me', { hasText: 'question 0' })).toHaveCount(0)

    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs-left .chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expectRestoredMidScroll(scroller, saved)
  })

  test('parked workspace chat stays live and only its owner reloads on idle', async ({ page }) => {
    const active = seed.loops.filter((l) => l.group === 'active')
    const working = active[0]!
    const other = active.find((l) => l.id !== working.id)!
    const reloads: unknown[][] = []
    let recordReloads = false
    let text = 'initial owned transcript'
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a) => {
          if (recordReloads) reloads.push([a[0], a[1]])
          return [{ role: 'assistant', kind: 'text', text: a[0] === working.id ? text : 'other transcript' }]
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .chat-canvas', { hasText: text })).toBeVisible()
    await openWorkspaceByGoal(page, other.goal)
    await expect(page.locator('#v-detail .chat-canvas', { hasText: 'other transcript' })).toBeVisible()
    await expect(page.locator('#wsv-park .wsv-shell')).toHaveCount(1)

    await page.evaluate((workspaceId) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'directing' },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: 'parked live reply', seq: 1, replace: true, done: true },
      })
    }, working.id)
    await expect(page.locator('#wsv-park .chat-canvas', { hasText: 'parked live reply' })).toHaveCount(1)
    await expect(page.locator('#v-detail .chat-canvas', { hasText: 'parked live reply' })).toHaveCount(0)

    text = 'settled owned transcript'
    recordReloads = true
    await page.evaluate((workspaceId) => {
      ;(window as any)._wails.dispatchWailsEvent({
        name: 'agent.updated',
        data: { workspaceId, agentId: `${workspaceId}-MA`, status: 'idle' },
      })
    }, working.id)
    await expect(page.locator('#wsv-park .chat-canvas', { hasText: text })).toHaveCount(1)
    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .chat-canvas', { hasText: text })).toBeVisible()
    expect(reloads).toEqual([[working.id, 'MA']])
  })

  test('a bottom-pinned in-flight chat stays on the tail after growth while parked', async ({ page }) => {
    const active = seed.loops.filter((l: any) => l.group === 'active')
    const working = active[0]!
    const other = active.find((l: any) => l.id !== working.id)!
    const items = tallTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          if (code !== 'MA') return []
          if (a[0] === working.id) return items
          return a[0] === other.id ? [{ role: 'assistant', kind: 'text', text: 'other earlier' }] : []
        },
        SendWorkspaceMessage: () => new Promise(() => { /* hang — in-flight while parked */ }),
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)
    await dock.locator('.chat-in .chat-composer').fill('keep going')
    await dock.locator('.chat-in .send:not(.stop)').click()
    await expect(dock.locator('.chat-in .send.stop')).toBeVisible()
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)

    await openWorkspaceByGoal(page, other.goal)
    await expect(page.locator('#v-detail .obs-left .chat-row.me', { hasText: 'question 0' })).toHaveCount(0)
    // Stream only after the other view's transcript load, or it replaces the live row.
    await expect(page.locator('#v-detail .obs-left .chat-canvas', { hasText: 'other earlier' })).toBeVisible()

    const grown = `grown-tail ${'paragraph '.repeat(40)}`
    const visibleReply = `visible reply ${'paragraph '.repeat(40)}`
    await page.evaluate(({ workspaceId, otherId, grown, visibleReply }) => {
      const w = window as any
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId, code: 'MA', text: grown, seq: 1, replace: true },
      })
      w._wails.dispatchWailsEvent({
        name: 'session.delta',
        data: { workspaceId: otherId, code: 'MA', text: visibleReply, seq: 1, replace: true },
      })
    }, { workspaceId: working.id, otherId: other.id, grown, visibleReply })
    // Let the visible stream finish its reveal. The parked stream's data is
    // current, but it must not spend the same frames parsing and painting.
    await expect(page.locator('#v-detail .chat-canvas', { hasText: 'visible reply' })).toHaveText(visibleReply.trim())
    await expect(page.locator('#wsv-park .chat-canvas', { hasText: 'grown-tail' })).toHaveCount(0)

    await openWorkspaceByGoal(page, working.goal)
    await expect(page.locator('#v-detail .obs-left .chat-canvas', { hasText: 'grown-tail' })).toBeVisible()
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)
  })

  test('a bottom-pinned transcript stays pinned after switching chat tabs', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const items = tallTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expect.poll(() => scroller.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeGreaterThan(400)
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'New chat' })).toHaveAttribute('aria-selected', 'true')
    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'Loop' })).toHaveAttribute('aria-selected', 'true')
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)
  })

  test('a mid-scrolled shorter tab is not pinned when entered from a longer tab', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const loopItems = tallTranscript()
    const pairItems = tallTranscript().slice(0, 24)
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          if (a[0] !== working.id) return []
          if (code === 'MA') return loopItems
          if (code === 'PA') return pairItems
          return []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    const saved = await leaveMidScroll(scroller, 320)

    await dock.locator('.wsv-chat-tab', { hasText: 'Loop' }).click()
    await expect.poll(
      () => scroller.evaluate((el, slack) => el.scrollHeight - el.scrollTop - el.clientHeight < slack, NEAR_BOTTOM_PX),
    ).toBe(true)

    await dock.locator('.wsv-chat-tab', { hasText: 'New chat' }).click()
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expectRestoredMidScroll(scroller, saved)
  })

  test('transcript fades an edge only while more content sits past it', async ({ page }) => {
    const working = seed.loops.find((l: any) => l.group === 'active')!
    const items = tallTranscript()
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => {
          const code = a[1] || 'MA'
          return a[0] === working.id && code === 'MA' ? items : []
        },
      },
    })
    await gotoApp(page)
    await openWorkspaceByGoal(page, working.goal)

    const dock = page.locator('#v-detail .obs-left')
    const scroller = dock.locator('.chat-scroll')
    await expect(dock.locator('.chat-row.me', { hasText: 'question 0' })).toBeVisible()
    await expect.poll(() => scroller.evaluate((el) => el.scrollHeight - el.clientHeight)).toBeGreaterThan(400)
    await expect.poll(
      () => scroller.evaluate((el, slack) => {
        const dist = el.scrollHeight - el.scrollTop - el.clientHeight
        return dist < slack && el.classList.contains('fade-top') && !el.classList.contains('fade-bot')
      }, NEAR_BOTTOM_PX),
    ).toBe(true)

    await expect.poll(async () => {
      await scroller.evaluate((el) => {
        el.scrollTop = 0
        el.dispatchEvent(new Event('scroll'))
      })
      await scroller.evaluate(() => new Promise((r) => requestAnimationFrame(r)))
      return scroller.evaluate((el) => el.scrollTop <= 1 && el.scrollHeight - el.scrollTop - el.clientHeight > 80)
    }).toBe(true)
    await expect(scroller).not.toHaveClass(/fade-top/)
    await expect(scroller).toHaveClass(/fade-bot/)

    await leaveMidScroll(scroller, 360)
    await expect(scroller).toHaveClass(/fade-top/)
    await expect(scroller).toHaveClass(/fade-bot/)
  })
})

test.describe('Turn-end findings digest', () => {
  const findingInput = JSON.stringify({
    file: 'store.go',
    line: 1276,
    end_line: 1286,
    severity: 'high',
    message: 'activity hook can miss a turn',
    summary: 'Hook misses turn',
  })
  const findingResult = JSON.stringify({
    id: 'f-hook',
    file: 'store.go',
    line: 1276,
    endLine: 1286,
    severity: 'high',
    message: 'activity hook can miss a turn',
    summary: 'Hook misses turn',
    status: 'open',
  })

  test('renders the findings summary widget after a Codex-style report_finding turn', async ({ page }) => {
    const transcript = [
      { role: 'user', kind: 'text', text: 'review this' },
      {
        role: 'assistant',
        kind: 'tool',
        tool: 'mcp__recur__report_finding',
        input: findingInput,
        result: findingResult,
      },
      { role: 'assistant', kind: 'text', text: 'Filed 1 finding.' },
      { role: 'assistant', kind: 'turn', durationMs: 8400 },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const dock = page.locator('#v-detail .obs-left')
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await expect(dock.locator('.turn-end-link', { hasText: '1 finding' })).toBeVisible()
    const widget = dock.locator('.turn-end-wrap [aria-label="Findings filed"]')
    await expect(widget).toBeVisible()
    await expect(widget.locator('.tc-finding-title')).toContainText('activity hook can miss a turn')
  })

  test('does not render the findings widget when the turn footer is missing', async ({ page }) => {
    // Codex used to omit DurationMS, so flattenTranscript emitted no turn item
    // and the digest never mounted — individual "Reported finding" rows still showed.
    const transcript = [
      { role: 'user', kind: 'text', text: 'review this' },
      {
        role: 'assistant',
        kind: 'tool',
        tool: 'mcp__recur__report_finding',
        input: findingInput,
        result: findingResult,
      },
      { role: 'assistant', kind: 'text', text: 'Filed 1 finding.' },
    ]
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const dock = page.locator('#v-detail .obs-left')
    await expect(dock.locator('.chat-canvas', { hasText: 'Filed 1 finding.' })).toBeVisible()
    await expect(dock.locator('.turn-end-wrap [aria-label="Findings filed"]')).toHaveCount(0)
  })
})

test.describe('Chat fork', () => {
  test('fork control on a finished turn opens a new chat tab', async ({ page }) => {
    const transcript = [
      { role: 'user', kind: 'text', text: 'ship it' },
      { role: 'assistant', kind: 'text', text: 'done' },
      { role: 'assistant', kind: 'turn', durationMs: 1200, messageId: 'msg-turn-1' },
    ]
    let forkedAt = ''
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => (a[1] === 'PA2' ? [] : transcript),
        ForkWorkspaceChat: (a: any[], s: any) => {
          forkedAt = a[2]
          const by = ((s as any).chatsByLoop ??= {})
          const tab = { code: 'PA2', title: 'ship it', updatedAt: Date.now() }
          by[a[0]] = [...(by[a[0]] ?? []).filter((t: any) => t.code !== 'PA2'), tab]
          return tab
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const dock = page.locator('#v-detail .obs-left')
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await dock.locator('.turn-end-fork').click()
    await dock.locator('.turn-end-fork-item', { hasText: 'Fork chat' }).click()
    expect(forkedAt).toBe('msg-turn-1')
    await expect(dock.locator('.wsv-chat-tab', { hasText: 'ship it' })).toBeVisible()
  })

  test('fork to new workspace opens the created workspace', async ({ page }) => {
    const transcript = [
      { role: 'user', kind: 'text', text: 'ship it' },
      { role: 'assistant', kind: 'text', text: 'done' },
      { role: 'assistant', kind: 'turn', durationMs: 1200, messageId: 'msg-turn-1' },
    ]
    let forked: string[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        WorkspaceChatItems: (a: any[]) => (a[0] === 'WS-FORK' ? [] : transcript),
        ForkWorkspaceChatToWorkspace: (a: any[], s: any) => {
          forked = a
          const src = (s as any).loopById?.[a[0]]
          const ws = {
            id: 'WS-FORK',
            projectId: src?.projectId,
            type: 'solo',
            goal: 'ship it',
            short: 'ship it',
            group: 'active',
            statusLabel: 'Idle',
            phase: 'editing',
            branch: 'recur/fork',
            baseBranch: 'main',
          }
          ;(s as any).loopById[ws.id] = ws
          s.loops = [...s.loops, ws]
          return ws
        },
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const dock = page.locator('#v-detail .obs-left')
    await expect(dock.locator('.turn-end', { hasText: 'completed in' })).toBeVisible()
    await dock.locator('.turn-end-fork').click()
    await dock.locator('.turn-end-fork-item', { hasText: 'Fork to new workspace' }).click()
    expect(forked[2]).toBe('msg-turn-1')
    await expect(page.locator('.rail .tloop[data-workspace-id="WS-FORK"]')).toBeVisible()
  })
})

test.describe('External markdown links', () => {
  test('http links in agent output open via window.open (OS browser in desktop)', async ({ page }) => {
    const transcript: any[] = []
    await installBridge(page, structuredClone(seed), {
      resolvers: {
        ListLoopAgents: () => [],
        SendWorkspaceMessage: (a: any[]) => {
          transcript.push({ role: 'user', kind: 'text', text: a[2] })
          transcript.push({
            role: 'assistant',
            kind: 'text',
            text: 'See [the docs](https://example.com/guide) for details.',
          })
          return 'See [the docs](https://example.com/guide) for details.'
        },
        WorkspaceChatItems: () => transcript,
      },
    })
    await gotoApp(page)
    const working = seed.loops.find((l: any) => l.group === 'active')!
    await openWorkspaceByGoal(page, working.goal)
    const textarea = page.locator('#v-detail .obs-left .chat-in .chat-composer')
    await textarea.fill('link please')
    await page.locator('#v-detail .obs-left .chat-in .send:not(.stop)').click()

    const link = page.locator('#v-detail .obs-left .chat-canvas a', { hasText: 'the docs' })
    await expect(link).toHaveAttribute('href', 'https://example.com/guide')

    await page.evaluate(() => {
      ;(window as any).__opened = []
      window.open = (url?: string | URL, target?: string, feat?: string) => {
        ;(window as any).__opened.push({ url: String(url), target, feat })
        return null
      }
    })
    await link.click()
    await expect.poll(() => page.evaluate(() => (window as any).__opened)).toEqual([
      { url: 'https://example.com/guide', target: '_blank', feat: 'noopener,noreferrer' },
    ])
  })
})
