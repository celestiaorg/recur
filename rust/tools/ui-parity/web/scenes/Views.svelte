<script lang="ts">
  // Scenes of the home, briefing, project and palette view crates (`home`, `briefing`,
  // `projects` and `palettes` in scenes.json): the whole window as Shell.svelte mounts it, over
  // the scene's edited seed, then the scene's `route` once the app has loaded, as the crates'
  // galleries navigate. Only the routed view shows in `.main`; modal and palette routes leave it
  // empty behind their scrim. A palette route's path stands in for the focused editor.
  import { onMount, tick } from 'svelte'
  import App from '../../../../../frontend/src/App.svelte'
  import {
    initEditorTheme, initEditorPrefs, initPanelWidths, initCodePanelSides, initExplorerPrefs,
    initRailCollapsed, initTerminalPrefs, go, openNewTask, openNewProject, openProject,
    paletteOpen, commandsOpen, shortcutsOpen, openInOpen, openWorkspace, openSyntaxModePicker,
  } from '../../../../../frontend/src/lib/stores'
  import { setEditorFocusProvider } from '../../../../../frontend/src/lib/commands'
  import { openRunMenu } from '../../../../../frontend/src/lib/run-dev'

  let { onready }: { onready: () => void } = $props()

  initEditorTheme()
  initEditorPrefs()
  initPanelWidths()
  initCodePanelSides()
  initExplorerPrefs()
  initRailCollapsed()
  initTerminalPrefs()

  const q = new URLSearchParams(location.search)
  const route = q.get('route') ?? ''
  const frames = (n: number) =>
    new Promise<void>((r) => {
      const step = () => (--n > 0 ? requestAnimationFrame(step) : r())
      requestAnimationFrame(step)
    })
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

  function navigate() {
    const [kind, id, section] = route.split(':')
    switch (kind) {
      case 'welcome': go('welcome'); break
      case 'briefing': go('briefing'); break
      case 'issues': go('issues'); break
      case 'schedules': go('schedules'); break
      case 'newtask': openNewTask(); break
      case 'newproject': openNewProject(); break
      case 'project': openProject(id ?? ''); void section; break
      case 'palette': paletteOpen.set(true); break
      case 'commands':
        setEditorFocusProvider(() => ({ scope: section ?? '', path: id ?? '' }))
        commandsOpen.set(true)
        break
      case 'shortcuts': shortcutsOpen.set(true); break
      // Landing on the workspace closes palettes: open them once it has.
      case 'openin': openWorkspace(id ?? ''); setTimeout(() => openInOpen.set(true), 300); break
      case 'run': openWorkspace(id ?? ''); setTimeout(() => openRunMenu(id ?? ''), 300); break
      case 'syntax':
        setEditorFocusProvider(() => ({ scope: '', path: id ?? '' }))
        openSyntaxModePicker()
        break
    }
  }

  onMount(() => {
    const wait = async (left: number) => {
      if (!document.querySelector('.rail') && left > 0) {
        setTimeout(() => wait(left - 1), 50)
        return
      }
      await sleep(300)
      navigate()
      await tick()
      await sleep(600)
      await frames(3)
      onready()
    }
    void wait(100)
  })
</script>

<div class="views" data-route={route.split(':')[0]}>
  <App />
</div>

<style>
  :global(.views .main > *) {
    visibility: hidden !important;
  }
  :global(.views[data-route='welcome'] .main > #v-welcome.view.on),
  :global(.views[data-route='briefing'] .main > #v-briefing.view.on),
  :global(.views[data-route='issues'] .main > #v-issues.view.on),
  :global(.views[data-route='schedules'] .main > #v-schedules.view.on),
  :global(.views[data-route='project'] .main > #v-project.view.on) {
    visibility: visible !important;
  }
</style>
