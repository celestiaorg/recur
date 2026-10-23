<script lang="ts">
  // Settings scenes (`settings` in scenes.json): the whole window as Shell.svelte mounts it,
  // over the scene's edited seed, with Settings shown on `section`. `act` then drives one
  // interaction the way `recur_view_settings::gallery` does: open the Add provider modal,
  // start the Cursor SDK install with progress, open the default policy menu or its help, open
  // the sidebar's usage meter, or open the keybinding menu.
  import { onMount, tick } from 'svelte'
  import App from '../../../../../frontend/src/App.svelte'
  import {
    initEditorTheme, initEditorPrefs, initPanelWidths, initCodePanelSides, initExplorerPrefs,
    initRailCollapsed, initTerminalPrefs, openSettings,
  } from '../../../../../frontend/src/lib/stores'
  import { openKeybindingMenu } from '../../../../../frontend/src/lib/keybinding-menu'

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
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

  function clickButton(scope: string, label: string) {
    const root = document.querySelector(scope)
    const b = [...(root?.querySelectorAll<HTMLButtonElement>('button') ?? [])].find(
      (el) => el.textContent?.trim() === label || el.getAttribute('aria-label') === label,
    )
    b?.click()
  }

  async function act(name: string) {
    switch (name) {
      case 'compat-modal':
        clickButton('#v-settings', 'Add provider')
        break
      case 'install':
        clickButton('#v-settings .acct-sdk-note', 'Install SDK')
        await sleep(50)
        ;(window as any)._wails.dispatchWailsEvent({
          name: 'cursor.install',
          data: { phase: 'download', message: 'Downloading Cursor SDK…', percent: 42 },
        })
        break
      case 'policy-menu':
        clickButton('#v-settings', 'Policy')
        break
      case 'policy-tip':
        clickButton('#v-settings', 'What is a policy?')
        break
      case 'usage':
        document.querySelector<HTMLElement>('.use-meter .use-btn')?.click()
        break
      case 'keybinding-menu':
        openKeybindingMenu(
          [
            { id: 'a', label: 'Sort Lines Ascending', category: 'Text', order: 10 },
            { id: 'b', label: 'Sort Lines Descending', category: 'Text', order: 20 },
            { id: 'c', label: 'Format JSON', category: 'JSON', order: 10 },
          ],
          () => {},
        )
        break
    }
  }

  onMount(() => {
    const wait = async (left: number) => {
      if (!document.querySelector('.rail-tree .tproj') && left > 0) {
        setTimeout(() => wait(left - 1), 50)
        return
      }
      openSettings((q.get('section') ?? 'general') as Parameters<typeof openSettings>[0])
      await tick()
      await sleep(600)
      await frames(3)
      const a = q.get('act')
      if (a) {
        await act(a)
        await sleep(300)
        await frames(3)
      }
      onready()
    }
    void wait(100)
  })
</script>

<App />

<style>
  /* Shell.svelte's global rule hides every routed view; these scenes show Settings. */
  :global(.main > #v-settings.view.on) {
    visibility: visible !important;
  }
</style>
