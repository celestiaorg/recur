import type { Project } from '../../bindings/recur/internal/domain/models'

export type RunCommand = { name: string; cmd: string }

/** Fields `projectRuns` / `defaultRun` actually read. */
export type ProjectRunSource = Pick<Project, 'runCommands' | 'pinnedRun'>

/** Named runs from project.toml [[run]]. */
export function projectRuns(p: ProjectRunSource | undefined | null): RunCommand[] {
  if (!p) return []
  const listed = (p.runCommands ?? [])
    .map((r) => ({ name: (r.name ?? '').trim(), cmd: (r.cmd ?? '').trim() }))
    .filter((r) => r.cmd)
    .map((r) => ({ name: r.name || 'run', cmd: r.cmd }))
  return listed.length ? uniquify(listed) : []
}

/** Pinned command when it still exists, otherwise the first. */
export function defaultRun(p: ProjectRunSource | undefined | null): RunCommand | null {
  const runs = projectRuns(p)
  if (!runs.length) return null
  const pin = (p?.pinnedRun ?? '').trim()
  if (pin) {
    const hit = runs.find((r) => r.name === pin)
    if (hit) return hit
  }
  return runs[0]!
}

export function runLabel(verb: 'Run' | 'Stop', name: string): string {
  const n = name.trim() || 'dev'
  return `${verb} ${n}`
}

/** Substring match on name or command. Empty query keeps the incoming order. */
export function filterRuns<T extends { name: string; cmd: string }>(list: T[], query: string): T[] {
  const q = query.trim().toLowerCase()
  if (!q) return list
  return list.filter(
    (r) => r.name.toLowerCase().includes(q) || r.cmd.toLowerCase().includes(q),
  )
}

/** Move the pinned run to the front; no-op if it isn't in the list. */
export function orderRuns<T extends { name: string }>(list: T[], pinned: string): T[] {
  if (!pinned) return list
  const i = list.findIndex((t) => t.name === pinned)
  if (i <= 0) return list
  return [list[i]!, ...list.slice(0, i), ...list.slice(i + 1)]
}

function uniquify(runs: RunCommand[]): RunCommand[] {
  const used = new Map<string, number>()
  return runs.map((r) => {
    let name = r.name
    const n = used.get(name) ?? 0
    used.set(name, n + 1)
    if (n > 0) {
      let i = n + 1
      name = `${r.name}-${i}`
      while (used.has(name)) {
        i++
        name = `${r.name}-${i}`
      }
      used.set(name, 1)
    }
    return { name, cmd: r.cmd }
  })
}
