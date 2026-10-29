<script lang="ts">
  import { briefing, inbox, inboxByWorkspace, view } from '../lib/stores'
  import WorkspaceBriefCard from '../components/WorkspaceBriefCard.svelte'
  import Icon from '../components/Icon.svelte'
  import Select from '../components/Select.svelte'

  type BucketFilter = '' | 'needs' | 'working' | 'pickup'
  let bucketFilter = $state<BucketFilter>('')
  let unreadOnly = $state(false)
  let projectFilter = $state<string[]>([])

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

  const projectOptions = $derived.by(() => {
    const seen = new Map<string, string>()
    for (const w of $briefing.workspaces) {
      if (w.projectId && !seen.has(w.projectId)) seen.set(w.projectId, w.projectName || w.projectId)
    }
    return [...seen].map(([value, label]) => ({ value, label }))
  })

  function clearFilters() {
    bucketFilter = ''
    unreadOnly = false
    projectFilter = []
  }

</script>

<section class="view briefing" class:on={$view === 'briefing'} id="v-briefing">
  <div class="briefing-body">
    <div class="briefing-col">
      <div class="briefing-filters">
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
          {#if !needs.length && !working.length && !pickup.length}
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
              {#each pickup as b (b.workspaceId)}
                <WorkspaceBriefCard brief={b} notes={notesBy[b.workspaceId] ?? []} compact />
              {/each}
            </section>
          {/if}
        </div>
      </div>
    </div>
  </div>
</section>
