<script lang="ts">
  import { view, openPR, projects, prListSeg, prListFacets, prListProjectIds, prListSort, prListScoped, PR_LIST_SEGS, PR_LIST_SORTS } from '../lib/stores'
  import { prChromeColor, prGroupColor, prGroupLabel, prIconName, prStatusLabel } from '../lib/colors'
  import { prAttentionLabels, prChecksRollup, prHasCheckStatus } from '../lib/pr-attention'
  import { PR_LIST_FACETS, PR_LIST_GROUP_ORDER, prListGroup, prMatchesListFilter, sortPRList, type PRListSort } from '../lib/pr-list-filter'
  import type { PR } from '../lib/types'
  import Icon from '../components/Icon.svelte'
  import Select from '../components/Select.svelte'
  import PRPeopleStack from '../components/PRPeopleStack.svelte'

  const scoped = $derived($prListScoped)
  const gitProjects = $derived($projects.filter((p) => p.isGit && p.id !== 'files'))
  const showProjectFilter = $derived(gitProjects.length > 1)
  const projectOptions = $derived(gitProjects.map((p) => ({ value: p.id, label: p.name || p.id })))
  const sortOptions = PR_LIST_SORTS.map((s) => ({ value: s.id, label: s.label }))
  const sortIcon = $derived(PR_LIST_SORTS.find((s) => s.id === $prListSort)?.icon ?? 'clock-arrow-down')
  const visible = $derived(scoped.filter((p) => prMatchesListFilter(p, $prListSeg, $prListFacets)))
  const grouped = $derived.by(() => {
    const by: Record<string, typeof visible> = {}
    for (const p of visible) {
      const g = prListGroup(p)
      ;(by[g] ??= []).push(p)
    }
    const rest = Object.keys(by).filter((g) => !(PR_LIST_GROUP_ORDER as readonly string[]).includes(g))
    return [...PR_LIST_GROUP_ORDER, ...rest]
      .filter((g) => by[g]?.length)
      .map((g) => ({ g, items: sortPRList(by[g] ?? [], $prListSort) }))
  })

  function projectName(id: string | undefined): string {
    const pid = (id ?? '').trim()
    if (!pid) return ''
    return $projects.find((p) => p.id === pid)?.name || pid
  }

  function rowSub(p: PR): string {
    const proj = projectName(p.projectId)
    const loc = p.workspaceId || proj
    const bits = [loc, p.branch].filter(Boolean)
    return bits.join(' · ')
  }

</script>

<section class="view briefing" class:on={$view === 'pr'} id="v-pr" aria-label="Pull requests">
  <div class="briefing-body">
    <div class="briefing-col">
      <div class="briefing-filters">
        <div class="sp"></div>
        {#if showProjectFilter}
          <Select
            multiple
            ghost
            class="briefing-proj-filter"
            values={$prListProjectIds}
            options={projectOptions}
            label="Filter by project"
            placeholder="All projects"
            onvalues={(ids) => prListProjectIds.set(ids)}
          />
        {/if}
        <Select
          multiple
          ghost
          class="pr-status-filter"
          values={$prListFacets}
          options={PR_LIST_FACETS}
          label="Filter by status"
          placeholder="Any status"
          onvalues={(ids) => prListFacets.set(ids)}
        />
        <Select
          ghost
          class="pr-sort"
          value={$prListSort}
          options={sortOptions}
          label="Sort pull requests"
          onchange={(id) => prListSort.set(id as PRListSort)}
        >
          {#snippet icon()}
            <Icon name={sortIcon} />
          {/snippet}
        </Select>
        <div class="seg" role="group" aria-label="Filter pull requests">
          {#each PR_LIST_SEGS as s (s.id)}
            <button type="button" class:on={$prListSeg === s.id} onclick={() => prListSeg.set(s.id)}>{s.label}</button>
          {/each}
        </div>
      </div>
      <div class="briefing-scroll">
        <div class="briefing-overview prlist">
          {#if grouped.length === 0}
            <div class="empty"><div class="et">Nothing here</div><div class="ed">No pull requests match this filter.</div></div>
          {:else}
            {#each grouped as { g, items } (g)}
              <section class="briefing-bucket">
                <div class="briefing-h"><span class="bdot" style="background:{prGroupColor[g]}"></span><span class="briefing-h-lbl">{prGroupLabel[g]}</span><span class="briefing-h-n">{items.length}</span></div>
                {#each items as p (p.id)}
                  {@const tint = prChromeColor(p)}
                  {@const why = prAttentionLabels(p)}
                  {@const showChecks = prHasCheckStatus(p)}
                  {@const checks = prChecksRollup(p)}
                  <div class="prrow" class:needs={g === 'you'} role="button" tabindex="0" onclick={() => openPR(p.id)} onkeydown={(e) => e.key === 'Enter' && openPR(p.id)}>
                    <div class="pr-ico" style="background:color-mix(in srgb, {tint} 12%, transparent)">
                      <span style="color:{tint}"><Icon name={prIconName(p)} /></span>
                    </div>
                    <div class="pr-main">
                      <div class="pr-t">
                        <span class="pr-num">
                          <span class="pr-src" title={p.source === 'gh' ? 'GitHub PR' : 'Local PR'}>
                            {#if p.source === 'gh'}
                              <Icon name="github" />
                            {:else}
                              <Icon name="hard-drive" />
                            {/if}
                          </span>
                          #{p.number}
                        </span>
                        <span class="pr-t-text">{p.title}</span>
                        {#if p.source === 'gh' && p.url}
                          <a
                            class="pr-out"
                            href={p.url}
                            title="Open in GitHub"
                            aria-label="Open in GitHub"
                            onclick={(e) => e.stopPropagation()}
                            onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') e.stopPropagation() }}
                          ><Icon name="queue-pop-out" /></a>
                        {/if}
                      </div>
                      <div class="pr-sub">{rowSub(p)}</div>
                    </div>
                    <div class="pr-under">
                      <PRPeopleStack pr={p} />
                      <div class="pr-why">{why.join(' · ')}</div>
                    </div>
                    <div class="pr-checks"><span class="d" style="background:{tint};box-shadow:0 0 5px {tint}"></span><span style="color:{tint}">{prStatusLabel(p)}</span></div>
                    <div class="pr-diff"><span class="a">+{p.diff.add}</span><span class="r">−{p.diff.del}</span></div>
                    <div class="pr-files">{p.fileCount} {p.fileCount === 1 ? 'file' : 'files'}</div>
                    <div class="pr-ci">
                      {#if showChecks && checks.kind !== 'none'}
                        <span
                          class="pr-ci-mark"
                          class:ok={checks.kind === 'pass'}
                          class:fail={checks.kind === 'fail'}
                          class:pending={checks.kind === 'pending'}
                          class:spin={checks.kind === 'pending'}
                          title={checks.kind === 'fail' ? 'Checks failing' : checks.kind === 'pending' ? 'Checks in progress' : 'Checks passing'}
                        >
                          <Icon name={checks.icon} strokeWidth={2.4} />
                        </span>
                      {/if}
                    </div>
                    <div class="pr-when">{p.when}</div>
                  </div>
                {/each}
              </section>
            {/each}
          {/if}
        </div>
      </div>
    </div>
  </div>
</section>
