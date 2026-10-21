import type { Page } from '@playwright/test'
import { test, expect, seed, gotoApp } from './helpers/fixtures'
import { BridgeError, installBridge } from './helpers/mock-bridge'

const signedOut = { authenticated: false, accountId: '', email: '', accounts: [], modelCount: 0, error: '' }
const browserURL = 'https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&scope=openid+profile+email+offline_access'
const signedIn = {
  authenticated: true, accountId: 'recur-registration', email: 'direct@example.com',
  accounts: [{ id: 'recur-registration', email: 'direct@example.com' }], modelCount: 1, error: '',
}

function directSeed() {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, disabledProviders: ['claude-code', 'cursor', 'codex'], chatGPTAccountId: '', chatGPTModels: [] }
  return s
}

async function providers(page: Page) {
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await page.locator('#v-settings .set-nav-item', { hasText: 'Providers' }).click()
  return page.locator('.set-subsec-nested').filter({ has: page.getByRole('heading', { name: 'ChatGPT account', exact: true }) })
}

test('direct ChatGPT sign-in works with every CLI provider disabled and persists after restart', async ({ page }) => {
  const s = directSeed()
  let finish!: () => void
  const completed = new Promise<void>((resolve) => { finish = resolve })
  let openedURL = ''
  let codexCalls = 0
  let refreshes = 0
  await page.addInitScript(() => {
    Object.defineProperty(window, 'webkit', { value: { messageHandlers: { external: { postMessage() {} } } } })
    window.open = () => { throw new Error('Direct ChatGPT must open the native browser') }
  })
  await installBridge(page, s, { resolvers: {
    ListProjects: () => [], ListWorkspaces: () => [],
    GetCodexStatus: () => { codexCalls++; return null },
    StartCodexLogin: () => { codexCalls++; return null },
    GetChatGPTStatus: () => s.settings.chatGPTAccountId ? signedIn : signedOut,
    StartChatGPTLogin: (args) => {
      expect(args).toEqual(['', 'browser'])
      return { loginId: 'direct-login', authUrl: browserURL, method: 'browser' }
    },
    WaitChatGPTLogin: async () => {
      await completed
      s.settings = { ...s.settings, chatGPTAccountId: signedIn.accountId, chatGPTModels: [{ id: 'gpt-direct', label: 'Direct GPT' }] }
      return signedIn
    },
    RefreshChatGPTModels: () => { refreshes++; return signedIn },
    DisconnectChatGPT: () => {
      s.settings = { ...s.settings, chatGPTAccountId: '', chatGPTModels: [] }
      return { ...signedOut, accounts: signedIn.accounts }
    },
    ListModelCatalog: () => s.settings.chatGPTAccountId ? [{ provider: 'chatgpt', label: 'ChatGPT', models: [{ ref: 'chatgpt/gpt-direct', label: 'Direct GPT' }] }] : [],
  } })
  await page.route('**/wails/runtime', async (route) => {
    const body = route.request().postDataJSON()
    if (body.object !== 9 || body.method !== 0) return route.fallback()
    openedURL = body.args.url
    return route.fulfill({ status: 200, contentType: 'application/json', body: 'null' })
  })
  await gotoApp(page)
  const card = await providers(page)
  await expect(page.locator('#v-settings .set-subsecs > .set-subsec').filter({ has: page.getByRole('heading', { name: 'OpenAI', exact: true }) })).toHaveCount(1)
  const providerTitles = await page.locator('#v-settings .set-subsecs > .set-subsec > .set-subhead .set-subhead-title').allTextContents()
  expect(providerTitles).not.toContain('ChatGPT')
  expect(providerTitles).not.toContain('Codex')
  await expect(card).toContainText('No Codex installation or API key is needed')
  await card.getByRole('button', { name: 'Continue with ChatGPT', exact: true }).click()
  await expect(card).toContainText('Complete sign-in')
  await expect.poll(() => openedURL).toBe(browserURL)
  await expect(card.getByRole('textbox', { name: 'ChatGPT sign-in link' })).toHaveValue(browserURL)
  finish()
  await expect(card).toContainText('direct@example.com')
  await expect(card).toContainText('1 model available')
  await card.getByRole('button', { name: 'Refresh ChatGPT models' }).click()
  await expect.poll(() => refreshes).toBe(1)
  await page.reload({ waitUntil: 'domcontentloaded' })
  await expect(page.locator('#v-welcome .wprov li[data-provider="openai"]')).toHaveClass(/on/)
  await expect(page.locator('#v-welcome .ws[data-step="2"]')).toHaveClass(/current/)
  await providers(page)
  await card.getByRole('button', { name: 'Sign out of ChatGPT' }).click()
  await expect(card.getByRole('button', { name: 'Sign out of ChatGPT' })).toHaveCount(0)
  expect(s.settings.chatGPTAccountId).toBe('')
  expect(codexCalls).toBe(0)
})

