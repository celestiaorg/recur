// marked + fence highlighting. No DOM — safe to run in a Worker.
// The main thread still runs DOMPurify (see markdown.ts) before {@html}.
import { marked, Marked } from 'marked'
import type { Renderer, RendererObject, Tokens } from 'marked'
import { highlightCode, langFromInfo } from './codehl'
import { expandEmojiShortcodes } from './emoji'

const MARKED_OPTS = {
  gfm: true,
  // Chat-friendly: a single newline is a line break (Claude often writes tight
  // lists/paragraphs that read wrong when joined).
  breaks: true,
  async: false as const,
}

marked.setOptions(MARKED_OPTS)

// Cursor cites code with a fenced block whose "language" is the reference
// "startLine:endLine:path" (e.g. ```22:23:src/App.svelte). Render those as a
// labeled file card with the body escaped — never as a code block tagged with a
// bogus language. The classes here are the only ones the sanitizer keeps (see
// ALLOWED_CLASSES in markdown.ts), so injected markup can't reuse them.
//
// ```mermaid fences become a placeholder; MarkdownHtml hydrates them into SVG
// after sanitize (mermaid is async / stateful and must not run inside {@html}).
// Other fences get the same lightweight sh-* highlighting as tool-card snippets.
const CODE_REF_INFO = /^(\d+):(\d+):(.+)$/

function renderCode({ text, lang }: Tokens.Code): string {
  const info = (lang ?? '').trim()
  const name = info.split(/\s+/)[0]?.toLowerCase() ?? ''
  if (name === 'mermaid') {
    return `<div class="md-mermaid"><pre class="md-mermaid-src"><code>${escapeText(text)}</code></pre></div>`
  }
  const m = info.match(CODE_REF_INFO)
  if (m) {
    return `<div class="mdref"><div class="mdref-file">${escapeText(m[3])}</div>` +
      `<pre class="mdref-code"><code>${escapeText(text)}</code></pre></div>`
  }
  return `<pre><code>${highlightCode(text, langFromInfo(info))}</code></pre>\n`
}

// Depth of nested links so a demoted image inside `[![alt](img)](href)`
// returns text (not another <a>) and the outer href stays the destination.
let linkDepth = 0

function renderLink(this: Renderer, { href, title, tokens }: Tokens.Link): string {
  // Agents often wrap a whole finding paragraph in [prose](path/to/file.go)
  // (or emit raw <a href="…">…</a>). That underlines the entire block — only
  // real http(s) links with short labels stay clickable.
  linkDepth++
  try {
    const text = this.parser.parseInline(tokens)
    const plain = text.replace(/<[^>]+>/g, '')
    const ok = !!href && /^https?:\/\//i.test(href) && plain.length <= 120
    if (!ok) return text
    const t = title ? ` title="${escapeText(title)}"` : ''
    return `<a href="${escapeText(href)}"${t}>${text}</a>`
  } finally {
    linkDepth--
  }
}

function renderHtml({ text }: Tokens.HTML | Tokens.Tag): string {
  // Keep inner text; don't surface raw tags the model emitted.
  return escapeText(text.replace(/<[^>]+>/g, ''))
}

/** GitHub paste uses HTML <img>; keep those (proxied) and still strip other tags. */
function renderHtmlWithGitHubImages({ text }: Tokens.HTML | Tokens.Tag): string {
  const imgs: string[] = []
  const withSlots = text.replace(/<img\b[^>]*>/gi, (tag) => {
    const src = htmlAttr(tag, 'src')
    if (!isGitHubImageURL(src)) return ''
    const alt = htmlAttr(tag, 'alt')
    // Wrap in <p>: a fragment that starts with <img> is dropped by DOMPurify
    // in happy-dom (markdown images already come wrapped by marked).
    imgs.push(`<p><img src="${escapeText(githubImageSrc(src))}" alt="${escapeText(alt)}"></p>`)
    return `\x00IMG${imgs.length - 1}\x00`
  })
  return escapeText(withSlots.replace(/<[^>]+>/g, '')).replace(/\x00IMG(\d+)\x00/g, (_, i) => imgs[Number(i)] ?? '')
}

