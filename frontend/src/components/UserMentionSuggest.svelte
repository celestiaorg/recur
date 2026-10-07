<script lang="ts">
  import { get } from 'svelte/store'
  import { applyUserMention, mentionLogin, userMentionSuggest } from '../lib/user-mention'
  import { participantAvatarUrl, participantInitials, participantLabel } from '../lib/pr-participants'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { menuFixedBox, menuPreferUp, portalToBody, watchMenuViewport } from '../lib/menu-space'
  import { textareaCaretRect } from '../lib/textarea-caret'

  const MENU_WIDTH = 240

  let box = $state<{ top?: number; bottom?: number; left: number; width: number; spacePx: number } | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let hover = createHoverGate()

  const suggestEl = $derived($userMentionSuggest?.el)
  const suggestStart = $derived($userMentionSuggest?.start)

  function place() {
    const s = get(userMentionSuggest)
    if (!s) {
      box = null
      return
    }
    const caret = textareaCaretRect(s.el, s.start)
    const trigger = { top: caret.top, bottom: caret.bottom, left: caret.left, width: 0 }
    const up = menuPreferUp(trigger, { gap: 4 })
    const next = menuFixedBox(trigger, { up, minWidth: MENU_WIDTH, gap: 4 })
    box = { top: next.top, bottom: next.bottom, left: next.left, width: next.width, spacePx: next.spacePx }
  }

  $effect(() => {
    const el = suggestEl
    const start = suggestStart
    const menu = listEl
    if (!el || start == null) {
      box = null
      return
    }
    place()
    resetHoverGate(hover)
    return watchMenuViewport(place, menu)
  })

  $effect(() => {
    void $userMentionSuggest?.sel
    queueMicrotask(() => {
      listEl?.querySelector<HTMLElement>('.chat-mention-row.on')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function onMove(i: number, e: MouseEvent) {
    if (hoverGateAllows(hover, e.clientX, e.clientY)) {
      userMentionSuggest.update((s) => (s ? { ...s, sel: i } : s))
    }
  }

  function hideBrokenAvatar(e: Event) {
    const img = e.currentTarget
    if (img instanceof HTMLImageElement) img.style.display = 'none'
  }
</script>

{#if $userMentionSuggest && box}
  <div
    class="emoji-suggest user-mention"
    role="listbox"
    aria-label="Mention suggestions"
    bind:this={listEl}
    style:top={box.top !== undefined ? `${box.top}px` : 'auto'}
    style:bottom={box.bottom !== undefined ? `${box.bottom}px` : 'auto'}
    style:left={`${box.left}px`}
    style:width={`${box.width}px`}
    style:--sel-space="{box.spacePx}px"
    {@attach portalToBody}
    onmousedown={(e) => e.preventDefault()}
  >
    {#each $userMentionSuggest.hits as h, i (mentionLogin(h))}
      {@const url = participantAvatarUrl(h)}
      {@const login = mentionLogin(h)}
      {@const name = (h.name ?? '').trim()}
      <button
        type="button"
        class="chat-mention-row"
        class:on={i === $userMentionSuggest.sel}
        role="option"
        aria-selected={i === $userMentionSuggest.sel}
        onmousemove={(e) => onMove(i, e)}
        onclick={() => {
          const s = $userMentionSuggest
          if (s) applyUserMention(s, h)
        }}
      >
        <span class="pr-face" style={h.avatarCss || undefined}>
          {participantInitials(h)}
          {#if url}<img src={url} alt="" onerror={hideBrokenAvatar} />{/if}
        </span>
        <span class="chat-mention-name">{participantLabel(h)}</span>
        {#if name && name.toLowerCase() !== login.toLowerCase()}
          <span class="chat-mention-path">{name}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}
