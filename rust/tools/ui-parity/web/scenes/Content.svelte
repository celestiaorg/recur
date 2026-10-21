<script lang="ts">
  // Rich content scenes (`content` in scenes.json): markdown, streaming markdown, Mermaid,
  // PDF, file previews and the lightbox, mounted as the app mounts them, with the fixtures in
  // `rust/tools/ui-parity/content/` that `recur_content::gallery` reads too. Ready once
  // images, diagrams and pages have painted.
  import { onMount, tick } from 'svelte'
  import MarkdownHtml from '../../../../../frontend/src/components/MarkdownHtml.svelte'
  import StreamingMarkdown from '../../../../../frontend/src/components/StreamingMarkdown.svelte'
  import FilePreview from '../../../../../frontend/src/components/FilePreview.svelte'
  import PdfPreview from '../../../../../frontend/src/components/PdfPreview.svelte'
  import AttachmentLightbox from '../../../../../frontend/src/components/AttachmentLightbox.svelte'
  import { createPreviewScrollStore } from '../../../../../frontend/src/lib/preview-scroll'
  import chat from '../../content/chat.md?raw'
  import readme from '../../content/readme.md?raw'
  import streaming from '../../content/streaming.md?raw'
  import sequence from '../../content/sequence.md?raw'
  import flow from '../../content/flow.mmd?raw'
  import logo from '../../content/logo.svg?raw'
  import pdfUrl from '../../content/page.pdf?url'
  import shotUrl from '../../content/screenshot.png?url'

  let { name, onready }: { name: string; onready: () => void } = $props()
  const variant = $derived(name.split('/').slice(1).join('/'))
  const kind = $derived(name.split('/')[0])
  const scroll = createPreviewScrollStore()
  let root: HTMLElement | undefined = $state()
  // Mount once every face is in, as in the app (fonts load at boot): mermaid measures text when
  // it renders.
  let fonts = $state(false)

  function settled(): boolean {
    if (!root) return false
    for (const img of root.querySelectorAll('img')) if (!img.complete || !img.naturalWidth) return false
    if (kind === 'markdown' && !root.querySelector('.chat-canvas, .md-doc')?.childElementCount) return false
    if (kind === 'mermaid') {
      if (root.querySelector('.md-mermaid:not([data-rendered])')) return false
      if (variant === 'flowchart' && !root.querySelector('.mmd-host svg')) return false
      if (root.querySelector('.mmd-pending')) return false
    }
    if (kind === 'pdf' && !root.querySelector('.obs-pdf-page-slot canvas[width]:not([width="0"])')) return false
    return true
  }

  onMount(async () => {
    await document.fonts.ready
    await Promise.all([...document.fonts].map((f) => f.load().catch(() => undefined)))
    // Diagrams render on a settled page, as in the app (mermaid lays out from live text
    // measurement, which differs while the page is still starting).
    if (kind === 'mermaid') await new Promise((r) => setTimeout(r, 500))
    fonts = true
    await tick()
    const done = () => requestAnimationFrame(() => requestAnimationFrame(onready))
    const wait = (left: number) => {
      if (settled() || left <= 0) {
        // PDF pages paint after the canvas is sized; give pdf.js its frames.
        if (kind === 'pdf') setTimeout(done, 500)
        else void tick().then(done)
      } else setTimeout(() => wait(left - 1), 50)
    }
    wait(200)
  })
</script>

<div class="content-scene" bind:this={root}>
  {#if !fonts}
  {:else if name === 'markdown/chat' || name === 'markdown/paper'}
    <div class="pad"><MarkdownHtml text={chat} /></div>
  {:else if name === 'markdown/streaming'}
    <div class="pad"><StreamingMarkdown text={streaming} active={true} complete={false} /></div>
  {:else if name === 'mermaid/sequence'}
    <div class="pad"><MarkdownHtml text={sequence} /></div>
  {:else if name === 'mermaid/flowchart'}
    <FilePreview kind="mermaid" content={flow} path="flow.mmd" {scroll} />
  {:else if kind === 'file-preview'}
    {#if variant === 'svg'}
      <FilePreview kind="svg" content={logo} path="logo.svg" {scroll} />
    {:else}
      <FilePreview kind="markdown" content={readme} path="README.md" {scroll} />
    {/if}
  {:else if kind === 'pdf'}
    <div class="obs"><div class="obs-pdf"><PdfPreview src={pdfUrl} path="page.pdf" /></div></div>
  {:else if kind === 'lightbox'}
    <div class="pad"><MarkdownHtml text={'Screenshot attached.'} /></div>
    <AttachmentLightbox src={shotUrl} name="screenshot.png" onclose={() => {}} />
  {/if}
</div>

<style>
  .content-scene { position: absolute; inset: 0; display: flex; flex-direction: column; }
  .pad { padding: 16px; }
</style>
