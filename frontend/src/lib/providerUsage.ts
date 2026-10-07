import type { Snapshot as UsageSnapshot } from '../../bindings/recur/internal/usage/models'
import { providerEnabled } from './providers'

export type ProviderUsageSnapshot = UsageSnapshot

/** Extra / credit balances are overage, not the session chip. */
export function isChipWindow(w: { id?: string; usedPct?: number } | null | undefined): boolean {
  if (!w || w.id === 'extra') return false
  return Number.isFinite(Number(w.usedPct))
}

export type ChipRing = { id: string; label: string; pct: number }

export type ProviderChip = {
  id: string
  name: string
  rings: ChipRing[]
  headline: number
  ready: boolean
}

const CHIP_ORDER = ['claude-code', 'cursor', 'codex']

function ringRank(id: string): number {
  if (id === 'five_hour') return 0
  if (id === 'seven_day') return 1
  if (id.startsWith('seven_day_')) return 2
  if (id === 'billing') return 3
  return 4
}

/** Cursor usage is Recur Connect only — hide a row with no account. */
export function cursorUsageVisible(p: { id?: string; email?: string; plan?: string; sourceId?: string } | null | undefined): boolean {
  if (!p || p.id !== 'cursor') return true
  return !!p.email || !!p.plan || p.sourceId === 'cursor_connect'
}

/** Footer meter/popover hide providers that are off in Settings. */
export function usageProviderOn(
  id: string | undefined,
  disabled?: string[] | null,
): boolean {
  return !!id && providerEnabled(disabled, id)
}

/** One footer chip per provider that has session quota (not extra/credits). */
export function providerChips(s: UsageSnapshot | null | undefined, disabled?: string[] | null): ProviderChip[] {
  const out: ProviderChip[] = []
  for (const p of s?.providers ?? []) {
    if (!usageProviderOn(p.id, disabled)) continue
    if (!cursorUsageVisible(p)) continue
    const rings = (p.windows ?? [])
      .filter(isChipWindow)
      .map((w) => ({ id: w.id, label: w.label, pct: Number(w.usedPct) }))
      .sort((a, b) => ringRank(a.id) - ringRank(b.id) || a.id.localeCompare(b.id))
    if (!rings.length) continue
    out.push({
      id: p.id,
      name: p.name,
      rings,
      headline: Math.max(...rings.map((r) => r.pct)),
      ready: true,
    })
  }
  return out.sort((a, b) => {
    const ai = CHIP_ORDER.indexOf(a.id)
    const bi = CHIP_ORDER.indexOf(b.id)
    return (ai < 0 ? 99 : ai) - (bi < 0 ? 99 : bi)
  })
}

export function snapshotHasData(s: UsageSnapshot | null | undefined, disabled?: string[] | null): boolean {
  return !!s?.providers?.some((p) =>
    usageProviderOn(p.id, disabled) &&
    ((p.windows?.length ?? 0) > 0 || !!p.error || p.connecting || !!p.hint || !!p.sourceId))
}

/** True when an enabled Claude row can recover via `claude auth status`. */
export function snapshotHasProviderError(
  s: UsageSnapshot | null | undefined,
  disabled?: string[] | null,
): boolean {
  return !!s?.providers?.some((p) =>
    usageProviderOn(p.id, disabled) &&
    p.id === 'claude-code' &&
    (p.error === 'Claude Code login expired' || p.error === 'Not signed in to Claude Code'))
}

export function formatResetIn(ms: number, now = Date.now()): string {
  if (!Number.isFinite(ms) || ms <= 0) return ''
  const sec = Math.max(0, Math.round((ms - now) / 1000))
  if (sec < 60) return 'resets now'
  if (sec < 3600) return `resets in ${Math.floor(sec / 60)}m`
  if (sec < 86400) return `resets in ${Math.floor(sec / 3600)}h`
  return `resets in ${Math.floor(sec / 86400)}d`
}

/** Dashboard-style included-usage percent (`27.1%`). */
export function formatUsagePct(n: number): string {
  if (!Number.isFinite(n)) return '0.0%'
  return `${(Math.round(n * 10) / 10).toFixed(1)}%`
}

export type ModelBucket = {
  id: 'cursor' | 'other'
  label: string
  tokens: number
  usedPct: number
  models: Array<{ name: string; tokens: number; usedPct?: number; bucket?: string }>
}

/** Group Cursor aggregations the way the dashboard does. */
export function modelBuckets(models: ModelBucket['models'] | undefined | null): ModelBucket[] {
  const buckets: Record<'cursor' | 'other', ModelBucket> = {
    cursor: { id: 'cursor', label: 'Cursor Models', tokens: 0, usedPct: 0, models: [] },
    other: { id: 'other', label: 'Other Models', tokens: 0, usedPct: 0, models: [] },
  }
  for (const m of models ?? []) {
    const id = m.bucket === 'other' ? 'other' : 'cursor'
    buckets[id].models.push(m)
    buckets[id].tokens += m.tokens || 0
    buckets[id].usedPct += m.usedPct || 0
  }
  return [buckets.cursor, buckets.other]
}

export function ringColor(pct: number, ready: boolean): string {
  if (!ready) return 'var(--line)'
  if (pct >= 90) return 'var(--coral)'
  if (pct >= 70) return 'var(--amber)'
  return 'var(--muted)'
}

export function cursorQuota(s: UsageSnapshot | null | undefined) {
  return s?.providers?.find((p) => p.id === 'cursor') ?? null
}

export function canDisconnectCursor(s: UsageSnapshot | null | undefined): boolean {
  const q = cursorQuota(s)
  return q?.sourceId === 'cursor_connect'
}
