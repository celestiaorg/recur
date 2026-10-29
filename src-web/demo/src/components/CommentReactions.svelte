<script lang="ts">
  import { untrack } from 'svelte'
  import type { PRComment } from '../lib/types'
  import { popoverTop, portalToBody } from '../lib/menu-space'
  import {
    GH_REACTIONS,
    reactionIcon,
    reactionLabel,
    reactionMine,
    toggleReactions,
    visibleReactions,
  } from '../lib/comment-reactions'
  import Icon from './Icon.svelte'

  let { comment }: { comment?: PRComment | null } = $props()

  // simplify: reactions toggle locally; the demo has no GitHub to write to.
  let reactions = $state(untrack(() => comment?.reactions ?? []))
  const pills = $derived(visibleReactions(reactions))
  let open = $state(false)
  let box = $state<{ top: number; left: number } | null>(null)
  let anchor: DOMRect | null = null

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
    anchor = r
    box = { top: popoverTop(r), left }
    open = true
  }

  function fitPicker(node: HTMLElement) {
    const r = anchor
    if (!r) return
    untrack(() => {
      if (box) box = { ...box, top: popoverTop(r, { menuHeight: node.offsetHeight }) }
    })
  }

  function react(content: string) {
    close()
    reactions = toggleReactions(reactions, content)
  }

  function onKey(e: KeyboardEvent) {
    if (!open || e.key !== 'Escape') return
    e.preventDefault()
    e.stopPropagation()
    close()
  }
</script>

<svelte:window onmousedown={() => close()} onkeydowncapture={onKey} onwheel={() => close()} />

{#if comment?.id}
  <div class="rxn">
    <button type="button" class="rxn-add" title="Add reaction" aria-label="Add reaction" aria-haspopup="menu" aria-expanded={open} onclick={openPicker}>
      <Icon name="smile" size={14} />
    </button>
    {#each pills as r (r.content)}
      {@const label = reactionLabel(r.content)}
      {@const icon = reactionIcon(r.content)}
      <button
        type="button"
        class="rxn-pill"
        class:mine={!!r.mine}
        data-rxn={r.content}
        aria-pressed={!!r.mine}
        aria-label={r.mine ? `Remove ${label} reaction` : `React with ${label}`}
        title={r.mine ? `Remove ${label}` : `React with ${label}`}
        onclick={() => react(r.content)}
      >
        {#if icon}<Icon name={icon} size={13} />{/if}
        <span class="rxn-n">{r.count}</span>
      </button>
    {/each}
  </div>
{/if}

{#if open && box}
  <div class="rxn-pick" style="top:{box.top}px;left:{box.left}px" role="menu" tabindex="-1" {@attach portalToBody} {@attach fitPicker} onmousedown={(e) => e.stopPropagation()}>
    {#each GH_REACTIONS as r (r.content)}
      <button
        type="button"
        class="rxn-pick-item"
        class:mine={reactionMine(reactions, r.content)}
        data-rxn={r.content}
        role="menuitem"
        aria-label="React with {r.label}"
        title={r.label}
        onclick={() => react(r.content)}
      ><Icon name={r.icon} size={16} /></button>
    {/each}
  </div>
{/if}
