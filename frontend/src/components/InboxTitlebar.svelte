<script lang="ts">
  // Inbox chrome folded into the app titlebar (same pattern as PRListTitlebar):
  // counts next to the crumb, actions on the right. The chat-dock toggle lives
  // in the pane headers (like the PR agent dock), not here.
  import { inbox, briefing } from '../lib/stores'
  import { markAllInboxRead } from '../lib/inbox-read'

  let { part }: { part: 'counts' | 'actions' } = $props()

  const unread = $derived($inbox.filter((i) => i.unread).length)
  const needs = $derived($briefing.counts.needs)
  const subText = $derived([
    needs ? `${needs} ${needs === 1 ? 'needs' : 'need'} you` : '',
    unread ? `${unread} unread` : 'all caught up',
  ].filter(Boolean).join(' · '))
</script>

{#if part === 'counts'}
  <span class="tb-sub">{subText}</span>
{:else}
  {#if unread}
    <button type="button" class="tbtn tb-mark-read" onclick={markAllInboxRead}>Mark all read</button>
  {/if}
{/if}
