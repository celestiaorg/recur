<script lang="ts">
  // Preview surface for dual-nature files (markdown / svg / html / mermaid).
  import { tick, untrack } from 'svelte'
  import MarkdownHtml from './MarkdownHtml.svelte'
  import MermaidPreview from './MermaidPreview.svelte'
  import Icon from './Icon.svelte'
  import {
    htmlPreviewSrcdoc, svgDataUrl,
    PREVIEW_IFRAME_FIND, PREVIEW_IFRAME_FIND_KEY, PREVIEW_IFRAME_FIND_RESULT,
    type PreviewKind,
  } from '../lib/preview'
  import type { PreviewScrollStore } from '../lib/preview-scroll'
  import {
    applyPreviewFindHighlights, clearPreviewFindHighlights, collectPreviewFindRanges,
    previewFindCountLabel, revealPreviewFindRange, type PreviewFindMatch,
  } from '../lib/preview-find'
  import { isCmdOrCtrl, isLetterKey } from '../lib/shortcut'

  let {
    kind,
    content = '',
    path = '',
    loaded = path,
    workspace = '',
    scroll,
    active = true,
    onSearchOpenChange,
  }: {
    kind: PreviewKind
    content?: string
    path?: string
    loaded?: string
    workspace?: string
    scroll: PreviewScrollStore
    active?: boolean
    onSearchOpenChange?: (open: boolean) => void
  } = $props()

  const svgSrc = $derived(kind === 'svg' ? svgDataUrl(content) : '')
  // allow-scripts + allow-forms only — never allow-same-origin for untrusted agent HTML (D-html-preview-scripts).
  const htmlSrcdoc = $derived(kind === 'html' ? htmlPreviewSrcdoc(content) : '')

  let findOpen = $state(false)
  let query = $state('')
  let caseSensitive = $state(false)
  let regexp = $state(false)
  let wholeWord = $state(false)
  let matchTotal = $state(0)
  let matchIndex = $state(0)
  let matchCapped = $state(false)
  let queryValid = $state(true)
  let findInput: HTMLInputElement | undefined = $state()
  let contentEl: HTMLElement | undefined = $state()
  let iframeEl: HTMLIFrameElement | undefined = $state()

  let liveRanges: Range[] = []

  const findable = $derived(kind !== 'svg')
  const countLabel = $derived(
    !query || !queryValid ? '' : previewFindCountLabel(matchIndex, matchTotal, matchCapped),
  )

  function setFindOpen(open: boolean) {
    if (findOpen === open) return
    findOpen = open
    onSearchOpenChange?.(open)
    if (!open) {
      liveRanges = []
      matchTotal = 0
      matchIndex = 0
      matchCapped = false
      queryValid = true
      if (active) clearPreviewFindHighlights()
      postFind({ op: 'clear' })
    }
  }

  function postFind(payload: Record<string, unknown>) {
    if (kind !== 'html') return
    iframeEl?.contentWindow?.postMessage({ type: PREVIEW_IFRAME_FIND, ...payload }, '*')
  }

  function sendQuery() {
    postFind({ op: 'query', search: query, caseSensitive, regexp, wholeWord })
  }

  function refreshInPage() {
    const root = contentEl
    if (!root || !query) {
      liveRanges = []
      matchTotal = 0
      matchIndex = 0
      matchCapped = false
      queryValid = true
      if (active) clearPreviewFindHighlights()
      return
    }
    const found: PreviewFindMatch = collectPreviewFindRanges(root, query, { caseSensitive, regexp, wholeWord })
    liveRanges = found.ranges
    matchTotal = found.ranges.length
    matchCapped = found.capped
    queryValid = found.valid
    const prev = untrack(() => matchIndex)
    const next = found.ranges.length === 0 ? 0 : (prev >= 1 && prev <= found.ranges.length ? prev : 1)
    matchIndex = next
    if (!active) return
    applyPreviewFindHighlights(found.ranges, next - 1)
    if (next > 0) revealPreviewFindRange(found.ranges[next - 1])
  }

  $effect(() => {
    void content
    void htmlSrcdoc
    void query
    void caseSensitive
    void regexp
    void wholeWord
    void active
    if (!findable) {
      if (findOpen) setFindOpen(false)
      return
    }
    if (!findOpen) return
    if (!active) return
    if (kind === 'html') {
      sendQuery()
      return
    }
    let cancelled = false
    void tick().then(() => { if (!cancelled) refreshInPage() })
    return () => { cancelled = true }
  })

  $effect(() => {
    const root = contentEl
    if (!root || kind === 'html' || !findable) return
    const mo = new MutationObserver(() => { if (findOpen && active) refreshInPage() })
    mo.observe(root, { subtree: true, childList: true, characterData: true })
    return () => mo.disconnect()
  })

  export function isSearchOpen(): boolean {
    return findOpen
  }

  export function toggleSearch(): boolean {
    if (!findable) return false
    if (findOpen) closeSearch()
    else void openSearch()
    return true
  }

  export async function openSearch() {
    if (!findable) return
    setFindOpen(true)
    await tick()
    findInput?.focus({ preventScroll: true })
    findInput?.select()
  }

  export function closeSearch() {
    setFindOpen(false)
  }

  function step(dir: 1 | -1) {
    if (!findOpen) return
    if (kind === 'html') {
      postFind({ op: dir > 0 ? 'next' : 'prev' })
      return
    }
    if (!liveRanges.length) return
    let i = matchIndex + dir
    if (i < 1) i = liveRanges.length
    if (i > liveRanges.length) i = 1
    matchIndex = i
    if (!active) return
    applyPreviewFindHighlights(liveRanges, i - 1)
    revealPreviewFindRange(liveRanges[i - 1])
  }

  function onFindInputKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault()
      step(e.shiftKey ? -1 : 1)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      closeSearch()
    }
  }

  function onWinKey(e: KeyboardEvent) {
    if (!active || !findable) return
    const t = e.target instanceof HTMLElement ? e.target : null
    const inFind = !!t?.closest('.file-preview-find')
    if (!inFind && t?.closest('.cm-editor')) return
    const typing = !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)
    const mod = isCmdOrCtrl(e)

    if (e.key === 'Escape' && findOpen) {
      if (typing && !inFind) return
      e.preventDefault()
      e.stopPropagation()
      closeSearch()
      return
    }
    if (mod && !e.altKey && !e.shiftKey && isLetterKey(e, 'f')) {
      if (typing && !inFind) return
      e.preventDefault()
      e.stopPropagation()
      void openSearch()
      return
    }
    if (!findOpen) return
    if ((mod && !e.altKey && isLetterKey(e, 'g')) || e.key === 'F3') {
      if (typing && !inFind) return
      e.preventDefault()
      e.stopPropagation()
      step(e.shiftKey ? -1 : 1)
      return
    }
    if (inFind && mod && e.altKey && !e.shiftKey && isLetterKey(e, 'c')) {
      e.preventDefault()
      caseSensitive = !caseSensitive
      return
    }
    if (inFind && mod && e.altKey && !e.shiftKey && isLetterKey(e, 'r')) {
      e.preventDefault()
      regexp = !regexp
      return
    }
    if (inFind && mod && e.altKey && !e.shiftKey && isLetterKey(e, 'w')) {
      e.preventDefault()
      wholeWord = !wholeWord
    }
  }

  function onWinMsg(e: MessageEvent) {
    if (kind !== 'html' || e.source !== iframeEl?.contentWindow) return
    const d = e.data
    if (!d || typeof d !== 'object') return
    if (d.type === PREVIEW_IFRAME_FIND_RESULT) {
      matchTotal = Number(d.total) || 0
      matchIndex = Number(d.index) || 0
      matchCapped = matchTotal >= 999
      queryValid = d.valid !== false
      return
    }
    if (d.type !== PREVIEW_IFRAME_FIND_KEY || !active) return
    if (d.key === 'f') void openSearch()
    else if (d.key === 'g') { if (findOpen) step(1) }
    else if (d.key === 'G') { if (findOpen) step(-1) }
    else if (d.key === 'Escape' && findOpen) closeSearch()
  }
