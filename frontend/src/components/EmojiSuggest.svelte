<script lang="ts">
  import { get } from 'svelte/store'
  import { applyEmojiSuggestion, emojiSuggest } from '../lib/emoji'
  import { createHoverGate, hoverGateAllows, resetHoverGate } from '../lib/hoverGate'
  import { menuFixedBox, menuPreferUp, portalToBody, watchMenuViewport } from '../lib/menu-space'
  import { textareaCaretRect } from '../lib/textarea-caret'

  const MENU_WIDTH = 240

  let box = $state<{ top?: number; bottom?: number; left: number; width: number; spacePx: number } | null>(null)
  let listEl = $state<HTMLDivElement | null>(null)
  let hover = createHoverGate()

  const suggestEl = $derived($emojiSuggest?.el)
  const suggestStart = $derived($emojiSuggest?.start)

  function place() {
    const s = get(emojiSuggest)
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
    void $emojiSuggest?.sel
    queueMicrotask(() => {
      listEl?.querySelector<HTMLElement>('.chat-mention-row.on')?.scrollIntoView({ block: 'nearest' })
    })
  })

  function onMove(i: number, e: MouseEvent) {
    if (hoverGateAllows(hover, e.clientX, e.clientY)) {
      emojiSuggest.update((s) => (s ? { ...s, sel: i } : s))
    }
  }
</script>

{#if $emojiSuggest && box}
  <div
    class="emoji-suggest"
    role="listbox"
    tabindex="-1"
    aria-label="Emoji suggestions"
    bind:this={listEl}
    style:top={box.top !== undefined ? `${box.top}px` : 'auto'}
    style:bottom={box.bottom !== undefined ? `${box.bottom}px` : 'auto'}
    style:left={`${box.left}px`}
    style:width={`${box.width}px`}
    style:--sel-space="{box.spacePx}px"
    {@attach portalToBody}
    onmousedown={(e) => e.preventDefault()}
  >
    {#each $emojiSuggest.hits as h, i (h.name)}
      <button
        type="button"
        class="chat-mention-row"
        class:on={i === $emojiSuggest.sel}
        role="option"
        aria-selected={i === $emojiSuggest.sel}
        onmousemove={(e) => onMove(i, e)}
        onclick={() => {
          const s = $emojiSuggest
          if (s) applyEmojiSuggestion(s, h)
        }}
      >
        <span class="emoji-suggest-glyph">{h.emoji}</span>
        <span class="chat-mention-name">{h.name}</span>
      </button>
    {/each}
  </div>
{/if}
