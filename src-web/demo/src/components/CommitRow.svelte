<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { Commit } from '../lib/types'
  import { pushToast } from '../lib/stores'
  import { commitByline, commitSha } from '../lib/commit-meta'
  import Icon from './Icon.svelte'

  let {
    commit: c,
    onOpen,
    chevron = true,
    trailing,
  }: {
    commit: Commit
    onOpen?: () => void
    chevron?: boolean
    trailing?: Snippet
  } = $props()

  let copied = $state(false)
  let copyTimer: ReturnType<typeof setTimeout> | undefined
  const sha = $derived(commitSha(c))
  const interactive = $derived(!!onOpen)

  async function copySha(e: MouseEvent) {
    e.stopPropagation()
    if (!sha) return
    try {
      await navigator.clipboard.writeText(sha)
      copied = true
      clearTimeout(copyTimer)
      copyTimer = setTimeout(() => { copied = false }, 1200)
    } catch {
      pushToast('error', 'Could not copy SHA')
    }
  }

  function onKey(e: KeyboardEvent) {
    if (!onOpen) return
    if (e.key === 'Enter' && e.target === e.currentTarget) onOpen()
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="commit"
  role={interactive ? 'button' : undefined}
  tabindex={interactive ? 0 : undefined}
  onclick={onOpen}
  onkeydown={onKey}
>
  <div class="cav" style="background:{c.color}">
    {#if c.avatarUrl}<img src={c.avatarUrl} alt="" />{:else}{c.agentCode}{/if}
  </div>
  <div class="cmsg">
    <div class="cm-t">{c.message}</div>
    <div class="cm-by">
      {#if c.authorRole && c.authorRole !== 'Author'}
        {c.authorRole}{#if c.iterLabel} · <span class="iter">{c.iterLabel}</span>{/if}
      {:else}
        {commitByline(c)}
      {/if}
    </div>
  </div>
  {@render trailing?.()}
  <span class="cdf"><span class="a">+{c.diff?.add ?? 0}</span><span class="r">−{c.diff?.del ?? 0}</span></span>
  <span class="chash">{c.hash}</span>
  <button type="button" class="chash-copy" class:copied title={copied ? 'Copied' : 'Copy full SHA'} aria-label={copied ? 'Copied' : 'Copy full SHA'} onclick={copySha}>
    {#if copied}
      <Icon name="check" strokeWidth={2.4} />
    {:else}
      <Icon name="copy" />
    {/if}
  </button>
  <span class="ctime">{c.when}</span>
  {#if chevron}
    <Icon name="chevron-right" class="cchev" />
  {/if}
</div>
