<script lang="ts">
  import {
    projects, workspaces, prs, inbox, attention, view, selectedWorkspaceId, go, openWorkspace, openPR,
    navKey, workspacesRunning,
  } from '../lib/stores'
  import type { Project, Workspace } from '../lib/types'
  import { formatCompactCount } from '../lib/format'
  import { prForWorkspace, prIconName, prLifecycle } from '../lib/colors'
  import { workspaceIsRunning, parkedNeedsYouByWorkspace, sidebarUnreadTitle } from '../lib/modes'
  import { nestWorkspaces, isNestedChild } from '../lib/workspace-nav'
  import Icon from './Icon.svelte'
  import ProjectPic from './ProjectPic.svelte'
  import ResizeHandle from './ResizeHandle.svelte'

  let collapsed = $state<Record<string, boolean>>({})

  const unread = $derived($inbox.filter((i) => i.unread).length)
  const active = $derived(navKey($view))
  const projLabel = $derived($projects.length === 1 ? '1 project' : `${$projects.length} projects`)
  const visibleLoops = $derived($workspaces.filter((l) => !l.archived && !l.isMain && !(l.hostPrId ?? '').trim()))
  const parkedByWs = $derived(parkedNeedsYouByWorkspace($workspaces))
  const needsYou = $derived(visibleLoops.filter((w) => parkedByWs.get(w.id)).length)
  const running = $derived(visibleLoops.filter((w) => workspaceIsRunning(w, $workspacesRunning)).length)

  function taskRowOn(id: string) {
    return $view === 'detail' && id === $selectedWorkspaceId
  }
</script>

{#snippet unreadCt(unseen: number, parked: number)}
  {@const n = unseen + parked}
  {#if n > 0}
    <span class="ct" title={sidebarUnreadTitle(unseen, parked)}>{n > 9 ? '9+' : n}</span>
  {/if}
{/snippet}

{#snippet loopRows(rows: Workspace[])}
  {#each rows as l, li (l.id)}
    {@const wsPR = prForWorkspace($prs, l.id)}
    {@const wsPRLife = prLifecycle(wsPR) ?? (l.state === 'merged' ? 'merged' : null)}
    {@const nested = isNestedChild(l, rows)}
    {@const nestedLast = nested && !(rows[li + 1] && rows[li + 1].parentId === l.parentId && isNestedChild(rows[li + 1], rows))}
    <div
      class="tloop"
      class:tchild={nested}
      class:tchild-last={nestedLast}
      class:on={taskRowOn(l.id)}
      class:merged={wsPRLife === 'merged'}
      data-workspace-id={l.id}
    >
      <button type="button" class="tloop-hit" onclick={() => openWorkspace(l.id)}>
        {#if workspaceIsRunning(l, $workspacesRunning)}
          <span class="sd pulse" title="Agent running" aria-label="Agent running"></span>
        {:else if wsPRLife}
          <Icon class="br pr-{wsPRLife}" name={wsPR ? prIconName(wsPR) : 'git-merge'} />
        {:else}
          <Icon class="br" name="git-branch" />
        {/if}
        <span class="tn">{l.short}</span>
        <span class="tloop-tail">
          {#if l.diff.add || l.diff.del}
            <span class="df" title="+{l.diff.add} −{l.diff.del}"><span class="a">+{formatCompactCount(l.diff.add)}</span><span class="r">−{formatCompactCount(l.diff.del)}</span></span>
          {/if}
          {@render unreadCt($attention.byWorkspace[l.id] ?? 0, parkedByWs.get(l.id) ?? 0)}
        </span>
      </button>
      {#if wsPR}
        <span class="tloop-acts">
          <button
            type="button"
            class="lact lact-pr"
            title={wsPR.number ? `View PR #${wsPR.number}` : 'View PR'}
            aria-label={wsPR.number ? `View PR #${wsPR.number}` : 'View PR'}
            onclick={() => openPR(wsPR.id)}
          >
            <Icon name={prIconName(wsPR)} />
          </button>
        </span>
      {/if}
    </div>
  {/each}
{/snippet}

{#snippet projectCard(p: Project)}
  {@const items = nestWorkspaces(visibleLoops.filter((l) => l.projectId === p.id))}
  <div class="tproj" role="listitem" data-project-id={p.id} class:collapsed={collapsed[p.id]}>
    <button class="tproj-h" onclick={() => (collapsed = { ...collapsed, [p.id]: !collapsed[p.id] })}>
      <span class="tico">
        <ProjectPic class="pic" pic={p.pic} color={p.color} name={p.name} />
        <Icon class="car" name="chevron-down" strokeWidth={2.4} />
      </span>
      <span class="pn">{p.name}</span>
      {@render unreadCt($attention.byProject[p.id] ?? 0, items.reduce((n, l) => n + (parkedByWs.get(l.id) ?? 0), 0))}
    </button>
    <div class="tloops">
      {#if items.length}
        {@render loopRows(items)}
      {:else}
        <div class="tproj-empty">no active workspaces</div>
      {/if}
    </div>
  </div>
{/snippet}

<aside class="rail" aria-label="Sidebar">
  <div class="rail-top">
    <button class="nav inbox-nav" class:on={active === 'briefing'} onclick={() => go('briefing')}>
      <Icon name="inbox" />Inbox<span class="ct" class:zero={unread === 0}>{unread}</span>
    </button>
    <button class="nav" class:on={active === 'pr'} onclick={() => go('pr')}>
      <Icon name="git-pull-request" />
      Pull requests
      <span class="ct">{$prs.length}</span>
    </button>
  </div>

  <div class="rail-scroll">
    <div class="sec">
      <span class="st">Projects</span>
    </div>
    <div class="rail-tree-shell">
      <div id="tree" class="rail-tree" role="list">
        {#each $projects as p (p.id)}
          {@render projectCard(p)}
        {/each}
      </div>
    </div>
  </div>

  <div class="rail-foot">
    <div class="rail-foot-stats">
      <div class="rail-stat"><span>{projLabel}</span></div>
      {#if needsYou > 0}
        <button type="button" class="rail-stat needs" title="Open Inbox" onclick={() => go('briefing')}>
          <span>{needsYou} {needsYou === 1 ? 'needs' : 'need'} you</span>
        </button>
      {:else}
        <div class="rail-stat" class:run={running > 0}><span>{running} {running === 1 ? 'workspace' : 'workspaces'} running</span></div>
      {/if}
    </div>
  </div>
  <ResizeHandle panel="rail" edge="end" />
</aside>
