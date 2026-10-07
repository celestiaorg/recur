export type ArmedConfirm = {
  armed: boolean
  eatClick: boolean
}

export function createArmedConfirm(): ArmedConfirm {
  return { armed: false, eatClick: false }
}

export function resetArmedConfirm(g: ArmedConfirm): void {
  g.armed = false
  g.eatClick = false
}

/** Swallow the click that follows a pointer-up confirm. Clear even if the control unmounts. */
export function markClickEcho(g: { eatClick: boolean }): void {
  g.eatClick = true
  setTimeout(() => {
    g.eatClick = false
  }, 0)
}

export function takeClickEcho(g: { eatClick: boolean }): boolean {
  if (!g.eatClick) return false
  g.eatClick = false
  return true
}

/** True when this release should confirm. Does not arm. Drag-out never calls this. */
export function armedPointerUp(
  g: ArmedConfirm,
  button = 0,
  disabled = false,
  inspectReady = true,
): boolean {
  if (button !== 0 || disabled || !g.armed || !inspectReady) return false
  g.armed = false
  markClickEcho(g)
  return true
}

export function armedClick(g: ArmedConfirm, disabled = false, inspectReady = true): 'arm' | 'confirm' | null {
  if (takeClickEcho(g)) return null
  if (disabled) return null
  if (!g.armed) {
    g.armed = true
    return 'arm'
  }
  if (!inspectReady) return null
  g.armed = false
  return 'confirm'
}