test('OpenAI contains ChatGPT, Codex, and API setup under one provider toggle', async ({ page }) => {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, disabledProviders: [], compatibleProviders: [
    { id: 'openai', name: 'OpenAI', baseUrl: 'https://api.openai.com', models: [{ id: 'gpt-api', label: 'API GPT' }] },
  ] }
  await installBridge(page, s)
  await gotoApp(page)
  await providers(page)
  const openAI = page.locator('#v-settings .set-subsecs > .set-subsec').filter({ has: page.getByRole('heading', { name: 'OpenAI', exact: true }) })
  await expect(openAI).toHaveCount(1)
  await expect(openAI.getByRole('heading', { name: 'ChatGPT account', exact: true })).toBeVisible()
  await expect(openAI.getByRole('heading', { name: 'Codex runtime', exact: true })).toBeVisible()
  await expect(openAI.getByRole('heading', { name: 'API endpoint', exact: true })).toBeVisible()
  await openAI.getByRole('button', { name: 'Enable OpenAI', exact: true }).click()
  await expect.poll(() => s.settings.disabledProviders).toEqual(expect.arrayContaining(['chatgpt', 'codex', 'openai']))
  await openAI.getByRole('button', { name: 'Enable OpenAI', exact: true }).click()
  await expect.poll(() => s.settings.disabledProviders).toEqual([])
})

test('OpenAI cannot be turned off when it is the only ready provider', async ({ page }) => {
  const s = directSeed()
  s.settings = { ...s.settings, chatGPTAccountId: signedIn.accountId, chatGPTModels: [{ id: 'gpt-direct', label: 'Direct GPT' }] }
  await installBridge(page, s, { resolvers: { GetChatGPTStatus: () => signedIn } })
  await gotoApp(page)
  const card = await providers(page)
  await expect(card).toContainText('direct@example.com')
  const toggle = page.getByRole('button', { name: 'Enable OpenAI', exact: true })
  await toggle.click()
  await expect(toggle).toHaveAttribute('aria-pressed', 'true')
  expect(s.settings.disabledProviders).toEqual(['claude-code', 'cursor', 'codex'])
  await expect(page.getByText('Keep at least one provider on', { exact: true })).toBeVisible()
})

for (const method of ['browser', 'headless']) for (const leave of [false, true]) {
  test(`ChatGPT ${method} login cancels on ${leave ? 'leaving Providers' : 'Cancel'}`, async ({ page }) => {
    const s = directSeed()
    let finish!: () => void
    const completed = new Promise<void>((resolve) => { finish = resolve })
    let cancelled = ''
    await page.addInitScript(() => { window.open = () => null })
    await installBridge(page, s, { resolvers: {
      GetChatGPTStatus: () => signedOut,
      StartChatGPTLogin: (args) => {
        expect(args).toEqual(['', method])
        return { loginId: 'cancel-direct', authUrl: method === 'browser' ? browserURL : 'https://auth.openai.com/codex/device', method, userCode: method === 'headless' ? 'ABCD-1234' : '' }
      },
      WaitChatGPTLogin: async () => { await completed; return new BridgeError('cancelled') },
      CancelChatGPTLogin: (args) => { cancelled = args[0]; finish(); return null },
    } })
    await gotoApp(page)
    const card = await providers(page)
    if (method === 'headless') {
      await card.getByRole('combobox', { name: 'ChatGPT sign-in method', exact: true }).click()
      await page.locator('body > .sel-menu .sel-opt', { hasText: 'Headless (device code)' }).click()
    }
    await card.getByRole('button', { name: 'Continue with ChatGPT', exact: true }).click()
    await expect(card.getByRole('button', { name: 'Cancel ChatGPT sign-in' })).toBeVisible()
    if (leave) await page.locator('#v-settings .set-nav-item', { hasText: 'Appearance' }).click()
    else await card.getByRole('button', { name: 'Cancel ChatGPT sign-in' }).click()
    await expect.poll(() => cancelled).toBe('cancel-direct')
    if (!leave) await expect(card.getByRole('button', { name: 'Continue with ChatGPT', exact: true })).toBeEnabled()
    expect(s.settings.chatGPTAccountId).toBe('')
  })
}

