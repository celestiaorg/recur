import { test, expect, gotoApp, seed } from './helpers/fixtures'
import { installBridge, reloadApp, BridgeError } from './helpers/mock-bridge'

test('schedules save, run, pause, edit, and delete', async ({ page }) => {
  const schedules: any[] = []
  const project = seed.projects.find((p) => p.open) ?? seed.projects[0]
      const workspace = { ...seed.loops.find((w) => w.projectId === project.id)!, id: 'scheduled-test', goal: 'Check tests', short: 'Check tests', state: '', phase: 'editing', group: 'active', archived: false, type: 'solo', runStarted: false, scheduleId: 'schedule-test', last: 'Chat turn finished' }
  await installBridge(page, structuredClone(seed), {
    resolvers: {
      ListScheduledPrompts: () => schedules,
      PreviewScheduledPrompt: (a) => a[0].cron === 'bad cron' ? new BridgeError('Invalid cron expression') : [Date.now() + 86400000, Date.now() + 172800000, Date.now() + 259200000],
      SaveScheduledPrompt: (a) => {
        const p = { ...a[0], id: a[0].id || 'schedule-test', nextRunAt: Date.now() + 86400000 }
        const index = schedules.findIndex((s) => s.id === p.id)
        if (index < 0) schedules.push(p)
        else schedules[index] = p
        return p
      },
      RunScheduledPromptNow: (_a, s) => {
        schedules[0].lastRun = { id: 'run-test', scheduleId: schedules[0].id, dueAt: Date.now(), workspaceId: workspace.id, status: 'finished', detail: 'Open the workspace to inspect the result.' }
        s.loops.push(workspace)
        s.loopById[workspace.id] = workspace
        return workspace.id
      },
      PauseScheduledPrompt: (a) => { schedules.find((s) => s.id === a[0]).paused = a[1]; return null },
      DeleteScheduledPrompt: () => { schedules.splice(0); return null },
      LastAssistantText: (a) => a[0] === workspace.id ? 'All tests passed on the nightly run.' : '',
      // Inspect is slower than a double-click; confirm must not wait for it.
      WorkspaceDiscardRisk: async () => {
        await new Promise((r) => setTimeout(r, 750))
        return ''
      },
    },
  })
  await gotoApp(page)
  async function openSchedules() {
    const row = page.locator('.tproj', { has: page.locator('.pn', { hasText: project.name }) })
    await row.locator('.tproj-h').hover()
    await row.locator('.tproj-h').hover()
    await row.locator('.pset').click()
    await page.locator('#v-project .set-nav-item', { hasText: 'Schedules' }).click()
  }
  await openSchedules()
  const panel = page.locator('#v-project')
  await expect(panel.getByText('No schedules yet')).toBeVisible()
  await panel.getByRole('button', { name: 'New schedule', exact: true }).click()
  const form = page.getByRole('dialog', { name: 'New schedule' })
  await expect(form).toBeVisible()
  await expect(form.getByRole('combobox', { name: 'Repository' })).toContainText(project.name)
  await expect(form.getByRole('combobox', { name: 'Base branch' })).toBeVisible()
  await expect(form.getByLabel('Prompt', { exact: true })).toBeFocused()
  await form.getByRole('combobox', { name: 'Repository' }).focus()
  await page.keyboard.press('Shift+Tab')
  await expect(form.getByRole('button', { name: 'Simplify mode' })).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(form.getByRole('combobox', { name: 'Repository' })).toBeFocused()
  await form.getByLabel('Prompt', { exact: true }).fill('Check tests')
  await form.getByRole('combobox', { name: 'Time zone', exact: true }).click()
  await page.getByRole('searchbox', { name: 'Filter Time zone' }).fill('New York')
  await page.getByRole('option', { name: 'America/New York', exact: true }).click()
  await expect(form.getByRole('combobox', { name: 'Time zone', exact: true })).toContainText('America/New York')
  await form.getByRole('combobox', { name: 'Repeat', exact: true }).click()
  await page.locator('.sel-opt', { hasText: 'Once' }).click()
  await expect(form.getByLabel('Date', { exact: true })).toBeVisible()
  await form.getByRole('combobox', { name: 'Repeat', exact: true }).click()
  await page.locator('.sel-opt', { hasText: 'Cron' }).click()
  await expect(form.getByLabel('Date', { exact: true })).toHaveCount(0)
  await form.getByLabel('Cron expression', { exact: true }).fill('bad cron')
  await expect(form.getByRole('alert')).toContainText('Invalid cron expression')
  await expect(form.getByRole('button', { name: /^Schedule/ })).toBeDisabled()
  await form.getByLabel('Cron expression', { exact: true }).fill('*/15 9-17 * * MON-FRI')
  await form.getByRole('button', { name: 'Cron format help' }).click()
  await expect(form.getByRole('tooltip')).toContainText('@reboot')
  await expect(form.getByRole('combobox', { name: 'Mode', exact: true })).toContainText('Ask')
  await page.keyboard.press('Meta+.')
  await expect(form.getByRole('combobox', { name: 'Mode', exact: true })).toContainText('Plan')
  await form.getByRole('combobox', { name: 'Mode', exact: true }).click()
  await expect(page.getByRole('option')).toHaveCount(5)
  await page.getByRole('option', { name: /^Plan/ }).click()
  await form.getByRole('combobox', { name: 'Model flags', exact: true }).click()
  await page.getByRole('option', { name: 'Low', exact: true }).click()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toBeVisible()
  await form.getByRole('button', { name: 'Simplify mode' }).click()
  await expect(form.getByRole('button', { name: 'Simplify mode' })).toHaveAttribute('aria-pressed', 'false')
  await expect(form.getByRole('status')).toContainText('Next ')
  await page.screenshot({ path: test.info().outputPath('scheduled-prompts.png') })
  await form.getByRole('button', { name: /^Schedule/ }).click()
  const card = panel.getByRole('article', { name: 'Check tests', exact: true })
  await expect(card).toContainText('*/15 9-17 * * MON-FRI')
  await expect(card).toContainText('America/New_York')
  expect(schedules[0]).toMatchObject({ mode: 'plan', flags: { effort: 'low' }, simplify: false })
  await card.getByRole('button', { name: 'Run now', exact: true }).click()
  await expect(card).toContainText('Finished')
  await page.screenshot({ path: test.info().outputPath('settings-list.png') })
  // The engine created this workspace on its own; a workspace.updated tick must add it to the sidebar.
  await page.evaluate((id) => (window as any)._wails.dispatchWailsEvent({ name: 'workspace.updated', data: { workspaceId: id, phase: 'editing' } }), workspace.id)
  const projRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: project.name }) })
  await projRow.locator('.tsched-h', { hasText: 'Scheduled' }).click()
  await expect(page.locator('.rail .tloop', { hasText: 'Check tests' })).toBeVisible()
  await expect(card).toContainText('1 workspace')
  await card.locator('.sc-status').click()
  const runList = page.getByRole('list', { name: 'Workspaces for Check tests' })
  await expect(runList.getByRole('button', { name: /Check tests/ })).toBeVisible()
  await expect(runList).toContainText('All tests passed on the nightly run.')
  await expect(runList).not.toContainText('Chat turn finished')
  await expect(runList.getByRole('button', { name: 'Select all' })).toHaveCount(0)
  const run = runList.getByRole('listitem')
  const rm = run.locator('.sc-run-rm')
  await run.hover()
  await expect(rm).toHaveAttribute('aria-label', 'Remove')
  await rm.dblclick()
  await expect(page.locator('.toast.success', { hasText: 'Workspace removed' })).toBeVisible()
  await expect(runList).toHaveCount(0)
  await expect(card).not.toContainText('1 workspace')
  await expect(page.locator('.rail .tloop', { hasText: 'Check tests' })).toHaveCount(0)
  await card.getByRole('button', { name: 'Pause', exact: true }).click()
  await expect(card.getByRole('button', { name: 'Resume', exact: true })).toBeVisible()
  await card.getByRole('button', { name: 'Edit', exact: true }).click()
  const edit = page.getByRole('dialog', { name: 'Edit schedule' })
  await expect(edit.getByRole('combobox', { name: 'Model flags', exact: true })).toContainText('Low')
  await expect(edit.getByRole('button', { name: 'Simplify mode' })).toHaveAttribute('aria-pressed', 'false')
  await edit.getByLabel('Prompt', { exact: true }).fill('Find missing integration tests')
  await edit.getByRole('combobox', { name: 'Mode', exact: true }).click()
  await page.getByRole('option', { name: /^Loop/ }).click()
  await expect(edit.getByRole('combobox', { name: 'Loop', exact: true })).toBeVisible()
  await expect(edit.getByRole('combobox', { name: 'Policy', exact: true })).toBeVisible()
  await edit.getByLabel('Cron expression', { exact: true }).fill('0 9 1,15 * *')
  await edit.getByRole('button', { name: 'Save schedule' }).click()
  await expect.poll(() => schedules[0].prompt).toBe('Find missing integration tests')
  expect(schedules[0]).toMatchObject({ mode: 'loop', cron: '0 9 1,15 * *', flags: { effort: 'low' }, simplify: false })
  expect(schedules[0].loopId).toBeTruthy()
  expect(schedules[0].policyId).toBeTruthy()
  await card.getByRole('button', { name: 'Delete', exact: true }).click()
  await card.getByRole('button', { name: 'Confirm delete', exact: true }).click()
  await expect(panel.getByText('No schedules yet')).toBeVisible()
  await panel.getByRole('button', { name: 'New schedule', exact: true }).click()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await expect(panel.getByRole('button', { name: 'New schedule', exact: true })).toBeFocused()
})

