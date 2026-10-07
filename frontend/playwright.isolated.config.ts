import base from './playwright.config'
import { defineConfig } from '@playwright/test'

const PORT = Number(process.env.E2E_PORT || 9351)
export default defineConfig({
  ...base,
  use: { ...base.use, baseURL: `http://localhost:${PORT}` },
  webServer: {
    command: `WAILS_VITE_PORT=${PORT} pnpm run dev`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 120_000,
    stdout: 'ignore',
    stderr: 'pipe',
  },
})
