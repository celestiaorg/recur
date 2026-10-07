import { get, writable } from 'svelte/store'
import { ShellService } from '../../bindings/recur/internal/app'

/** Open schedule rows on the dashboard (and project Settings → Schedules). */
export const scheduleExpanded = writable<Record<string, boolean>>({})

export function expandedFromIds(ids: readonly string[] | null | undefined): Record<string, boolean> {
  const next: Record<string, boolean> = {}
  for (const id of ids ?? []) {
    if (typeof id === 'string' && id) next[id] = true
  }
  return next
}

export function idsFromExpanded(expanded: Record<string, boolean>): string[] {
  return Object.keys(expanded).filter((id) => expanded[id])
}

/** Load expanded rows from Go-side schedule-view.json (same lifecycle as shell.json). */
export async function loadScheduleExpanded(): Promise<void> {
  try {
    scheduleExpanded.set(expandedFromIds(await ShellService.GetExpandedSchedules()))
  } catch {
    /* first run / no bridge */
  }
}

export function toggleScheduleExpanded(id: string): void {
  const next = { ...get(scheduleExpanded) }
  if (next[id]) delete next[id]
  else next[id] = true
  scheduleExpanded.set(next)
  void ShellService.SetExpandedSchedules(idsFromExpanded(next)).catch(() => {})
}
