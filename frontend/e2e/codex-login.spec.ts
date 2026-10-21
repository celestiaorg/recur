import type { Page } from '@playwright/test'
import { test, expect, seed, gotoApp } from './helpers/fixtures'
import { BridgeError, installBridge } from './helpers/mock-bridge'

const signedOut = {
  binInstalled: true, binPath: '/usr/local/bin/codex', authenticated: false,
  loginMethod: '', email: '', plan: '', error: '',
}
const signedIn = {
  ...signedOut, authenticated: true, loginMethod: 'ChatGPT account',
  email: 'browser@example.com', plan: 'Plus',
}

async function providers(page: Page) {
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await page.locator('#v-settings .set-nav-item', { hasText: 'Providers' }).click()
  return page.locator('.set-subsec-nested').filter({ has: page.getByRole('heading', { name: 'Codex runtime', exact: true }) })
}

test('Codex sign-in reports a missing binary instead of silently disabling the button', async ({ page }) => {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, codexChatGPTAuth: false, codexApiKey: '', disabledProviders: [] }
  const error = 'Codex binary not found. Install Codex or set its Binary path.'
  await installBridge(page, s, { resolvers: {
    GetCodexStatus: () => ({ ...signedOut, binInstalled: false, binPath: '', error }),
    StartCodexLogin: () => new BridgeError(error),
  } })
  const codex = await providers(page)
  const signIn = codex.getByRole('button', { name: 'Sign in with ChatGPT', exact: true })
  await expect(signIn).toBeEnabled()
  await signIn.click()
  await expect(codex.getByRole('alert')).toContainText(error)
})

test('saving the Codex binary path rechecks discovery without leaving Providers', async ({ page }) => {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, codexChatGPTAuth: false, codexApiKey: '', codexBinPath: '', disabledProviders: [] }
  await installBridge(page, s, { resolvers: {
    GetCodexStatus: () => s.settings.codexBinPath
      ? { ...signedOut, binPath: s.settings.codexBinPath }
      : { ...signedOut, binInstalled: false, binPath: '', error: 'Codex binary not found' },
  } })
  const codex = await providers(page)
  const binary = codex.getByRole('textbox', { name: 'Codex binary path', exact: true })
  await binary.fill('/opt/codex/bin/codex')
  await binary.press('Tab')
  await expect(codex.locator('.set-desc', { hasText: 'Found' })).toContainText('/opt/codex/bin/codex')
})

for (const browserError of ['', 'No default browser configured']) {
  test(browserError ? 'Codex sign-in reports desktop browser errors' : 'Codex sign-in opens the native desktop browser bridge', async ({ page }) => {
    const s = structuredClone(seed)
    s.settings = { ...s.settings, codexChatGPTAuth: false, codexApiKey: '', disabledProviders: [] }
    let browserURL = ''
    let cancelled = false
    await page.addInitScript(() => {
      Object.defineProperty(window, 'webkit', {
        value: { messageHandlers: { external: { postMessage() {} } } },
      })
      window.open = () => { throw new Error('Desktop sign-in must use the native browser bridge') }
    })
    await installBridge(page, s, { resolvers: {
      GetCodexStatus: () => signedOut,
      StartCodexLogin: () => ({ loginId: 'native-login', authUrl: 'https://auth.openai.com/test-login' }),
      WaitCodexLogin: () => {
        s.settings = { ...s.settings, codexChatGPTAuth: true }
        return signedIn
      },
      CancelCodexLogin: () => { cancelled = true; return null },
    } })
    await page.route('**/wails/runtime', async (route) => {
      const body = route.request().postDataJSON()
      // Wails' Browser object is 9; OpenURL is method 0.
      if (body.object !== 9 || body.method !== 0) return route.fallback()
      browserURL = body.args.url
      return route.fulfill(browserError
        ? { status: 500, contentType: 'text/plain', body: browserError }
        : { status: 200, contentType: 'application/json', body: 'null' })
    })
    const codex = await providers(page)
    await codex.getByRole('button', { name: 'Sign in with ChatGPT', exact: true }).click()
    await expect.poll(() => browserURL).toBe('https://auth.openai.com/test-login')
    if (browserError) {
      await expect(codex.getByRole('alert')).toContainText(browserError)
      await expect.poll(() => cancelled).toBe(true)
    } else {
      await expect(codex).toContainText('browser@example.com')
    }
  })
}

