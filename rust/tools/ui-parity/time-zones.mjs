// Writes WebKit's `Intl.supportedValuesOf('timeZone')`, the schedule editor's zone golden:
// `node time-zones.mjs > ../../crates/views/home/tests/golden/time-zones.json`
import { webkit } from '@playwright/test'

const browser = await webkit.launch()
const page = await browser.newPage()
const zones = await page.evaluate(() => Intl.supportedValuesOf('timeZone'))
console.log(JSON.stringify(zones, null, 1))
await browser.close()
