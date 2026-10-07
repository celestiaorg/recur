<script lang="ts">
  import { fgOnBackground, letterFromName, parseProjectPic } from '../lib/project-pic'
  import LucideIcon from './LucideIcon.svelte'

  let {
    pic = '',
    color = '',
    name = '',
    class: className = '',
  }: {
    pic?: string
    color?: string
    name?: string
    class?: string
  } = $props()

  const parsed = $derived(parseProjectPic(pic, letterFromName(name)))
  const fg = $derived(parsed.kind === 'image' ? undefined : fgOnBackground(color))
</script>

<span
  class="proj-pic {className}"
  class:is-img={parsed.kind === 'image'}
  style:background={color || undefined}
  style:color={fg}
  aria-hidden="true"
>
  {#if parsed.kind === 'image'}
    <img src={parsed.src} alt="" draggable="false" />
  {:else if parsed.kind === 'icon'}
    <LucideIcon name={parsed.name} />
  {:else}
    {parsed.text}
  {/if}
</span>

<style>
  .proj-pic {
    display: grid;
    place-items: center;
    overflow: hidden;
    line-height: 1;
    font-family: var(--mono);
    font-weight: 700;
    color: #0A0C10;
  }
  .proj-pic :global(svg) { width: 62%; height: 62%; }
  .proj-pic :global(img) { width: 100%; height: 100%; object-fit: cover; display: block; }
</style>
