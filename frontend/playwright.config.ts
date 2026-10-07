import { defineConfig, devices } from '@playwright/test'

// E2E config for the Recur frontend. Tests drive the real Svelte UI in headless
// Chromium against a MOCKED Wails bridge (see e2e/helpers/mock-bridge.ts), so they
// run deterministically with no live Go backend, no `claude`, and no native window —
// ideal for CI and for catching UI regressions.
//
// The webServer below starts the Vite dev server (port 9245, from vite.config.ts);
// Playwright intercepts the bridge calls, so Vite only needs to serve the assets.
export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: process.env.CI
    ? [['github'], ['html', { open: 'never' }]]
    : [['list']],
  use: {
    baseURL: 'http://localhost:9245',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'pnpm run dev',
    url: 'http://localhost:9245',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
})
