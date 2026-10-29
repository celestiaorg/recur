// GitHub-flavored markdown → sanitized HTML for chat, PR comments and README
// preview. Rendered via {@html}, so it always passes through DOMPurify.
import DOMPurify from 'dompurify'
import { marked } from 'marked'

const cache = new Map<string, string>()

export function renderMarkdown(src: string): string {
  let html = cache.get(src)
  if (html === undefined) {
    html = DOMPurify.sanitize(marked.parse(src, { gfm: true, breaks: true, async: false }))
    cache.set(src, html)
  }
  return html
}
