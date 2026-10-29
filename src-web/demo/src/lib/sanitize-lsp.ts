import DOMPurify from 'dompurify'
import { highlightHtmlCodeBlocks } from './codehl'

export function stripDangerousHtml(html: string): string {
  return html
    .replace(/<script\b[\s\S]*?<\/script>/gi, '')
    .replace(/\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, '')
    .replace(/javascript:/gi, '')
}

export function sanitizeLspHTML(html: string): string {
  const tinted = highlightHtmlCodeBlocks(html)
  let clean = tinted
  try {
    if (typeof DOMPurify?.sanitize === 'function') clean = DOMPurify.sanitize(tinted)
  } catch {
    /* no Window / incomplete DOM */
  }
  return stripDangerousHtml(clean)
}
