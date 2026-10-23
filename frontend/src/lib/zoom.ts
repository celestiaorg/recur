import { get, writable } from 'svelte/store'
import { ShellService } from '../../bindings/recur/internal/app'
import { pushToast } from './stores'

export const ZOOM_STEPS = [0.5, 0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const
const DEFAULT_ZOOM = 1
const PERSIST_DELAY_MS = 300
const EPS = 1e-6

/** Whole-UI zoom factor (CSS zoom on the root element). Persisted in Go-side ui-view.json. */
export const uiZoom = writable<number>(DEFAULT_ZOOM)

let persistTimer: ReturnType<typeof setTimeout> | undefined

/** Apply a factor to the document; the resize event makes xterm and CodeMirror re-measure. */
export function applyZoom(zoom: number): void {
  document.documentElement.style.zoom = String(zoom)
  window.dispatchEvent(new Event('resize'))
}

function clampToSteps(v: number): number {
  if (!Number.isFinite(v)) return DEFAULT_ZOOM
  return Math.min(ZOOM_STEPS[ZOOM_STEPS.length - 1], Math.max(ZOOM_STEPS[0], v))
}

function setZoom(next: number): void {
  if (Math.abs(next - get(uiZoom)) < EPS) return
  uiZoom.set(next)
  applyZoom(next)
  pushToast('info', `Zoom ${Math.round(next * 100)}%`, { replace: 'ui-zoom' })
  clearTimeout(persistTimer)
  persistTimer = setTimeout(() => {
    void ShellService.SetUIZoom(next).catch(() => {})
  }, PERSIST_DELAY_MS)
}

export function zoomIn(): void {
  const cur = get(uiZoom)
  setZoom(ZOOM_STEPS.find((s) => s > cur + EPS) ?? ZOOM_STEPS[ZOOM_STEPS.length - 1])
}

export function zoomOut(): void {
  const cur = get(uiZoom)
  setZoom([...ZOOM_STEPS].reverse().find((s) => s < cur - EPS) ?? ZOOM_STEPS[0])
}

export function zoomReset(): void {
  setZoom(DEFAULT_ZOOM)
}

/** Load the saved level and apply it without a toast. */
export async function initZoom(): Promise<void> {
  try {
    const zoom = clampToSteps(await ShellService.GetUIZoom())
    uiZoom.set(zoom)
    if (Math.abs(zoom - DEFAULT_ZOOM) > EPS) applyZoom(zoom)
  } catch {
    /* first run / no bridge */
  }
}
