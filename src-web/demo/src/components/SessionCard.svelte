<script lang="ts">
  // Specialist row in the Loop cockpit fanout card (in-workspace run, not a
  // child worktree). Adjacent specialists share one card, like a wave.
  // Live sessions start open and stream inside.
  import AgentChat, { type AgentMsg } from './AgentChat.svelte'
  import Icon from './Icon.svelte'
  import { chatModeMeta } from '../lib/modes'
  import { specialistMode } from '../lib/loop-cockpit'

  let {
    code = '',
    name = '',
    messages = [],
    live = false,
  }: {
    code?: string
    name?: string
    messages?: AgentMsg[]
    /** The session is currently streaming: render expanded, pin the header. */
    live?: boolean
  } = $props()

  let expandedPref = $state<boolean | null>(null)
  const expanded = $derived(expandedPref ?? live)
  const mm = $derived(chatModeMeta(specialistMode(code)))

  const lastMeta = $derived.by(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i]
      if (m.me || m.tool || m.thinking || m.compact || m.turnEnd || m.stopped || m.error || m.checkpoint) continue
      const t = (m.body ?? '').trim()
      if (!t) continue
      return m.noteHtml ? { html: t } : { text: t }
    }
    return null
  })

  function toggle() {
    expandedPref = !expanded
  }
</script>

<button
  type="button"
  class="fanout-row clickable expandable sess-head"
  class:expanded
  class:live
  aria-expanded={expanded}
  onclick={toggle}
>
  <span class="fanout-steps">
    <span class="fanout-chips">
      <span class="sess-mode" style:color={mm.tint} title={mm.label} aria-hidden="true"><Icon name={mm.icon} /></span>
    </span>
    <span class="fanout-car" aria-hidden="true"><Icon name="chevron-down" strokeWidth={2.4} /></span>
  </span>
  <span class="fanout-name">
    <span class="sess-name">{name || code}</span>
    {#if !live && !expanded && lastMeta}
      <span class="fanout-note sess-meta">{#if lastMeta.html}{@html lastMeta.html}{:else}{lastMeta.text}{/if}</span>
    {/if}
  </span>
  <span class="fanout-state" class:running={live} class:sess-live={live}>
    {#if live}
      <span class="sd" aria-label="running"></span>
      working
    {:else}
      finished
    {/if}
  </span>
</button>
{#if expanded}
  <div class="fanout-transcript">
    <div class="sess-body fanout-body">
      <AgentChat {messages} readonly emptyText="No output yet." />
    </div>
  </div>
{/if}
