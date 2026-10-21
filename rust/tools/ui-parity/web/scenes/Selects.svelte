<script lang="ts">
  import Select from '../../../../../frontend/src/components/Select.svelte'

  let { variant }: { variant: string } = $props()
  const models = [
    { value: 'opus', label: 'Opus 5.5', group: 'Anthropic', provider: 'claude', hint: 'Most capable' },
    { value: 'sonnet', label: 'Sonnet 5.5', group: 'Anthropic', provider: 'claude', hint: 'Fast' },
    { value: 'gpt', label: 'GPT-6', group: 'OpenAI', provider: 'codex' },
    { value: 'llama', label: 'Llama 5', group: 'Local', provider: 'ollama' },
  ]
  const density = [
    { value: 'comfortable', label: 'Comfortable' },
    { value: 'compact', label: 'Compact' },
    { value: 'spacious', label: 'Spacious' },
  ]
  const modes = [
    { value: 'agent', label: 'Agent', icon: 'box', tint: 'var(--amber)' },
    { value: 'shell', label: 'Shell', icon: 'terminal', tint: 'var(--cyan)' },
  ]
</script>

<div class="wrap">
  {#if variant.startsWith('closed')}
    <div class="row">
      <Select label="Model" options={models} value="opus" />
      <Select label="Density" options={density} value="compact" size="sm" />
      <Select label="Mode" options={modes} value="agent" ghost />
      <Select label="Locked" options={density} value="comfortable" disabled />
    </div>
  {:else if variant === 'open'}
    <Select label="Model" options={models} value="sonnet" />
  {:else if variant === 'searchable'}
    <Select label="Model" options={models} value="opus" searchable />
  {:else if variant === 'multiple'}
    <Select label="Density" options={density} values={['compact', 'spacious']} multiple placeholder="Any" />
  {:else if variant === 'focus'}
    <Select label="Density" options={density} value="compact" />
  {/if}
</div>

<style>
  .wrap { padding: 16px; }
  .row { display: flex; align-items: center; gap: 12px; }
</style>
