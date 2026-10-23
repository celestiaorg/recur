import { test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { installBridge, loadSeed } from '../../../frontend/e2e/helpers/mock-bridge'
test('probe', async ({ page }) => {
  await installBridge(page, structuredClone(loadSeed()), {})
  await page.goto(`/?scene=settings/models&theme=default&section=${process.env.SECTION ?? 'models'}`)
  await page.waitForSelector('[data-scene-ready]')
  const out = await page.evaluate((sel) => {
    const el = document.querySelector(sel) as HTMLElement
    const chain: string[] = []
    for (let e: HTMLElement | null = el; e; e = e.parentElement) {
      const cs = getComputedStyle(e)
      chain.push(`${e.tagName}.${e.className} ff=${cs.fontFamily} fs=${cs.fontSize} lh=${cs.lineHeight}`)
    }
    return chain.join('\n')
  }, process.env.SEL ?? '#v-settings .sel-val')
  console.log(out)
})
