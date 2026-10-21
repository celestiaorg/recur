// Serves the reference scenes: real frontend components and styles, mounted one per page.
// The frontend's own Wails plugin is reused from its node_modules so bindings resolve the same
// way they do in the app and the e2e suite.
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { defineConfig } from 'vite'
import { svelte, vitePreprocess } from '@sveltejs/vite-plugin-svelte'

const here = dirname(fileURLToPath(import.meta.url))
const repo = join(here, '..', '..', '..')
const frontend = join(repo, 'frontend')
const { default: wails } = await import(join(frontend, 'node_modules/@wailsio/runtime/dist/plugins/vite.js'))

export const PORT = Number(process.env.UI_PARITY_PORT) || 9246

export default defineConfig({
  root: join(here, 'web'),
  // The frontend's own preprocessing (`frontend/svelte.config.js`), so components with full
  // TypeScript (CodeEditor) compile as they do in the app.
  plugins: [svelte({ preprocess: vitePreprocess({ script: true }) }), wails(join(frontend, 'bindings'))],
  // One Svelte runtime for the harness and the frontend components it mounts.
  resolve: { dedupe: ['svelte'] },
  server: {
    host: '127.0.0.1',
    port: PORT,
    strictPort: true,
    fs: { allow: [repo] },
  },
})
