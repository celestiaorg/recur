// Loop + policy catalog for the chat dock's Loop pickers.

const DEFAULT_LOOP_ID = 'recur.loops.pr'
const DEFAULT_POLICY_ID = 'recur.policies.balanced'

export type LoopInfo = { id: string; name: string; hint?: string }
export type PolicyInfo = { id: string; name: string; hint?: string }

export const LOOPS: LoopInfo[] = [{ id: DEFAULT_LOOP_ID, name: 'To a PR' }]

export const POLICIES: PolicyInfo[] = [
  { id: 'recur.policies.cautious', name: 'Cautious', hint: 'Close every finding; escalate after 2 rounds' },
  { id: DEFAULT_POLICY_ID, name: 'Balanced', hint: 'Fix medium and up; escalate after 4 rounds' },
  { id: 'recur.policies.aggressive', name: 'Aggressive', hint: 'Ship-blockers only; proceed after 6 rounds' },
]

type LoopSettings = { defaultLoop?: string; defaultPolicy?: string }
type LoopWorkspace = { loopId?: string; policyId?: string }

export function workspaceLoopId(ws?: LoopWorkspace | null, set?: LoopSettings | null): string {
  return ws?.loopId?.trim() || set?.defaultLoop?.trim() || DEFAULT_LOOP_ID
}

export function workspacePolicyId(ws?: LoopWorkspace | null, set?: LoopSettings | null): string {
  return ws?.policyId?.trim() || set?.defaultPolicy?.trim() || DEFAULT_POLICY_ID
}

export function loopSelectOptions(list: LoopInfo[]) {
  return list.map((l) => ({ value: l.id, label: l.name || l.id, hint: l.hint ?? '' }))
}

export function policySelectOptions(list: PolicyInfo[]) {
  return list.map((p) => ({ value: p.id, label: p.name || p.id, hint: p.hint ?? '' }))
}
