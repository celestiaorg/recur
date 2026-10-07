// Open http(s) links in the OS browser. Recur's Wails WKWebView has no
// createWebViewWithConfiguration, so <a target="_blank"> (what markdown.ts
// stamps on every anchor) is a silent no-op. Same for xterm's default
// window.open. Route through the Wails Browser bridge in the desktop
// webview; fall back to window.open for browser / e2e previews.
//
// Absolute http(s) always leaves the app — including URLs that match this
// page's origin — so agent markdown and terminal text cannot navigate the
// Recur webview. Relative hrefs (in-app chrome like /workspaces/… shots)
// stay in the webview.
import { Browser } from '@wailsio/runtime'

/** True when this page is a Wails desktop webview (not Playwright / Vite preview). */
export function isWailsDesktop(): boolean {
  if (typeof window === 'undefined') return false
  const w = window as Window & {
    webkit?: { messageHandlers?: Record<string, unknown> }
    chrome?: { webview?: unknown }
    wails?: { invoke?: unknown }
  }
  return !!(w.webkit?.messageHandlers?.external || w.chrome?.webview || w.wails?.invoke)
}

/** Absolute http(s) URL. Relative in-app hrefs (`/workspaces/…`) are not. */
export function isExternalHttpUrl(href: string): boolean {
  const raw = href.trim()
  if (!raw) return false
  try {
    const u = new URL(raw)
    return u.protocol === 'http:' || u.protocol === 'https:'
  } catch {
    return false
  }
}

/** Resolved href to open, or null if the click should stay in the webview. */
export function externalHrefFromAnchor(a: HTMLAnchorElement): string | null {
  const raw = (a.getAttribute('href') ?? '').trim()
  if (!isExternalHttpUrl(raw)) return null
  return new URL(raw).href
}

export async function openExternalUrl(href: string): Promise<void> {
  const url = href.trim()
  if (!isExternalHttpUrl(url)) return
  if (isWailsDesktop()) {
    await Browser.OpenURL(url)
    return
  }
  window.open(url, '_blank', 'noopener,noreferrer')
}

/** Capture-phase handler: http(s) anchors open outside the app. */
export function handleExternalLinkClick(e: MouseEvent): boolean {
  if (e.defaultPrevented) return false
  const left = e.type === 'click' && e.button === 0
  const middle = e.type === 'auxclick' && e.button === 1
  if (!left && !middle) return false
  const t = e.target
  if (!(t instanceof Element)) return false
  // Markdown images zoom in AttachmentLightbox; don't steal the click for the wrapping <a>.
  if (t instanceof HTMLImageElement) return false
  const a = t.closest('a')
  if (!(a instanceof HTMLAnchorElement)) return false
  const href = externalHrefFromAnchor(a)
  if (!href) return false
  e.preventDefault()
  e.stopPropagation()
  void openExternalUrl(href)
  return true
}

let wired = false

/** Install once on document. Safe to call from main. */
export function wireExternalLinks(): void {
  if (wired || typeof document === 'undefined') return
  wired = true
  document.addEventListener('click', handleExternalLinkClick, true)
  document.addEventListener('auxclick', handleExternalLinkClick, true)
}
