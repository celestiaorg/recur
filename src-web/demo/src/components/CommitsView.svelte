<script lang="ts">
  import type { Commit } from '../../bindings/recur/internal/domain/models'
  import CommitRow from './CommitRow.svelte'
  import Icon from './Icon.svelte'

  let {
    commits = [] as Commit[],
    loading = false,
    error = false,
    baseLabel = '',
    workingDirtyCount = 0,
    onOpenCommit,
    onOpenWorking,
  }: {
    commits?: Commit[]
    loading?: boolean
    error?: boolean
    baseLabel?: string
    workingDirtyCount?: number
    onOpenCommit?: (c: Commit) => void
    onOpenWorking?: () => void
  } = $props()

  const vsLabel = $derived(baseLabel.trim())
  const showWorking = $derived(workingDirtyCount > 0)
  const listEmpty = $derived(!commits.length && !showWorking)

  function onWorkingKey(e: KeyboardEvent) {
    if (!onOpenWorking) return
    if (e.target !== e.currentTarget) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      onOpenWorking()
    }
  }
</script>

<div class="obs-review prpane on" data-review="commits">
  <div class="commits-wrap">
    {#if loading && listEmpty}
      <div class="empty"><div class="et">Loading commits…</div></div>
    {:else if error && listEmpty}
      <div class="empty"><div class="et">Could not load commits</div></div>
    {:else if listEmpty}
      <div class="empty"><div class="et">{vsLabel ? `No commits since ${vsLabel}` : 'No commits yet'}</div></div>
    {:else}
      <div class="commits-list">
        {#if showWorking}
          <div
            class="commit wip"
            data-working="true"
            role="button"
            tabindex="0"
            aria-label="Uncommitted changes"
            onclick={() => onOpenWorking?.()}
            onkeydown={onWorkingKey}
          >
            <div class="cav wip" aria-hidden="true">WIP</div>
            <div class="cmsg">
              <div class="cm-t">Uncommitted changes</div>
              <div class="cm-by">Working tree vs HEAD</div>
            </div>
            <span class="chash">—</span>
            <Icon name="chevron-right" class="cchev" />
          </div>
        {/if}
        {#each commits as c (c.hash)}
          <CommitRow commit={c} onOpen={() => onOpenCommit?.(c)} />
        {/each}
      </div>
      {#if error && !commits.length}
        <div class="empty"><div class="et">Could not load commits</div></div>
      {:else if loading && !commits.length}
        <div class="empty"><div class="et">Loading commits…</div></div>
      {:else}
        <div class="hint" style="text-align:left;padding:14px 2px 0">{vsLabel ? `Commits on this branch since ${vsLabel}` : 'Recent commits on this branch'}. Click any to see its diff.</div>
      {/if}
    {/if}
  </div>
</div>
