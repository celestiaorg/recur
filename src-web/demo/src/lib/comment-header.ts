/** Approve / request-changes on a timeline or conversation row. */
export type ReviewDecision = 'approved' | 'changes' | ''

export function reviewDecision(
  c: { kind?: string; verb?: string; bodyMd?: string } | null | undefined,
): ReviewDecision {
  const verb = (c?.verb ?? '').trim().toLowerCase()
  const body = (c?.bodyMd ?? '').trim().toLowerCase()
  const s = verb || (c?.kind === 'action' ? body : '')
  if (s === 'approved' || s.startsWith('approved ')) return 'approved'
  if (s === 'requested changes' || s.includes('requested changes')) return 'changes'
  return ''
}

/** Header verb after the author on a conversation comment card. */
export function commentHeaderVerb(c: { kind?: string; verb?: string } | null | undefined): string {
  if (!c || c.kind === 'description' || c.kind === 'action') return ''
  const v = c.verb?.trim()
  if (!v) return 'commented'
  if (v.toLowerCase() === 'approved') return 'approved these changes'
  return v
}
