<script lang="ts">
  import { renderMarkdown } from '../lib/markdown'

  let { text = '', class: className = 'chat-canvas' }: { text?: string; class?: string } = $props()
</script>

<div class={className}>{@html text ? renderMarkdown(text) : ''}</div>
