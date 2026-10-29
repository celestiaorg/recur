/** How many transcript blocks may mount at once once the list is long. */
export const CHAT_HYDRATE_MAX = 36
/** Extra blocks around the selected (usually the live tail). */
export const CHAT_HYDRATE_OVERSCAN = 4
/** Skip windowing entirely under this length — the `{#each}` is cheap. */
export const CHAT_HYDRATE_ALL_BELOW = 48
/** Always mount this many trailing blocks so streaming never paints a stub. */
export const CHAT_TAIL_LIVE = 3
/** Prefetch band above/below the scrollport, in px. */
export const CHAT_VIEW_PAD_PX = 600
/** Collapsed step-group header. */
export const CHAT_BLOCK_HEAD_PX = 36

export function estimateChatBlockHeight(block: {
  kind: 'msg' | 'steps'
  me?: boolean
  turnEnd?: boolean
  body?: string
}): number {
  if (block.kind === 'steps') return CHAT_BLOCK_HEAD_PX
  if (block.me) return 56
  if (block.turnEnd) return 28
  const n = block.body?.length ?? 0
  if (!n) return CHAT_BLOCK_HEAD_PX
  // simplify: ~72 columns; ceiling is a measured stub after first mount.
  return Math.min(720, Math.max(48, Math.ceil(n / 72) * 20))
}

export function reservedChatHeight(
  live: boolean,
  measured: number | undefined,
  estimate: number,
): number | undefined {
  if (live) return undefined
  return measured && measured > 0 ? measured : estimate
}