test('ChatGPT login reports an authorization failure and allows retry', async ({ page }) => {
  const s = directSeed()
  await page.addInitScript(() => { window.open = () => null })
  await installBridge(page, s, { resolvers: {
    GetChatGPTStatus: () => signedOut,
    StartChatGPTLogin: () => ({ loginId: 'denied-direct', authUrl: browserURL, method: 'browser' }),
    WaitChatGPTLogin: () => new BridgeError('ChatGPT sign-in was declined or failed'),
    CancelChatGPTLogin: () => null,
  } })
  await gotoApp(page)
  const card = await providers(page)
  await card.getByRole('button', { name: 'Continue with ChatGPT', exact: true }).click()
  await expect(card.getByRole('alert')).toContainText('sign-in was declined or failed')
  await expect(card.getByRole('button', { name: 'Continue with ChatGPT', exact: true })).toBeEnabled()
})

test('headless ChatGPT sign-in shows the device URL and code without automatically opening a browser', async ({ page }) => {
  const s = directSeed()
  let finish!: () => void
  const completed = new Promise<void>((resolve) => { finish = resolve })
  await page.addInitScript(() => {
    ;(window as any).opened = 0
    window.open = () => { (window as any).opened++; return null }
  })
  await installBridge(page, s, { resolvers: {
    GetChatGPTStatus: () => signedOut,
    StartChatGPTLogin: (args) => {
      expect(args).toEqual(['', 'headless'])
      return { loginId: 'device-login', method: 'headless', authUrl: 'https://auth.openai.com/codex/device', userCode: 'ABCD-1234' }
    },
    WaitChatGPTLogin: async () => {
      await completed
      s.settings = { ...s.settings, chatGPTAccountId: signedIn.accountId, chatGPTModels: [{ id: 'gpt-direct', label: 'Direct GPT' }] }
      return signedIn
    },
  } })
  await gotoApp(page)
  const card = await providers(page)
  await card.getByRole('combobox', { name: 'ChatGPT sign-in method', exact: true }).click()
  await page.locator('body > .sel-menu .sel-opt', { hasText: 'Headless (device code)' }).click()
  await card.getByRole('button', { name: 'Continue with ChatGPT', exact: true }).click()
  await expect(card.getByRole('textbox', { name: 'ChatGPT sign-in link' })).toHaveValue('https://auth.openai.com/codex/device')
  await expect(card.getByLabel('ChatGPT device code')).toHaveText('ABCD-1234')
  expect(await page.evaluate(() => (window as any).opened)).toBe(0)
  await card.getByRole('button', { name: 'Open sign-in page' }).click()
  await expect.poll(() => page.evaluate(() => (window as any).opened)).toBe(1)
  finish()
  await expect(card).toContainText('direct@example.com')
  await expect(card.getByLabel('ChatGPT device code')).toHaveCount(0)
})

test('browser launch failure keeps the ChatGPT sign-in link available for manual opening', async ({ page }) => {
  const s = directSeed()
  let finish!: () => void
  const completed = new Promise<void>((resolve) => { finish = resolve })
  let cancelled = false
  await page.addInitScript(() => {
    Object.defineProperty(window, 'webkit', { value: { messageHandlers: { external: { postMessage() {} } } } })
  })
  await installBridge(page, s, { resolvers: {
    GetChatGPTStatus: () => signedOut,
    StartChatGPTLogin: () => ({ loginId: 'manual-login', authUrl: browserURL, method: 'browser' }),
    WaitChatGPTLogin: async () => { await completed; return new BridgeError('cancelled') },
    CancelChatGPTLogin: () => { cancelled = true; finish(); return null },
  } })
  await page.route('**/wails/runtime', async (route) => {
    const body = route.request().postDataJSON()
    if (body.object !== 9 || body.method !== 0) return route.fallback()
    return route.fulfill({ status: 500, contentType: 'text/plain', body: 'No default browser available' })
  })
  await gotoApp(page)
  const card = await providers(page)
  await card.getByRole('button', { name: 'Continue with ChatGPT', exact: true }).click()
  await expect(card.getByRole('alert')).toContainText('Open the sign-in link manually')
  await expect(card.getByRole('textbox', { name: 'ChatGPT sign-in link' })).toHaveValue(browserURL)
  expect(cancelled).toBe(false)
  await card.getByRole('button', { name: 'Cancel ChatGPT sign-in' }).click()
  await expect.poll(() => cancelled).toBe(true)
})
