/** Max faces in the PR-list stack, including a +N overflow chip. */
export const PR_FACE_CAP = 6

export type PRPerson = {
  login?: string
  name?: string
  avatarUrl?: string
  avatarCss?: string
  code?: string
  reviewState?: string
}

export type PRReviewer = PRPerson & { state?: string }

const REVIEW_RANK: Record<string, number> = {
  changes_requested: 0,
  pending: 1,
  approved: 2,
  commented: 3,
}

export function reviewStateLabel(state: string | undefined): string {
  switch ((state ?? '').trim()) {
    case 'pending':
      return 'Review requested'
    case 'approved':
      return 'Approved'
    case 'changes_requested':
      return 'Changes requested'
    case 'commented':
      return 'Commented'
    default:
      return ''
  }
}

export function prReviewers(pr: { reviewers?: PRReviewer[] } | null | undefined): PRReviewer[] {
  return (pr?.reviewers ?? []).filter((r) => (r.state ?? '').trim())
}

/** Participants with review state attached; reviewers who are not already in the stack are appended. */
export function prPeopleWithReviews(pr: Parameters<typeof prParticipants>[0] & { reviewers?: PRReviewer[] }): PRPerson[] {
  const reviewers = prReviewers(pr)
  const byKey = new Map<string, string>()
  for (const r of reviewers) {
    const k = personKey(r)
    if (k) byKey.set(k, (r.state ?? '').trim())
  }
  const seen = new Set<string>()
  const out: PRPerson[] = []
  const add = (p: PRPerson) => {
    const k = personKey(p)
    if (!k || seen.has(k)) return
    seen.add(k)
    const reviewState = byKey.get(k) || p.reviewState
    out.push(reviewState ? { ...p, reviewState } : p)
  }
  for (const r of reviewers) add(r)
  for (const p of prParticipants(pr)) add(p)
  out.sort((a, b) => {
    const ra = REVIEW_RANK[a.reviewState ?? ''] ?? 8
    const rb = REVIEW_RANK[b.reviewState ?? ''] ?? 8
    if (ra !== rb) return ra - rb
    return 0
  })
  return out
}

export function participantLabel(p: PRPerson | null | undefined): string {
  const login = (p?.login ?? '').trim().replace(/^@/, '')
  if (login) return `@${login}`
  return (p?.name ?? '').trim()
}

export function participantInitials(p: PRPerson | null | undefined): string {
  const code = (p?.code ?? '').trim()
  if (code) return code.slice(0, 2).toUpperCase()
  const s = ((p?.login || p?.name || '?').trim()).replace(/^@/, '')
  return s.slice(0, 2).toUpperCase()
}

export function participantAvatarUrl(p: PRPerson | null | undefined): string {
  const provided = (p?.avatarUrl ?? '').trim()
  if (provided) return provided
  if ((p?.avatarCss ?? '').trim()) return ''
  let login = (p?.login ?? '').trim().replace(/^@/, '')
  if (!login || login.toLowerCase() === 'someone') return ''
  login = login.replace(/\[bot\]$/i, '')
  if (!/^[-A-Za-z0-9_]+$/.test(login)) return ''
  return `https://github.com/${login}.png?size=40`
}

export function prFaceSlots(people: PRPerson[]): { shown: PRPerson[]; extra: number; rest: PRPerson[] } {
  if (people.length <= PR_FACE_CAP) return { shown: people, extra: 0, rest: [] }
  const shown = people.slice(0, PR_FACE_CAP - 1)
  const rest = people.slice(PR_FACE_CAP - 1)
  return { shown, extra: rest.length, rest }
}

export function personKey(p: PRPerson): string {
  const login = (p.login ?? '').trim().toLowerCase()
  if (login) return `login:${login}`
  const name = (p.name ?? '').trim().toLowerCase()
  if (name) return `name:${name}`
  const code = (p.code ?? '').trim().toLowerCase()
  if (code) return `code:${code}`
  return ''
}

/** Stored participants, or a fallback derived from author + comments. */
export function prParticipants(pr: {
  participants?: PRPerson[]
  authorLogin?: string
  comments?: Array<{ kind?: string; authorLogin?: string; author?: string; avatarUrl?: string; avatarCss?: string; code?: string }>
} | null | undefined): PRPerson[] {
  const stored = pr?.participants ?? []
  if (stored.length) return stored
  const out: PRPerson[] = []
  const seen = new Set<string>()
  const add = (p: PRPerson) => {
    const k = personKey(p)
    if (!k || seen.has(k)) return
    seen.add(k)
    out.push(p)
  }
  const author = (pr?.authorLogin ?? '').trim()
  if (author) add({ login: author })
  for (const c of pr?.comments ?? []) {
    if (c.kind === 'hidden') continue
    add({
      login: c.authorLogin,
      name: c.author,
      avatarUrl: c.avatarUrl,
      avatarCss: c.avatarCss,
      code: c.code,
    })
  }
  return out
}
