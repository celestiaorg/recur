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

export function contextUsageFromTurn(it: {
  inputTokens?: number
  cacheCreationTokens?: number
  cacheReadTokens?: number
  contextWindow?: number
  compacting?: boolean
  compacted?: boolean
} | null | undefined): ContextUsage | null {
  if (!it) return null
  const u: ContextUsage = {
    inputTokens: it.inputTokens || 0,
    cacheCreationTokens: it.cacheCreationTokens || 0,
    cacheReadTokens: it.cacheReadTokens || 0,
    contextWindow: it.contextWindow || 0,
    compacting: !!it.compacting,
    compacted: !!it.compacted,
  }
  if ((u.contextWindow || 0) <= 0) return null
  if (contextUsedTokens(u) <= 0 && !u.compacted) return null
  return u
}

/** Drop pre-compact fill. Occupied tokens after compact are unknown until the next usage frame. */
export function contextUsageAfterCompact(prev: ContextUsage | null | undefined): ContextUsage | null {
  if (!prev) return null
  return {
    inputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    contextWindow: prev.contextWindow || 0,
    compacting: false,
    compacted: true,
  }
}

/** Latest usable turn's context stats. A compact newer than that turn resets fill. */
export function contextUsageFromChatItems(
  items: Array<{
    kind?: string
    inputTokens?: number
    cacheCreationTokens?: number
    cacheReadTokens?: number
    contextWindow?: number
  }> | null | undefined,
): ContextUsage | null {
  if (!items?.length) return null
  let lastUsable = -1
  let lastCompact = -1
  for (let i = 0; i < items.length; i++) {
    const it = items[i]
    if (it.kind === 'compact') lastCompact = i
    else if (it.kind === 'turn' && contextUsageFromTurn(it)) lastUsable = i
  }
  if (lastUsable < 0) return null
  const turn = contextUsageFromTurn(items[lastUsable])
  if (!turn) return null
  if (lastCompact > lastUsable) return contextUsageAfterCompact(turn)
  return lastCompact >= 0 ? { ...turn, compacted: true } : turn
}
