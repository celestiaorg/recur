import { test } from '@playwright/test'
import { installBridge, loadSeed } from '../../../frontend/e2e/helpers/mock-bridge'
test('dbg', async ({ page }) => {
  await installBridge(page, structuredClone(loadSeed()))
  await page.goto('/?scene=toasts/default&theme=default')
  await page.waitForSelector('[data-scene-ready]')
  await page.evaluate(() => document.fonts.ready)
  await page.evaluate(() => document.getAnimations().forEach((a) => a.finish?.()))
  console.log(await page.evaluate(() => [...document.querySelectorAll('.toast, .toast *')].map((e) => { const r = e.getBoundingClientRect(); return `${e.tagName}.${[...e.classList].filter(c=>!c.startsWith('svelte')).join('.')} x=${r.x} y=${r.y} w=${r.width} h=${r.height}` }).join('\n')))
})
