/** Inputs the titlebar merge-slot picker needs. Extra PR fields are ignored. */
import { checksRollup } from './pr-checks'

export const MERGE_METHODS = ['merge', 'squash', 'rebase'] as const
export type MergeMethodKey = (typeof MERGE_METHODS)[number]

export type PRMergeInput = {
  workspaceId?: string
  source?: string
  state?: string
  group?: string
  draft?: boolean
  updatingBase?: string
  behindBase?: number
  checksOk?: boolean
  hasChecks?: boolean
  checks?: { passed: boolean; status?: string; detail?: string }[]
  viewerLogin?: string
  viewerCanWrite?: boolean
  authorLogin?: string
  mergeable?: string
  mergeStateStatus?: string
  reviewDecision?: string
  allowedMergeMethods?: string[]
  base?: string
  mergeQueueEnabled?: boolean
  mergeQueued?: boolean
}

export type MergeActionKind =
  | 'hidden'
  | 'updating'
  | 'ready'
  | 'update-base'
  | 'conflicts'
  | 'checks'
  | 'blocked'
  | 'queued'
  | 'merge'

export type MergeAction = {
  kind: MergeActionKind
  label: string
  title: string
  disabled: boolean
}

const HIDDEN: MergeAction = { kind: 'hidden', label: '', title: '', disabled: true }

function upper(s?: string): string {
  return (s ?? '').trim().toUpperCase()
}

function viewerIsAuthor(pr: PRMergeInput): boolean {
  const viewer = (pr.viewerLogin ?? '').trim()
  const author = (pr.authorLogin ?? '').trim()
  return !!viewer && !!author && viewer.toLowerCase() === author.toLowerCase()
}

function reviewApproved(pr: PRMergeInput): boolean {
  if (upper(pr.reviewDecision) === 'APPROVED') return true
  return (pr.state ?? '') === 'Approved'
}

function reviewBlocks(pr: PRMergeInput): boolean {
  const decision = upper(pr.reviewDecision)
  if (decision === 'REVIEW_REQUIRED' || decision === 'CHANGES_REQUESTED') return true
  return (pr.state ?? '') === 'Changes requested'
}

/** One merge-adjacent titlebar slot for this PR. Local PRs always merge. */
export function prMergeAction(pr: PRMergeInput | null | undefined, opts?: { awaitingUpdate?: boolean }): MergeAction {
  if (!pr) return HIDDEN
  const state = pr.state ?? ''
  const group = pr.group ?? ''
  if (state === 'Merged' || group === 'merged' || state === 'Closed') return HIDDEN

  const updating = !!(opts?.awaitingUpdate || pr.updatingBase)
  if (updating) {
    return { kind: 'updating', label: 'Updating…', title: '', disabled: true }
  }

  const isLocal = pr.source !== 'gh'
  if (isLocal) {
    return {
      kind: 'merge',
      label: 'Merge',
      title: 'Local merge — you are the only gate; merges the branch into the base on disk',
      disabled: false,
    }
  }

  if (pr.mergeQueued) {
    return { kind: 'queued', label: 'Queued', title: 'Waiting in the GitHub merge queue', disabled: true }
  }

  const status = upper(pr.mergeStateStatus)
  const mergeable = upper(pr.mergeable)
  const dirty = status === 'DIRTY' || mergeable === 'CONFLICTING'
  const draft = !!pr.draft || status === 'DRAFT'
  const canWrite = !!pr.viewerCanWrite
  const knownViewer = !!(pr.viewerLogin ?? '').trim()
  const canReady = !knownViewer || canWrite || viewerIsAuthor(pr)

  if (draft && canReady) {
    return {
      kind: 'ready',
      label: 'Ready for review',
      title: 'Mark this draft ready for review on GitHub',
      disabled: false,
    }
  }

  if (knownViewer && !canWrite) return HIDDEN

  if (dirty) {
    return {
      kind: 'conflicts',
      label: 'Conflicts',
      title: 'Conflicts — resolve them on the branch, then refresh',
      disabled: true,
    }
  }
  const hasWorkspace = !!(pr.workspaceId ?? '').trim()
  const base = (pr.base ?? 'base').trim() || 'base'
  if (hasWorkspace && status === 'BEHIND') {
    return {
      kind: 'update-base',
      label: updateWithBaseLabel('merge', base),
      title: updateBaseTitle(pr, base),
      disabled: false,
    }
  }

  if (status === 'BLOCKED') {
    if (checksRollup(pr.checks, { checksOk: pr.checksOk, hasChecks: pr.hasChecks }).kind === 'fail') {
      return {
        kind: 'checks',
        label: 'Fix checks',
        title: 'Ask an agent to fix the failing GitHub checks',
        disabled: false,
      }
    }
    if (reviewBlocks(pr) && !reviewApproved(pr)) {
      return HIDDEN
    }
    return {
      kind: 'blocked',
      label: 'Merge blocked',
      title: 'Merge blocked on GitHub — required reviews, checks, or unresolved conversations',
      disabled: true,
    }
  }

  const unstable = status === 'UNSTABLE'
  if (pr.mergeQueueEnabled) {
    return {
      kind: 'merge',
      label: 'Add to merge queue',
      title: 'The base branch requires a merge queue; GitHub merges the PR once it passes the queue',
      disabled: false,
    }
  }
  return {
    kind: 'merge',
    label: 'Merge',
    title: unstable
      ? 'Some checks are not green, but GitHub still allows merge'
      : 'Merge the PR',
    disabled: false,
  }
}