function renderLiveImage({ href, title, text }: Tokens.Image): string {
  const src = githubImageSrc((href || '').trim())
  if (!src) return escapeText(text)
  const t = title ? ` title="${escapeText(title)}"` : ''
  return `<img src="${escapeText(src)}" alt="${escapeText(text)}"${t}>`
}

/** Never emit <img> — fetched pages must not load third-party URLs in the webview. */
function renderImage({ href, title, text }: Tokens.Image): string {
  const label = text.trim() || 'image'
  // Already inside a link (README badge). Outer <a> owns the href.
  if (linkDepth > 0) return escapeText(label)
  const src = (href || '').trim()
  if (src && /^https?:\/\//i.test(src) && label.length <= 120) {
    const t = title ? ` title="${escapeText(title)}"` : ''
    return `<a href="${escapeText(src)}"${t}>${escapeText(label)}</a>`
  }
  return escapeText(label)
}

const baseRenderer: RendererObject = {
  code: renderCode,
  link: renderLink,
  html: renderHtml,
}

marked.use({
  renderer: {
    code: renderCode,
    link: renderLink,
    html: renderHtmlWithGitHubImages,
    image: renderLiveImage,
  },
})

// Separate engine so agent chat / file preview keep live images; WebFetch cards
// opt in via parseMarkdown(..., { demoteImages: true }).
const markedDemote = new Marked()
markedDemote.setOptions(MARKED_OPTS)
markedDemote.use({ renderer: { ...baseRenderer, image: renderImage } })

// A model sometimes writes the code-reference fence mid-sentence instead of on
// its own line. marked then stops treating it as a code block and the body —
// often literal example HTML — leaks into the output as *live* DOM, where it can
// adopt the app's own class names (a `<div class="sb live">` becomes a real,
// absolutely-positioned status chip and collides with the surrounding text).
// Force such fences onto their own lines so they always parse as an (escaped)
// code block and nothing leaks.
function normalizeCodeRefs(src: string): string {
  return src.replace(
    /```(\d+:\d+:[^\n`]+)\n([\s\S]*?)```[ \t]*/g,
    (_m, info: string, body: string) => `\n\n\`\`\`${info}\n${body.replace(/\n?$/, '\n')}\`\`\`\n\n`,
  )
}

export function escapeText(src: string): string {
  return src
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

/** Asset-server path that streams GitHub attachment bytes with the user's gh token. */
export const GH_IMAGE_PATH = '/gh-image'

const REPO_ASSETS = /^\/[^/]+\/[^/]+\/assets\//

function htmlAttr(tag: string, name: string): string {
  const m = tag.match(new RegExp(`\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)')`, 'i'))
  return (m?.[1] ?? m?.[2] ?? '').replace(/&amp;/g, '&')
}

/** GitHub-hosted upload / CDN URLs that 404 in the webview without a token. */
export function isGitHubImageURL(href: string): boolean {
  let u: URL
  try {
    u = new URL(href.trim())
  } catch {
    return false
  }
  if (u.protocol !== 'https:') return false
  const host = u.hostname.toLowerCase()
  if (host === 'githubusercontent.com' || host.endsWith('.githubusercontent.com')) return true
  if (host !== 'github.com' && host !== 'www.github.com') return false
  const path = u.pathname
  return path.startsWith('/user-attachments/') || REPO_ASSETS.test(path)
}

/** Rewrite GitHub image URLs onto /gh-image so the asset server can authenticate. */
export function githubImageSrc(href: string): string {
  const src = (href || '').trim()
  if (!isGitHubImageURL(src)) return src
  return `${GH_IMAGE_PATH}?url=${encodeURIComponent(src)}`
}

export type ParseMarkdownOpts = {
  /** WebFetch cards: turn images into links/alt instead of live <img>. */
  demoteImages?: boolean
}

/** Unsanitized HTML from marked + highlightCode. Sanitize before {@html}. */
export function parseMarkdown(src: string, opts?: ParseMarkdownOpts): string {
  if (!src) return ''
  const prepared = normalizeCodeRefs(expandEmojiShortcodes(src, { skipCode: true }))
  const engine = opts?.demoteImages ? markedDemote : marked
  return engine.parse(prepared) as string
}
