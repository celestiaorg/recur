<script module lang="ts">
  let streamMdKey = 0
</script>

<script lang="ts">
  // Display-only unit reveal for live assistant text / thinking. The `text` prop
  // is the complete string received so far (append-only, in order). A block with
  // no newline paints as a unit; after the first newline, the unfinished tail
  // waits for the next newline or `complete` (end of this text block).
  //
  // Mermaid is intentionally NOT hydrated here — incomplete fences + rAF
  // re-parses would thrash. AgentChat swaps to MarkdownHtml when streaming
  // ends; that is the only settled path (do not render final text through this).
  import { escapeText } from '../lib/markdown'
  import { renderMarkdownAsync } from '../lib/markdown-host'
  import { completeStreamPrefix } from '../lib/stream-reveal'

  let {
    text = '',
    active = true,
    complete = false,
  }: {
    text?: string
    active?: boolean
    complete?: boolean
  } = $props()

  let revealed = $state(0)
  let html = $state('')
  let documentVisible = $state(typeof document === 'undefined' || !document.hidden)
  const showing = $derived(active && documentVisible)
  let revealedN = 0
  let wasShowing = true
  const parseKey = 'sm:' + (++streamMdKey)

  // No presentation while hidden. Returning to the chat shows the latest text
  // immediately instead of replaying the hidden interval's units.
  $effect(() => {
    if (!showing) {
      wasShowing = false
      return
    }
    const src = text
    const len = src.length
    if (!wasShowing || len < revealedN) {
      revealedN = len
      revealed = len
      wasShowing = true
      return
    }
    wasShowing = true
    const next = completeStreamPrefix(src, revealedN, complete)
    if (next !== revealedN) {
      revealedN = next
      revealed = next
    }
  })

  const shown = $derived(text.slice(0, revealed))

  $effect(() => {
    if (!showing) return
    const src = shown
    if (!src) {
      html = ''
      return
    }
    let cancelled = false
    void renderMarkdownAsync(src, { key: parseKey }).then((next) => {
      if (cancelled) return
      html = next
    }).catch((err: unknown) => {
      if (cancelled) return
      const name = err instanceof Error ? err.name : ''
      if (name === 'AbortError' || (name === 'TimeoutError' && html)) return
      console.warn('markdown render failed', err)
      html = escapeText(src).split('\n').join('<br>')
    })
    return () => { cancelled = true }
  })
</script>

<svelte:document onvisibilitychange={() => { documentVisible = !document.hidden }} />

<div class="chat-canvas">{@html html}</div>
