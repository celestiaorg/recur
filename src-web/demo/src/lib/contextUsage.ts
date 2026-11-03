import { formatCompactCount } from './format'

/** Last-turn context fill for the composer meter. */
export type ContextUsage = {
  inputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  contextWindow: number
  /** Provider is summarizing context right now. */
  compacting?: boolean
  /** This session compacted at least once (last turn or live). */
  compacted?: boolean
}

/** Prompt-side tokens occupying the window (excludes output). */
export function contextUsedTokens(u: ContextUsage | null | undefined): number {
  if (!u) return 0
  return (u.inputTokens || 0) + (u.cacheReadTokens || 0) + (u.cacheCreationTokens || 0)
}

/** True when we can render a fill ring (limit known, and either fill or a post-compact empty). */
export function contextUsageReady(u: ContextUsage | null | undefined): boolean {
  if (!u || (u.contextWindow || 0) <= 0) return false
  return contextUsedTokens(u) > 0 || !!u.compacted
}

/** Compact token label: 460, 1.2k, 184k, 1.0M. */
export function formatTokens(n: number): string {
  return formatCompactCount(n)
}
