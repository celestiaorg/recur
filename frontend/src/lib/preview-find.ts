/** Find-in-page for Observatory FilePreview (markdown / mermaid / in-page DOM). */

export const PREVIEW_FIND_CAP = 999
export const PREVIEW_FIND_HL = 'preview-find'
export const PREVIEW_FIND_HL_CUR = 'preview-find-current'

export type PreviewFindOpts = {
  caseSensitive?: boolean
  regexp?: boolean
  wholeWord?: boolean
}

export type PreviewFindMatch = { ranges: Range[]; capped: boolean; valid: boolean }

const WORD_CHAR = /[\p{L}\p{N}_]/u
const INLINE_DISPLAY = new Set(['inline', 'inline-block', 'inline-flex', 'inline-grid', 'contents', 'ruby'])
const SKIP_TAGS = /^(SCRIPT|STYLE|NOSCRIPT)$/
const SCROLL_OVERFLOW = /^(auto|scroll|overlay)$/

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function isWordCharAt(s: string, i: number): boolean {
  if (i < 0 || i >= s.length) return false
  return WORD_CHAR.test(s[i]!)
}

function isWholeWord(hay: string, start: number, end: number): boolean {
  return !isWordCharAt(hay, start - 1) && !isWordCharAt(hay, end)
}

function compileFindRegExp(query: string, opts: PreviewFindOpts): RegExp | null {
  try {
    const src = opts.regexp ? query : escapeRegExp(query)
    return new RegExp(src, opts.caseSensitive ? 'g' : 'gi')
  } catch {
    return null
  }
}

function matchesIn(hay: string, rx: RegExp, wholeWord: boolean): { start: number; end: number }[] {
  rx.lastIndex = 0
  const out: { start: number; end: number }[] = []
  for (;;) {
    const m = rx.exec(hay)
    if (!m) break
    const start = m.index
    const end = start + m[0].length
    if (end === start) {
      rx.lastIndex = start + 1
      continue
    }
    if (!wholeWord || isWholeWord(hay, start, end)) out.push({ start, end })
    if (out.length >= PREVIEW_FIND_CAP) break
  }
  return out
}

/** Substring / regexp matches in one string. `null` means the regexp is invalid. */
export function findInText(
  hay: string,
  query: string,
  opts: PreviewFindOpts = {},
): { start: number; end: number }[] | null {
  if (!query) return []
  const rx = compileFindRegExp(query, opts)
  if (!rx) return null
  return matchesIn(hay, rx, !!opts.wholeWord)
}

function isInlineEl(el: Element): boolean {
  try {
    const d = getComputedStyle(el).display
    return !d || INLINE_DISPLAY.has(d)
  } catch {
    return true
  }
}

function isUnrendered(el: Element): boolean {
  if ((el as HTMLElement).hidden) return true
  try {
    const s = getComputedStyle(el)
    return s.display === 'none' || s.visibility === 'hidden'
  } catch {
    return false
  }
}

function skipText(n: Node): boolean {
  const p = n.parentElement
  if (!p || !n.nodeValue) return true
  if (SKIP_TAGS.test(p.tagName)) return true
  if (p.closest('.file-preview-find, .mmd-toolbar')) return true
  for (let e: Element | null = p; e; e = e.parentElement) {
    if (isUnrendered(e)) return true
  }
  return false
}

function blockAncestor(n: Node, root: ParentNode): Node {
  let e = n.parentElement
  while (e && e !== root) {
    if (!isInlineEl(e)) return e
    e = e.parentElement
  }
  return root
}

type Piece = { node: Text; from: number }

function locate(pieces: Piece[], offset: number, isEnd: boolean): { node: Text; offset: number } | null {
  for (const p of pieces) {
    const to = p.from + (p.node.nodeValue?.length ?? 0)
    if (isEnd) {
      if (offset > p.from && offset <= to) return { node: p.node, offset: offset - p.from }
    } else if (offset >= p.from && offset < to) {
      return { node: p.node, offset: offset - p.from }
    }
  }
  return null
}

function collectHay(root: ParentNode): { hay: string; pieces: Piece[] } {
  const pieces: Piece[] = []
  let hay = ''
  let prevBlock: Node | null = null
  let pendingBr = false
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT)
  let node: Node | null
  while ((node = walker.nextNode())) {
    if (node.nodeType === Node.ELEMENT_NODE) {
      if ((node as Element).tagName === 'BR') pendingBr = true
      continue
    }
    if (skipText(node)) continue
    const block = blockAncestor(node, root)
    if (hay && (pendingBr || block !== prevBlock)) hay += '\n'
    pendingBr = false
    prevBlock = block
    pieces.push({ node: node as Text, from: hay.length })
    hay += node.nodeValue ?? ''
  }
  return { hay, pieces }
}

