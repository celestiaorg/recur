// Recur Bundles: palette typescript Sort Lines + mocked willSave gofmt transform.
import { test, expect, gotoApp, seed, openWorkspaceByGoal } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

async function openLoopCode(page: import('@playwright/test').Page) {
  await gotoApp(page)
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await expect(page.locator('#v-detail .obs')).toBeVisible()
  return working
}

async function openFile(page: import('@playwright/test').Page, name: string) {
  await page.locator('#v-detail .obs-files .obs-node', { hasText: name }).click()
  const code = page.locator('#v-detail .obs-view-seg button', { hasText: 'Code' })
  if (await code.count()) await code.click()
  await expect(page.locator('#v-detail .cm-editor')).toBeVisible()
}

test('⌘⇧P run-command palette runs Sort Lines Ascending on the open buffer', async ({ page }) => {
  let written = ''
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: () => ({
        path: 'src/main.go',
        language: 'go',
        content: 'package main\n\nzed\nalpha\nbeta\n',
        size: 40,
        writable: true,
      }),
      WriteFile: (a) => {
        written = a[3] as string
        return null
      },
    },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await expect(page.locator('#v-detail .cm-content')).toContainText('zed')

  await page.keyboard.press('ControlOrMeta+Shift+P')
  const palInput = page.getByRole('textbox', { name: /Search commands/i })
  await expect(palInput).toBeVisible()
  await palInput.fill('Sort Lines Ascending')
  const row = page.locator('.palette-bg.open .cmd-palette .pal-row', { hasText: 'Sort Lines Ascending' })
  await expect(row).toBeVisible()
  await palInput.press('Enter')

  // Sorted: alpha before beta before zed (blank / "package main" sort around them).
  await expect.poll(async () => {
    const text = await page.locator('#v-detail .cm-content').innerText()
    const lines = text.split('\n').filter((l) => l === 'alpha' || l === 'beta' || l === 'zed')
    return lines.join(',')
  }).toBe('alpha,beta,zed')

  // Replacement stays selected and editor is refocused — Save should work without a click.
  await page.keyboard.press('ControlOrMeta+s')
  await expect.poll(() => written.includes('alpha')).toBeTruthy()
})

test('willSave runs mocked gofmt transform before WriteFile', async ({ page }) => {
  const writes: string[] = []
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ReadFile: () => ({
        path: 'src/main.go',
        language: 'go',
        content: 'package main\n\n\nfunc main() {}   \n',
        size: 40,
        writable: true,
      }),
      WriteFile: (a) => {
        writes.push(a[3] as string)
        return null
      },
    },
  })
  await openLoopCode(page)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  // Touch the buffer so Save is enabled, without needing a real keystroke race.
  await page.keyboard.type(' ')
  await page.keyboard.press('Backspace')
  await page.keyboard.press('ControlOrMeta+s')
  await expect.poll(() => writes.length).toBeGreaterThan(0)
  // Mock RunCommand collapses 3+ newlines and strips trailing spaces.
  expect(writes[0]).not.toMatch(/[ \t]+\n/)
  expect(writes[0]).not.toMatch(/\n{3,}/)
})

test('Settings → Bundles can disable a willSave command', async ({ page }) => {
  const gofmtID = 'recur.go.gofmt-on-save'
  const writes: string[] = []
  const fixture = structuredClone(seed)
  await installBridge(page, fixture, {
    resolvers: {
      ReadFile: () => ({
        path: 'src/main.go',
        language: 'go',
        content: 'package main\n\n\nfunc main() {}   \n',
        size: 40,
        writable: true,
      }),
      WriteFile: (a) => {
        writes.push(a[3] as string)
        return null
      },
    },
  })
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await expect(page.locator('#v-settings')).toHaveClass(/on/)
  await page.locator('#v-settings .set-nav-item', { hasText: 'Commands' }).click()
  await expect(page.locator('#v-settings .set-head h2')).toHaveText('Commands')
  await expect(page.locator('#v-settings').getByRole('heading', { name: 'Go', exact: true })).toBeVisible()
  const tog = page.locator('#v-settings button.tog[aria-label="Disable Gofmt on Save"]')
  await expect(tog).toHaveAttribute('aria-pressed', 'true')
  await tog.click()
  await expect(page.locator('#v-settings button.tog[aria-label="Enable Gofmt on Save"]')).toBeVisible()
  expect(fixture.settings.disabledBundleCommands).toContain(gofmtID)

  // Back to a workspace — save must not run the disabled willSave transform.
  const working = seed.loops.find((l) => l.group === 'active')!
  await openWorkspaceByGoal(page, working.goal)
  await openFile(page, 'main.go')
  await page.locator('#v-detail .cm-content').click()
  await page.keyboard.type(' ')
  await page.keyboard.press('Backspace')
  await page.keyboard.press('ControlOrMeta+s')
  await expect.poll(() => writes.length).toBeGreaterThan(0)
  // Unformatted buffer still has trailing spaces / stacked blanks (gofmt mock would strip them).
  expect(writes[0]).toMatch(/[ \t]+\n/)
  expect(writes[0]).toMatch(/\n{3,}/)
})