/** Update with base is independent of the merge slot — review-blocked PRs still catch up. */
export function showPRUpdate(pr: PRMergeInput | null | undefined, action: MergeAction): boolean {
  if (!pr || !(pr.workspaceId ?? '').trim()) return false
  if (action.kind === 'updating') return false
  if (action.kind === 'update-base') return true
  if ((pr.behindBase ?? 0) <= 0) return false
  const state = pr.state ?? ''
  if (state === 'Merged' || pr.group === 'merged' || state === 'Closed') return false
  const known = !!(pr.viewerLogin ?? '').trim()
  if (known && !pr.viewerCanWrite) return false
  return true
}

function isMergeMethod(s: string): s is MergeMethodKey {
  return (MERGE_METHODS as readonly string[]).includes(s)
}

/** Author or write access can flip GitHub draft state. Unknown viewer is allowed. */
export function canSetPRDraft(pr?: PRMergeInput | null): boolean {
  if (!pr || pr.source !== 'gh') return false
  const state = pr.state ?? ''
  if (state === 'Merged' || pr.group === 'merged' || state === 'Closed') return false
  const known = !!(pr.viewerLogin ?? '').trim()
  if (!known) return true
  return !!pr.viewerCanWrite || viewerIsAuthor(pr)
}

/** GitHub forbids Approve / Request changes on your own PR. Local and unknown viewer still offer it. */
export function canApprovePRReview(pr?: PRMergeInput | null): boolean {
  if (!pr) return false
  const state = pr.state ?? ''
  if (state === 'Merged' || pr.group === 'merged' || state === 'Closed') return false
  if (pr.source !== 'gh') return true
  if (viewerIsAuthor(pr)) return false
  const known = !!(pr.viewerLogin ?? '').trim()
  if (!known) return true
  return !!pr.viewerCanWrite
}

/** Repo-allowed merge types. Empty/unknown (or Local) → all three. */
export function allowedMergeMethods(pr?: Pick<PRMergeInput, 'source' | 'allowedMergeMethods'> | null): MergeMethodKey[] {
  if (!pr || pr.source !== 'gh') return [...MERGE_METHODS]
  const raw = (pr.allowedMergeMethods ?? [])
    .map((s) => s.trim().toLowerCase())
    .filter(isMergeMethod)
  return raw.length ? raw : [...MERGE_METHODS]
}

export function pickAllowedMergeMethod(preferred: string, allowed: readonly string[]): MergeMethodKey {
  const pref = preferred.trim().toLowerCase()
  if (isMergeMethod(pref) && allowed.includes(pref)) return pref
  const first = allowed[0]
  return first && isMergeMethod(first) ? first : 'merge'
}

export const UPDATE_BASE_METHODS = ['merge', 'rebase'] as const
export type UpdateBaseMethodKey = (typeof UPDATE_BASE_METHODS)[number]

const UPDATE_BASE_METHOD_SET = new Set<string>(UPDATE_BASE_METHODS)

export function isUpdateBaseMethod(s: string): s is UpdateBaseMethodKey {
  return UPDATE_BASE_METHOD_SET.has(s)
}

/** Strip origin/upstream so the titlebar can say "Update with main". */
export function shortBaseName(base?: string): string {
  const b = (base ?? '').trim()
  const m = /^(origin|upstream)\/(.+)$/.exec(b)
  return (m ? m[2] : b) || 'base'
}

/** Project override → settings default → merge, then an optional UI pick. */
export function resolveUpdateBaseMethod(
  preferred?: string,
  fallback?: string,
  override?: string | null,
): UpdateBaseMethodKey {
  if (override && isUpdateBaseMethod(override)) return override
  if (preferred && isUpdateBaseMethod(preferred)) return preferred
  if (fallback && isUpdateBaseMethod(fallback)) return fallback
  return 'merge'
}

export function updateWithBaseLabel(method: string, base?: string): string {
  const name = shortBaseName(base)
  return method === 'rebase' ? `Rebase onto ${name}` : `Update with ${name}`
}

function updateBaseTitle(pr: PRMergeInput, base: string): string {
  const n = pr.behindBase ?? 0
  const name = shortBaseName(base)
  if (n > 0) return `${n} commit${n === 1 ? '' : 's'} on ${name} not in this branch`
  return `This branch is behind ${name} on GitHub`
}
