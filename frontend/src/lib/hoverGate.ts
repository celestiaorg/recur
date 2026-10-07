/**
 * Ignore hover selection until the pointer actually moves.
 *
 * Menus that open under a stationary cursor receive a synthetic mouseenter
 * (and sometimes a same-coordinate mousemove). Without a gate, that steals
 * keyboard default selection — classic slash/@ menu bug.
 *
 * A small pixel threshold also absorbs trackpad jitter that would otherwise
 * arm the gate immediately on open.
 */

export type HoverGate = {
  armed: boolean
  hasOrigin: boolean
  originX: number
  originY: number
}

/** Minimum movement (px) before hover may change selection. */
export const HOVER_GATE_THRESHOLD_PX = 4

export function createHoverGate(): HoverGate {
  return { armed: false, hasOrigin: false, originX: 0, originY: 0 }
}

export function resetHoverGate(g: HoverGate): void {
  g.armed = false
  g.hasOrigin = false
}

/** True once the pointer has moved past the threshold from the first seen position. */
export function hoverGateAllows(
  g: HoverGate,
  x: number,
  y: number,
  thresholdPx = HOVER_GATE_THRESHOLD_PX,
): boolean {
  if (g.armed) return true
  if (!g.hasOrigin) {
    g.originX = x
    g.originY = y
    g.hasOrigin = true
    return false
  }
  const dx = x - g.originX
  const dy = y - g.originY
  if (dx * dx + dy * dy < thresholdPx * thresholdPx) return false
  g.armed = true
  return true
}
