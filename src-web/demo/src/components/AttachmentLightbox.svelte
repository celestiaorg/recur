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
    video = false,
    startTime = 0,
    onclose,
  }: {
    src: string
    name: string
    video?: boolean
    /** Resume point for a video zoomed out of an inline player. */
    startTime?: number
    /** Receives the video's playback position so an inline player can resume from it. */
    onclose: (currentTime?: number) => void
  } = $props()

  /** Image is shrunk to fit the window; click toggles actual size. */
  let scaled = $state(false)
  let actual = $state(false)
  let imgEl = $state<HTMLImageElement>()
  let videoEl = $state<HTMLVideoElement>()

  function measure() {
    if (imgEl && !actual) scaled = imgEl.naturalWidth > imgEl.clientWidth || imgEl.naturalHeight > imgEl.clientHeight
  }

  $effect(() => {
    window.addEventListener('resize', measure)
    return () => window.removeEventListener('resize', measure)
  })

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
    onclose(videoEl?.currentTime)
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
        close(e)
        return
      }
      if (!video && isCmdOrCtrl(e) && !e.shiftKey && !e.altKey &&
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

<div
  class="chat-att-lb"
  class:actual
  role="dialog"
  tabindex="-1"
  aria-modal="true"
  aria-label={name}
  {@attach portalToBody}
>
  <div class="chat-att-lb-actions">
    {#if !video}
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
    {/if}
    <button type="button" class="chat-att-lb-btn" aria-label="Close" title="Close" onclick={close}>
      <Icon name="x" strokeWidth={2.2} />
    </button>
  </div>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="chat-att-lb-scroll" onclick={(e) => { if (e.target === e.currentTarget) close() }}>
    {#if video}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        bind:this={videoEl}
        class="chat-att-lb-img"
        {src}
        controls
        autoplay
        onloadedmetadata={(e) => { e.currentTarget.currentTime = startTime }}
      ></video>
    {:else}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <img
        bind:this={imgEl}
        class="chat-att-lb-img"
        class:zoomable={scaled}
        {src}
        alt={name}
        onload={measure}
        onclick={() => { if (scaled) actual = !actual }}
        oncontextmenu={(e) => { imgCtx = openImageCtx(e, src) }}
      />
    {/if}
  </div>
</div>
<ImageContextMenu bind:menu={imgCtx} />