test('main navigation shows schedules across projects and restores after reload', async ({ page }) => {
  const [first, second] = seed.projects
  const schedules = [first, second].map((project, i) => ({
    id: `schedule-${i}`, projectId: project.id, name: `Check ${project.name}`, prompt: 'Check tests',
    baseBranch: project.baseBranch, mode: 'ask', model: 'claude-code/sonnet', cadence: 'daily',
    date: '', time: '09:00', timeZone: 'UTC', paused: false, nextRunAt: Date.now() + 86400000,
  }))
  await installBridge(page, structuredClone(seed), { resolvers: {
    PreviewScheduledPrompt: () => [Date.now() + 86400000],
    ListScheduledPrompts: (a) => schedules.filter((p) => !a[0] || p.projectId === a[0]),
    SaveScheduledPrompt: (a) => { const p = { ...a[0], id: 'created' }; schedules.push(p); return p },
  } })
  await gotoApp(page)
  const nav = page.locator('.rail-top')
  await expect(nav.locator('button.nav')).toHaveText([/Inbox/, /Pull requests/, /Schedules/])
  await nav.locator('button.nav', { hasText: 'Schedules' }).click()
  const panel = page.locator('#v-schedules')
  await expect(nav.locator('button.nav', { hasText: 'Schedules' })).toHaveClass(/on/)
  await expect(panel.getByRole('article', { name: `Check ${first.name}`, exact: true })).toBeVisible()
  await expect(panel.getByRole('article', { name: `Check ${second.name}`, exact: true })).toBeVisible()
  await expect(panel.locator('.briefing-h', { hasText: first.name })).toBeVisible()
  await expect(panel.locator('.briefing-h', { hasText: second.name })).toBeVisible()
  await reloadApp(page)
  await expect(panel).toBeVisible()
  await expect(page.locator('.titlebar')).toContainText('Schedules')
  await expect(page.locator('.titlebar .tb-sub')).toContainText('2 active')
  await expect(page.locator('.titlebar').getByRole('button', { name: 'New schedule' })).toHaveCount(0)
  await panel.locator('.briefing-filters').getByRole('button', { name: 'New schedule', exact: true }).click()
  const form = page.getByRole('dialog', { name: 'New schedule' })
  await form.getByRole('combobox', { name: 'Repository' }).click()
  await page.getByRole('option', { name: second.name, exact: true }).click()
  await form.getByLabel('Prompt', { exact: true }).fill('New global schedule\nCheck recent changes')
  await form.getByRole('button', { name: /^Schedule/ }).click()
  await expect(panel.getByRole('article', { name: 'New global schedule', exact: true })).toBeVisible()
  expect(schedules.at(-1)?.projectId).toBe(second.id)
  await page.screenshot({ path: test.info().outputPath('all-scheduled-prompts.png') })
  const row = page.locator('.tproj', { has: page.locator('.pn', { hasText: first.name }) })
  await row.locator('.tproj-h').hover()
  await row.locator('.tproj-h').hover()
  await row.locator('.pset').click()
  await page.locator('#v-project .set-nav-item', { hasText: 'Schedules' }).click()
  await expect(page.locator('#v-project').getByRole('article')).toHaveCount(1)
  await expect(page.locator('#v-project').getByRole('article')).toContainText(`Check ${first.name}`)
  // Sidebar hover action, like New workspace on a project row.
  await nav.locator('button.nav', { hasText: 'Schedules' }).hover()
  await nav.getByRole('button', { name: 'New schedule', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'New schedule' })).toBeVisible()
  await expect(page.locator('#v-project')).toHaveClass(/on/)
  await expect(panel).toHaveCount(0)
  await page.keyboard.press('Escape')
  // Project row quick link preselects that project.
  const secondRow = page.locator('.tproj', { has: page.locator('.pn', { hasText: second.name }) })
  await secondRow.locator('.tproj-h').hover()
  await secondRow.getByRole('button', { name: `New schedule in ${second.name}`, exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'New schedule' })
  await expect(dialog.getByRole('combobox', { name: 'Repository' })).toContainText(second.name)
  await page.keyboard.press('Escape')
  // Command palette action.
  await page.keyboard.press('Meta+k')
  await page.keyboard.type('New schedule')
  await page.locator('.pal-row', { hasText: 'New schedule' }).first().click()
  await expect(dialog).toBeVisible()
})

