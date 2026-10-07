<script lang="ts">
  // Workspace detail = the UNIFIED workspace view for every type (workbench
  // redesign): the code Observatory with Agent chat (Loop on the conductor tab)
  // and a Plan panel. This shell only guards routing: stale selections bail out.
  // Loop escalations render inline in the conductor transcript (same card as ask_user).
  //
  // The same WorkspaceView instance also backs the PR-detail agent dock: while
  // a PR with a linked workspace is open, we keep that workspace mounted here
  // (even if the sidebar selection is different) so chat state survives the
  // view switch. Observatory portals the left dock into PR detail.
  //
  // Hot shells: at most two WorkspaceViews stay mounted (visible + previous).
  // The live one is moved into #v-detail; the parked one lives in #wsv-park so
  // Playwright `#v-detail .obs` stays unique and WKWebView does not keep every
  // workbench in the layout tree. A→B→A is a node move, not a remount.
  import { untrack } from 'svelte'
  import { get } from 'svelte/store'
  import { view, workspaces, selectedWorkspaceId, selectedPRId, prs, go, observatoryJump } from '../lib/stores'
  import { PRsService, call, upsertWorkspace } from '../lib/api'
  import { hotShellsEqual, liveWorkspaceIds, touchHotShells } from '../lib/hot-shells'
  import { restoreHotShellGeometry, stashHotShellGeometry } from '../lib/editor-view-state'
  import { prAgentWorkspaceId } from '../lib/pr-workspace'
  import WorkspaceView from './WorkspaceView.svelte'

  const prWorkspaceId = $derived(
    $view === 'prdetail'
      ? prAgentWorkspaceId($prs.find((p) => p.id === $selectedPRId), $workspaces)
      : '',
  )
  const targetWsId = $derived($view === 'prdetail' ? prWorkspaceId : $selectedWorkspaceId)
  // Keep the last PR/workspace instance mounted after leaving PR detail so an
  // in-flight turn is not torn down when the sidebar has no selection.
  const liveIds = $derived(liveWorkspaceIds($workspaces))
  let keepWsId = $state('')
  $effect(() => {
    if (targetWsId && liveIds.has(targetWsId)) keepWsId = targetWsId
  })
  // PR detail binds only to the linked worktree or hidden host — never the
  // previously selected workspace (that parked PR-only chats on main).
  const boundWsId = $derived.by(() => {
    const id = $view === 'prdetail' ? prWorkspaceId : (targetWsId || keepWsId)
    return liveIds.has(id) ? id : ''
  })

  let hotIds = $state<string[]>([])
  $effect.pre(() => {
    const id = boundWsId
    const live = liveIds
    untrack(() => {
      const next = touchHotShells(hotIds, id, { live })
      if (!hotShellsEqual(hotIds, next)) hotIds = next
    })
  })

  let liveSlot = $state<HTMLElement | undefined>()
  let parkSlot = $state<HTMLElement | undefined>()

  function syncParkBox() {
    const live = liveSlot
    const park = parkSlot
    if (!live || !park) return
    const r = live.getBoundingClientRect()
    if (r.width < 8 || r.height < 8) return
    park.style.top = `${r.top}px`
    park.style.left = `${r.left}px`
    park.style.width = `${r.width}px`
    park.style.height = `${r.height}px`
    park.style.right = 'auto'
    park.style.bottom = 'auto'
  }

  $effect(() => {
    const live = liveSlot
    const park = parkSlot
    if (!live || !park) return
    syncParkBox()
    const ro = new ResizeObserver(syncParkBox)
    ro.observe(live)
    window.addEventListener('resize', syncParkBox)
    return () => {
      ro.disconnect()
      window.removeEventListener('resize', syncParkBox)
    }
  })

  function attachHotShell(id: string) {
    return (node: HTMLElement) => {
      const homeEl = node.parentElement
      const home = () => {
        if (homeEl?.classList.contains('wsv-anchor')) return homeEl
        const parent = node.parentElement
        if (parent?.classList.contains('wsv-anchor')) return parent
        return parkSlot?.querySelector(`[data-anchor-id="${CSS.escape(id)}"]`) ?? null
      }
      $effect.pre(() => {
        const dest = id === boundWsId && liveIds.has(id) ? liveSlot : home()
        if (!dest || node.parentNode === dest) return
        if (dest !== liveSlot) {
          syncParkBox()
          stashHotShellGeometry(node)
        }
        dest.appendChild(node)
        if (dest === liveSlot) {
          const jump = get(observatoryJump)
          restoreHotShellGeometry(node, {
            skipEditor: !!(jump?.path && (!jump.workspaceId || jump.workspaceId === id)),
          })
        }
      })
      return () => {
        const h = home()
        if (h && node.parentNode && node.parentNode !== h) h.appendChild(node)
      }
    }
  }

  let hostAttempt = ''
  $effect(() => {
    if ($view !== 'prdetail') {
      hostAttempt = ''
      return
    }
    const pr = $prs.find((p) => p.id === $selectedPRId)
    if (!pr?.id) return
    if (pr.state === 'Merged' || pr.state === 'Closed' || pr.group === 'merged') return
    const leftoverHost = $workspaces.some((w) => !w.archived && (w.hostPrId ?? '').trim() === pr.id)
    const linkedLive = $workspaces.some(
      (w) => !w.archived && w.id === (pr.workspaceId ?? '').trim() && !(w.hostPrId ?? '').trim(),
    )
    if (prWorkspaceId && !(leftoverHost && linkedLive)) return
    if (hostAttempt === pr.id) return
    hostAttempt = pr.id
    void call(PRsService.EnsurePRChatHost(pr.id), 'Could not start pull request chat').then((ws) => {
      if (ws?.id) upsertWorkspace(ws)
    })
  })

  // Phase 2.2: bail out of a stale/removed selection instead of rendering blank.
  $effect(() => {
    if ($view !== 'detail') return
    const id = $selectedWorkspaceId
    if (!id || !liveIds.has(id)) {
      go('briefing')
    }
  })
</script>

<section
  class="view"
  class:on={$view === 'detail'}
  id="v-detail"
  inert={$view !== 'detail' || undefined}
  aria-hidden={$view !== 'detail' || undefined}
>
  <div class="wsv-live" bind:this={liveSlot}></div>
</section>
<div
  id="wsv-park"
  bind:this={parkSlot}
  aria-hidden="true"
  data-hot-ids={hotIds.join(',')}
>
  {#each hotIds as id (id)}
    {#if liveIds.has(id)}
      <div class="wsv-anchor" data-anchor-id={id}>
        <div
          class="wsv-shell"
          class:on={id === boundWsId}
          data-workspace-id={id}
          inert={id !== boundWsId}
          {@attach attachHotShell(id)}
        >
          <!-- A render error in one workbench must not abort the app-wide flush:
               that leaves the previous shell stuck in .wsv-live (stacked
               observatories) and every other component half-updated. -->
          <svelte:boundary onerror={(e) => console.error(`workspace ${id} failed to render`, e)}>
            <WorkspaceView workspaceId={id} shellActive={id === boundWsId} />
            {#snippet failed(error, reset)}
              <div class="wsv-failed" role="alert">
                <p>This workspace view failed to render.</p>
                <pre>{error instanceof Error ? error.message : String(error)}</pre>
                <button type="button" onclick={reset}>Retry</button>
              </div>
            {/snippet}
          </svelte:boundary>
        </div>
      </div>
    {/if}
  {/each}
</div>
