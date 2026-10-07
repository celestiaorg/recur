import { writable, derived, get } from 'svelte/store'

/** One LSP diagnostic surfaced in the Problems panel. */
export type LspProblem = {
  uri: string
  /** Repo-relative path when known. */
  path: string
  severity: 'error' | 'warning' | 'info' | 'hint'
  message: string
  line: number
  character: number
  endLine: number
  endCharacter: number
  source?: string
  code?: string | number
  sessionId?: string
  workspaceId?: string
}

const byUri = writable<Map<string, LspProblem[]>>(new Map())

/** Flat list sorted by path, then line. */
export const lspProblems = derived(byUri, (m) => {
  const all: LspProblem[] = []
  for (const list of m.values()) all.push(...list)
  all.sort((a, b) => a.path.localeCompare(b.path) || a.line - b.line || a.character - b.character)
  return all
})

export function lspProblemsForWorkspace(list: LspProblem[], workspaceId: string): LspProblem[] {
  if (!workspaceId) return list
  return list.filter((p) => p.workspaceId === workspaceId)
}

export function countLspProblems(list: LspProblem[]) {
  let errors = 0
  let warnings = 0
  for (const p of list) {
    if (p.severity === 'error') errors++
    else if (p.severity === 'warning') warnings++
  }
  return { errors, warnings, total: list.length }
}

export const lspProblemCounts = derived(lspProblems, countLspProblems)

export type LspPathProblemFlag = {
  errors: number
  warnings: number
  count: number
  /** Worst severity on this path (for explorer tint). */
  severity: 'error' | 'warning' | 'info'
}

export function groupLspProblemsByPath(list: LspProblem[]): Map<string, LspPathProblemFlag> {
  const m = new Map<string, LspPathProblemFlag>()
  for (const p of list) {
    if (!p.path) continue
    let cur = m.get(p.path)
    if (!cur) cur = { errors: 0, warnings: 0, count: 0, severity: 'info' }
    cur.count++
    if (p.severity === 'error') {
      cur.errors++
      cur.severity = 'error'
    } else if (p.severity === 'warning') {
      cur.warnings++
      if (cur.severity !== 'error') cur.severity = 'warning'
    }
    m.set(p.path, cur)
  }
  return m
}

/** Per repo-relative path → error/warning counts for explorer badges. */
export const lspProblemsByPath = derived(lspProblems, groupLspProblemsByPath)

export function lspProblemsForPath(
  byPath: Map<string, LspPathProblemFlag>,
  path: string,
): LspPathProblemFlag | null {
  if (!path) return null
  return byPath.get(path) ?? null
}

export function setLspProblemsForUri(uri: string, problems: LspProblem[], sessionKey = '') {
  byUri.update((m) => {
    const next = new Map(m)
    const key = sessionKey + '\0' + uri
    if (problems.length === 0) next.delete(key)
    else next.set(key, problems)
    return next
  })
}

export function clearLspProblems() {
  byUri.set(new Map())
}

export function clearLspProblemsForSessionUris(uris: string[]) {
  if (!uris.length) return
  byUri.update((m) => {
    const next = new Map(m)
    for (const u of uris) next.delete(u)
    return next
  })
}

export function peekLspProblems(): LspProblem[] {
  return get(lspProblems)
}
