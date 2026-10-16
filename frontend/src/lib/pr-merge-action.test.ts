import { describe, expect, it } from 'vitest'
import { allowedMergeMethods, canApprovePRReview, canSetPRDraft, pickAllowedMergeMethod, prMergeAction, resolveUpdateBaseMethod, shortBaseName, showPRUpdate, updateWithBaseLabel, type PRMergeInput } from './pr-merge-action'

const gh = (over: Partial<PRMergeInput> = {}): PRMergeInput => ({
  source: 'gh',
  state: 'In review',
  group: 'you',
  checksOk: true,
  viewerCanWrite: true,
  viewerLogin: 'josh',
  workspaceId: 'ws-1',
  ...over,
})

describe('prMergeAction', () => {
  it('hides merged and closed PRs', () => {
    expect(prMergeAction(gh({ state: 'Merged', group: 'merged' })).kind).toBe('hidden')
    expect(prMergeAction(gh({ state: 'Closed' })).kind).toBe('hidden')
  })

  it('labels merge-queue branches and disables Queued while waiting', () => {
    expect(prMergeAction(gh({ mergeStateStatus: 'CLEAN', mergeQueueEnabled: true })).label).toBe('Add to merge queue')
    const queued = prMergeAction(gh({ mergeQueueEnabled: true, mergeQueued: true }))
    expect(queued.kind).toBe('queued')
    expect(queued.disabled).toBe(true)
  })

  it('shows Updating while catching up', () => {
    expect(prMergeAction(gh({ updatingBase: 'git' })).kind).toBe('updating')
    expect(prMergeAction(gh(), { awaitingUpdate: true }).kind).toBe('updating')
  })

  it('keeps Merge for Local PRs even when behind', () => {
    const act = prMergeAction({ source: 'local', state: 'In review', behindBase: 2 })
    expect(act.kind).toBe('merge')
    expect(act.title).toMatch(/Local merge/)
  })

  it('offers Ready for review on drafts', () => {
    expect(prMergeAction(gh({ draft: true })).kind).toBe('ready')
    expect(prMergeAction(gh({ mergeStateStatus: 'DRAFT' })).kind).toBe('ready')
  })

  it('keeps Ready for review on a behind draft (Update is a separate button)', () => {
    expect(prMergeAction(gh({ draft: true, behindBase: 2 })).kind).toBe('ready')
  })

  it('lets the author mark a draft ready without write', () => {
    expect(prMergeAction(gh({
      draft: true, viewerCanWrite: false, authorLogin: 'josh',
    })).kind).toBe('ready')
  })

  it('hides merge when the viewer has no write access', () => {
    expect(prMergeAction(gh({
      viewerLogin: 'guest', viewerCanWrite: false, mergeStateStatus: 'CLEAN',
    })).kind).toBe('hidden')
  })

  it('replaces Merge with Update when GitHub reports BEHIND', () => {
    expect(prMergeAction(gh({ mergeStateStatus: 'BEHIND' })).kind).toBe('update-base')
    expect(prMergeAction(gh({ mergeStateStatus: 'BEHIND', behindBase: 3, base: 'main' })).title).toMatch(/3 commits on main/)
  })

  it('keeps Merge when GitHub still allows it even if the branch is behind', () => {
    expect(prMergeAction(gh({ mergeStateStatus: 'CLEAN', behindBase: 2 })).kind).toBe('merge')
    expect(prMergeAction(gh({ behindBase: 2 })).kind).toBe('merge')
  })

  it('offers Resolve conflicts through update-with-base when a workspace exists', () => {
    const pr = gh({ mergeStateStatus: 'DIRTY', mergeable: 'CONFLICTING', behindBase: 0 })
    const act = prMergeAction(pr)
    expect(act).toMatchObject({ kind: 'resolve', label: 'Resolve conflicts', disabled: false })
    expect(showPRUpdate(pr, act)).toBe(true)
  })

  it('does not offer Update with base without a workspace', () => {
    expect(prMergeAction(gh({ workspaceId: '', mergeStateStatus: 'BEHIND' })).kind).not.toBe('update-base')
  })

  it('disables Conflicts when dirty without a workspace', () => {
    const act = prMergeAction(gh({ workspaceId: '', mergeable: 'CONFLICTING', mergeStateStatus: 'DIRTY' }))
    expect(act.kind).toBe('conflicts')
    expect(act.disabled).toBe(true)
  })

  it('shows Fix checks when BLOCKED and a check has failed', () => {
    expect(prMergeAction(gh({
      mergeStateStatus: 'BLOCKED', checksOk: false, reviewDecision: 'APPROVED',
      checks: [{ passed: false, status: 'fail' }, { passed: false, status: 'pending' }],
    })).kind).toBe('checks')
    expect(prMergeAction(gh({
      mergeStateStatus: 'BLOCKED', checksOk: false, reviewDecision: 'APPROVED',
    })).label).toBe('Fix checks')
  })

  it('does not show Fix checks while checks are still running', () => {
    expect(prMergeAction(gh({
      mergeStateStatus: 'BLOCKED', checksOk: false, reviewDecision: 'APPROVED',
      checks: [{ passed: false, status: 'pending' }],
    })).kind).toBe('blocked')
  })

  it('hides merge when BLOCKED on a required review', () => {
    expect(prMergeAction(gh({
      mergeStateStatus: 'BLOCKED', reviewDecision: 'REVIEW_REQUIRED',
    })).kind).toBe('hidden')
    expect(prMergeAction(gh({
      mergeStateStatus: 'BLOCKED', reviewDecision: 'CHANGES_REQUESTED',
    })).kind).toBe('hidden')
  })

  it('still offers Update when behind and merge is hidden for review', () => {
    for (const reviewDecision of ['REVIEW_REQUIRED', 'CHANGES_REQUESTED'] as const) {
      const pr = gh({ mergeStateStatus: 'BLOCKED', reviewDecision, behindBase: 2, checksOk: true })
      const action = prMergeAction(pr)
      expect(action.kind).toBe('hidden')
      expect(showPRUpdate(pr, action)).toBe(true)
    }
  })

  it('does not offer Update when merge is hidden for no write or closed', () => {
    const guest = gh({ viewerLogin: 'guest', viewerCanWrite: false, behindBase: 2 })
    expect(showPRUpdate(guest, prMergeAction(guest))).toBe(false)
    const closed = gh({ state: 'Closed', behindBase: 2 })
    expect(showPRUpdate(closed, prMergeAction(closed))).toBe(false)
  })

  it('disables Merge blocked when BLOCKED for another reason', () => {
    const act = prMergeAction(gh({
      mergeStateStatus: 'BLOCKED', reviewDecision: 'APPROVED', checksOk: true,
    }))
    expect(act.kind).toBe('blocked')
    expect(act.disabled).toBe(true)
  })

  it('lets the author or a writer flip GitHub draft state', () => {
    expect(canSetPRDraft(gh({ viewerCanWrite: true }))).toBe(true)
    expect(canSetPRDraft(gh({ viewerCanWrite: false, authorLogin: 'josh' }))).toBe(true)
    expect(canSetPRDraft(gh({ viewerLogin: 'guest', viewerCanWrite: false }))).toBe(false)
    expect(canSetPRDraft(gh({ state: 'Merged', group: 'merged' }))).toBe(false)
    expect(canSetPRDraft({ source: 'local', state: 'In review' })).toBe(false)
  })

  it('hides Approve on your own GitHub PR', () => {
    expect(canApprovePRReview(gh({ authorLogin: 'josh', viewerCanWrite: true }))).toBe(false)
    expect(canApprovePRReview(gh({ authorLogin: 'loopbot', viewerCanWrite: true }))).toBe(true)
    expect(canApprovePRReview(gh({ viewerLogin: 'guest', viewerCanWrite: false }))).toBe(false)
    expect(canApprovePRReview(gh({ viewerLogin: '' }))).toBe(true)
    expect(canApprovePRReview({ source: 'local' })).toBe(true)
    expect(canApprovePRReview(gh({ state: 'Merged', group: 'merged', authorLogin: 'loopbot' }))).toBe(false)
    expect(canApprovePRReview(gh({ state: 'Closed', authorLogin: 'loopbot' }))).toBe(false)
  })

  it('filters merge methods to what the GitHub repo allows', () => {
    expect(allowedMergeMethods({ source: 'local' })).toEqual(['merge', 'squash', 'rebase'])
    expect(allowedMergeMethods({ source: 'gh' })).toEqual(['merge', 'squash', 'rebase'])
    expect(allowedMergeMethods({ source: 'gh', allowedMergeMethods: ['squash'] })).toEqual(['squash'])
    expect(pickAllowedMergeMethod('merge', ['squash', 'rebase'])).toBe('squash')
    expect(pickAllowedMergeMethod('rebase', ['squash', 'rebase'])).toBe('rebase')
  })

  it('still offers Merge when UNSTABLE, CLEAN, HAS_HOOKS, or unknown', () => {
    expect(prMergeAction(gh({ mergeStateStatus: 'UNSTABLE' })).title).toMatch(/not green/)
    expect(prMergeAction(gh({ mergeStateStatus: 'CLEAN' })).kind).toBe('merge')
    expect(prMergeAction(gh({ mergeStateStatus: 'HAS_HOOKS' })).kind).toBe('merge')
    expect(prMergeAction(gh({ mergeStateStatus: 'UNKNOWN' })).kind).toBe('merge')
    expect(prMergeAction(gh({})).kind).toBe('merge')
  })
})

describe('update with base labels', () => {
  it('strips remote prefixes from the base name', () => {
    expect(shortBaseName('origin/develop')).toBe('develop')
    expect(shortBaseName('upstream/main')).toBe('main')
    expect(shortBaseName('main')).toBe('main')
    expect(shortBaseName('')).toBe('base')
  })

  it('resolves project then settings then merge, with a UI override', () => {
    expect(resolveUpdateBaseMethod(undefined, undefined)).toBe('merge')
    expect(resolveUpdateBaseMethod('rebase', 'merge')).toBe('rebase')
    expect(resolveUpdateBaseMethod('nope', 'rebase')).toBe('rebase')
    expect(resolveUpdateBaseMethod('rebase', 'merge', 'merge')).toBe('merge')
  })

  it('names the branch on Update / Rebase', () => {
    expect(updateWithBaseLabel('merge', 'origin/main')).toBe('Update with main')
    expect(updateWithBaseLabel('rebase', 'develop')).toBe('Rebase onto develop')
    expect(prMergeAction(gh({ mergeStateStatus: 'BEHIND', behindBase: 2, base: 'origin/main' })).label).toBe('Update with main')
  })
})
