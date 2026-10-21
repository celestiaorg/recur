import { defineConfig } from '@playwright/test'

const port = Number(process.env.UI_PARITY_PORT) || 9246
// WebKit is the engine behind WKWebView, which renders the shipped app (ui-parity.md).
const browserName = (process.env.UI_PARITY_BROWSER as 'webkit' | 'chromium' | undefined) ?? 'webkit'

// Reference capture for the Rust UI parity suite (rust/docs/ui-parity.md). Scenes render in
// headless WebKit at device scale 2 against the e2e suite's mocked Wails bridge and seed.
export default defineConfig({
  testDir: '.',
  testMatch: process.env.UI_PARITY_SPEC ?? 'capture.spec.ts',
  fullyParallel: true,
  // The first page load can race Vite's dependency optimizer, which reloads the page.
  retries: 1,
  reporter: [['list']],
  use: { baseURL: `http://127.0.0.1:${port}` },
  projects: [{ name: browserName, use: { browserName } }],
  webServer: {
    command: 'pnpm exec vite',
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    timeout: 120_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
})
