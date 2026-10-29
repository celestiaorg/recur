// Lazy Mermaid rendering for markdown ```mermaid fences and whole-file
// (.mmd / .mermaid) Observatory Preview. Kept out of the boot graph.
import DOMPurify from 'dompurify'
import type mermaidAPI from 'mermaid'
import { get } from 'svelte/store'
import { isLightTheme, theme } from './stores'

let api: typeof mermaidAPI | null = null
let loading: Promise<typeof mermaidAPI> | null = null
let seq = 0

// Match mermaid's own post-render DOMPurify config (defense in depth — we do
// not inject its SVG via innerHTML unsanitized). The markdown.ts hook skips
// stripping id/style/class on SVG-namespace nodes so gradients/markers survive.
function sanitizeMermaidSvg(svg: string): string {
  return DOMPurify.sanitize(svg, {
    USE_PROFILES: { svg: true, svgFilters: true },
    ADD_TAGS: ['foreignobject'],
    ADD_ATTR: ['dominant-baseline'],
    HTML_INTEGRATION_POINTS: { foreignobject: true },
  })
}

function themeVariablesFromCSS(): Record<string, string> {
  const cs = getComputedStyle(document.documentElement)
  const v = (name: string) => cs.getPropertyValue(name).trim()
  return {
    background: v('--pane') || '#1c1c1c',
    primaryColor: v('--pane-2') || '#262626',
    primaryTextColor: v('--text') || '#e6e4e1',
    primaryBorderColor: v('--line-bright') || '#3d3d3d',
    secondaryColor: v('--pane') || '#1c1c1c',
    tertiaryColor: v('--bg') || '#141414',
    lineColor: v('--muted') || '#969490',
    textColor: v('--text') || '#e6e4e1',
    mainBkg: v('--pane-2') || '#262626',
    nodeBorder: v('--line-bright') || '#3d3d3d',
    clusterBkg: v('--pane') || '#1c1c1c',
    titleColor: v('--text') || '#e6e4e1',
    edgeLabelBackground: v('--pane') || '#1c1c1c',
  }
}

function configure(m: typeof mermaidAPI) {
  // Paper is light; other chrome themes are dark. themeVariables track the
  // active CSS vars so a theme switch picks up new colors on the next hydrate.
  const light = isLightTheme(get(theme))
  m.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    htmlLabels: false,
    theme: light ? 'default' : 'dark',
    themeVariables: themeVariablesFromCSS(),
    fontFamily: 'ui-sans-serif, system-ui, sans-serif',
    flowchart: { padding: 12 },
  })
}

/** Let node/edge labels paint outside mermaid's tight boxes (zoom + SVG text). */
export function unclipMermaidSvg(root: ParentNode) {
  const svg = root.querySelector('svg')
  if (!svg) return
  svg.setAttribute('overflow', 'visible')
  svg.querySelectorAll('g, foreignObject, .label, .node, .cluster, .edgeLabel, .nodeLabel').forEach((n) => {
    n.setAttribute('overflow', 'visible')
    if (n instanceof SVGElement || n instanceof HTMLElement) n.style.overflow = 'visible'
  })
}

async function loadMermaid(): Promise<typeof mermaidAPI> {
  if (api) return api
  if (!loading) {
    loading = import('mermaid').then((mod) => {
      const m = mod.default
      configure(m)
      api = m
      return m
    })
  }
  return loading
}

/** Render a Mermaid source string to a sanitized SVG (for whole-file Preview). */
export async function renderMermaidSvg(src: string): Promise<string> {
  const mermaid = await loadMermaid()
  configure(mermaid)
  const id = `file-mmd-${++seq}`
  const { svg } = await mermaid.render(id, src)
  return sanitizeMermaidSvg(svg)
}

/**
 * Find unsettled `.md-mermaid` placeholders under `root` and replace each with
 * a rendered SVG. Returns a cancel function for when the host unmounts or the
 * markdown HTML is about to be replaced.
 */
export function hydrateMermaid(root: HTMLElement): () => void {
  let cancelled = false
  const nodes = Array.from(root.querySelectorAll<HTMLElement>('.md-mermaid:not([data-rendered])'))
  if (!nodes.length) return () => { cancelled = true }

  void (async () => {
    let mermaid: typeof mermaidAPI
    try {
      mermaid = await loadMermaid()
      // Refresh CSS-derived colors (user may have switched app theme).
      configure(mermaid)
    } catch {
      for (const el of nodes) {
        if (!cancelled) el.setAttribute('data-rendered', 'error')
      }
      return
    }
    for (const el of nodes) {
      if (cancelled || !el.isConnected) return
      const src = el.querySelector('.md-mermaid-src')?.textContent ?? ''
      if (!src.trim()) {
        el.setAttribute('data-rendered', 'empty')
        continue
      }
      const id = `md-mmd-${++seq}`
      try {
        // mermaid.render serializes concurrent calls; temp nodes are cleaned up
        // by mermaid after each render.
        const { svg } = await mermaid.render(id, src)
        if (cancelled || !el.isConnected) return
        el.setAttribute('data-rendered', '1')
        el.innerHTML = sanitizeMermaidSvg(svg)
        unclipMermaidSvg(el)
      } catch {
        if (!cancelled && el.isConnected) el.setAttribute('data-rendered', 'error')
      }
    }
  })()

  return () => { cancelled = true }
}
