<script lang="ts">
  // The whole window as the app mounts it (frontend/src/main.ts), over the scene's edited seed
  // (capture.spec.ts), with every routed view hidden: the shell parity scenes compare the
  // titlebar, the sidebar and an empty main area. Ready once the project tree has loaded (and
  // been scrolled, for `treeScroll` scenes).
  import { onMount } from 'svelte'
  import App from '../../../../../frontend/src/App.svelte'
  import {
    initEditorTheme, initEditorPrefs, initPanelWidths, initCodePanelSides, initExplorerPrefs,
    initRailCollapsed, initTerminalPrefs,
  } from '../../../../../frontend/src/lib/stores'

  let { onready }: { onready: () => void } = $props()

  initEditorTheme()
  initEditorPrefs()
  initPanelWidths()
  initCodePanelSides()
  initExplorerPrefs()
  initRailCollapsed()
  initTerminalPrefs()

  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })

  onMount(() => {
    const scroll = Number(new URLSearchParams(location.search).get('treeScroll') ?? 0)
    const wait = async (left: number) => {
      if (!document.querySelector('.rail-tree .tproj') && left > 0) {
        setTimeout(() => wait(left - 1), 50)
        return
      }
      await frames(3)
      const tree = document.querySelector<HTMLElement>('#tree')
      if (scroll && tree) {
        tree.scrollTop = scroll
        await frames(3)
      }
      onready()
    }
    void wait(100)
  })
</script>

<App />

<style>
  :global(.main > *) {
    visibility: hidden !important;
  }
</style>
