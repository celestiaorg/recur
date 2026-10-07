<script lang="ts">
  // Settled markdown → sanitized HTML, with Mermaid placeholders hydrated to SVG
  // and copy buttons attached to fenced code. StreamingMarkdown skips both
  // (incomplete fences + rAF re-parses); parents swap here when streaming ends.
  //
  // Sync on purpose: tab-swap scroll restore needs real heights on the first
  // frame. Hits the shared settled cache (or parses and stores).
  import { renderMarkdownCached } from '../lib/markdown-host'
  import { enhanceCodeCopy } from '../lib/mdcopy'
  import { hydrateMermaid } from '../lib/mermaid'
  import AttachmentLightbox from './AttachmentLightbox.svelte'

  let {
    text = '',
    class: className = 'chat-canvas',
    demoteImages = false,
  }: {
    text?: string
    class?: string
    /** WebFetch cards: no live <img> from third-party page markdown. */
    demoteImages?: boolean
  } = $props()

  let root: HTMLElement | undefined = $state()
  let lightbox = $state<{ src: string; name: string } | null>(null)
  const html = $derived.by(() => {
    const src = text ?? ''
    if (!src) return ''
    return renderMarkdownCached(src, demoteImages ? { demoteImages: true } : undefined)
  })

  $effect(() => {
    // Re-run after {@html} swaps in new placeholders / fences.
    void html
    const el = root
    if (!el) return
    enhanceCodeCopy(el)
    for (const img of el.querySelectorAll('img')) {
      img.setAttribute('tabindex', '0')
      img.setAttribute('role', 'button')
      if (!img.getAttribute('aria-label')) {
        img.setAttribute('aria-label', img.alt ? `View ${img.alt}` : 'View image')
      }
    }
    return hydrateMermaid(el)
  })

  function imgFromEvent(e: Event): HTMLImageElement | null {
    const t = e.target
    return t instanceof HTMLImageElement && (t.currentSrc || t.src) ? t : null
  }

  function openFromImg(img: HTMLImageElement) {
    const src = img.currentSrc || img.src
    if (!src) return
    lightbox = { src, name: img.alt || 'Image' }
  }

  function onClick(e: MouseEvent) {
    const img = imgFromEvent(e)
    if (!img) return
    e.preventDefault()
    e.stopPropagation()
    openFromImg(img)
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key !== 'Enter' && e.key !== ' ') return
    const img = imgFromEvent(e)
    if (!img) return
    e.preventDefault()
    e.stopPropagation()
    openFromImg(img)
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class={className} bind:this={root} onclick={onClick} onkeydown={onKeyDown}>{@html html}</div>
{#if lightbox}
  <AttachmentLightbox src={lightbox.src} name={lightbox.name} onclose={() => { lightbox = null }} />
{/if}

<style>
  div :global(img) {
    cursor: zoom-in;
  }
</style>