test('schedule dashboard remembers expanded rows after leaving and reload', async ({ page }) => {
  const project = seed.projects.find((p) => p.open) ?? seed.projects[0]
  const workspace = {
    ...seed.loops.find((w) => w.projectId === project.id)!,
    id: 'sched-dash-run',
    goal: 'Nightly check',
    short: 'Nightly check',
    state: '',
    phase: 'editing',
    group: 'active',
    archived: false,
    type: 'solo',
    runStarted: false,
    scheduleId: 'schedule-dash',
    hostPrId: '',
    last: 'Chat turn finished',
  }
  const data = structuredClone(seed)
  data.loops.push(workspace)
  data.loopById[workspace.id] = workspace
  const schedules = [{
    id: 'schedule-dash', projectId: project.id, name: 'Nightly check', prompt: 'Check tests',
    baseBranch: project.baseBranch, mode: 'ask', model: 'claude-code/sonnet', cadence: 'daily',
    date: '', time: '09:00', timeZone: 'UTC', paused: false, nextRunAt: Date.now() + 86400000,
    lastRun: { id: 'run-dash', scheduleId: 'schedule-dash', dueAt: Date.now(), workspaceId: workspace.id, status: 'finished', detail: '' },
  }]
  await installBridge(page, data, {
    resolvers: {
      ListScheduledPrompts: (a) => schedules.filter((p) => !a[0] || p.projectId === a[0]),
      LastAssistantText: (a) => a[0] === workspace.id ? 'All tests passed.' : '',
    },
  })
  await gotoApp(page)
  const nav = page.locator('.rail-top')
  await nav.locator('button.nav', { hasText: 'Schedules' }).click()
  const card = () => page.locator('#v-schedules').getByRole('article', { name: 'Nightly check', exact: true })
  await expect(card()).toHaveAttribute('aria-expanded', 'false')
  await card().locator('.sc-status').click()
  await expect(card()).toHaveAttribute('aria-expanded', 'true')
  await expect(page.getByRole('list', { name: 'Workspaces for Nightly check' })).toBeVisible()
  await nav.locator('button.nav', { hasText: 'Inbox' }).click()
  await expect(page.locator('#v-schedules')).toHaveCount(0)
  await nav.locator('button.nav', { hasText: 'Schedules' }).click()
  await expect(card()).toHaveAttribute('aria-expanded', 'true')
  await expect(page.getByRole('list', { name: 'Workspaces for Nightly check' })).toBeVisible()
  await reloadApp(page)
  await expect(card()).toHaveAttribute('aria-expanded', 'true')
  await card().locator('.sc-status').click()
  await expect(card()).toHaveAttribute('aria-expanded', 'false')
  await reloadApp(page)
  await expect(card()).toHaveAttribute('aria-expanded', 'false')
})

