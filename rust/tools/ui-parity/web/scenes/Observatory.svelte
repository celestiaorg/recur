<script lang="ts">
  // Observatory scenes (`observatory` in scenes.json): the Observatory as the workspace view
  // mounts it for LOOP-2231, over `content/observatory.json` (capture.spec answers the tree,
  // file, git and editor-session calls with it). Each scene's editor session picks the tabs,
  // view mode, main column and side panel; `terminal` opens the dock.
  import { onMount, tick } from 'svelte'
  import Observatory from '../../../../../frontend/src/components/Observatory.svelte'
  import { loadInitial } from '../../../../../frontend/src/lib/api'
  import {
    setTerminalBeside,
    setTerminalDockOpen,
    view,
  } from '../../../../../frontend/src/lib/stores'
  import fixtures from '../../content/observatory.json'

  let { name, onready }: { name: string; onready: () => void } = $props()
  const variant = name.split('/').slice(1).join('/').replace(/-paper$/, '')
  const scene = (fixtures.scenes as Record<string, { terminal?: string }>)[variant] ?? {}
  let mounted = $state(false)

  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })

  onMount(async () => {
    view.set('detail')
    await loadInitial()
    const ws = fixtures.workspaceId
    if (scene.terminal === 'open') setTerminalDockOpen(ws, true)
    if (scene.terminal === 'beside') setTerminalBeside(ws, true)
    await document.fonts.ready
    mounted = true
    await tick()
    await new Promise((r) => setTimeout(r, 600))
    await frames(3)
    onready()
  })
</script>

<!-- `.win` carries --chrome-foot-h and --win-corner-pad, as the app window does. -->
<div class="win obs-scene">
  {#if mounted}
    <Observatory projectId={fixtures.projectId} workspaceId={fixtures.workspaceId} shellActive />
  {/if}
</div>

<style>
  .obs-scene { position: absolute; inset: 0; display: flex; flex-direction: column; }
</style>
