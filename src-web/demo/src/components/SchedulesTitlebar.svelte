<script lang="ts">
  // Schedules chrome folded into the app titlebar (same pattern as InboxTitlebar):
  // counts next to the crumb. New lives on the pane header, like a new chat tab.
  import { schedules } from '../lib/stores'

  let { part }: { part: 'counts' | 'actions' } = $props()

  const active = $derived($schedules.filter((s) => !s.paused))
  const paused = $derived($schedules.length - active.length)
  const next = $derived(Math.min(...active.map((s) => s.nextRunAt || Infinity)))
  const subText = $derived([
    `${active.length} active`,
    paused ? `${paused} paused` : '',
    Number.isFinite(next) ? `next ${new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(next)}` : '',
  ].filter(Boolean).join(' · '))
</script>

{#if part === 'counts'}
  <span class="tb-sub">{subText}</span>
{/if}
