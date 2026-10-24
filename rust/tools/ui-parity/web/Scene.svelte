<script lang="ts">
  import { onMount } from 'svelte'
  import Tokens from './scenes/Tokens.svelte'
  import Calibration from './scenes/Calibration.svelte'
  import Kbd from './scenes/Kbd.svelte'
  import Buttons from './scenes/Buttons.svelte'
  import Toggles from './scenes/Toggles.svelte'
  import Icons from './scenes/Icons.svelte'
  import Ring from './scenes/Ring.svelte'
  import ProjectPic from './scenes/ProjectPic.svelte'
  import Settings from './scenes/Settings.svelte'
  import Armed from './scenes/Armed.svelte'
  import ToastsScene from './scenes/Toasts.svelte'
  import Tooltip from './scenes/Tooltip.svelte'
  import Tabs from './scenes/Tabs.svelte'
  import Selects from './scenes/Selects.svelte'
  import IconPick from './scenes/IconPick.svelte'
  import Resize from './scenes/Resize.svelte'
  import Overlays from './scenes/Overlays.svelte'
  import Scrollbars from './scenes/Scrollbars.svelte'
  import Editor from './scenes/Editor.svelte'
  import Terminal from './scenes/Terminal.svelte'
  import Shell from './scenes/Shell.svelte'
  import Content from './scenes/Content.svelte'
  import Chat from './scenes/Chat.svelte'
  import Review from './scenes/Review.svelte'
  import ObservatoryScene from './scenes/Observatory.svelte'
  import WorkspaceScene from './scenes/Workspace.svelte'
  import PrsScene from './scenes/Prs.svelte'
  import SettingsPage from './scenes/SettingsPage.svelte'
  import Views from './scenes/Views.svelte'

  let { name }: { name: string } = $props()
  const kind = $derived(name.split('/')[0])
  const variant = $derived(name.split('/').slice(1).join('/'))
  let ready = $state(false)
  const CONTENT = ['markdown', 'mermaid', 'pdf', 'file-preview', 'lightbox']
  const VIEWS = ['home', 'briefing', 'projects']
  // Editor scenes are ready once TextMate has painted tokens.
  function painted(): boolean {
    return kind !== 'editor' || !!document.querySelector('.cm-line span')
  }
  onMount(() => {
    // Terminal and shell scenes report their own readiness.
    if (kind === 'terminal' || kind === 'shell' || kind === 'chat' || kind === 'review' || kind === 'observatory' || kind === 'workspace' || kind === 'prs' || kind === 'settings' || VIEWS.includes(kind) || CONTENT.includes(kind)) return
    const settle = () => requestAnimationFrame(() => requestAnimationFrame(() => (ready = true)))
    const wait = (left: number) => (painted() || left <= 0 ? settle() : setTimeout(() => wait(left - 1), 50))
    wait(100)
  })
</script>

<div class="scene" data-scene-ready={ready ? '' : undefined}>
  {#if kind === 'tokens'}
    <Tokens />
  {:else if kind === 'text'}
    <Calibration />
  {:else if kind === 'kbd'}
    <Kbd />
  {:else if kind === 'buttons'}
    <Buttons />
  {:else if kind === 'toggles'}
    <Toggles />
  {:else if kind === 'icons'}
    <Icons {variant} />
  {:else if kind === 'ring'}
    <Ring />
  {:else if kind === 'project-pic'}
    <ProjectPic />
  {:else if kind === 'settings-group'}
    <Settings />
  {:else if kind === 'armed-button'}
    <Armed />
  {:else if kind === 'toasts'}
    <ToastsScene />
  {:else if kind === 'tooltip'}
    <Tooltip />
  {:else if kind === 'tab-strip'}
    <Tabs {variant} />
  {:else if kind === 'select'}
    <Selects {variant} />
  {:else if kind === 'icon-picker'}
    <IconPick />
  {:else if kind === 'resize-handle'}
    <Resize />
  {:else if kind === 'overlays'}
    <Overlays {variant} />
  {:else if kind === 'scrollbar'}
    <Scrollbars />
  {:else if kind === 'editor'}
    <Editor {variant} />
  {:else if kind === 'terminal'}
    <Terminal {name} onready={() => (ready = true)} />
  {:else if CONTENT.includes(kind)}
    <Content {name} onready={() => (ready = true)} />
  {:else if kind === 'shell'}
    <Shell onready={() => (ready = true)} />
  {:else if kind === 'chat'}
    <Chat {name} onready={() => (ready = true)} />
  {:else if kind === 'review'}
    <Review {name} onready={() => (ready = true)} />
  {:else if kind === 'observatory'}
    <ObservatoryScene {name} onready={() => (ready = true)} />
  {:else if kind === 'workspace'}
    <WorkspaceScene onready={() => (ready = true)} />
  {:else if kind === 'prs'}
    <PrsScene onready={() => (ready = true)} />
  {:else if kind === 'settings'}
    <SettingsPage onready={() => (ready = true)} />
  {:else if VIEWS.includes(kind)}
    <Views onready={() => (ready = true)} />
  {:else}
    <p>unknown scene {name} ({variant})</p>
  {/if}
</div>

<style>
  .scene { position: relative; width: 100vw; height: 100vh; overflow: hidden; }
</style>
