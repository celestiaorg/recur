<script lang="ts">
  // Workspace scenes (`workspace` in scenes.json): the detail route as App.svelte mounts it in
  // `.main`, bound to LOOP-2231, over `content/workspace.json` (each scene's chat rows,
  // tabs, sessions, facts and findings) and `content/observatory.json` (the worktree, by the
  // scene's `session`). capture.spec answers both, as `recur_view_workspace::gallery` does.
  import { onMount, tick } from 'svelte'
  import WorkspaceDetail from '../../../../../frontend/src/views/WorkspaceDetail.svelte'
  import { loadInitial } from '../../../../../frontend/src/lib/api'
  import { openWorkspace } from '../../../../../frontend/src/lib/stores'
  import fixtures from '../../content/workspace.json'

  let { onready }: { onready: () => void } = $props()
  let mounted = $state(false)

  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })

  onMount(async () => {
    await loadInitial()
    openWorkspace(fixtures.workspaceId)
    await document.fonts.ready
    mounted = true
    await tick()
    await new Promise((r) => setTimeout(r, 800))
    await frames(3)
    onready()
  })
</script>

<!-- `.win` carries --chrome-foot-h and --win-corner-pad; `.body > .main` is the routed area. -->
<div class="win ws-scene">
  <div class="body">
    <main class="main">
      {#if mounted}
        <WorkspaceDetail />
      {/if}
    </main>
  </div>
</div>

<style>
  .ws-scene { position: absolute; inset: 0; display: flex; flex-direction: column; height: auto; }
  /* Shell.svelte's global rule hides every routed view; this scene shows the detail route. */
  .ws-scene :global(.main > #v-detail.view.on) { visibility: visible !important; }
</style>
