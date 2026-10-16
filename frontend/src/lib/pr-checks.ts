/** One Checks-tab row (GitHub rollup or Local Recur check). */
export type CheckRow = {
  passed: boolean
  status?: string
  detail?: string
  name: string
  url?: string
  startedAt?: string
}

const PENDING_STATUSES = new Set([
  'pending',
  'queued',
  'in_progress',
  'waiting',
  'requested',
  'expected',
])

export function isSkippedCheck(c: { status?: string; detail?: string }): boolean {
  const status = (c.status ?? '').toLowerCase()
  if (status === 'skipping' || status === 'skipped' || status === 'skip') return true
  return /\bskipped\b/i.test(c.detail ?? '')
}

export function isPendingCheck(c: { status?: string }): boolean {
  const status = (c.status ?? '').toLowerCase().replace(/[\s-]+/g, '_')
  return PENDING_STATUSES.has(status)
}

export function groupChecks(checks: CheckRow[]) {
  const fail: CheckRow[] = []
  const pending: CheckRow[] = []
  const pass: CheckRow[] = []
  const skip: CheckRow[] = []
  for (const c of checks) {
    if (isSkippedCheck(c)) skip.push(c)
    else if (isPendingCheck(c)) pending.push(c)
    else if (c.passed) pass.push(c)
    else fail.push(c)
  }
  return { fail, pending, pass, skip }
}

export type ChecksRollupKind = 'none' | 'fail' | 'pending' | 'pass'

export type ChecksRollup = {
  kind: ChecksRollupKind
  icon: 'check' | 'x' | 'loader'
  color: string
}

/** Worst-wins: failed, then in progress, then passed. Empty is none. */
export function checksRollup(
  checks: { passed: boolean; status?: string; detail?: string }[] | undefined,
  opts?: { checksOk?: boolean; hasChecks?: boolean },
): ChecksRollup {
  const list = checks ?? []
  if (list.length) {
    const failed = list.some((c) => !c.passed && !isSkippedCheck(c) && !isPendingCheck(c))
    if (failed) return { kind: 'fail', icon: 'x', color: 'var(--coral)' }
    if (list.some((c) => isPendingCheck(c))) return { kind: 'pending', icon: 'loader', color: 'var(--amber)' }
    return { kind: 'pass', icon: 'check', color: 'var(--emerald)' }
  }
  if (opts?.checksOk === false) return { kind: 'fail', icon: 'x', color: 'var(--coral)' }
  if (opts?.hasChecks) return { kind: 'pass', icon: 'check', color: 'var(--emerald)' }
  return { kind: 'none', icon: 'check', color: 'var(--faint)' }
}

export function checksHeading(checks: { passed: boolean; status?: string; detail?: string }[] | undefined, source?: string) {
  const list = checks ?? []
  if (!list.length) return source === 'gh' ? 'No GitHub checks yet' : 'No checks'
  const roll = checksRollup(list)
  if (roll.kind === 'fail') return 'Some checks failed'
  if (roll.kind === 'pending') return 'Checks in progress'
  return 'All checks passed'
}

/** Conversation-card default: hide the boring groups, keep fail/pending open. */
const COMPACT_COLLAPSED_GROUPS = new Set(['pass', 'skip'])

export function checkGroupCollapsed(
  key: string,
  overrides: Record<string, boolean>,
  compact: boolean,
): boolean {
  if (Object.prototype.hasOwnProperty.call(overrides, key)) return !!overrides[key]
  return compact && COMPACT_COLLAPSED_GROUPS.has(key)
}

export function checksBoxByline(g: ReturnType<typeof groupChecks>) {
  const bits: string[] = []
  if (g.fail.length) bits.push(`${g.fail.length} failed`)
  if (g.pending.length) bits.push(`${g.pending.length} in progress`)
  if (g.pass.length) bits.push(`${g.pass.length} successful`)
  if (g.skip.length) bits.push(`${g.skip.length} skipped`)
  return bits.join(', ')
}

/** GitHub PR Checks tab (`/pull/N/checks`), which includes queued/expected runs. */
export function githubPRChecksTabUrl(prUrl?: string): string {
  const raw = (prUrl ?? '').trim().replace(/\/+$/, '')
  if (!raw) return ''
  if (/\/checks$/i.test(raw)) return raw
  return `${raw}/checks`
}

function isGitHubActionsUrl(url: string): boolean {
  try {
    return new URL(url).pathname.includes('/actions/runs/')
  } catch {
    return url.includes('/actions/runs/')
  }
}

/**
 * Link for a check row. Pending Actions job URLs do not show queued checks;
 * GitHub's PR Checks tab does. Empty pending rows fall back to that tab.
 */
export function checkRunHref(
  url?: string,
  opts?: { prUrl?: string; prNumber?: number; status?: string },
): string {
  const tab = githubPRChecksTabUrl(opts?.prUrl)
  const pending = isPendingCheck({ status: opts?.status })
  const raw = (url ?? '').trim()
  if (pending && (!raw || isGitHubActionsUrl(raw))) return tab || raw
  if (!raw) return tab
  if (!opts?.prNumber) return raw
  try {
    const u = new URL(raw)
    if (!u.searchParams.has('pr') && !u.pathname.includes('/pull/')) {
      u.searchParams.set('pr', String(opts.prNumber))
    }
    return u.href
  } catch {
    return raw
  }
}
