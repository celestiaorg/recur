import type { Commit } from '../../bindings/recur/internal/domain/models'

export function commitSha(c: Commit): string {
  return (c.sha || c.hash || '').trim()
}

export function commitByline(c: Commit): string {
  if (c.authorRole && c.authorRole !== 'Author') {
    return c.iterLabel ? `${c.authorRole} · ${c.iterLabel}` : c.authorRole
  }
  return c.author || c.authorRole || 'Author'
}
