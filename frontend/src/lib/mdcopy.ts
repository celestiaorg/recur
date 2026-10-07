// Post-sanitize copy chrome for markdown code fences.
// renderMarkdown emits plain <pre>/<code> (and .mdref cards); MarkdownHtml
// mounts this after {@html} so buttons are real DOM (not LLM-controlled HTML)
// and clipboard writes go through the Wails-safe copyText helper.
import { copyText } from './clipboard'

const COPY_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/>' +
  '<path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>' +
  '</svg>'

const CHECK_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="M5 12l4 4L19 6"/>' +
  '</svg>'

function codeText(pre: HTMLElement): string {
  const code = pre.querySelector('code')
  return (code?.textContent ?? pre.textContent ?? '').replace(/\n$/, '')
}

function makeCopyButton(): HTMLButtonElement {
  const btn = document.createElement('button')
  btn.type = 'button'
  btn.className = 'md-copy'
  btn.title = 'Copy'
  btn.setAttribute('aria-label', 'Copy code')
  btn.innerHTML = COPY_ICON
  return btn
}

function flashCopied(btn: HTMLButtonElement) {
  btn.classList.add('copied')
  btn.innerHTML = CHECK_ICON
  btn.title = 'Copied'
  window.setTimeout(() => {
    btn.classList.remove('copied')
    btn.innerHTML = COPY_ICON
    btn.title = 'Copy'
  }, 1200)
}

function onCopyClick(btn: HTMLButtonElement, text: string, e: Event) {
  e.preventDefault()
  e.stopPropagation()
  if (!text) return
  void copyText(text).then(() => flashCopied(btn))
}

/** Wrap a bare <pre> and attach a hover copy control. */
function enhancePre(pre: HTMLPreElement) {
  if (pre.parentElement?.classList.contains('md-fence')) return
  const wrap = document.createElement('div')
  wrap.className = 'md-fence'
  pre.replaceWith(wrap)
  wrap.appendChild(pre)
  const btn = makeCopyButton()
  btn.addEventListener('click', (e) => onCopyClick(btn, codeText(pre), e))
  wrap.appendChild(btn)
}

/** Put a copy control on Cursor ```start:end:path``` reference cards. */
function enhanceMdref(card: HTMLElement) {
  if (card.querySelector(':scope > .md-copy')) return
  const pre = card.querySelector<HTMLPreElement>(':scope > .mdref-code')
  if (!pre) return
  const btn = makeCopyButton()
  btn.addEventListener('click', (e) => onCopyClick(btn, codeText(pre), e))
  card.appendChild(btn)
}

/**
 * Attach copy buttons to fenced code (and .mdref cards) under `root`.
 * Idempotent per node; returns a no-op cancel (DOM is discarded with {@html}).
 */
export function enhanceCodeCopy(root: HTMLElement): () => void {
  for (const pre of Array.from(root.querySelectorAll<HTMLPreElement>('pre'))) {
    if (pre.classList.contains('mdref-code')) continue
    if (pre.classList.contains('md-mermaid-src')) continue
    if (pre.closest('.md-mermaid')) continue
    enhancePre(pre)
  }
  for (const card of Array.from(root.querySelectorAll<HTMLElement>('.mdref'))) {
    enhanceMdref(card)
  }
  return () => {}
}
