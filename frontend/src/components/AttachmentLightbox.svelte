<script lang="ts">
  import { copyImage } from '../lib/clipboard'
  import { isCmdOrCtrl } from '../lib/shortcut'
  import { setChatOccluder } from '../lib/chat-occlusion'
  import { portalToBody } from '../lib/menu-space'
  import ImageContextMenu, { openImageCtx, type ImageCtxMenu } from './ImageContextMenu.svelte'
  import Icon from './Icon.svelte'

  let {
    src,
    name,
    onclose,
  }: {
    src: string
    name: string
    onclose: () => void
  } = $props()

  let copied = $state(false)
  let copying = $state(false)
  let copyTimer: ReturnType<typeof setTimeout> | undefined
  let imgCtx = $state<ImageCtxMenu | null>(null)

  // Full-screen over chat — finishes while open should still raise unseen (D-attention).
  $effect(() => {
    setChatOccluder('attachment-lightbox', true)
    return () => setChatOccluder('attachment-lightbox', false)
  })

  function close(e?: Event) {
    e?.stopPropagation()
    onclose()
  }

  async function copy(e?: Event) {
    e?.stopPropagation()
    if (copying) return
    copying = true
    try {
      await copyImage(src)
      copied = true
      clearTimeout(copyTimer)
      copyTimer = setTimeout(() => { copied = false }, 1400)
    } catch {
      copied = false
    } finally {
      copying = false
    }
  }

  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Let the image context menu own Escape while it's open.
      if (document.querySelector('.att-ctx')) return
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onclose()
        return
      }
      if (isCmdOrCtrl(e) && !e.shiftKey && !e.altKey &&
          (e.key === 'c' || e.key === 'C')) {
        e.preventDefault()
        e.stopPropagation()
        void copy()
      }
    }
    // Capture so ⌘C / Esc win over the surrounding modal or chat shortcuts.
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      clearTimeout(copyTimer)
    }
  })
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  class="chat-att-lb"
  role="dialog"
  tabindex="-1"
  aria-modal="true"
  aria-label={name}
  {@attach portalToBody}
  onclick={(e) => { if (e.target === e.currentTarget) onclose() }}
>
  <div class="chat-att-lb-actions">
    <button
      type="button"
      class="chat-att-lb-btn"
      class:copied
      aria-label={copied ? 'Copied' : 'Copy image'}
      title={copied ? 'Copied' : 'Copy image'}
      disabled={copying}
      onclick={(e) => void copy(e)}
    >
      {#if copied}
        <Icon name="check" strokeWidth={2.4} />
      {:else}
        <Icon name="copy" />
      {/if}
    </button>
    <button type="button" class="chat-att-lb-btn" aria-label="Close" title="Close" onclick={close}>
      <Icon name="x" strokeWidth={2.2} />
    </button>
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <img
    class="chat-att-lb-img"
    {src}
    alt={name}
    oncontextmenu={(e) => { imgCtx = openImageCtx(e, src) }}
  />
</div>
<ImageContextMenu bind:menu={imgCtx} />
