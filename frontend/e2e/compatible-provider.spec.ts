import { test, expect, seed, gotoApp } from './helpers/fixtures'
import { installBridge } from './helpers/mock-bridge'

const effort = { id: 'effort', label: 'Effort', wire: 'reasoning.effort', options: [
  { value: 'low', label: 'Low' }, { value: 'medium', label: 'Medium', default: true }, { value: 'high', label: 'High' }] }
const verbosity = { id: 'verbosity', label: 'Verbosity', wire: 'text.verbosity', options: [
  { value: 'low', label: 'Low' }, { value: 'medium', label: 'Medium', default: true }, { value: 'high', label: 'High' }] }

test('model picker controls: presets summarize, Custom opens the editor', async ({ page }) => {
  const s = structuredClone(seed)
  s.settings = { ...s.settings, compatibleProviders: [{ id: 'lmstudio', name: 'LM Studio', baseUrl: 'http://localhost:1234', models: [
    { id: 'qwen', label: 'Qwen', flags: [effort, verbosity] }] }] }
  await installBridge(page, s, { resolvers: { CompatibleFlagPreset: () => [effort, verbosity] } })
  await gotoApp(page)
  await page.locator('.rail-foot button[aria-label="Settings"]').click()
  await page.locator('#v-settings .set-nav-item', { hasText: 'Providers' }).click()
  const models = page.locator('#v-settings .set-row', { hasText: 'Each id is stored as' })
  const picker = models.locator('.sel-btn[aria-label="Picker controls"]')
  const pick = async (label: string) => {
    await picker.click()
    await page.locator('body > .sel-menu .sel-opt', { hasText: label }).click()
  }

  await expect(picker).toContainText('Effort, verbosity')
  await expect(models.locator('.compat-flag-summary')).toContainText('Low, Medium (default), High')
  const heads = models.locator('.compat-model.compat-cols > span')
  const inputs = models.locator('.compat-model-block .compat-model').first().locator('.inp, .sel')
  for (let c = 0; c < 4; c++) {
    const [h, inp] = [await heads.nth(c).boundingBox(), await inputs.nth(c).boundingBox()]
    expect(Math.abs(h!.x - inp!.x), `column ${c} header aligns with its field`).toBeLessThan(1)
  }
  await expect(models.locator('.compat-flag-card')).toHaveCount(0)

  await pick('Custom')
  await expect(picker).toContainText('Custom')
  await expect(models.locator('.compat-flag-card')).toHaveCount(2)
  await expect(models.locator('.compat-flag-summary')).toHaveCount(0)

  const medium = models.locator('.compat-default[aria-label="Default Medium"]').first()
  await expect(medium).toHaveAttribute('aria-pressed', 'true')
  await models.locator('.compat-default[aria-label="Default High"]').first().click()
  await expect(models.locator('.compat-default[aria-label="Default High"]').first()).toHaveAttribute('aria-pressed', 'true')
  await expect(medium).toHaveAttribute('aria-pressed', 'false')

  await pick('Effort, verbosity')
  await expect(models.locator('.compat-flag-card')).toHaveCount(0)
})
