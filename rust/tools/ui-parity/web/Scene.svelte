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

  let { name }: { name: string } = $props()
  const kind = $derived(name.split('/')[0])
  const variant = $derived(name.split('/').slice(1).join('/'))
  let ready = $state(false)
  onMount(() => {
    requestAnimationFrame(() => requestAnimationFrame(() => (ready = true)))
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
  {:else}
    <p>unknown scene {name} ({variant})</p>
  {/if}
</div>

<style>
  .scene { position: relative; width: 100vw; height: 100vh; overflow: hidden; }
</style>
