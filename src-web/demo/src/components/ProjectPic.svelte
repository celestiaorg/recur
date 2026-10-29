<script lang="ts">
  import { fgOnBackground, letterFromName } from '../lib/project-pic'

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

  const text = $derived([...pic.trim()].slice(0, 2).join('') || letterFromName(name))
</script>

<span
  class="proj-pic {className}"
  style:background={color || undefined}
  style:color={fgOnBackground(color)}
  aria-hidden="true"
>
  {text}
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
</style>
