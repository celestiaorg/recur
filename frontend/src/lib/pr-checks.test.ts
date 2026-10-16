import { describe, expect, it } from 'vitest'
import {
  checkGroupCollapsed,
  checkRunHref,
  checksBoxByline,
  checksHeading,
  checksRollup,
  githubPRChecksTabUrl,
  groupChecks,
  isPendingCheck,
  isSkippedCheck,
} from './pr-checks'

describe('isPendingCheck', () => {
  it('treats GitHub queued and expected buckets as in progress', () => {
    expect(isPendingCheck({ status: 'pending' })).toBe(true)
    expect(isPendingCheck({ status: 'QUEUED' })).toBe(true)
    expect(isPendingCheck({ status: 'in_progress' })).toBe(true)
    expect(isPendingCheck({ status: 'EXPECTED' })).toBe(true)
    expect(isPendingCheck({ status: 'stale' })).toBe(false)
    expect(isPendingCheck({ status: 'pass' })).toBe(false)
    expect(isPendingCheck({ status: '' })).toBe(false)
  })
})

describe('groupChecks', () => {
  it('keeps pending GitHub checks out of the failed group', () => {
    const g = groupChecks([
      { name: 'test', passed: true, status: 'pass' },
      { name: 'build', passed: false, status: 'pending' },
      { name: 'license', passed: false, status: 'expected' },
      { name: 'lint', passed: false, status: 'fail' },
      { name: 'deploy', passed: false, status: 'skipping', detail: 'skipped' },
    ])
    expect(g.pending.map((c) => c.name)).toEqual(['build', 'license'])
    expect(g.fail.map((c) => c.name)).toEqual(['lint'])
    expect(g.pass.map((c) => c.name)).toEqual(['test'])
    expect(g.skip.map((c) => c.name)).toEqual(['deploy'])
    expect(isSkippedCheck(g.skip[0])).toBe(true)
  })
})

describe('checkGroupCollapsed', () => {
  it('collapses successful and skipped groups on the conversation card', () => {
    expect(checkGroupCollapsed('pass', {}, true)).toBe(true)
    expect(checkGroupCollapsed('skip', {}, true)).toBe(true)
    expect(checkGroupCollapsed('fail', {}, true)).toBe(false)
    expect(checkGroupCollapsed('pending', {}, true)).toBe(false)
  })

  it('keeps every group expanded on the Checks tab until toggled', () => {
    expect(checkGroupCollapsed('pass', {}, false)).toBe(false)
    expect(checkGroupCollapsed('skip', {}, false)).toBe(false)
    expect(checkGroupCollapsed('pass', { pass: true }, false)).toBe(true)
    expect(checkGroupCollapsed('pass', { pass: false }, true)).toBe(false)
  })
})

describe('checksHeading', () => {
  it('names in-progress GitHub checks even when none have passed', () => {
    expect(checksHeading([{ passed: false, status: 'pending' }], 'gh')).toBe('Checks in progress')
    expect(checksHeading([], 'gh')).toBe('No GitHub checks yet')
  })

  it('names a failure even when other checks are still running', () => {
    expect(checksHeading([
      { passed: false, status: 'fail' },
      { passed: false, status: 'pending' },
    ])).toBe('Some checks failed')
  })
})

describe('checksRollup', () => {
  it('is fail, then pending, then pass', () => {
    expect(checksRollup([{ passed: false, status: 'fail' }]).kind).toBe('fail')
    expect(checksRollup([{ passed: false, status: 'pending' }])).toMatchObject({ kind: 'pending', icon: 'loader' })
    expect(checksRollup([{ passed: true, status: 'pass' }])).toMatchObject({ kind: 'pass', icon: 'check', color: 'var(--emerald)' })
    expect(checksRollup([
      { passed: false, status: 'fail' },
      { passed: false, status: 'pending' },
    ]).kind).toBe('fail')
  })

  it('uses listing bits when the checks array is empty', () => {
    expect(checksRollup([], { checksOk: false }).kind).toBe('fail')
    expect(checksRollup([], { hasChecks: true, checksOk: true }).kind).toBe('pass')
    expect(checksRollup([]).kind).toBe('none')
  })
})

describe('checksBoxByline', () => {
  it('counts in-progress rows', () => {
    expect(checksBoxByline(groupChecks([
      { name: 'a', passed: false, status: 'pending' },
      { name: 'b', passed: true, status: 'pass' },
    ]))).toBe('1 in progress, 1 successful')
  })
})

describe('githubPRChecksTabUrl', () => {
  it('appends /checks on a PR url', () => {
    expect(githubPRChecksTabUrl('https://github.com/acme/app/pull/11')).toBe(
      'https://github.com/acme/app/pull/11/checks',
    )
    expect(githubPRChecksTabUrl('https://github.com/acme/app/pull/11/')).toBe(
      'https://github.com/acme/app/pull/11/checks',
    )
  })
})

describe('checkRunHref', () => {
  const prUrl = 'https://github.com/acme/app/pull/11'

  it('sends pending Actions jobs to the PR Checks tab', () => {
    expect(checkRunHref('https://github.com/acme/app/actions/runs/1/job/2', {
      prUrl, prNumber: 11, status: 'pending',
    })).toBe('https://github.com/acme/app/pull/11/checks')
  })

  it('links a pending row with no url to the PR Checks tab', () => {
    expect(checkRunHref('', { prUrl, prNumber: 11, status: 'queued' })).toBe(
      'https://github.com/acme/app/pull/11/checks',
    )
  })

  it('keeps a check-run permalink and adds pr=', () => {
    expect(checkRunHref('https://github.com/acme/app/runs/99', {
      prUrl, prNumber: 11, status: 'pending',
    })).toBe('https://github.com/acme/app/runs/99?pr=11')
  })

  it('falls back to the check url when the PR url is missing', () => {
    expect(checkRunHref('https://github.com/acme/app/actions/runs/1/job/2', {
      prNumber: 11, status: 'pending',
    })).toBe('https://github.com/acme/app/actions/runs/1/job/2')
  })
})
