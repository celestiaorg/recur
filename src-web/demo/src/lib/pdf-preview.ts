export const MIN_SCALE = 0.1
export const MAX_SCALE = 8
export const ZOOM_STEP = 1.25

function clampScale(s: number): number {
  if (!Number.isFinite(s)) return 1
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, s))
}

export function clampPage(n: number, pages: number): number {
  if (!Number.isFinite(n) || pages < 1) return 1
  return Math.min(pages, Math.max(1, Math.round(n)))
}

export function zoomStep(scale: number, dir: 1 | -1): number {
  return clampScale(dir > 0 ? scale * ZOOM_STEP : scale / ZOOM_STEP)
}

export function fitWidthScale(pageWidthPx: number, viewportWidthPx: number): number {
  if (!(pageWidthPx > 0) || !(viewportWidthPx > 0)) return 1
  return clampScale(viewportWidthPx / pageWidthPx)
}

export function pageAtScroll(
  scrollTop: number,
  scaledHeights: number[],
  gap: number,
  maxScrollTop?: number,
  currentPage?: number,
): number {
  if (scaledHeights.length === 0) return 1
  const y = Number.isFinite(scrollTop) ? scrollTop : 0
  let offset = 0
  let best = 1
  let bestDist = Infinity
  for (let i = 0; i < scaledHeights.length; i++) {
    const h = scaledHeights[i] ?? 0
    const top = offset
    const bottom = offset + h
    const dist = y < top ? top - y : y > bottom ? y - bottom : 0
    if (dist < bestDist) {
      bestDist = dist
      best = i + 1
    }
    offset = bottom + gap
  }
  if (
    currentPage != null &&
    currentPage > best &&
    maxScrollTop != null &&
    maxScrollTop > 0 &&
    Number.isFinite(maxScrollTop) &&
    y >= maxScrollTop - 1
  ) {
    return clampPage(currentPage, scaledHeights.length)
  }
  return best
}

export function pageOriginInScroller(
  scroller: Pick<HTMLElement, 'getBoundingClientRect' | 'scrollTop'>,
  firstSlot: Pick<HTMLElement, 'getBoundingClientRect'> | null,
  fallback: number,
): number {
  if (!firstSlot) return fallback
  return firstSlot.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop
}

export function discardPageCanvas(canvas: HTMLCanvasElement): void {
  canvas.width = 0
  canvas.height = 0
}

export function pdfjsAssetDirs(baseURI: string, basePath = '/'): {
  cMapUrl: string
  standardFontDataUrl: string
  wasmUrl: string
} {
  const root = new URL(basePath.endsWith('/') ? basePath : `${basePath}/`, baseURI).href
  return {
    cMapUrl: new URL('pdfjs/cmaps/', root).href,
    standardFontDataUrl: new URL('pdfjs/standard_fonts/', root).href,
    wasmUrl: new URL('pdfjs/wasm/', root).href,
  }
}
