<script lang="ts">
  import type { PRComment } from '../../bindings/recur/internal/domain/models'
  import { appOverlayOpen } from '../lib/stores'
  import { portalToBody } from '../lib/menu-space'
  import {
    GH_REACTIONS,
    reactionIcon,
    reactionLabel,
    reactionMine,
    togglePRCommentReaction,
    visibleReactions,
  } from '../lib/comment-reactions'
  import Icon from './Icon.svelte'

  let { comment, canReact = false }: { comment?: PRComment | null; canReact?: boolean } = $props()

  const pills = $derived(visibleReactions(comment?.reactions))
  const show = $derived(!!comment?.id && (pills.length > 0 || canReact))
  let open = $state(false)
  let box = $state<{ top: number; left: number } | null>(null)

  $effect(() => {
    if ($appOverlayOpen) open = false
  })

  function close() {
    open = false
    box = null
  }

  function openPicker(e: MouseEvent) {
    e.stopPropagation()
    if (open) {
      close()
      return
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
    const width = 252
    const left = Math.min(r.left, Math.max(8, window.innerWidth - width - 8))
    box = { top: r.bottom + 4, left }
    open = true
  }

  async function react(content: string) {
    if (!canReact || !comment) return
    close()
    await togglePRCommentReaction(comment, content)
  }

  function onKey(e: KeyboardEvent) {
    if (!open || e.key !== 'Escape') return
    e.preventDefault()
    e.stopPropagation()
    close()
  }
</script>

<svelte:window onmousedown={() => close()} onkeydowncapture={onKey} onwheel={() => close()} />

{#if show && comment}
  <div class="rxn">
    {#if canReact}
      <button type="button" class="rxn-add" title="Add reaction" aria-label="Add reaction" aria-haspopup="menu" aria-expanded={open} onclick={openPicker}>
        <Icon name="smile" size={14} />
      </button>
    {/if}
    {#each pills as r (r.content)}
      {@const label = reactionLabel(r.content)}
      {@const icon = reactionIcon(r.content)}
      <button
        type="button"
        class="rxn-pill"
        class:mine={!!r.mine}
        data-rxn={r.content}
        disabled={!canReact}
        aria-pressed={!!r.mine}
        aria-label={r.mine ? `Remove ${label} reaction` : `React with ${label}`}
        title={r.mine ? `Remove ${label}` : `React with ${label}`}
        onclick={() => void react(r.content)}
      >
        {#if icon}<Icon name={icon} size={13} />{/if}
        <span class="rxn-n">{r.count}</span>
      </button>
    {/each}
  </div>
{/if}

{#if open && box}
  <div class="rxn-pick" style="top:{box.top}px;left:{box.left}px" role="menu" {@attach portalToBody} onmousedown={(e) => e.stopPropagation()}>
    {#each GH_REACTIONS as r (r.content)}
      <button
        type="button"
        class="rxn-pick-item"
        class:mine={reactionMine(comment?.reactions, r.content)}
        data-rxn={r.content}
        role="menuitem"
        aria-label="React with {r.label}"
        title={r.label}
        onclick={() => void react(r.content)}
      ><Icon name={r.icon} size={16} /></button>
    {/each}
  </div>
{/if}
