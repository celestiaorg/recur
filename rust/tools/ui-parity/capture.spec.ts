// Captures one reference PNG per scene in scenes.json into out/reference/.
//
// Fonts: index.html links Google Fonts exactly like frontend/index.html. That request is
// answered with @font-face rules for the very files recur-ui bundles, so both renderers
// rasterize identical font binaries and the capture never touches the network.
import { test, type Page } from '@playwright/test'
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { installBridge, loadSeed } from '../../../frontend/e2e/helpers/mock-bridge'

const here = dirname(fileURLToPath(import.meta.url))
const fontsDir = join(here, '..', '..', 'crates', 'ui', 'assets', 'fonts')
const out = join(here, 'out', 'reference')
const { scenes } = JSON.parse(readFileSync(join(here, 'scenes.json'), 'utf8')) as { scenes: Scene[] }
const only = process.env.UI_PARITY_SCENES?.split(',').filter(Boolean)
const seed = loadSeed()

type Step =
  | { move: [number, number] }
  | { down: [number, number] }
  | { up: [number, number] }
  | { click: [number, number] }
  | { key: string }
  | { type: string }
  | { wait: number }
type Scene = { name: string; width: number; height: number; theme: string; steps?: Step[]; animationTime?: number }

const FAMILY: Record<string, string> = {
  Inter: 'Inter', SpaceGrotesk: 'Space Grotesk', JetBrainsMono: 'JetBrains Mono',
  IBMPlexMono: 'IBM Plex Mono', SourceCodePro: 'Source Code Pro',
}

function fontsCss(): string {
  return readdirSync(fontsDir)
    .filter((f) => f.endsWith('.ttf'))
    .map((f) => {
      const [, stem, weight] = /^(.+)-(\d+)\.ttf$/.exec(f)!
      return `@font-face{font-family:'${FAMILY[stem]}';font-style:normal;font-weight:${weight};font-display:block;src:url(/__fonts/${f}) format('truetype');}`
    })
    .join('\n')
}

async function run(page: Page, step: Step) {
  if ('move' in step) await page.mouse.move(...step.move)
  else if ('down' in step) {
    await page.mouse.move(...step.down)
    await page.mouse.down()
  } else if ('up' in step) {
    await page.mouse.move(...step.up)
    await page.mouse.up()
  } else if ('click' in step) await page.mouse.click(...step.click)
  else if ('key' in step) await page.keyboard.press(step.key)
  else if ('type' in step) await page.keyboard.type(step.type)
  else if ('wait' in step) await page.waitForTimeout(step.wait)
}

test.beforeAll(() => mkdirSync(out, { recursive: true }))

for (const scene of scenes) {
  if (only && !only.includes(scene.name)) continue
  test(scene.name, async ({ browser }) => {
    const context = await browser.newContext({
      viewport: { width: scene.width, height: scene.height },
      deviceScaleFactor: 2,
    })
    const page = await context.newPage()
    const errors: string[] = []
    page.on('pageerror', (e) => errors.push(e.message))
    await page.route('https://fonts.googleapis.com/**', (r) => r.fulfill({ contentType: 'text/css', body: fontsCss() }))
    await page.route('https://fonts.gstatic.com/**', (r) => r.abort())
    await page.route('**/__fonts/*', (r) =>
      r.fulfill({ contentType: 'font/ttf', body: readFileSync(join(fontsDir, new URL(r.request().url()).pathname.split('/').pop()!)) }),
    )
    await installBridge(page, structuredClone(seed))
    await page.goto(`/?scene=${encodeURIComponent(scene.name)}&theme=${scene.theme}`)
    await page.waitForSelector('[data-scene-ready]')
    await page.evaluate(() => document.fonts.ready)
    for (const step of scene.steps ?? []) await run(page, step)
    // Freeze every animation: finite ones at their end (or at `animationTime`), infinite ones
    // at `animationTime` (default 0). The Rust renderer freezes recur-ui motion the same way.
    await page.evaluate((t) => {
      for (const a of document.getAnimations()) {
        a.pause()
        const end = Number(a.effect?.getComputedTiming().endTime ?? 0)
        a.currentTime = t ?? (Number.isFinite(end) ? end : 0)
      }
    }, scene.animationTime ?? null)
    await page.screenshot({ path: join(out, `${scene.name.replaceAll('/', '__')}.png`), caret: 'hide' })
    if (scene.name === 'text/calibration') {
      const metrics = await page.evaluate(() =>
        [...document.querySelectorAll<HTMLElement>('[data-m]')].map((el) => {
          const r = el.getBoundingClientRect()
          const b = el.querySelector<HTMLElement>('.bl')!.getBoundingClientRect()
          return { id: el.dataset.m, width: r.width, height: r.height, baseline: b.top - r.top }
        }),
      )
      writeFileSync(join(out, 'metrics.json'), JSON.stringify(metrics, null, 1))
    }
    if (errors.length) throw new Error(`page errors: ${errors.join('; ')}`)
    await context.close()
  })
}
