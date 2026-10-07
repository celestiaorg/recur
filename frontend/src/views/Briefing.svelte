<script lang="ts">
  import { attention, briefing, briefingFocus, go, inbox, inboxByWorkspace, inboxChatOpen, setInboxChatOpen, routeTarget, view } from '../lib/stores'
  import { markInboxItemRead } from '../lib/inbox-read'
  import type { InboxItem } from '../../bindings/recur/internal/domain/models'
  import WorkspaceBriefCard from '../components/WorkspaceBriefCard.svelte'
  import AppChat from '../components/AppChat.svelte'
  import Icon, { type IconName } from '../components/Icon.svelte'
  import Select from '../components/Select.svelte'
  import ResizeHandle from '../components/ResizeHandle.svelte'

  const FILTERS_KEY = 'recur.briefingFilters'
  const WIDE_MIN = 1100

  let wide = $state(typeof window !== 'undefined' ? window.innerWidth >= WIDE_MIN : true)

  $effect(() => {
    const onResize = () => { wide = window.innerWidth >= WIDE_MIN }
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  })

  const PICKUP_PREVIEW = 8

  const rowIcons: Record<string, IconName> = {
    pause: 'circle-pause',
    pr: 'git-pull-request',
    loop: 'refresh-cw',
    merge: 'check',
    chat: 'message-code',
  }

  type BucketFilter = '' | 'needs' | 'working' | 'pickup'
  type SavedFilters = { unreadOnly?: boolean; project?: string; projects?: string[] }
  // The unread/project scope is a standing preference (like the Ask dock);
  // the bucket chip is transient navigation and deliberately not persisted.
  const savedFilters = ((): { unreadOnly: boolean; projects: string[] } => {
    try {
      const saved = JSON.parse(localStorage.getItem(FILTERS_KEY) || '{}') as SavedFilters
      const projects = Array.isArray(saved.projects)
        ? saved.projects.filter((id): id is string => typeof id === 'string' && id !== '')
        : saved.project ? [saved.project] : []
      return { unreadOnly: !!saved.unreadOnly, projects }
    } catch {
      return { unreadOnly: false, projects: [] }
    }
  })()
  let bucketFilter = $state<BucketFilter>('')
  let unreadOnly = $state(savedFilters.unreadOnly)
  let projectFilter = $state<string[]>(savedFilters.projects)

  $effect(() => {
    try { localStorage.setItem(FILTERS_KEY, JSON.stringify({ unreadOnly, projects: projectFilter })) } catch { /* ignore */ }
  })

  const notesBy = $derived($inboxByWorkspace)

  // Gmail model, one list: workspace cards are the threads, their unread
  // notifications render inside them, and label-style filters narrow the list.
  // Bucket chips keep their counts under the project/unread filters so numbers
  // stay stable while toggling buckets.
  const projectSet = $derived(new Set(projectFilter))
  const briefs = $derived($briefing.workspaces.filter((w) =>
    (!projectFilter.length || (!!w.projectId && projectSet.has(w.projectId))) &&
    (!unreadOnly || (notesBy[w.workspaceId]?.length ?? 0) > 0)))
  const needsAll = $derived(briefs.filter((w) => w.bucket === 'needs'))
  const workingAll = $derived(briefs.filter((w) => w.bucket === 'working'))
  const pickupAll = $derived(briefs.filter((w) => w.bucket === 'pickup'))
  const needs = $derived(bucketFilter && bucketFilter !== 'needs' ? [] : needsAll)
  const working = $derived(bucketFilter && bucketFilter !== 'working' ? [] : workingAll)
  const pickup = $derived(bucketFilter && bucketFilter !== 'pickup' ? [] : pickupAll)
  const filtered = $derived(!!bucketFilter || unreadOnly || projectFilter.length > 0)
  const unread = $derived($inbox.filter((i) => i.unread).length)

  // Notifications with no card to live on (no workspace, or one the Overview
  // does not surface) — merged PRs, host-PR chats, app-level events.
  const orphans = $derived.by(() => {
    if (bucketFilter || projectFilter.length) return []
    const carded = new Set($briefing.workspaces.map((w) => w.workspaceId))
    return $inbox.filter((i) => (!i.workspaceId || !carded.has(i.workspaceId)) && (!unreadOnly || i.unread))
  })

  let pickupExpanded = $state(false)
  const pickupShown = $derived(pickupExpanded || bucketFilter === 'pickup' ? pickup : pickup.slice(0, PICKUP_PREVIEW))

  const projectOptions = $derived.by(() => {
    const seen = new Map<string, string>()
    for (const w of $briefing.workspaces) {
      if (w.projectId && !seen.has(w.projectId)) seen.set(w.projectId, w.projectName || w.projectId)
    }
    return [...seen].map(([value, label]) => ({ value, label }))
  })

  // A remembered project that no longer has cards would be an invisible
  // filter (the Select hides itself below two projects) — drop it.
  $effect(() => {
    if (!projectFilter.length || !$briefing.workspaces.length) return
    const known = new Set(projectOptions.map((o) => o.value))
    const next = projectFilter.filter((id) => known.has(id))
    if (next.length !== projectFilter.length) projectFilter = next
  })

  function clearFilters() {
    bucketFilter = ''
    unreadOnly = false
    projectFilter = []
  }

  function openRow(x: InboxItem) {
    markInboxItemRead(x.id)
    routeTarget(x.target)
  }

  $effect(() => {
    const focus = $briefingFocus
    if (!focus) return
    // A footer/palette jump must be able to reach its section: drop the
    // filters that could hide it (the project scope is a deliberate choice).
    bucketFilter = ''
    unreadOnly = false
    queueMicrotask(() => {
      document.getElementById(`briefing-${focus}`)?.scrollIntoView({ block: 'start', behavior: 'smooth' })
      briefingFocus.set('')
    })
  })
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape' && $inboxChatOpen && !wide && $view === 'briefing') {
      setInboxChatOpen(false)
      e.preventDefault()
    }
  }}
