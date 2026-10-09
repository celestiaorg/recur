<script lang="ts">
  import { onMount } from 'svelte'
  import { SettingsService } from '../../bindings/recur/internal/app'
  import type { AssignedIssue } from '../../bindings/recur/internal/app/models'
  import { formatRelativeAge } from '../lib/format'
  import { openExternalUrl } from '../lib/open-external'
  import Icon from './Icon.svelte'
  import PRPeopleStack from './PRPeopleStack.svelte'
  import Select from './Select.svelte'
  import { PR_LIST_SORTS } from '../lib/pr-list-filter'
  import { issuesStale, projects, openNewTask, openWorkspace, workspaces } from '../lib/stores'

  const GROUPS = [
    { id: 'progress', label: 'In progress', color: 'var(--amber)' },
    { id: 'todo', label: 'Todo', color: 'var(--emerald)' },
    { id: 'backlog', label: 'Backlog', color: 'var(--faint)' },
  ]
  const SEGS = [
    { id: 'all', label: 'All' },
    { id: 'github', label: 'GitHub' },
    { id: 'linear', label: 'Linear' },
  ]
  const OWNER_SEGS = [
    { id: 'all', label: 'All' },
    { id: 'mine', label: 'Mine' },
    { id: 'unassigned', label: 'Unassigned' },
  ]
  const SORTS = [...PR_LIST_SORTS.slice(0, 4), { id: 'priority', label: 'Priority', icon: 'sev-high' as const }, PR_LIST_SORTS[4]]
  const PRIORITY_COLOR: Record<number, string> = { 1: 'var(--coral)', 2: 'var(--amber)' }

  let issues = $state.raw<AssignedIssue[]>([])
  let loading = $state(true)
  let seg = $state('all')
  let owner = $state('all')
  let groupIds = $state<string[]>([])
  let sort = $state('updated')
  let projectIds = $state<string[]>([])
  const LAYOUT_KEY = 'recur.issuesLayout'
  let layout = $state(localStorage.getItem(LAYOUT_KEY) === 'board' ? 'board' : 'list')

  const hasLinear = $derived(issues.some((i) => i.source === 'linear'))
  const hasGithub = $derived(issues.some((i) => i.source === 'github'))
  const UNLINKED = '__unlinked'
  const projectOptions = $derived.by(() => {
    const ids = new Set(issues.map((i) => i.projectId || UNLINKED))
    const opts = $projects.filter((p) => ids.has(p.id)).map((p) => ({ value: p.id, label: p.name || p.id }))
    return ids.has(UNLINKED) ? [...opts, { value: UNLINKED, label: 'Unlinked' }] : opts
  })
  const visible = $derived(
    issues.filter((i) =>
      (seg === 'all' || i.source === seg)
      && (owner === 'all' || (owner === 'mine') === !!i.assignees?.length)
      && (projectIds.length === 0 || projectIds.includes(i.projectId || UNLINKED))),
  )
  // One column per status (merged by name across teams), in list order:
  // In progress → Todo → Backlog, furthest Linear workflow position first.
  const columns = $derived.by(() => {
    const cols = new Map<string, { id: string; label: string; color: string; rank: number; pos: number; items: AssignedIssue[] }>()
    for (const i of visible) {
      const rank = Math.max(0, GROUPS.findIndex((g) => g.id === (i.group || 'todo')))
      if (groupIds.length && !groupIds.includes(GROUPS[rank].id)) continue
      const label = i.state || GROUPS[rank].label
      const id = `${rank}:${label}`
      let c = cols.get(id)
      if (!c) cols.set(id, (c = { id, label, color: i.source === 'github' ? 'var(--emerald)' : hex(i.stateColor), rank, pos: i.statePosition, items: [] }))
      c.pos = Math.min(c.pos, i.statePosition)
      c.items.push(i)
    }
    return [...cols.values()]
      .sort((a, b) => a.rank - b.rank || b.pos - a.pos || a.label.localeCompare(b.label))
      .map((c) => ({ ...c, items: c.items.sort(compare) }))
  })
  const grouped = $derived(columns.filter((g) => g.items.length))

  function setLayout(v: string) {
    layout = v
    localStorage.setItem(LAYOUT_KEY, v)
  }

  const ms = (t: string) => Date.parse(t) || 0
  const rank = (p: number) => p || 5

  function compare(a: AssignedIssue, b: AssignedIssue): number {
    switch (sort) {
      case 'updated-asc': return ms(a.updatedAt) - ms(b.updatedAt)
      case 'newest': return ms(b.createdAt) - ms(a.createdAt)
      case 'oldest': return ms(a.createdAt) - ms(b.createdAt)
      case 'priority': return rank(a.priority) - rank(b.priority) || ms(b.updatedAt) - ms(a.updatedAt)
      case 'title': return a.title.localeCompare(b.title)
    }
    return ms(b.updatedAt) - ms(a.updatedAt)
  }

  async function openIssue(i: AssignedIssue) {
    if (i.source === 'linear' && (await SettingsService.OpenLinearInApp(i.url).catch(() => false))) return
    await openExternalUrl(i.url)
  }

  function startWorkspace(i: AssignedIssue) {
    const text = `/${i.source} ${i.ref}`
    const linearKey = i.source === 'linear' && !i.projectId ? i.linearProject || i.linearTeam : undefined
    openNewTask(i.projectId, { text, linearKey })
  }

  function hex(c: string): string {
    return c ? (c.startsWith('#') ? c : `#${c}`) : 'var(--faint)'
  }

  function load() {
    loading = true
    SettingsService.ListAssignedIssues()
      .then((rows) => { issues = rows ?? [] })
      .catch(() => {})
      .finally(() => { loading = false })
  }

  onMount(() => issuesStale.subscribe(load))
