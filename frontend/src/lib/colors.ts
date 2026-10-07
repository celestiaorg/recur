// Shared color/label maps (agcolor, GROUPS, PG, IG).

/** Sidebar / picker avatar colors — keep in sync with `projectColors` in services.go. */
export const projectColors = [
  'var(--cyan)',
  'var(--amber)',
  'var(--indigo)',
  'var(--emerald)',
  'var(--coral)',
] as const

export function projectPicFromName(name: string): string {
  const t = name.trim()
  if (!t) return ''
  return [...t][0]!.toLocaleUpperCase()
}

export const agColor: Record<string, string> = {
  MA: 'var(--amber)', CO: 'var(--cyan)', QA: 'var(--coral)',
  RV: 'var(--indigo)', TE: 'var(--emerald)', PR: '#b08bd8',
}

export const groupColor: Record<string, string> = {
  needs: 'var(--coral)', active: 'var(--amber)', ready: 'var(--emerald)',
}
export const groupLabel: Record<string, string> = {
  needs: 'Needs you', active: 'Working', ready: 'Ready for you',
}

// PR list groups
export const prGroupColor: Record<string, string> = {
  you: 'var(--coral)', loop: 'var(--amber)', open: 'var(--emerald)', merged: 'var(--indigo)',
}
export const prGroupLabel: Record<string, string> = {
  you: 'Needs your attention', loop: 'Workspace is responding', open: 'Open', merged: 'Merged',
}

/** GitHub-style PR lifecycle for chrome (sidebar icon tint). */
export type PRLifecycle = 'open' | 'draft' | 'merged' | 'closed'

type PRChrome = {
  state?: string
  group?: string
  draft?: boolean
  mergeStateStatus?: string
}

export function prLifecycle(pr: PRChrome | null | undefined): PRLifecycle | null {
  if (!pr) return null
  const state = (pr.state ?? '').toLowerCase()
  if (state.includes('merged')) return 'merged'
  if (state.includes('closed')) return 'closed'
  if (pr.group === 'merged') return 'merged'
  if (pr.draft || (pr.mergeStateStatus ?? '').toUpperCase() === 'DRAFT') return 'draft'
  return 'open'
}

/** Label for the PR status chip. Drafts are Draft, not the underlying review state. */
export function prStatusLabel(pr: PRChrome | null | undefined): string {
  if (prLifecycle(pr) === 'draft') return 'Draft'
  return (pr?.state ?? '').trim()
}

/** Merged and closed PRs cannot take new comments or reviews in Recur. */
export function prConversationLocked(pr: { state?: string; group?: string; draft?: boolean } | null | undefined): boolean {
  const life = prLifecycle(pr)
  return life === 'merged' || life === 'closed'
}

/** Keep a local Merged lifecycle when a stale GetPR or event still looks open. */
export function keepMergedPR<T extends PRChrome>(prev: T | undefined | null, next: T): T {
  if (!prev) return next
  if (prLifecycle(prev) === 'merged' && prLifecycle(next) !== 'merged') {
    return { ...next, state: prev.state, group: prev.group }
  }
  return next
}

/** Chrome glyph for a PR — draft dotted, closed crossed, merged git-merge. */
export function prIconName(pr?: PRChrome | null): 'git-pull-request' | 'git-pull-request-draft' | 'git-pull-request-closed' | 'git-merge' {
  const life = prLifecycle(pr)
  if (life === 'merged') return 'git-merge'
  if (life === 'closed') return 'git-pull-request-closed'
  return life === 'draft' ? 'git-pull-request-draft' : 'git-pull-request'
}

/** Sidebar / dashboard icon tint. Draft is cyan so it is not green (open) or purple (merged). Changes requested is coral — it is a blocker, not a healthy open state. */
export function prChromeColor(pr: PRChrome | null | undefined): string {
  switch (prLifecycle(pr)) {
    case 'merged':
      return 'var(--indigo)'
    case 'closed':
      return 'var(--coral)'
    case 'draft':
      return 'var(--cyan)'
    default:
      return prChangesRequested(pr) ? 'var(--coral)' : 'var(--emerald)'
  }
}

function prChangesRequested(pr: PRChrome | null | undefined): boolean {
  const state = (pr?.state ?? '').toLowerCase()
  return state.includes('changes requested')
}

function isLivePR(life: PRLifecycle | null): boolean {
  return life === 'open' || life === 'draft'
}

type WorkspacePR = {
  workspaceId?: string
  state?: string
  group?: string
  draft?: boolean
  source?: string
}

/** Lower is better: GitHub over Local, then live over settled. */
function workspacePRRank(p: WorkspacePR): number {
  const github = p.source === 'gh' ? 0 : 1
  const live = isLivePR(prLifecycle(p)) ? 0 : 1
  return github * 2 + live
}

/**
 * PR used for workspace chrome (sidebar icon, View PR).
 * A leftover Local PR must not hide a GitHub PR on the same workspace.
 */
export function prForWorkspace<T extends WorkspacePR>(
  list: readonly T[],
  workspaceId: string,
): T | undefined {
  let best: T | undefined
  let bestRank = Infinity
  for (const p of list) {
    if (p.workspaceId !== workspaceId) continue
    const rank = workspacePRRank(p)
    if (!best || rank < bestRank) {
      best = p
      bestRank = rank
    }
  }
  return best
}

// Inbox groups
export const ibGroupColor: Record<string, string> = {
  needs: 'var(--coral)', ready: 'var(--emerald)', upd: 'var(--indigo)',
}
export const ibGroupLabel: Record<string, string> = {
  needs: 'Needs you', ready: 'Ready for you', upd: 'Updates',
}