/>

<section class="view briefing" class:on={$view === 'briefing'} id="v-briefing">
  <div class="briefing-body" class:ask-open={$inboxChatOpen}>
    {#if $inboxChatOpen}
      <aside class="briefing-ask" class:drawer={!wide}>
        <AppChat visible={$inboxChatOpen && $view === 'briefing'} />
        {#if wide}
          <ResizeHandle panel="inboxChat" edge="end" />
        {/if}
      </aside>
    {/if}
    <div class="briefing-col">
      <div class="briefing-filters">
        {#if !$inboxChatOpen}
          <button
            type="button"
            class="obs-pane-toggle briefing-ask-reopen"
            onclick={() => setInboxChatOpen(true)}
            title="Show chat panel"
            aria-label="Show chat panel"
            aria-pressed={false}
          >
            <Icon name="panel-left" />
            {#if $attention.appUnseen}<span class="ask-unseen-dot" title="Unseen reply"></span>{/if}
          </button>
        {/if}
        <div class="sp"></div>
        {#if projectOptions.length > 1}
          <Select
            multiple
            ghost
            class="briefing-proj-filter"
            values={projectFilter}
            options={projectOptions}
            label="Filter by project"
            placeholder="All projects"
            onvalues={(ids) => (projectFilter = ids)}
          />
        {/if}
        <div class="seg" role="group" aria-label="Filter by read state">
          <button type="button" class:on={unreadOnly} aria-pressed={unreadOnly} onclick={() => (unreadOnly = !unreadOnly)}>Unread{#if unread}<span class="cnt">{unread}</span>{/if}</button>
        </div>
        <div class="seg" role="group" aria-label="Filter by bucket">
          <button type="button" class:on={bucketFilter === ''} onclick={() => (bucketFilter = '')}>All</button>
          <button type="button" class:on={bucketFilter === 'needs'} onclick={() => (bucketFilter = bucketFilter === 'needs' ? '' : 'needs')}>Needs you{#if needsAll.length}<span class="cnt">{needsAll.length}</span>{/if}</button>
          <button type="button" class:on={bucketFilter === 'working'} onclick={() => (bucketFilter = bucketFilter === 'working' ? '' : 'working')}>Working{#if workingAll.length}<span class="cnt">{workingAll.length}</span>{/if}</button>
          <button type="button" class:on={bucketFilter === 'pickup'} onclick={() => (bucketFilter = bucketFilter === 'pickup' ? '' : 'pickup')}>Pick up{#if pickupAll.length}<span class="cnt">{pickupAll.length}</span>{/if}</button>
        </div>
      </div>
      <div class="briefing-scroll">
        <div class="briefing-overview">
          {#if !needs.length && !working.length && !pickup.length && !orphans.length}
            {#if filtered}
              <div class="empty">
                <div class="ei"><Icon name="filter" strokeWidth={1.8} /></div>
                <div class="et">Nothing matches these filters</div>
                <div class="ed"><button type="button" class="briefing-clear" onclick={clearFilters}>Clear filters</button></div>
              </div>
            {:else}
              <div class="empty">
                <div class="ei"><Icon name="inbox-empty" strokeWidth={1.8} /></div>
                <div class="et">Nothing in flight</div>
                <div class="ed">When a workspace needs you, is working, or is ready to pick up, it shows up here with its notifications.</div>
              </div>
            {/if}
          {/if}
          {#if needs.length}
            <section class="briefing-bucket" id="briefing-needs">
              <div class="briefing-h"><span class="bdot" style="background:var(--coral)"></span><span class="briefing-h-lbl">Needs you</span><span class="briefing-h-n">{needs.length}</span></div>
              {#each needs as b (b.workspaceId)}
                <WorkspaceBriefCard brief={b} notes={notesBy[b.workspaceId] ?? []} compact />
              {/each}
            </section>
          {/if}
          {#if working.length}
            <section class="briefing-bucket" id="briefing-working">
              <div class="briefing-h"><span class="bdot" style="background:var(--amber)"></span><span class="briefing-h-lbl">Working</span><span class="briefing-h-n">{working.length}</span></div>
              {#each working as b (b.workspaceId)}
                <WorkspaceBriefCard brief={b} notes={notesBy[b.workspaceId] ?? []} compact />
              {/each}
            </section>
          {/if}
          {#if pickup.length}
            <section class="briefing-bucket" id="briefing-pickup">
              <div class="briefing-h"><span class="bdot" style="background:var(--indigo)"></span><span class="briefing-h-lbl">Pick up</span><span class="briefing-h-n">{pickup.length}</span></div>
              {#each pickupShown as b (b.workspaceId)}
                <WorkspaceBriefCard brief={b} notes={notesBy[b.workspaceId] ?? []} compact />
              {/each}
              {#if pickupShown.length < pickup.length || (pickupExpanded && pickup.length > PICKUP_PREVIEW)}
                <button type="button" class="briefing-more" onclick={() => (pickupExpanded = !pickupExpanded)}>
                  {pickupExpanded ? 'Show fewer' : `Show all ${pickup.length}`}
                </button>
              {/if}
            </section>
          {/if}
          {#if orphans.length}
            <section class="briefing-bucket briefing-elsewhere" id="briefing-elsewhere">
              <div class="briefing-h"><span class="bdot" style="background:var(--faint)"></span><span class="briefing-h-lbl">Elsewhere</span><span class="briefing-h-n">{orphans.length}</span></div>
              {#each orphans as x (x.id)}
                <div class="ibrow" class:read={!x.unread} role="button" tabindex="0" onclick={() => openRow(x)} onkeydown={(e) => e.key === 'Enter' && openRow(x)}>
                  <span class="udot"></span>
                  <div class="ity" style="background:color-mix(in srgb, {x.color} 14%, transparent);color:{x.color}">{#if rowIcons[x.kind]}<Icon name={rowIcons[x.kind]} />{/if}</div>
                  <div class="ibmain"><div class="ibt">{@html x.title}</div><div class="ibs">{x.sub}</div></div>
                  <span class="ibtime">{x.time}</span>
                </div>
              {/each}
            </section>
          {/if}
        </div>
      </div>
    </div>
  </div>
</section>
