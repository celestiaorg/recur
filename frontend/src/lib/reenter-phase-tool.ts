// Display helpers for mcp__recur__reenter_phase chat cards.
// Keep these pure so the "no JSON dump / friendly label" contracts stay testable
// without mounting AgentChat.

export function isReenterPhaseTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__reenter_phase') || tool === 'reenter_phase'
}

export function parseReenterPhaseID(raw: unknown): string {
  if (!raw || typeof raw !== 'object') return ''
  const o = raw as Record<string, unknown>
  const v = o.phase_id ?? o.phaseId
  return typeof v === 'string' ? v.trim() : ''
}

export function reenterPhaseFailed(result: string, flags?: { error?: boolean }): boolean {
  if (flags?.error) return true
  const r = (result ?? '').trim()
  if (!r) return false
  return /^Refused:/i.test(r) || /^reenter_phase:/i.test(r)
}

/** Collapsed-row title — never the args JSON, never the engine ack. */
export function reenterPhaseLabel(opts: {
  pending: boolean
  failed: boolean
  phase: string
}): string {
  if (opts.failed) return 'Re-enter failed'
  if (opts.pending) return opts.phase ? `Re-entering ${opts.phase}` : 'Re-entering phase'
  return opts.phase ? `Re-entered ${opts.phase}` : 'Re-entered phase'
}
