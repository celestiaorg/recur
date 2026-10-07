// GitHub-flavored markdown → sanitized HTML for the agent chat and file preview.
// marked + highlighting run in markdown-parse.ts (Worker-safe). This file owns
// DOMPurify — chat content is LLM output rendered via {@html}, so raw HTML
// must never pass through unsanitized.
import DOMPurify from 'dompurify'
import { parseMarkdown, type ParseMarkdownOpts } from './markdown-parse'

export { escapeText } from './markdown-parse'

// The rendered HTML is embedded in the app's own DOM via {@html}, so it must
// never carry styling hooks: `class`/`id`/`style` from LLM- or file-authored
// HTML could hijack app selectors or position elements over the UI. Strip them
// all except the handful of classes this module emits itself.
const ALLOWED_CLASSES = new Set([
  'mdref', 'mdref-file', 'mdref-code',
  'md-mermaid', 'md-mermaid-src',
  // codehl / shhl token spans inside fenced blocks
  'sh-kw', 'sh-str', 'sh-com', 'sh-num', 'sh-flag', 'sh-var', 'sh-op', 'sh-prompt',
])
const SVG_NS = 'http://www.w3.org/2000/svg'
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.nodeType !== 1) return
  const el = node as Element
  // target=_blank is a no-op in the Wails webview (no createWebView);
  // open-external.ts intercepts the click and calls Browser.OpenURL.
  if (el.tagName === 'A') {
    el.setAttribute('target', '_blank')
    el.setAttribute('rel', 'noopener noreferrer')
  }
  // Mermaid SVGs (sanitized separately with USE_PROFILES.svg) need id / style /
  // class for gradients, markers, and theme CSS. DOMPurify has already stripped
  // scripts and event handlers by the time this hook runs.
  if (el.namespaceURI === SVG_NS) return
  el.removeAttribute('style')
  el.removeAttribute('id')
  if (el.hasAttribute('class')) {
    const kept = el.getAttribute('class')!.split(/\s+/).filter((c) => ALLOWED_CLASSES.has(c))
    if (kept.length) el.setAttribute('class', kept.join(' '))
    else el.removeAttribute('class')
  }
})

export function sanitizeMarkdown(html: string): string {
  if (!html) return ''
  return DOMPurify.sanitize(html, { ADD_ATTR: ['target'] })
}

export function renderMarkdown(src: string, opts?: ParseMarkdownOpts): string {
  if (!src) return ''
  return sanitizeMarkdown(parseMarkdown(src, opts))
}
