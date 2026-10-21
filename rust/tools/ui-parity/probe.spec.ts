// Prints element boxes for a scene (development aid; not part of the capture).
import { test } from '@playwright/test'
import { installBridge, loadSeed } from '../../../frontend/e2e/helpers/mock-bridge'

const scene = process.env.PROBE_SCENE ?? 'chat/transcript'
const sels = (process.env.PROBE_SELS ?? '').split('|').filter(Boolean)
const [w, h] = (process.env.PROBE_SIZE ?? '640x900').split('x').map(Number)
const steps = JSON.parse(process.env.PROBE_STEPS ?? '[]')

test('probe', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: w, height: h }, deviceScaleFactor: 2 })
  const page = await context.newPage()
  await installBridge(page, structuredClone(loadSeed()), {})
  await page.goto(`/?scene=${encodeURIComponent(scene)}&theme=default`)
  await page.waitForSelector('[data-scene-ready]')
  for (const s of steps) if (s.click) await page.mouse.click(s.click[0], s.click[1])
  for (const sel of sels) {
    const boxes = await page.$$eval(sel, (els) => els.map((e) => {
      const r = e.getBoundingClientRect()
      const cs = getComputedStyle(e)
      return `${r.x.toFixed(2)},${r.y.toFixed(2)} ${r.width.toFixed(2)}x${r.height.toFixed(2)} lh=${cs.lineHeight} fs=${cs.fontSize} mh=${cs.maxHeight} st=${(e as HTMLElement).getAttribute("style") ?? ""}`
    }))
    console.log(sel, '=>', boxes.slice(0, 6).join(' | '))
  }
  await context.close()
})