test('Settings → Bundles is a nav group with a page per contribution kind', async ({ page }) => {
  await installBridge(page, structuredClone(seed))
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await expect(page.locator('#v-settings')).toHaveClass(/on/)
  await expect(page.locator('#v-settings .set-group', { hasText: 'Bundles' })).toBeVisible()
  await expect(page.locator('#v-settings .set-nav-item', { hasText: 'Commands' })).toBeVisible()
  await expect(page.locator('#v-settings .set-nav-item', { hasText: 'Prompt actions' })).toBeVisible()
  await expect(page.locator('#v-settings .set-nav-item', { hasText: 'Grammars' })).toBeVisible()
  await expect(page.locator('#v-settings .set-nav-item', { hasText: 'Themes' })).toBeVisible()
  await expect(page.locator('#v-settings .set-nav-item', { hasText: 'Snippets' })).toBeVisible()

  await page.locator('#v-settings .set-nav-item', { hasText: 'Prompt actions' }).click()
  await expect(page.locator('#v-settings .set-head h2')).toHaveText('Prompt actions')
  await expect(page.locator('#v-settings button.tog[aria-label="Disable /commit"]')).toBeVisible()

  await page.locator('#v-settings .set-nav-item', { hasText: 'Grammars' }).click()
  await expect(page.locator('#v-settings .set-head h2')).toHaveText('Grammars')
  await expect(page.locator('#v-settings button.tog[aria-label="Disable go"]')).toBeVisible()

  await page.locator('#v-settings .set-nav-item', { hasText: 'Themes' }).click()
  await expect(page.locator('#v-settings .set-head h2')).toHaveText('Themes')
  await expect(page.locator('#v-settings button.tog[aria-label="Disable Charcoal"]')).toBeVisible()

  await page.locator('#v-settings .set-nav-item', { hasText: 'Snippets' }).click()
  await expect(page.locator('#v-settings .set-head h2')).toHaveText('Snippets')
  await expect(page.locator('#v-settings button.tog[aria-label="Disable If Err"]')).toBeVisible()
})

test('Settings → Bundles can disable a prompt action', async ({ page }) => {
  const fixture = structuredClone(seed)
  await installBridge(page, fixture)
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await expect(page.locator('#v-settings')).toHaveClass(/on/)
  await page.locator('#v-settings .set-nav-item', { hasText: 'Prompt actions' }).click()
  const tog = page.locator('#v-settings button.tog[aria-label="Disable /commit"]')
  await expect(tog).toHaveAttribute('aria-pressed', 'true')
  await tog.click()
  await expect(page.locator('#v-settings button.tog[aria-label="Enable /commit"]')).toBeVisible()
  expect(fixture.settings.disabledBundlePrompts).toContain('recur.chat.commit')
})

test('Settings → Bundles can disable a grammar and a theme', async ({ page }) => {
  const fixture = structuredClone(seed)
  await installBridge(page, fixture)
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await expect(page.locator('#v-settings')).toHaveClass(/on/)
  await page.locator('#v-settings .set-nav-item', { hasText: 'Grammars' }).click()
  await page.locator('#v-settings button.tog[aria-label="Disable go"]').click()
  await expect(page.locator('#v-settings button.tog[aria-label="Enable go"]')).toBeVisible()
  expect(fixture.settings.disabledBundleGrammars).toContain('recur.languages.go')
  await page.locator('#v-settings .set-nav-item', { hasText: 'Themes' }).click()
  await page.locator('#v-settings button.tog[aria-label="Disable Cool Glow"]').click()
  await expect(page.locator('#v-settings button.tog[aria-label="Enable Cool Glow"]')).toBeVisible()
  expect(fixture.settings.disabledBundleThemes).toContain('cool-glow')
})

test('Settings → Bundles can disable a snippet', async ({ page }) => {
  const fixture = structuredClone(seed)
  await installBridge(page, fixture)
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await expect(page.locator('#v-settings')).toHaveClass(/on/)
  await page.locator('#v-settings .set-nav-item', { hasText: 'Snippets' }).click()
  await page.locator('#v-settings button.tog[aria-label="Disable If Err"]').click()
  await expect(page.locator('#v-settings button.tog[aria-label="Enable If Err"]')).toBeVisible()
  expect(fixture.settings.disabledBundleSnippets).toContain('recur.go.ife')
})
