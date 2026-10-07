import type { Commit } from '../../bindings/recur/internal/domain/models'
import { commitSha } from './commit-meta'

/** Newest-first tape for Review → Commits: unpublished work, then real commits. */
export type ReviewTapeItem =
  | { kind: 'working' }
  | { kind: 'commit'; commit: Commit }

export function reviewTape(dirty: boolean, commits: readonly Commit[]): ReviewTapeItem[] {
  const items: ReviewTapeItem[] = dirty ? [{ kind: 'working' }] : []
  for (const c of commits) items.push({ kind: 'commit', commit: c })
  return items
}

export function reviewTapeIndex(
  tape: readonly ReviewTapeItem[],
  workingOpen: boolean,
  sha: string,
): number {
  if (workingOpen) return tape.findIndex((i) => i.kind === 'working')
  if (!sha) return -1
  return tape.findIndex((i) => i.kind === 'commit' && commitSha(i.commit) === sha)
}

export function canStepOnTape(idx: number, len: number): { older: boolean; newer: boolean } {
  return {
    older: idx >= 0 && idx < len - 1,
    newer: idx > 0,
  }
}

/** dir -1 = older (down the newest-first list), dir +1 = newer (up). */
export function stepTape(
  tape: readonly ReviewTapeItem[],
  idx: number,
  dir: -1 | 1,
): ReviewTapeItem | undefined {
  const next = idx + (dir === -1 ? 1 : -1)
  if (next < 0 || next >= tape.length) return undefined
  return tape[next]
}

/** Accessible name for a tape step control. WIP is not a commit. */
export function tapeStepLabel(item: ReviewTapeItem | undefined, dir: -1 | 1): string {
  if (item?.kind === 'working') return 'Uncommitted changes'
  return dir === -1 ? 'Previous commit' : 'Next commit'
}