test('schedule dashboard bulk-removes selected workspaces', async ({ page }) => {
  const project = seed.projects.find((p) => p.open) ?? seed.projects[0]
  const base = seed.loops.find((w) => w.projectId === project.id)!
  const settled = [0, 1].map((i) => ({
    ...base,
    id: `sched-bulk-${i}`,
    goal: `Nightly ${i + 1}`,
    short: `Nightly ${i + 1}`,
    state: '',
    phase: 'editing',
    group: 'active',
    archived: false,
    type: 'solo',
    runStarted: false,
    isMain: false,
    scheduleId: 'schedule-bulk',
    hostPrId: '',
    last: 'Chat turn finished',
  }))
  const running = {
    ...base,
    id: 'sched-bulk-live',
    goal: 'Nightly live',
    short: 'Nightly live',
    state: 'coding',
    phase: 'editing',
    group: 'active',
    archived: false,
    type: 'loop',
    runStarted: true,
    isMain: false,
    scheduleId: 'schedule-bulk',
    hostPrId: '',
    last: 'Still working',
  }
  const data = structuredClone(seed)
  for (const w of [...settled, running]) {
    data.loops.push(w)
    data.loopById[w.id] = w
  }
  const schedules = [{
    id: 'schedule-bulk', projectId: project.id, name: 'Nightly bulk', prompt: 'Check tests',
    baseBranch: project.baseBranch, mode: 'ask', model: 'claude-code/sonnet', cadence: 'daily',
    date: '', time: '09:00', timeZone: 'UTC', paused: false, nextRunAt: Date.now() + 86400000,
    lastRun: { id: 'run-bulk', scheduleId: 'schedule-bulk', dueAt: Date.now(), workspaceId: settled[1].id, status: 'finished', detail: '' },
  }]
  await installBridge(page, data, {
    resolvers: {
      ListScheduledPrompts: (a) => schedules.filter((p) => !a[0] || p.projectId === a[0]),
      LastAssistantText: (a) => a[0] === running.id ? 'Still working.' : 'All tests passed.',
      WorkspaceDiscardRisk: async () => {
        await new Promise((r) => setTimeout(r, 750))
        return ''
      },
      RemoveWorkspace: async (a, s) => {
        await new Promise((r) => setTimeout(r, 400))
        const id = a[0]
        const updated = { ...(s.loopById[id] ?? {}), archived: true, worktreePath: '' }
        s.loopById[id] = updated
        s.loops = s.loops.map((l) => (l.id === id ? updated : l))
        return updated
      },
    },
  })
  await gotoApp(page)
  await page.locator('.rail-top').locator('button.nav', { hasText: 'Schedules' }).click()
  const card = page.locator('#v-schedules').getByRole('article', { name: 'Nightly bulk', exact: true })
  await card.locator('.sc-status').click()
  const runList = page.getByRole('list', { name: 'Workspaces for Nightly bulk' })
  await expect(runList.getByRole('listitem')).toHaveCount(3)
  const selectAll = runList.getByRole('button', { name: 'Select all' })
  await expect(selectAll).toBeVisible()
  await expect(runList.getByRole('button', { name: 'Select Nightly live' })).toHaveCount(0)
  await selectAll.click()
  await expect(runList.getByRole('button', { name: 'Deselect all' })).toBeVisible()
  await expect(runList).toContainText('2 selected')
  await expect(runList.getByRole('button', { name: 'Deselect Nightly 1' })).toHaveAttribute('aria-pressed', 'true')
  await expect(runList.getByRole('button', { name: 'Select Nightly live' })).toHaveCount(0)
  const bulkRm = runList.locator('.sc-runs-bar').getByRole('button', { name: 'Remove' })
  await bulkRm.dblclick()
  await expect(runList.getByRole('listitem')).toHaveCount(1, { timeout: 200 })
  await expect(page.locator('.toast.success', { hasText: '2 workspaces removed' })).toBeVisible()
  await expect(runList).toContainText('Nightly live')
  await expect(runList.getByRole('button', { name: 'Select all' })).toHaveCount(0)
})
