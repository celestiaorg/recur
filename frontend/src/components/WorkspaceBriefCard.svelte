<script lang="ts">
  import { routeTarget } from '../lib/stores'
  import { markWorkspaceInboxRead } from '../lib/inbox-read'
  import { formatRelativeAge } from '../lib/format'
  import type { InboxItem, WorkspaceBrief } from '../../bindings/recur/internal/domain/models'
  import Icon, { type IconName } from './Icon.svelte'

  let { brief, notes = [], compact = false, openTarget = '' }: {
    brief: WorkspaceBrief
    notes?: InboxItem[]
    /** Dense full-width table row (the Inbox list, tool-result lists). */
    compact?: boolean
    /** Route override (e.g. a present_workspace pin's validated target). */
    openTarget?: string
  } = $props()

  const unread = $derived(notes.length)
  // Hover recall for "what happened since" — the note titles, tags stripped.
  const noteTitles = $derived(
    notes.map((n) => (n.title || '').replace(/<[^>]+>/g, '')).filter(Boolean).join(' · '),
  )

  const REASON_META: Record<string, { icon: IconName; color: string; label: string }> = {
    permission: { icon: 'shield', color: 'var(--coral)', label: 'Permission' },
    question: { icon: 'message-question', color: 'var(--coral)', label: 'Question' },
    escalation: { icon: 'triangle-alert', color: 'var(--coral)', label: 'Escalation' },
    retryable_failure: { icon: 'refresh-cw', color: 'var(--coral)', label: 'Stopped' },
    failure: { icon: 'octagon-x', color: 'var(--coral)', label: 'Failed' },
    plan_approval: { icon: 'list-todo', color: 'var(--warn)', label: 'Plan approval' },
    findings: { icon: 'hat-glasses', color: 'var(--warn)', label: 'Findings' },
    pr_attention: { icon: 'git-pull-request', color: 'var(--emerald)', label: 'PR' },
    unseen_chat: { icon: 'message-code', color: 'var(--cyan)', label: 'Unseen chat' },
    active_agent: { icon: 'bot', color: 'var(--amber)', label: 'Working' },
    unfinished_plan: { icon: 'list-todo', color: 'var(--indigo)', label: 'Plan' },
    dirty_idle: { icon: 'file-diff', color: 'var(--indigo)', label: 'Uncommitted' },
    recent_chat: { icon: 'message-square', color: 'var(--indigo)', label: 'Chat' },
  }
  const FALLBACK_META = { icon: 'box' as IconName, color: 'var(--muted)', label: '' }

  const primaryReason = $derived(brief.reasons?.[0] ?? '')
  const reasonMeta = $derived(REASON_META[primaryReason] ?? FALLBACK_META)
  const live = $derived(primaryReason === 'active_agent')

  const primaryTarget = $derived((brief.primary?.target || openTarget || '').trim())
  const hasDiff = $derived((brief.diffAdd ?? 0) + (brief.diffDel ?? 0) > 0)
  const age = $derived(formatRelativeAge(brief.lastActivityAt ?? 0))

  function open() {
    if (!primaryTarget) return
    markWorkspaceInboxRead(brief.workspaceId)
    routeTarget(primaryTarget)
  }

  function onCardClick(e: MouseEvent) {
    if ((e.target as HTMLElement | null)?.closest('button')) return
    open()
  }

  function onCardKey(e: KeyboardEvent) {
    if (e.target !== e.currentTarget) return
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      open()
    }
  }
</script>

{#if compact}
<!-- One full-width table row (PR-dashboard / gmail density). The row is the
     primary action; deep context lives in the workspace itself. -->
<div
  class="wbrief-row"
  class:has-unread={unread > 0 || brief.changedSinceViewed}
  role="button"
  tabindex="0"
  aria-label={brief.primary?.label ? `${brief.title}. ${brief.primary.label}` : brief.title}
  onclick={onCardClick}
  onkeydown={onCardKey}
>
  <span class="udot" title={brief.changedSinceViewed && !unread ? 'Changed since you last looked' : undefined}></span>
  <div class="ity" class:live style="background:color-mix(in srgb, {reasonMeta.color} 12%, transparent);color:{reasonMeta.color}">
    <Icon name={reasonMeta.icon} />
  </div>
  <div class="wbrief-row-main">
    <div class="wbrief-row-t">
      <span class="wbrief-row-title">{brief.title}</span>
      {#if brief.prNumber}<span class="wbrief-prn">#{brief.prNumber}</span>{/if}
      {#if brief.branch}<span class="wbrief-row-branch">{brief.branch}</span>{/if}
      {#if unread}<span class="wbrief-unread" title={noteTitles || `${unread} unread notification${unread === 1 ? '' : 's'}`}>{unread}</span>{/if}
    </div>
    <div class="wbrief-row-now">{brief.now || brief.next}</div>
  </div>
  <span class="wbrief-row-proj">{brief.projectName ?? ''}</span>
  <span class="wbrief-row-fact" title={brief.planTotal ? `${brief.planDone ?? 0} of ${brief.planTotal} plan steps done` : undefined}>
    {#if brief.planTotal}{brief.planDone ?? 0}/{brief.planTotal}{/if}
  </span>
  <span class="wbrief-row-fact">
    {#if hasDiff}
      <span class="wbrief-add">+{brief.diffAdd ?? 0}</span> <span class="wbrief-del">−{brief.diffDel ?? 0}</span>
    {/if}
  </span>
  <span class="wbrief-row-when" title={age?.full}>{age?.rel ?? ''}</span>
</div>
{:else}
<!-- Card variant: a single pinned workspace in a chat transcript
     (present_workspace). -->
<article
  class="wbrief"
  role="button"
  tabindex="0"
  aria-label={brief.primary?.label ? `${brief.title}. ${brief.primary.label}` : brief.title}
  onclick={onCardClick}
  onkeydown={onCardKey}
>
  <div class="ity" class:live style="background:color-mix(in srgb, {reasonMeta.color} 12%, transparent);color:{reasonMeta.color}">
    <Icon name={reasonMeta.icon} />
  </div>
  <div class="wbrief-main">
    <div class="wbrief-row-t">
      <span class="wbrief-row-title">{brief.title}</span>
      {#if brief.prNumber}<span class="wbrief-prn">#{brief.prNumber}</span>{/if}
    </div>
    {#if brief.projectName || brief.branch}
      <div class="wbrief-meta">
        {#if brief.projectName}<span>{brief.projectName}</span>{/if}
        {#if brief.branch}<span class="wbrief-row-branch">{brief.branch}</span>{/if}
      </div>
    {/if}
    {#if brief.now || brief.next}
      <div class="wbrief-next">{brief.now || brief.next}</div>
    {/if}
  </div>
</article>
{/if}
