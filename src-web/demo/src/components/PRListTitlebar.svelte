<script lang="ts">
  // PR dashboard chrome folded into the app titlebar (same pattern as InboxTitlebar /
  // SchedulesTitlebar): counts next to the crumb. Filters live on the list pane.
  import { prListScoped } from '../lib/stores'
  import { prListTitleCounts } from '../lib/pr-list-filter'

  let { part }: { part: 'counts' | 'actions' } = $props()

  const scoped = $derived($prListScoped)
  const counts = $derived(prListTitleCounts(scoped))
</script>

{#if part === 'counts'}
  <span class="tb-sub">{counts.you} need you · {counts.open} open · {counts.merged} merged</span>
{/if}