</script>

<svelte:window onkeydowncapture={onWinKey} onmessage={onWinMsg} />

<div class="file-preview-host">
  {#if findOpen}
    <div class="file-preview-find cm-recur-search">
      <div class="cm-recur-search-row">
        <span class="cm-recur-search-glyph" aria-hidden="true"><Icon name="search" /></span>
        <input
          bind:this={findInput}
          bind:value={query}
          type="text"
          class="cm-recur-search-input"
          name="search"
          placeholder="Find"
          aria-label="Find"
          spellcheck="false"
          autocomplete="off"
          onkeydown={onFindInputKey}
        />
        <span
          class="cm-recur-search-count"
          class:none={!!query && queryValid && matchTotal === 0}
          data-empty={!query || !queryValid ? '1' : '0'}
          aria-live="polite"
        >{countLabel}</span>
        <div class="cm-recur-search-nav">
          <button type="button" class="obs-icon-btn" title="Previous match (⇧⌘G)" aria-label="Previous match" onclick={() => step(-1)}>
            <Icon name="chevron-left" />
          </button>
          <button type="button" class="obs-icon-btn" title="Next match (⌘G)" aria-label="Next match" onclick={() => step(1)}>
            <Icon name="chevron-right" />
          </button>
        </div>
        <div class="cm-recur-search-toggles">
          <button
            type="button"
            class="obs-icon-btn cm-recur-search-toggle"
            class:on={caseSensitive}
            title="Match case (⌥⌘C)"
            aria-label="Match case"
            aria-pressed={caseSensitive}
            onclick={() => { caseSensitive = !caseSensitive }}
          >
            <Icon name="case-sensitive" />
          </button>
          <button
            type="button"
            class="obs-icon-btn cm-recur-search-toggle"
            class:on={regexp}
            title="Regular expression (⌥⌘R)"
            aria-label="Regular expression"
            aria-pressed={regexp}
            onclick={() => { regexp = !regexp }}
          >
            <Icon name="regex" />
          </button>
          <button
            type="button"
            class="obs-icon-btn cm-recur-search-toggle"
            class:on={wholeWord}
            title="Whole word (⌥⌘W)"
            aria-label="Whole word"
            aria-pressed={wholeWord}
            onclick={() => { wholeWord = !wholeWord }}
          >
            <Icon name="whole-word" />
          </button>
        </div>
        <button type="button" class="obs-icon-btn cm-recur-search-close" title="Close" aria-label="Close find" onclick={() => closeSearch()}>
          <Icon name="x" />
        </button>
      </div>
    </div>
  {/if}

  {#if kind === 'markdown'}
    <div class="file-preview file-preview-md" bind:this={contentEl} {@attach scroll.attachPreviewScroll(path, loaded, workspace)}>
      {#if content.trim()}
        <MarkdownHtml text={content} class="md-doc" />
      {:else}
        <div class="file-preview-empty">Empty markdown</div>
      {/if}
    </div>
  {:else if kind === 'svg'}
    <div class="file-preview file-preview-svg" bind:this={contentEl} {@attach scroll.attachPreviewScroll(path, loaded, workspace)}>
      {#if content.trim()}
        <img src={svgSrc} alt={path || 'SVG preview'} />
      {:else}
        <div class="file-preview-empty">Empty SVG</div>
      {/if}
    </div>
  {:else if kind === 'html'}
    <div class="file-preview file-preview-html" {@attach scroll.attachPreviewScroll(path, loaded, workspace)}>
      <iframe
        bind:this={iframeEl}
        title={path || 'HTML preview'}
        aria-label={path || 'HTML preview'}
        sandbox="allow-scripts allow-forms"
        srcdoc={htmlSrcdoc}
        onload={() => { if (findOpen && active) sendQuery() }}
      ></iframe>
    </div>
  {:else if kind === 'mermaid'}
    <div class="file-preview-slot" bind:this={contentEl}>
      <MermaidPreview {content} {path} remember={scroll.rememberPreviewScroll} peek={scroll.peekPreviewScroll} />
    </div>
  {/if}
</div>