</script>

{#snippet ref(i: AssignedIssue)}
  <span class="pr-num">
    <span class="pr-src" title={i.source === 'github' ? 'GitHub issue' : 'Linear issue'}><Icon name={i.source === 'github' ? 'github' : 'linear'} /></span>
    {i.source === 'github' ? `#${i.ref.split('#').pop()}` : i.ref}
  </span>
{/snippet}

{#snippet sub(i: AssignedIssue)}
  {@const linked = $projects.find((p) => p.id === i.projectId)}
  <div class="pr-sub">
    {#if linked}<span class="iss-proj" title="Recur project">{linked.name || linked.id}</span>{:else}<span title="Not linked to a Recur project">{i.project}</span>{/if}
    {#if i.comments > 0}<span>· {i.comments} {i.comments === 1 ? 'comment' : 'comments'}</span>{/if}
    {#each i.labels ?? [] as l (l.name)}
      <span class="iss-label"><span class="iss-ldot" style:background={hex(l.color)}></span>{l.name}</span>
    {/each}
  </div>
{/snippet}

{#snippet people(i: AssignedIssue)}
  {#if i.assignees?.length}
    <PRPeopleStack people={i.assignees} />
  {:else}
    <span class="iss-unassigned" title="Unassigned" aria-label="Unassigned"><Icon name="circle-user-round" size={20} strokeWidth={1.5} /></span>
  {/if}
{/snippet}

{#snippet action(i: AssignedIssue)}
  {@const ws = $workspaces.find((w) => w.issue?.source === i.source && w.issue.id === i.id)}
  {#if ws}
    <button
      type="button"
      class="tbtn iss-start"
      title="View this issue's workspace"
      onclick={(e) => { e.stopPropagation(); openWorkspace(ws.id) }}
      onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') e.stopPropagation() }}
    ><Icon name="box" />View workspace</button>
  {:else}
    <button
      type="button"
      class="tbtn iss-start"
      title="Create a Recur workspace for this issue"
      onclick={(e) => { e.stopPropagation(); startWorkspace(i) }}
      onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') e.stopPropagation() }}
    ><Icon name="plus" />Create workspace</button>
  {/if}
{/snippet}

<div class="briefing-body">
  <div class="briefing-col">
    <div class="briefing-filters">
      {#if loading && issues.length > 0}
        <span class="tb-refresh" role="status" aria-live="polite"><Icon name="loader" class="spin" />Refreshing</span>
      {/if}
      <div class="sp"></div>
      {#if projectOptions.length > 1}
        <Select
          multiple
          ghost
          class="briefing-proj-filter"
          values={projectIds}
          options={projectOptions}
          label="Filter by project"
          placeholder="All projects"
          onvalues={(ids) => (projectIds = ids)}
        />
      {/if}
      <Select
        multiple
        ghost
        class="iss-group-filter"
        values={groupIds}
        options={GROUPS.map((g) => ({ value: g.id, label: g.label }))}
        label="Filter by group"
        placeholder="Any status"
        onvalues={(ids) => (groupIds = ids)}
      />
      <Select
        ghost
        value={sort}
        options={SORTS.map((s) => ({ value: s.id, label: s.label }))}
        label="Sort issues"
        onchange={(id) => (sort = id)}
      >
        {#snippet icon()}
          <Icon name={SORTS.find((s) => s.id === sort)?.icon ?? 'clock-arrow-down'} />
        {/snippet}
      </Select>
      <button type="button" class="tbtn" onclick={load} disabled={loading} title="Refresh issues" aria-label="Refresh issues"><Icon name="refresh-cw" /></button>
      <div class="seg" role="group" aria-label="Filter issues by assignee">
        {#each OWNER_SEGS as s (s.id)}
          <button type="button" class:on={owner === s.id} onclick={() => (owner = s.id)}>{s.label}</button>
        {/each}
      </div>
      {#if hasLinear && hasGithub}
        <div class="seg" role="group" aria-label="Filter issues by source">
          {#each SEGS as s (s.id)}
            <button type="button" class:on={seg === s.id} onclick={() => (seg = s.id)}>{s.label}</button>
          {/each}
        </div>
      {/if}
      <div class="seg iss-layout" role="group" aria-label="Issues layout">
        <button type="button" class:on={layout === 'list'} onclick={() => setLayout('list')} title="List" aria-label="List view"><Icon name="rows-3" /></button>
        <button type="button" class:on={layout === 'board'} onclick={() => setLayout('board')} title="Board" aria-label="Board view"><Icon name="columns-3" /></button>
      </div>
    </div>
    {#if layout === 'board' && grouped.length > 0}
      <div class="iss-board">
        {#each columns.toReversed() as g (g.id)}
          <section class="iss-col">
            <div class="briefing-h"><span class="bdot" style="background:{g.color}"></span><span class="briefing-h-lbl">{g.label}</span><span class="briefing-h-n">{g.items.length}</span></div>
            <div class="iss-col-body">
              {#each g.items as i (i.url)}
                {@const tint = i.source === 'github' ? 'var(--emerald)' : hex(i.stateColor)}
                <div class="iss-card" role="button" tabindex="0" onclick={() => void openIssue(i)} onkeydown={(e) => e.key === 'Enter' && void openIssue(i)}>
                  <div class="pr-main">
                    <div class="pr-t">
                      {@render ref(i)}
                      <span class="sp"></span>
                      <span class="iss-card-state" style:color={tint}>{i.state}</span>
                    </div>
                    <div class="iss-card-title">{i.title}</div>
                    {@render sub(i)}
                  </div>
                  <div class="iss-card-foot">
                    {@render people(i)}
                    {#if i.priority > 0}<span class="iss-prio" style:color={PRIORITY_COLOR[i.priority]}>{i.priorityLabel}</span>{/if}
                    <span class="pr-when">{formatRelativeAge(Date.parse(i.updatedAt))?.rel ?? ''}</span>
                    <span class="sp"></span>
                    {@render action(i)}
                  </div>
                </div>
              {/each}
            </div>
          </section>
        {/each}
      </div>
    {:else}
    <div class="briefing-scroll">
      <div class="briefing-overview prlist">
        {#if loading && issues.length === 0}
          {#each [0, 1, 2, 3] as n (n)}
            <div class="skelrow"><div class="skel" style="width:32px;height:32px;border-radius:8px"></div><div class="skel" style="flex:1;height:14px"></div></div>
          {/each}
        {:else if issues.length === 0}
          <div class="empty">
            <div class="ei"><Icon name="circle-dot" strokeWidth={1.8} /></div>
            <div class="et">No open issues</div>
            <div class="ed">Open GitHub issues (via <code>gh auth login</code>) and Linear issues (via an API key in Settings) assigned to you, or unassigned in your project repos and Linear teams, show up here.</div>
          </div>
        {:else if grouped.length === 0}
          <div class="empty"><div class="et">Nothing here</div><div class="ed">No issues match this filter.</div></div>
        {:else}
          {#each grouped as g (g.id)}
            <section class="briefing-bucket">
              <div class="briefing-h"><span class="bdot" style="background:{g.color}"></span><span class="briefing-h-lbl">{g.label}</span><span class="briefing-h-n">{g.items.length}</span></div>
              {#each g.items as i (i.url)}
                {@const tint = i.source === 'github' ? 'var(--emerald)' : hex(i.stateColor)}
                <div class="prrow iss" role="button" tabindex="0" onclick={() => void openIssue(i)} onkeydown={(e) => e.key === 'Enter' && void openIssue(i)}>
                  <div class="pr-ico" style="background:color-mix(in srgb, {tint} 12%, transparent);color:{tint}">
                    <Icon name="circle-dot" />
                  </div>
                  <div class="pr-main">
                    <div class="pr-t">
                      {@render ref(i)}
                      <span class="pr-t-text">{i.title}</span>
                    </div>
                    {@render sub(i)}
                  </div>
                  {@render people(i)}
                  <div class="pr-checks"><span class="d" style="background:{tint};box-shadow:0 0 5px {tint}"></span><span style="color:{tint}">{i.state}</span></div>
                  <div class="iss-prio" style:color={PRIORITY_COLOR[i.priority]}>{i.priority > 0 ? i.priorityLabel : ''}</div>
                  <div class="pr-when">{formatRelativeAge(Date.parse(i.updatedAt))?.rel ?? ''}</div>
                  {@render action(i)}
                </div>
              {/each}
            </section>
          {/each}
        {/if}
      </div>
    </div>
    {/if}
  </div>
</div>

<style>
  .iss {
    grid-template-columns: 26px minmax(0, 1fr) 4rem 9.75rem 4.5rem 2.75rem auto;
    grid-template-areas: "ico main people status prio when act";
  }
  .iss-prio {
    grid-area: prio;
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--faint);
    text-align: right;
    white-space: nowrap;
  }
  .iss .pr-sub { overflow: hidden; }
  .iss-proj { color: var(--muted); }
  .iss-unassigned { grid-area: people; display: flex; color: var(--faint); }
  .iss-start { grid-area: act; }
  .iss-label {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    white-space: nowrap;
  }
  .iss-ldot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
  }
  .iss-layout button { display: inline-flex; align-items: center; }
  .iss-layout :global(svg) { width: 13px; height: 13px; }
  .iss-board {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-auto-flow: column;
    grid-auto-columns: minmax(325px, 375px);
    grid-template-rows: minmax(0, 1fr);
    justify-content: start;
    gap: 12px;
    padding: 12px 16px;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .iss-col {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
    min-height: 0;
  }
  .iss-col .briefing-h { flex: none; position: static; }
  .iss-col-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-bottom: 16px;
  }
  .iss-col-body > .iss-card { flex: none; }
  .iss-col .briefing-h { border: 1px solid var(--line-soft); border-radius: 6px; }
  .iss-card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--line-soft);
    border-radius: 8px;
    background: var(--pane);
    cursor: pointer;
  }
  .iss-card:hover { background: var(--pane-2); border-color: var(--line); }
  .iss-card:focus-visible { outline: 2px solid var(--amber); outline-offset: -2px; }
  .iss-card .pr-sub { flex-wrap: wrap; row-gap: 4px; }
  .iss-card .sp { flex: 1; }
  .iss-card-state { font-family: var(--mono); font-size: 10.5px; white-space: nowrap; }
  .iss-card-title {
    font-size: 13px;
    line-height: 1.4;
    margin-top: 6px;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .iss-card-foot { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .iss-card-foot .iss-start { height: 24px; padding: 0 8px; }
</style>
