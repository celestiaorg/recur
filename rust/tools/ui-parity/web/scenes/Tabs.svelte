<script lang="ts">
  import TabStrip from '../../../../../frontend/src/components/TabStrip.svelte'
  import TabChip from '../../../../../frontend/src/components/TabChip.svelte'
  import Icon from '../../../../../frontend/src/components/Icon.svelte'
  import { fileIconClass } from '../../../../../frontend/src/lib/file-icons'

  let { variant }: { variant: string } = $props()
  const many = ['alpha.ts', 'beta.rs', 'gamma.go', 'delta.py', 'epsilon.md', 'zeta.json', 'eta.toml', 'theta.css']
</script>

{#if variant === 'overflow'}
  <div class="narrow">
    <TabStrip label="Open files" chrome activeKey="theta.css">
      {#each many as f (f)}
        <TabChip tabKey={f} label={f} iconClass={fileIconClass(f)} active={f === 'theta.css'} onclose={() => {}} />
      {/each}
    </TabStrip>
  </div>
{:else}
  <TabStrip label="Open files" chrome activeKey="changes">
    <TabChip tabKey="main" label="main.rs" iconClass={fileIconClass('main.rs')} />
    <TabChip tabKey="changes" label="Changes" icon="git-branch" active lit badge={4} />
    <TabChip tabKey="notes" label="notes.md" iconClass={fileIconClass('notes.md')} dirty onclose={() => {}} />
    <TabChip tabKey="chat" label="Chat" icon="message-question" unseen />
    <TabChip tabKey="build" label="Build" icon="terminal" busy />
    <TabChip tabKey="old" label="old shell" icon="terminal" muted />
    {#snippet trailing()}
      <button class="tab-strip-act"><Icon name="plus" /></button>
      <button class="tab-strip-act on"><Icon name="maximize" /></button>
    {/snippet}
  </TabStrip>
{/if}

<style>
  .narrow { width: 300px; }
</style>
