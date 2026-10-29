/** Seconds the first thinking window stays open before collapsing like a settled turn. */
export const FIRST_CHECKPOINT_SEC = 20

export function isFirstCheckpoint(
  sending: boolean,
  elapsedSec: number,
  rows: { tool?: unknown; thinking?: boolean }[],
): boolean {
  if (!sending || elapsedSec >= FIRST_CHECKPOINT_SEC || rows.length === 0) return false
  return rows.every((r) => !!r.thinking && !r.tool)
}

/** Missing expand state: open only during the first thinking window. */
export function liveGroupOpen(explicit: boolean | undefined, firstCheckpoint: boolean): boolean {
  return explicit ?? firstCheckpoint
}

export type TurnBoundary = {
  me?: boolean
  turnEnd?: boolean
  log?: unknown
  tool?: unknown
  thinking?: boolean
  compact?: boolean
  fallback?: boolean
  streaming?: boolean
}

export function hasLaterStep(list: TurnBoundary[], i: number): boolean {
  for (let j = i + 1; j < list.length; j++) {
    const m = list[j]
    if (m.me || m.turnEnd || m.log) break
    if (m.tool || m.thinking || m.fallback) return true
  }
  return false
}

/** Inner "I'll look next" prose: hide as a bubble once more tools follow. */
export function foldInnerNarration(m: { streaming?: boolean }, laterStep: boolean): boolean {
  return laterStep && !m.streaming
}

export function foldSubagentNotice(
  m: { note?: boolean; noteLabel?: string },
  laterStep: boolean,
): boolean {
  return !!m.note && m.noteLabel === 'Subagent' && laterStep
}
