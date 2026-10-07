<script lang="ts">
  import { onMount } from 'svelte'
  import { ProjectsService, WorkspacesService } from '../../bindings/recur/internal/app'
  import { ScheduledPrompt, type Project } from '../../bindings/recur/internal/domain/models'
  import { upsertWorkspace } from '../lib/api'
  import { markClickEcho, takeClickEcho } from '../lib/armed-confirm'
  import { removeWorkspace, removeWorkspaces } from '../lib/remove-workspace'
  import { openWorkspace, projects, prs, schedules as schedulesStore, scheduleEditor, schedulesVersion, workspaces, workspacesRunning, attention, userQuestions, permissionRequests } from '../lib/stores'
  import { scheduleExpanded, toggleScheduleExpanded } from '../lib/schedule-expanded'
  import { chatModeMeta, parkedNeedsYouByWorkspace, sidebarUnreadTitle, workspaceCanRemove, workspaceIsRunning } from '../lib/modes'
  import { formatCompactCount, formatRelativeAge } from '../lib/format'
  import { prForWorkspace, prIconName, prLifecycle } from '../lib/colors'
  import Icon from './Icon.svelte'
  import Select from './Select.svelte'

  let { project }: { project?: Project } = $props()
  let schedules = $state.raw<ScheduledPrompt[]>([])
  let loading = $state(true)
  let busy = $state(false)
  let error = $state('')
  let deleting = $state('')
  let snippets = $state<Record<string, string>>({})
  let selected = $state<Record<string, boolean>>({})
  let projectFilter = $state<string[]>([])
  let stateFilter = $state<'' | 'active' | 'paused'>('')
  let mounted = false
  let refreshing = false
  async function refresh() {
    if (refreshing) return
    refreshing = true
    try {
      const rows = await ProjectsService.ListScheduledPrompts(project?.id ?? '')
      if (mounted) {
        schedules = rows
        if (!project) schedulesStore.set(rows)
      }
    } catch (e) {
      if (mounted) error = String(e)
    } finally {
      refreshing = false
      if (mounted) loading = false
    }
  }

  onMount(() => {
    mounted = true
    void refresh()
    const timer = setInterval(() => { void refresh() }, 5000)
    return () => { mounted = false; clearInterval(timer); clearArmedRemove() }
  })

  // Saves from the editor modal refresh the list.
  $effect(() => {
    void $schedulesVersion
    void refresh()
  })

  function scheduleLabel(p: ScheduledPrompt) {
    if (p.cadence === 'cron') return p.cron === '@reboot' ? 'When Recur starts' : p.cron
    return `${p.cadence === 'once' ? p.date : p.cadence === 'weekdays' ? 'Weekdays' : 'Daily'} at ${p.time}`
  }

  const visible = $derived(schedules.filter((p) =>
    (!projectFilter.length || projectFilter.includes(p.projectId))
    && (stateFilter === '' || (stateFilter === 'paused') === p.paused)))
  const filtered = $derived(projectFilter.length > 0 || stateFilter !== '')
  const grouped = $derived.by(() => {
    if (project) return [{ id: project.id, name: project.name, color: '', items: visible }]
    const by = new Map<string, ScheduledPrompt[]>()
    for (const p of visible) (by.get(p.projectId) ?? by.set(p.projectId, []).get(p.projectId)!).push(p)
    return [...by].map(([id, items]) => {
      const pr = $projects.find((x) => x.id === id)
      return { id, name: pr?.name ?? id, color: pr?.color ?? '', items }
    })
  })
  const parkedByWs = $derived(parkedNeedsYouByWorkspace($workspaces, $userQuestions, $permissionRequests))

  type Status = { label: string; tone: 'faint' | 'muted' | 'amber' | 'coral' | 'warn' | 'emerald'; live?: boolean }
  function statusOf(p: ScheduledPrompt): Status {
    if (p.paused) return { label: 'Paused', tone: 'faint' }
    switch (p.lastRun?.status) {
      case 'running': return { label: 'Running', tone: 'amber', live: true }
      case 'waiting': return { label: 'Needs you', tone: 'coral' }
      case 'failed': return { label: 'Failed', tone: 'coral' }
      case 'interrupted': return { label: 'Interrupted', tone: 'coral' }
      case 'missed': return { label: 'Missed', tone: 'warn' }
      case 'skipped': return { label: 'Skipped', tone: 'muted' }
      case 'stopped': return { label: 'Stopped', tone: 'muted' }
      case 'finished': return { label: 'Finished', tone: 'emerald' }
    }
    return { label: 'Scheduled', tone: 'muted' }
  }
  const toneColor: Record<Status['tone'], string> = {
    faint: 'var(--faint)', muted: 'var(--muted)', amber: 'var(--amber)', coral: 'var(--coral)', warn: 'var(--warn)', emerald: 'var(--emerald)',
  }

  function nextOf(p: ScheduledPrompt) {
    if (p.paused) return ''
    if (p.cadence === 'cron' && p.cron === '@reboot') return 'when Recur starts'
    return p.nextRunAt ? when(p.nextRunAt, p.timeZone) : ''
  }

  function openLastWorkspace(p: ScheduledPrompt) {
    const id = p.lastRun?.workspaceId
    if (!id) return
    void act(async () => {
      const ws = await WorkspacesService.GetWorkspace(id)
      upsertWorkspace(ws)
      openWorkspace(ws.id)
    })
  }

  function workspacesOf(p: ScheduledPrompt) {
    return $workspaces.filter((w) => !w.archived && !(w.hostPrId ?? '').trim() && w.scheduleId === p.id)
  }

  $effect(() => {
    const ids: string[] = []
    const ticks: number[] = []
    for (const p of schedules) {
      if (!$scheduleExpanded[p.id]) continue
      for (const w of $workspaces) {
        if (w.archived || (w.hostPrId ?? '').trim() || w.scheduleId !== p.id) continue
        ids.push(w.id)
        ticks.push(w.lastActivityAt ?? 0)
      }
    }
    void ticks.join()
    let cancelled = false
    void Promise.all(ids.map(async (id) => {
      try {
        return [id, await WorkspacesService.LastAssistantText(id)] as const
      } catch {
        return [id, ''] as const
      }
    })).then((rows) => {
      if (cancelled) return
      snippets = Object.fromEntries(rows)
    })
    return () => { cancelled = true }
  })

  function toggle(id: string, runs: number) {
    if (!runs) return
    if ($scheduleExpanded[id]) clearPickedForSchedule(id)
    toggleScheduleExpanded(id)
  }

  function pickedOf(ids: string[]) {
    return ids.filter((id) => selected[id])
  }

  function togglePick(id: string) {
    const next = { ...selected }
    if (next[id]) delete next[id]
    else next[id] = true
    selected = next
    if (armedRemoveId?.startsWith('bulk:')) clearArmedRemove()
  }

  function toggleSelectAll(ids: string[]) {
    const allOn = ids.length > 0 && ids.every((id) => selected[id])
    const next = { ...selected }
    if (allOn) for (const id of ids) delete next[id]
    else for (const id of ids) next[id] = true
    selected = next
    if (armedRemoveId?.startsWith('bulk:')) clearArmedRemove()
  }

  function clearPickedForSchedule(scheduleId: string) {
    const ids = new Set($workspaces.filter((w) => w.scheduleId === scheduleId).map((w) => w.id))
    const next = { ...selected }
    let changed = false
    for (const id of Object.keys(next)) {
      if (!ids.has(id)) continue
      delete next[id]
      changed = true
    }
    if (changed) selected = next
  }

  function dropPicked(ids: string[]) {
    const next = { ...selected }
    let changed = false
    for (const id of ids) {
      if (!(id in next)) continue
      delete next[id]
      changed = true
    }
    if (changed) selected = next
  }

  function onRowClick(e: MouseEvent, id: string, runs: number) {
    if ((e.target as HTMLElement | null)?.closest('button, a')) return
    toggle(id, runs)
  }

  function onRowKey(e: KeyboardEvent, id: string, runs: number) {
    if (e.target !== e.currentTarget) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      toggle(id, runs)
    }
  }

  function openRunWorkspace(id: string) {
    clearArmedRemove()
    void act(async () => {
      const ws = await WorkspacesService.GetWorkspace(id)
      upsertWorkspace(ws)
      openWorkspace(ws.id)
    })
  }

  function onOpenRun(e: MouseEvent, id: string, canPick: boolean, canRm: boolean) {
    if (canPick && canRm && (e.metaKey || e.ctrlKey)) {
      togglePick(id)
      return
    }
    openRunWorkspace(id)
  }

  // Two-click Remove: first press arms, a quick second press confirms (ArmedButton /
  // titlebar). Inspect is tooltip-only and must not block or disarm.
  const REMOVE_ARM_MS = 3000
  let armedRemoveId = $state<string | null>(null)
  let removeRiskById = $state<Record<string, string>>({})
  let removingId = $state<string | null>(null)
  let armTimer: ReturnType<typeof setTimeout> | undefined
  const removeEcho = { eatClick: false }
  const riskFetches = new Map<string, Promise<boolean>>()
  const riskFetchGen = new Map<string, number>()

  function clearArmedRemove() {
    if (armTimer) {
      clearTimeout(armTimer)
      armTimer = undefined
    }
    armedRemoveId = null
  }

  function restartArmTimer(id: string) {
    if (armTimer) clearTimeout(armTimer)
    armTimer = setTimeout(() => {
      armTimer = undefined
      if (armedRemoveId === id) armedRemoveId = null
    }, REMOVE_ARM_MS)
  }

  function dropRemoveRisk(id: string) {
    if (!(id in removeRiskById)) return
    const next = { ...removeRiskById }
    delete next[id]
    removeRiskById = next
  }

  function fetchRemoveRisk(id: string, force = false): Promise<boolean> {
    if (!force) {
      const inflight = riskFetches.get(id)
      if (inflight) return inflight
      if (Object.prototype.hasOwnProperty.call(removeRiskById, id)) return Promise.resolve(true)
    }
    const gen = (riskFetchGen.get(id) ?? 0) + 1
    riskFetchGen.set(id, gen)
    const p = WorkspacesService.WorkspaceDiscardRisk(id)
      .then((risk) => {
        if (riskFetchGen.get(id) !== gen) return false
        removeRiskById = { ...removeRiskById, [id]: risk ?? '' }
        return true
      })
      .catch(() => {
        if (riskFetchGen.get(id) !== gen) return false
        dropRemoveRisk(id)
        return false
      })
      .finally(() => {
        if (riskFetches.get(id) === p) riskFetches.delete(id)
      })
    riskFetches.set(id, p)
    return p
  }

  function armRemove(id: string) {
    armedRemoveId = id
    restartArmTimer(id)
    void fetchRemoveRisk(id, true)
  }

  function commitRemoveRun(id: string) {
    clearArmedRemove()
    removingId = id
    dropRemoveRisk(id)
    void removeWorkspace(id).finally(() => {
      dropPicked([id])
      if (removingId === id) removingId = null
    })
  }

  function onRemovePointerUp(e: PointerEvent, id: string, can: boolean) {
    if (e.button !== 0 || !can || removingId || armedRemoveId !== id) return
    markClickEcho(removeEcho)
    commitRemoveRun(id)
  }

  function onRemoveRun(id: string, can: boolean) {
    if (takeClickEcho(removeEcho)) return
    if (!can || removingId) return
    if (armedRemoveId !== id) {
      armRemove(id)
      return
    }
    commitRemoveRun(id)
  }

  function bulkKey(scheduleId: string) {
    return `bulk:${scheduleId}`
  }

  function armBulkRemove(scheduleId: string, ids: string[]) {
    const key = bulkKey(scheduleId)
    armedRemoveId = key
    restartArmTimer(key)
    for (const id of ids) void fetchRemoveRisk(id, true)
  }

  function commitRemoveBulk(scheduleId: string, ids: string[]) {
    const key = bulkKey(scheduleId)
    clearArmedRemove()
    removingId = key
    for (const id of ids) dropRemoveRisk(id)
    void removeWorkspaces(ids).finally(() => {
      dropPicked(ids)
      if (removingId === key) removingId = null
    })
  }

  function onRemoveBulkPointerUp(e: PointerEvent, scheduleId: string, ids: string[]) {
    if (e.button !== 0 || !ids.length || removingId || armedRemoveId !== bulkKey(scheduleId)) return
    markClickEcho(removeEcho)
    commitRemoveBulk(scheduleId, ids)
  }

  function onRemoveBulk(scheduleId: string, ids: string[]) {
    if (takeClickEcho(removeEcho)) return
    if (!ids.length || removingId) return
    const key = bulkKey(scheduleId)
    if (armedRemoveId !== key) {
      armBulkRemove(scheduleId, ids)
      return
    }
    commitRemoveBulk(scheduleId, ids)
  }

  function removeConfirmTitle(ids: string | string[]) {
    const list = Array.isArray(ids) ? ids : [ids]
    const risk = list.map((id) => removeRiskById[id]).find(Boolean) ?? ''
    return risk ? `Confirm remove — ${risk}` : 'Confirm remove'
  }

  function suppressRepeatKey(e: KeyboardEvent) {
    if (e.repeat) e.preventDefault()
  }

  function runSnippet(id: string) {
    return (snippets[id] || '').replace(/<[^>]+>/g, '').replace(/\s+/g, ' ').trim()
  }

  function edit(p?: ScheduledPrompt) {
    deleting = ''
    scheduleEditor.set({ projectId: project?.id, schedule: p })
  }

  async function act(action: () => Promise<unknown>) {
    if (busy) return
    busy = true
    error = ''
    try { await action(); await refresh() }
    catch (e) { if (mounted) error = String(e) }
    finally { if (mounted) busy = false }
  }

  function when(at: number, zone: string) {
    try { return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short', timeZone: zone }).format(at) }
    catch { return '' }
  }
</script>

{#snippet scheduleRows(items: ScheduledPrompt[])}
  {#each items as p (p.id)}
    {@const meta = chatModeMeta(p.mode)}
    {@const st = statusOf(p)}
    {@const next = nextOf(p)}
    {@const runs = workspacesOf(p)}
    {@const open = !!$scheduleExpanded[p.id]}
    <div class="sc-block">
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions, a11y_role_supports_aria_props_implicit -->
    <article
      class="ibrow scrow"
      class:read={p.paused}
      class:is-open={open}
      class:can-exp={runs.length > 0}
      aria-label={p.name}
      aria-expanded={runs.length ? open : undefined}
      onclick={(e) => onRowClick(e, p.id, runs.length)}
      onkeydown={(e) => onRowKey(e, p.id, runs.length)}
      tabindex={runs.length ? 0 : undefined}
    >
      <div class="ity" class:can-exp={runs.length > 0} style="background:color-mix(in srgb, {meta.tint} 14%, transparent);color:{meta.tint}">
        <span class="sc-mode">
          {#if st.live}<span class="sd pulse" title="Running" aria-label="Running"></span>{:else}<Icon name={meta.icon} />{/if}
        </span>
        {#if runs.length}
          <span class="sc-chev" aria-hidden="true"><Icon name="chevron-down" strokeWidth={2.4} /></span>
        {/if}
      </div>
      <div class="ibmain">
        <div class="ibt"><button type="button" class="sc-title" onclick={() => edit(p)}>{p.name}</button></div>
        <div class="sc-meta">
          <div class="ibs"><span class="sc-cronlbl">{scheduleLabel(p)}</span><span>{p.timeZone}</span></div>
          <div class="ibs"><span>{meta.label}</span><span>{p.baseBranch}</span></div>
        </div>
      </div>
      <div class="sc-side">
        <div class="sc-status" title={p.lastRun?.detail || ''} style="color:{toneColor[st.tone]}">
          <span class="d" style="background:{toneColor[st.tone]}"></span>{st.label}
        </div>
        <span class="ibtime sc-next">{next ? `Next ${next}` : ''}</span>
        {#if runs.length}
          <span class="sc-runs-n">{runs.length} {runs.length === 1 ? 'workspace' : 'workspaces'}</span>
        {/if}
      </div>
      <div class="sc-acts" role="group" aria-label="Actions for {p.name}">
        {#if p.lastRun?.workspaceId}
          <button type="button" class="tb-icon-btn" disabled={busy} title="Open last workspace" aria-label="Open workspace" onclick={() => openLastWorkspace(p)}><Icon name="box" /></button>
        {/if}
        <button type="button" class="tb-icon-btn" disabled={busy} title="Run now" aria-label="Run now" onclick={() => void act(async () => { await ProjectsService.RunScheduledPromptNow(p.id) })}><Icon name="zap" /></button>
        <button type="button" class="tb-icon-btn" disabled={busy} title={p.paused ? 'Resume' : 'Pause'} aria-label={p.paused ? 'Resume' : 'Pause'} onclick={() => void act(() => ProjectsService.PauseScheduledPrompt(p.id, !p.paused))}><Icon name={p.paused ? 'play' : 'pause'} /></button>
        <button type="button" class="tb-icon-btn" disabled={busy} title="Edit" aria-label="Edit" onclick={() => edit(p)}><Icon name="pencil" /></button>
        {#if deleting === p.id}
          <button type="button" class="tb-icon-btn sc-confirm" disabled={busy} aria-label="Confirm delete" onclick={() => void act(async () => { await ProjectsService.DeleteScheduledPrompt(p.id); deleting = '' })}>Delete?</button>
        {:else}
          <button type="button" class="tb-icon-btn sc-del" disabled={busy} title="Delete" aria-label="Delete" onclick={() => { deleting = p.id }}><Icon name="trash" /></button>
        {/if}
      </div>
    </article>
    {#if open && runs.length}
      {@const canPick = runs.length > 1}
      {@const removable = runs.filter((w) => workspaceCanRemove(w, $workspacesRunning.has(w.id))).map((w) => w.id)}
      {@const picked = pickedOf(removable)}
      {@const allOn = removable.length > 0 && removable.every((id) => selected[id])}
      {@const bulkId = bulkKey(p.id)}
      {@const bulkArmed = armedRemoveId === bulkId}
      <div class="sc-runs" class:can-pick={canPick} role="list" aria-label="Workspaces for {p.name}">
        {#if canPick}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="sc-runs-bar"
            class:is-armed={bulkArmed}
            onpointerenter={() => { for (const id of picked) void fetchRemoveRisk(id, false) }}
            onpointerleave={() => { if (armedRemoveId === bulkId) clearArmedRemove() }}
          >
            <button
              type="button"
              class="sc-runs-all"
              aria-pressed={allOn}
              aria-label={allOn ? 'Deselect all' : 'Select all'}
              disabled={!removable.length}
              onclick={() => toggleSelectAll(removable)}
            >
              {picked.length ? `${picked.length} selected` : 'Select all'}
            </button>
            <span class="sc-run-end">
              {#if picked.length}
                <button
                  type="button"
                  class="sc-run-rm sc-bulk-rm"
                  class:armed={bulkArmed}
                  title={bulkArmed ? removeConfirmTitle(picked) : 'Remove'}
                  aria-label={bulkArmed ? removeConfirmTitle(picked) : 'Remove'}
                  onpointerup={(e) => onRemoveBulkPointerUp(e, p.id, picked)}
                  onclick={() => onRemoveBulk(p.id, picked)}
                  onkeydown={suppressRepeatKey}
                >
                  <Icon name="trash" />
                </button>
              {/if}
            </span>
          </div>
        {/if}
        {#each runs as w (w.id)}
          {@const age = formatRelativeAge(w.lastActivityAt ?? 0)}
          {@const snippet = runSnippet(w.id)}
          {@const hasDiff = !!(w.diff?.add || w.diff?.del)}
          {@const wsPR = prForWorkspace($prs, w.id)}
          {@const wsPRLife = prLifecycle(wsPR) ?? (w.state === 'merged' ? 'merged' : null)}
          {@const canRm = workspaceCanRemove(w, $workspacesRunning.has(w.id))}
          {@const rmArmed = armedRemoveId === w.id}
          {@const unseen = $attention.byWorkspace[w.id] ?? 0}
          {@const parked = parkedByWs.get(w.id) ?? 0}
          {@const unread = unseen + parked}
          {@const isPicked = !!selected[w.id]}
          {#snippet runKind()}
            {#if workspaceIsRunning(w, $workspacesRunning)}
              <span class="sd pulse" title="Agent running" aria-label="Running"></span>
            {:else if wsPRLife}
              <Icon class="br pr-{wsPRLife}" name={wsPR ? prIconName(wsPR) : 'git-merge'} />
            {:else}
              <Icon class="br" name="git-branch" />
            {/if}
          {/snippet}
          <div
            class="sc-run"
            class:can-pick={canPick}
            class:is-picked={isPicked}
            class:is-armed={rmArmed}
            role="listitem"
            onpointerenter={() => { if (canRm) void fetchRemoveRisk(w.id, false) }}
            onpointerleave={() => { if (armedRemoveId === w.id) clearArmedRemove() }}
          >
            {#if canPick}
              {#if canRm}
                <button
                  type="button"
                  class="sc-run-ico sc-run-pick"
                  class:on={isPicked}
                  aria-pressed={isPicked}
                  aria-label={isPicked ? `Deselect ${w.short || w.goal}` : `Select ${w.short || w.goal}`}
                  onclick={() => togglePick(w.id)}
                >
                  <span class="sc-run-kind">{@render runKind()}</span>
                  <span class="sc-run-check" aria-hidden="true"><Icon name="square-check" /></span>
                </button>
              {:else}
                <span class="sc-run-ico">{@render runKind()}</span>
              {/if}
            {/if}
            <button type="button" class="sc-run-open" onclick={(e) => onOpenRun(e, w.id, canPick, canRm)}>
              {#if !canPick}
                <span class="sc-run-ico">{@render runKind()}</span>
              {/if}
              <div class="sc-run-main">
                <div class="sc-run-t">
                  <span class="sc-run-name">{w.short || w.goal}</span>
                  {#if w.id === p.lastRun?.workspaceId}<span class="sc-run-last">latest</span>{/if}
                  {#if unread}<span class="wbrief-unread" title={sidebarUnreadTitle(unseen, parked)}>{unread > 9 ? '9+' : unread}</span>{/if}
                </div>
                {#if snippet}<div class="sc-run-now">{snippet}</div>{/if}
              </div>
              <span class="sc-run-diff">
                {#if hasDiff}<span class="wbrief-add">+{formatCompactCount(w.diff.add)}</span> <span class="wbrief-del">−{formatCompactCount(w.diff.del)}</span>{/if}
              </span>
            </button>
            <span class="sc-run-end">
              <span class="sc-run-when" title={age?.full}>{age?.rel || w.elapsedLabel || ''}</span>
              <button
                type="button"
                class="sc-run-rm"
                class:armed={rmArmed}
                class:off={!canRm}
                title={canRm ? (rmArmed ? removeConfirmTitle(w.id) : 'Remove') : 'Stop the workspace first'}
                aria-label={canRm ? (rmArmed ? removeConfirmTitle(w.id) : 'Remove') : 'Stop the workspace first'}
                disabled={!canRm}
                onpointerup={(e) => onRemovePointerUp(e, w.id, canRm)}
                onclick={() => onRemoveRun(w.id, canRm)}
                onkeydown={suppressRepeatKey}
              >
                <Icon name="trash" />
              </button>
            </span>
          </div>
        {/each}
      </div>
    {/if}
    </div>
  {/each}
{/snippet}

{#snippet emptyState()}
  {#if loading}
    <div class="skelrow"><div class="skel" style="width:30px;height:30px;border-radius:8px"></div><div class="skel" style="flex:1;height:14px"></div></div>
  {:else if filtered && !visible.length}
    <div class="empty">
      <div class="ei"><Icon name="filter" strokeWidth={1.8} /></div>
      <div class="et">Nothing matches these filters</div>
      <div class="ed"><button type="button" class="briefing-clear" onclick={() => { projectFilter = []; stateFilter = '' }}>Clear filters</button></div>
    </div>
  {:else if !schedules.length}
    <div class="empty">
      <div class="ei"><Icon name="clock" strokeWidth={1.8} /></div>
      <div class="et">No schedules yet</div>
      <div class="ed">Run a prompt in a fresh workspace on a cron schedule or once at a set time. Recur must be open and your Mac awake; missed runs wait for you.</div>
      <div class="ea"><button type="button" class="tbtn" disabled={busy || (!project && !$projects.length)} onclick={() => edit()}><Icon name="plus" />New schedule</button></div>
    </div>
  {/if}
{/snippet}

{#if project}
  <div class="briefing-overview sc-embedded">
    {@render emptyState()}
    {@render scheduleRows(visible)}
    {#if schedules.length}
      <div class="sc-foot"><button type="button" class="tbtn" disabled={busy || loading} onclick={() => edit()}><Icon name="plus" />New schedule</button></div>
    {/if}
    {#if error}<p class="sc-listerr" role="alert">{error}</p>{/if}
  </div>
{:else}
  <div class="briefing-body">
    <div class="briefing-col">
      <div class="briefing-filters">
        <button type="button" class="tab-strip-act" title="New schedule" aria-label="New schedule" disabled={busy || !$projects.length} onclick={() => edit()}>
          <Icon name="plus" />
        </button>
        <div class="sp"></div>
        {#if $projects.length > 1}
          <Select multiple ghost class="briefing-proj-filter" values={projectFilter} options={$projects.map((p) => ({ value: p.id, label: p.name }))}
            label="Filter by project" placeholder="All projects" onvalues={(ids) => (projectFilter = ids)} />
        {/if}
        <div class="seg" role="group" aria-label="Filter by state">
          <button type="button" class:on={stateFilter === ''} onclick={() => (stateFilter = '')}>All</button>
          <button type="button" class:on={stateFilter === 'active'} onclick={() => (stateFilter = stateFilter === 'active' ? '' : 'active')}>Active{#if schedules.some((s) => !s.paused)}<span class="cnt">{schedules.filter((s) => !s.paused).length}</span>{/if}</button>
          <button type="button" class:on={stateFilter === 'paused'} onclick={() => (stateFilter = stateFilter === 'paused' ? '' : 'paused')}>Paused{#if schedules.some((s) => s.paused)}<span class="cnt">{schedules.filter((s) => s.paused).length}</span>{/if}</button>
        </div>
      </div>
      <div class="briefing-scroll">
        <div class="briefing-overview">
          {#if error}<p class="sc-listerr" role="alert">{error}</p>{/if}
          {@render emptyState()}
          {#each grouped as g (g.id)}
            <section class="briefing-bucket">
              <div class="briefing-h"><span class="bdot" style="background:{g.color || 'var(--muted)'}"></span><span class="briefing-h-lbl">{g.name}</span><span class="briefing-h-n">{g.items.length}</span></div>
              {@render scheduleRows(g.items)}
            </section>
          {/each}
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .sc-embedded { padding: 0; border: 1px solid var(--line); border-radius: 9px; background: var(--pane); overflow: hidden; }
  .sc-embedded .ibrow { border: none; border-bottom: 1px solid var(--line-soft); border-radius: 0; margin: 0; background: none; padding: 7px 16px; }
  .sc-embedded .ibrow:hover { background: var(--pane-2); }
  .sc-foot { padding: 10px 16px; }
  /* Same rest color as the new-chat-tab plus (.wsv-chat-new). */
  .briefing-filters .tab-strip-act { color: var(--muted); }
  .sc-listerr { margin: 0; padding: 8px 16px; font-size: 12px; color: var(--coral); border-bottom: 1px solid var(--line-soft); }
  .scrow { cursor: default; }
  .scrow.can-exp { cursor: pointer; }
  .scrow.can-exp:focus-visible { outline: 2px solid var(--amber); outline-offset: -2px; }
  .ity.can-exp { position: relative; }
  .ity .sc-mode, .ity .sc-chev { grid-area: 1 / 1; display: grid; place-items: center; }
  .ity .sc-chev { display: none; transform: rotate(-90deg); }
  .scrow:hover .ity.can-exp .sc-mode { display: none; }
  .scrow:hover .ity.can-exp .sc-chev { display: grid; }
  .scrow.is-open .ity .sc-chev { transform: none; }
  /* Page (table): one meta line, status and next as fixed columns. Settings card
     (.sc-embedded, narrow): two meta lines, status stacked over next. */
  .sc-meta { display: flex; align-items: center; gap: 8px; min-width: 0; }
  .scrow .ibs { display: block; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sc-meta .ibs + .ibs::before { content: '·'; margin-right: 8px; color: var(--line-bright); }
  .sc-embedded .sc-meta { display: block; }
  .sc-embedded .sc-meta .ibs + .ibs { margin-top: 1px; }
  .sc-embedded .sc-meta .ibs + .ibs::before { content: none; margin: 0; }
  .sc-title { padding: 0; border: 0; background: none; font: inherit; color: inherit; text-align: left; cursor: pointer; }
  .sc-title:hover { text-decoration: underline; }
  .sc-cronlbl { color: var(--muted); }
  .ibs > * + *::before { content: '·'; margin: 0 8px; color: var(--line-bright); }
  .sc-side { flex: none; display: flex; align-items: center; gap: 16px; }
  .sc-embedded .sc-side { flex-direction: column; align-items: flex-end; gap: 4px; }
  .sc-status { display: flex; align-items: center; gap: 7px; width: 6.5rem; font-family: var(--mono); font-size: 10.5px; white-space: nowrap; }
  .sc-embedded .sc-status { width: auto; }
  .sc-status .d { width: 6px; height: 6px; border-radius: 50%; flex: none; }
  .sc-next { width: 12rem; text-align: right; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-variant-numeric: tabular-nums; }
  .sc-embedded .sc-next { width: auto; }
  .sc-embedded .sc-next:empty { display: none; }
  .sc-runs-n { font-size: 11px; color: var(--muted); white-space: nowrap; }
  .sc-embedded .sc-runs-n { font-size: 10.5px; }
  .sc-acts { flex: none; display: flex; align-items: center; gap: 2px; opacity: 0; transition: opacity .12s ease; }
  .scrow:hover .sc-acts, .scrow:focus-within .sc-acts { opacity: 1; }
  .sc-acts .tb-icon-btn { height: 24px; font: inherit; font-size: 10.5px; cursor: pointer; }
  .sc-acts .tb-icon-btn:disabled { opacity: .4; cursor: default; }
  .sc-del:hover:not(:disabled), .sc-confirm { color: var(--coral); }
  .sc-confirm { font-family: var(--mono); background: var(--coral-d); }
  .sc-runs { display: flex; flex-direction: column; background: color-mix(in srgb, var(--text) 3%, transparent); }
  .sc-runs-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 7px 16px;
    border-bottom: 1px solid var(--line-soft);
    font-size: 11px;
    color: var(--muted);
  }
  .sc-runs-bar.is-armed { background: var(--pane-2); }
  .sc-runs-all {
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    font: inherit;
    font-size: 11px;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .sc-runs-all:hover:not(:disabled) { color: var(--text); }
  .sc-runs-all:disabled { opacity: .4; cursor: default; }
  .sc-run {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    width: 100%;
    padding: 7px 16px;
    border-bottom: 1px solid var(--line-soft);
  }
  .sc-run.can-pick { grid-template-columns: 24px minmax(0, 1fr) auto; gap: 12px; }
  .sc-run.can-pick .sc-run-open { grid-template-columns: minmax(0, 1fr) auto; }
  .sc-run:hover, .sc-run:focus-within, .sc-run.is-armed, .sc-run.is-picked { background: var(--pane-2); }
  .sc-run.is-picked:not(:hover):not(:focus-within):not(.is-armed) {
    background: color-mix(in srgb, var(--text) 5%, transparent);
  }
  .sc-run-open {
    display: grid;
    grid-template-columns: 24px minmax(0, 1fr) auto;
    gap: 12px;
    align-items: center;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .sc-run-end { display: grid; justify-items: end; align-items: center; min-width: 5rem; min-height: 18px; }
  .sc-run-rm {
    display: none;
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--faint);
    place-items: center;
    cursor: pointer;
  }
  .sc-run-rm :global(svg) { width: 12px; height: 12px; }
  .sc-run:hover .sc-run-when, .sc-run:focus-within .sc-run-when, .sc-run.is-armed .sc-run-when { display: none; }
  .sc-run:hover .sc-run-rm, .sc-run:focus-within .sc-run-rm, .sc-run.is-armed .sc-run-rm { display: grid; }
  .sc-bulk-rm { display: grid; }
  .sc-run-rm:hover:not(:disabled):not(.armed) { background: var(--pane-2); color: var(--text); }
  .sc-run-rm.armed { background: var(--coral); color: #0A0C10; }
  .sc-run-rm.off, .sc-run-rm:disabled { opacity: .4; cursor: default; }
  .sc-run-rm.off:hover, .sc-run-rm:disabled:hover { background: transparent; color: var(--faint); }
  .sc-run-ico { width: 14px; height: 14px; display: grid; place-items: center; color: var(--faint); }
  .sc-run-pick {
    padding: 0;
    border: 0;
    background: none;
    color: var(--faint);
    cursor: pointer;
  }
  .sc-run-pick.on, .sc-run-pick:hover { color: var(--text); }
  .sc-run-pick:focus-visible { outline: 2px solid var(--amber); outline-offset: 2px; }
  .sc-run-kind, .sc-run-check { grid-area: 1 / 1; display: grid; place-items: center; }
  .sc-run-check { display: none; }
  .sc-run-pick:hover .sc-run-kind, .sc-run-pick.on .sc-run-kind { display: none; }
  .sc-run-pick:hover .sc-run-check, .sc-run-pick.on .sc-run-check { display: grid; }
  .sc-run-ico :global(svg) { width: 14px; height: 14px; }
  .sc-run-ico :global(.sd) { width: 14px; height: 14px; display: grid; place-items: center; }
  .sc-run-ico :global(.sd)::before {
    content: ""; width: 6px; height: 6px; border-radius: 50%;
    background: var(--amber); box-shadow: 0 0 5px var(--amber);
  }
  .sc-run-ico :global(.br.pr-open) { color: var(--emerald); }
  .sc-run-ico :global(.br.pr-draft) { color: var(--cyan); }
  .sc-run-ico :global(.br.pr-merged) { color: var(--indigo); }
  .sc-run-ico :global(.br.pr-closed) { color: var(--coral); }
  .sc-run-main { min-width: 0; }
  .sc-run-t { display: flex; align-items: center; gap: 8px; min-width: 0; font-size: 13px; font-weight: 600; color: var(--text); }
  .sc-run-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sc-run-t :global(.wbrief-unread) { margin-left: 0; }
  .sc-run-last { font-family: var(--mono); font-size: 10.5px; font-weight: 400; color: var(--faint); }
  .sc-run-now { font-size: 12px; font-weight: 400; color: var(--muted); margin-top: 1px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sc-run-diff { font-family: var(--mono); font-size: 10.5px; white-space: nowrap; text-align: right; }
  .sc-run-when { font-family: var(--mono); font-size: 10.5px; color: var(--faint); text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
</style>
