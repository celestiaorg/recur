<script lang="ts">
  // PR scenes (`prs` in scenes.json): the whole window as Shell.svelte mounts it, over the
  // scene's edited seed, with the PR routes shown. `route` opens the dashboard or PR detail
  // (`pr`), `tab` picks PR detail's tab, `review` opens the review modal, `seg` the dashboard's
  // lifecycle, as `recur_view_prs::gallery` does. Every other routed view stays hidden.
  import { onMount } from 'svelte'
  import App from '../../../../../frontend/src/App.svelte'
  import {
    initEditorTheme, initEditorPrefs, initPanelWidths, initCodePanelSides, initExplorerPrefs,
    initRailCollapsed, initTerminalPrefs, go, openPR, openPRReview, prDetailTab, prAgentDockOpen, prListSeg,
  } from '../../../../../frontend/src/lib/stores'

  let { onready }: { onready: () => void } = $props()

  initEditorTheme()
  initEditorPrefs()
  initPanelWidths()
  initCodePanelSides()
  initExplorerPrefs()
  initRailCollapsed()
  initTerminalPrefs()

  const q = new URLSearchParams(location.search)
  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })

  onMount(() => {
    const wait = async (left: number) => {
      if (!document.querySelector('.rail-tree .tproj') && left > 0) {
        setTimeout(() => wait(left - 1), 50)
        return
      }
      const pr = q.get('pr') ?? ''
      if (q.get('route') === 'prdetail' && pr) {
        openPR(pr)
        if (q.get('dock') === 'closed') prAgentDockOpen.set(false)
        // After PRDetail's per-PR reset, which lands on Conversation.
        await frames(3)
        const tab = q.get('tab')
        if (tab) prDetailTab.set(tab as 'conv' | 'commits' | 'files' | 'checks')
        const review = q.get('review')
        if (review) openPRReview(review as 'approve' | 'request')
      } else {
        const seg = q.get('seg')
        if (seg) prListSeg.set(seg as 'open' | 'draft' | 'merged' | 'closed' | 'all')
        go('pr')
      }
      await new Promise((r) => setTimeout(r, 600))
      await frames(3)
      onready()
    }
    void wait(100)
  })
</script>

<App />

<style>
  /* Shell.svelte's global rule hides every routed view; these scenes show the PR routes. */
  :global(.main > #v-pr.view.on), :global(.main > #v-prdetail.view.on) {
    visibility: visible !important;
  }
</style>
