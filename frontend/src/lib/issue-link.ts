import { readable } from 'svelte/store'
import { SettingsService } from '../../bindings/recur/internal/app'
import type { AssignedIssue } from '../../bindings/recur/internal/app/models'
import type { IssueRef } from '../../bindings/recur/internal/domain/models'
import { issuesStale } from './stores'
import { openExternalUrl } from './open-external'

let fetchedAt = -1

/** Tracker issues for workspace links. Kept across subscribers so opening a
 *  workspace does not refetch every tracker; an issuesStale bump does. */
export const trackerIssues = readable<AssignedIssue[]>([], (set) =>
  issuesStale.subscribe((n) => {
    if (n === fetchedAt) return
    fetchedAt = n
    SettingsService.ListAssignedIssues().then((rows) => set(rows ?? []), () => {})
  }),
)

export const issueKey = (r: { source: string; id: string }) => `${r.source}:${r.id}`

// simplify: the URL comes from the Issues list, so a linked Linear issue that
// dropped off it (closed, reassigned) has no URL; store it on IssueRef if that matters.
export function issueUrl(ref: IssueRef | null | undefined, issues: AssignedIssue[]): string {
  if (!ref) return ''
  const hit = issues.find((i) => i.source === ref.source && i.id === ref.id)?.url
  if (hit) return hit
  const gh = ref.source === 'github' ? ref.key?.match(/^([^/\s]+\/[^#\s]+)#(\d+)$/) : null
  return gh ? `https://github.com/${gh[1]}/issues/${gh[2]}` : ''
}

/** Open an issue in the Linear app when it is installed, else the browser. */
export async function openIssueUrl(source: string, url: string) {
  if (source === 'linear' && (await SettingsService.OpenLinearInApp(url).catch(() => false))) return
  await openExternalUrl(url)
}
