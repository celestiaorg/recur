<script lang="ts">
  import { lucideInnerHtml } from '../lib/lucide-icons'

  let {
    name,
    class: className = '',
  }: {
    name: string
    class?: string
  } = $props()

  const html = $derived(lucideInnerHtml(name))
</script>

<svg
  class={className}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {@html html}
</svg>
