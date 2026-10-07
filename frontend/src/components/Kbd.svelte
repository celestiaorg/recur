<script lang="ts">
  import { shortcutParts } from '../lib/shortcut'

  let {
    text,
    class: className = 'kbd',
  }: {
    text: string
    class?: string
  } = $props()
</script>

<span class={className}>{#each shortcutParts(text) as p}{#if p.mod}<span class="kbd-mod">{p.text}</span>{:else}{p.text}{/if}{/each}</span>
