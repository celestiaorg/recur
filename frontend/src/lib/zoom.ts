import { get, writable } from 'svelte/store'
import { ShellService } from '../../bindings/recur/internal/app'
import { pushToast } from './stores'
import { isCmdOrCtrl } from './shortcut'

export const ZOOM_STEPS = [0.5, 0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const
const DEFAULT_ZOOM = 1
const EPS = 1e-6

/** Whole-UI zoom factor (CSS zoom on the root element). Persisted in Go-side ui-view.json. */
export const uiZoom = writable<number>(DEFAULT_ZOOM)

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
  void ShellService.SetUIZoom(next).catch(() => {})
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

/**
 * The zoom action for a keydown, or null. Shared by App and the terminal key
 * handler so both agree on the chord (Cmd on macOS, Ctrl elsewhere). Matched on
 * code and key because Shift turns = into + and layouts move the physical key.
 */
export function zoomChordAction(e: KeyboardEvent): (() => void) | null {
  if (!isCmdOrCtrl(e) || e.altKey) return null
  if (e.code === 'Equal' || e.code === 'NumpadAdd' || e.key === '=' || e.key === '+') return zoomIn
  if (e.code === 'Minus' || e.code === 'NumpadSubtract' || e.key === '-') return zoomOut
  if (e.code === 'Digit0' || e.code === 'Numpad0' || e.key === '0') return zoomReset
  return null
}
