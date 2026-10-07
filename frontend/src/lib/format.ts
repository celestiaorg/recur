/** Compact age for UI chrome (`just now`, `12m`, `3h`, `2d`, `3w`, `2mo`, `1y`)
 *  plus a full tooltip stamp. Days run through 13d; 14 days and up step to
 *  weeks, then months, then years. `ms` is unix milliseconds. */
export function formatRelativeAge(ms: number, now = Date.now()): { rel: string; full: string } | null {
  if (!Number.isFinite(ms) || ms <= 0) return null
  const full = new Date(ms).toLocaleString(undefined, {
    month: 'short', day: 'numeric', year: 'numeric',
    hour: 'numeric', minute: '2-digit',
  })
  const sec = Math.max(0, Math.round((now - ms) / 1000))
  const minute = 60
  const hour = 3600
  const day = 86400
  const week = 7 * day
  const month = 30 * day
  const year = 365 * day
  let rel: string
  if (sec < minute) rel = 'just now'
  else if (sec < hour) rel = `${Math.floor(sec / minute)}m`
  else if (sec < day) rel = `${Math.floor(sec / hour)}h`
  else if (sec < 14 * day) rel = `${Math.floor(sec / day)}d`
  else if (sec < month) rel = `${Math.floor(sec / week)}w`
  else if (sec < year) rel = `${Math.floor(sec / month)}mo`
  else rel = `${Math.floor(sec / year)}y`
  return { rel, full }
}

/** Live turn clock: `0:00` … `59:59`, then `1:00:00` (`h:mm:ss`). */
export function formatElapsed(sec: number): string {
  sec = Math.max(0, Math.floor(sec))
  const s = String(sec % 60).padStart(2, '0')
  if (sec >= 3600) {
    const h = Math.floor(sec / 3600)
    const m = Math.floor((sec % 3600) / 60)
    return `${h}:${String(m).padStart(2, '0')}:${s}`
  }
  return `${Math.floor(sec / 60)}:${s}`
}

/** Compact integer label for UI chrome: 460, 1.2k, 184k, 1.0M. */
export function formatCompactCount(n: number): string {
  const v = Math.max(0, Math.round(n))
  if (v < 1000) return String(v)
  if (v < 10_000) {
    const k = v / 1000
    return `${k.toFixed(1).replace(/\.0$/, '')}k`
  }
  if (v < 1_000_000) return `${Math.round(v / 1000)}k`
  const m = v / 1_000_000
  return `${m.toFixed(1).replace(/\.0$/, '')}M`
}
