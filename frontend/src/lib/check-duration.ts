import { isPendingCheck } from './pr-checks'

/** Clock origin for interpolating a pending check that has no startedAt. */
export type PendingClock = {
  detail: string
  baseSec: number
  originMs: number
}

export type CheckDurationInput = {
  name?: string
  url?: string
  status?: string
  detail?: string
  startedAt?: string
}

export function checkClockKey(c: { name?: string; url?: string }): string {
  return `${c.name ?? ''}\0${c.url ?? ''}`
}

export function prunePendingClocks(clocks: Map<string, PendingClock>, checks: CheckDurationInput[]) {
  const keep = new Set<string>()
  for (const c of checks) {
    if (isPendingCheck(c)) keep.add(checkClockKey(c))
  }
  for (const k of clocks.keys()) {
    if (!keep.has(k)) clocks.delete(k)
  }
}

/** Match Go formatCheckDuration: 45s, 1m 4s, 12m, 1h, 1h 2m. */
export function formatCheckDuration(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0) sec = 0
  sec = Math.round(sec)
  if (sec < 60) return `${sec}s`
  let m = Math.floor(sec / 60)
  const s = sec % 60
  if (m >= 60) {
    const h = Math.floor(m / 60)
    m %= 60
    if (m === 0) return `${h}h`
    return `${h}h ${m}m`
  }
  if (s === 0) return `${m}m`
  return `${m}m ${s}s`
}

export function parseCheckDuration(detail: string): number | null {
  const s = detail.trim()
  if (!s) return null
  let m = /^(\d+)h(?:\s+(\d+)m)?$/.exec(s)
  if (m) return Number(m[1]) * 3600 + Number(m[2] ?? 0) * 60
  m = /^(\d+)m(?:\s+(\d+)s)?$/.exec(s)
  if (m) return Number(m[1]) * 60 + Number(m[2] ?? 0)
  m = /^(\d+)s$/.exec(s)
  if (m) return Number(m[1])
  return null
}

/**
 * Live label for a check duration. Pending rows tick from startedAt, or from
 * the last snapshot in `detail` until the next refresh replaces it. Completed
 * rows return `detail` (the final duration) and drop any interpolating clock.
 */
export function liveCheckDetail(
  check: CheckDurationInput,
  nowMs: number,
  clocks: Map<string, PendingClock>,
  key = checkClockKey(check),
): string {
  const detail = check.detail ?? ''
  if (!isPendingCheck(check)) {
    clocks.delete(key)
    return detail
  }
  const start = Date.parse(check.startedAt ?? '')
  if (check.startedAt && !Number.isNaN(start)) {
    clocks.delete(key)
    return formatCheckDuration(Math.max(0, Math.floor((nowMs - start) / 1000)))
  }
  const parsed = parseCheckDuration(detail)
  if (parsed == null) return detail
  let clock = clocks.get(key)
  if (!clock || clock.detail !== detail) {
    clock = { detail, baseSec: parsed, originMs: nowMs }
    clocks.set(key, clock)
  }
  return formatCheckDuration(clock.baseSec + Math.max(0, Math.floor((nowMs - clock.originMs) / 1000)))
}
