<script lang="ts">
  import { onMount } from 'svelte'
  import { SettingsService } from '../../bindings/recur/internal/app'
  import type { AssignedIssue } from '../../bindings/recur/internal/app/models'
  import { formatRelativeAge } from '../lib/format'
  import { openExternalUrl } from '../lib/open-external'
  import Icon from './Icon.svelte'

  let issues = $state.raw<AssignedIssue[]>([])
  let loading = $state(true)

  onMount(() => {
    let alive = true
    SettingsService.ListAssignedIssues()
      .then((rows) => { if (alive) issues = rows ?? [] })
      .finally(() => { if (alive) loading = false })
    return () => { alive = false }
  })
</script>

<div class="issues">
  {#if loading}
    <p class="empty">Loading issues…</p>
  {:else if issues.length === 0}
    <p class="empty">No open issues assigned to you. GitHub needs <code>gh auth login</code>; Linear needs an API key in Settings.</p>
  {:else}
    {#each issues as i (i.url)}
      <button type="button" class="row" onclick={() => void openExternalUrl(i.url)}>
        <Icon name={i.source === 'github' ? 'github' : 'circle-dot'} />
        <span class="ref">{i.ref}</span>
        <span class="title">{i.title}</span>
        <span class="age">{formatRelativeAge(Date.parse(i.updatedAt))?.rel ?? ''}</span>
      </button>
    {/each}
  {/if}
</div>

<style>
  .issues { padding: 12px 16px; overflow-y: auto; }
  .empty { color: var(--muted); padding: 24px 8px; }
  .row { display: flex; align-items: center; gap: 10px; width: 100%; padding: 8px; border: 0; background: none; color: inherit; text-align: left; border-radius: 6px; cursor: pointer; }
  .row:hover { background: var(--hover, rgba(127, 127, 127, 0.12)); }
  .ref { color: var(--muted); flex: none; }
  .title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .age { color: var(--muted); flex: none; }
</style>