test('Codex browser sign-in opens the website, saves ChatGPT auth, and survives reload', async ({ page }) => {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, codexChatGPTAuth: false, codexApiKey: 'old-key', disabledProviders: [] }
  let finish!: () => void
  const completed = new Promise<void>((resolve) => { finish = resolve })
  await page.addInitScript(() => {
    window.open = ((url: string) => { (window as any).loginURL = url; return null }) as typeof window.open
  })
  await installBridge(page, s, { resolvers: {
    GetCodexStatus: () => s.settings.codexChatGPTAuth ? signedIn : signedOut,
    StartCodexLogin: () => ({ loginId: 'login-1', authUrl: 'https://auth.openai.com/test-login' }),
    WaitCodexLogin: async (a) => {
      expect(a).toEqual(['login-1'])
      await completed
      s.settings = { ...s.settings, codexChatGPTAuth: true, codexApiKey: '' }
      return signedIn
    },
  } })
  const codex = await providers(page)
  await codex.getByRole('button', { name: 'Sign in with ChatGPT', exact: true }).click()
  await expect(codex).toContainText('Complete sign-in in your browser')
  await expect.poll(() => page.evaluate(() => (window as any).loginURL)).toBe('https://auth.openai.com/test-login')
  await expect(codex.getByRole('button', { name: 'toggle Codex ChatGPT auth', exact: true })).toBeDisabled()
  finish()
  await expect(codex).toContainText('browser@example.com')
  await expect(codex.getByRole('button', { name: 'Switch ChatGPT account' })).toBeVisible()
  await expect(codex.getByRole('button', { name: 'toggle Codex ChatGPT auth', exact: true })).toHaveAttribute('aria-pressed', 'true')
  await expect(codex.getByLabel('Codex API key', { exact: true })).toHaveCount(0)
  await page.reload({ waitUntil: 'domcontentloaded' })
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await page.locator('#v-settings .set-nav-item', { hasText: 'Providers' }).click()
  await expect(codex).toContainText('browser@example.com')
})

for (const action of ['cancel', 'leave settings'] as const) {
  test(`Codex browser sign-in cancels when you ${action}`, async ({ page }) => {
    const s = structuredClone(seed)
    s.settings = { ...s.settings, codexChatGPTAuth: false, codexApiKey: '', disabledProviders: [] }
    let finish!: () => void
    const completed = new Promise<void>((resolve) => { finish = resolve })
    let cancelled = ''
    await page.addInitScript(() => { window.open = () => null })
    await installBridge(page, s, { resolvers: {
      GetCodexStatus: () => signedOut,
      StartCodexLogin: () => ({ loginId: 'login-cancel', authUrl: 'https://auth.openai.com/test-login' }),
      WaitCodexLogin: async () => { await completed; return signedIn },
      CancelCodexLogin: (a) => { cancelled = a[0]; finish(); return null },
    } })
    const codex = await providers(page)
    await codex.getByRole('button', { name: 'Sign in with ChatGPT', exact: true }).click()
    await expect(codex).toContainText('Complete sign-in in your browser')
    if (action === 'cancel') {
      await codex.getByRole('button', { name: 'Cancel sign-in' }).click()
    } else {
      await page.locator('#v-settings .set-nav-item', { hasText: 'Appearance' }).click()
    }
    await expect.poll(() => cancelled).toBe('login-cancel')
    if (action === 'leave settings') {
      await page.locator('#v-settings .set-nav-item', { hasText: 'Providers' }).click()
    }
    await expect(codex.getByRole('button', { name: 'Sign in with ChatGPT', exact: true })).toBeEnabled()
    await expect(codex).not.toContainText('browser@example.com')
    expect(s.settings.codexChatGPTAuth).toBe(false)
  })
}

test('Codex browser sign-in shows login errors and allows retry', async ({ page }) => {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, codexChatGPTAuth: false, codexApiKey: '', disabledProviders: [] }
  await page.addInitScript(() => { window.open = () => null })
  await installBridge(page, s, { resolvers: {
    GetCodexStatus: () => signedOut,
    StartCodexLogin: () => ({ loginId: 'login-fail', authUrl: 'https://auth.openai.com/test-login' }),
    WaitCodexLogin: () => new BridgeError('Codex sign-in failed: Login rejected'),
    CancelCodexLogin: () => null,
  } })
  const codex = await providers(page)
  const signIn = codex.getByRole('button', { name: 'Sign in with ChatGPT', exact: true })
  await signIn.click()
  await expect(codex.getByRole('alert')).toContainText('Login rejected')
  await expect(signIn).toBeEnabled()
})