/** Collect matches in `root`'s rendered text, spanning inline node boundaries. */
export function collectPreviewFindRanges(
  root: ParentNode,
  query: string,
  opts: PreviewFindOpts = {},
): PreviewFindMatch {
  if (!query) return { ranges: [], capped: false, valid: true }
  const rx = compileFindRegExp(query, opts)
  if (!rx) return { ranges: [], capped: false, valid: false }
  const { hay, pieces } = collectHay(root)
  const ranges: Range[] = []
  for (const hit of matchesIn(hay, rx, !!opts.wholeWord)) {
    const start = locate(pieces, hit.start, false)
    const end = locate(pieces, hit.end, true)
    if (!start || !end) continue
    const r = new Range()
    r.setStart(start.node, start.offset)
    r.setEnd(end.node, end.offset)
    ranges.push(r)
    if (ranges.length >= PREVIEW_FIND_CAP) return { ranges, capped: true, valid: true }
  }
  return { ranges, capped: false, valid: true }
}

function highlights(): HighlightRegistry | undefined {
  return typeof CSS !== 'undefined' ? CSS.highlights : undefined
}

export function applyPreviewFindHighlights(ranges: Range[], current: number): void {
  const css = highlights()
  if (!css || typeof Highlight === 'undefined') return
  const rest: Range[] = []
  let cur: Range | undefined
  for (let i = 0; i < ranges.length; i++) {
    if (i === current) cur = ranges[i]
    else rest.push(ranges[i])
  }
  if (rest.length) css.set(PREVIEW_FIND_HL, new Highlight(...rest))
  else css.delete(PREVIEW_FIND_HL)
  if (cur) css.set(PREVIEW_FIND_HL_CUR, new Highlight(cur))
  else css.delete(PREVIEW_FIND_HL_CUR)
}

export function clearPreviewFindHighlights(): void {
  const css = highlights()
  if (!css) return
  css.delete(PREVIEW_FIND_HL)
  css.delete(PREVIEW_FIND_HL_CUR)
}

function axisOverflow(el: HTMLElement, axis: 'x' | 'y'): boolean {
  const s = getComputedStyle(el)
  const ov = axis === 'y' ? (s.overflowY || s.overflow) : (s.overflowX || s.overflow)
  if (!SCROLL_OVERFLOW.test(ov)) return false
  return axis === 'y' ? el.scrollHeight > el.clientHeight + 1 : el.scrollWidth > el.clientWidth + 1
}

function panMermaidCamera(viewport: HTMLElement, box: DOMRect): void {
  const stage = viewport.querySelector('.mmd-stage')
  if (!(stage instanceof HTMLElement)) return
  const view = viewport.getBoundingClientRect()
  const dx = (view.left + view.right) / 2 - (box.left + box.right) / 2
  const dy = (view.top + view.bottom) / 2 - (box.top + box.bottom) / 2
  if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return
  const cur = /translate\(\s*([-\d.]+)px\s*,\s*([-\d.]+)px\s*\)/.exec(stage.style.transform)
  const x = (cur ? Number(cur[1]) : 0) + dx
  const y = (cur ? Number(cur[2]) : 0) + dy
  stage.style.transform = `translate(${x}px, ${y}px)`
  viewport.dispatchEvent(new CustomEvent('preview-find-reveal', { detail: { x, y } }))
}

export function revealPreviewFindRange(range: Range): void {
  try {
    const node = range.startContainer
    const el = node instanceof Element ? node : node.parentElement
    if (!el) return
    let box = range.getBoundingClientRect()
    if (!box.height && !box.width) return
    let p: HTMLElement | null = el instanceof HTMLElement ? el : el.parentElement
    while (p && p !== document.body && p !== document.documentElement) {
      if (p.classList.contains('file-preview-host')) break
      if (p.classList.contains('mmd-viewport')) {
        panMermaidCamera(p, box)
        break
      }
      const view = p.getBoundingClientRect()
      if (axisOverflow(p, 'y')) {
        const delta = (box.top + box.bottom) / 2 - (view.top + view.bottom) / 2
        if (Math.abs(delta) >= 1) {
          const max = Math.max(0, p.scrollHeight - p.clientHeight)
          p.scrollTop = Math.min(max, Math.max(0, p.scrollTop + delta))
        }
      }
      if (axisOverflow(p, 'x')) {
        const delta = (box.left + box.right) / 2 - (view.left + view.right) / 2
        if (Math.abs(delta) >= 1) {
          const max = Math.max(0, p.scrollWidth - p.clientWidth)
          p.scrollLeft = Math.min(max, Math.max(0, p.scrollLeft + delta))
        }
      }
      box = range.getBoundingClientRect()
      p = p.parentElement
    }
  } catch {
    /* detached */
  }
}

export function previewFindCountLabel(index: number, total: number, capped: boolean): string {
  if (total === 0) return 'No results'
  const totalLabel = capped ? `${PREVIEW_FIND_CAP}+` : String(total)
  return `${index} / ${totalLabel}`
}
