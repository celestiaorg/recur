/**
 * Bundle [[loop]] + [[policy]] catalog for New Task and the chat dock.
 * Uses Call.ByName so we don't wait on regenerated Wails IDs (same pattern
 * as bridge-bench). Official bindings replace this once generated.
 */
import { Call } from '@wailsio/runtime'
import * as BundleService from '../../bindings/recur/internal/app/bundleservice'
import * as WorkspacesService from '../../bindings/recur/internal/app/workspacesservice'
import type { Workspace } from '../../bindings/recur/internal/domain/models'

export const DEFAULT_LOOP_ID = 'recur.loops.pr'
export const CONVERGE_LOOP_ID = 'recur.loops.converge'
export const CONVERGE_PR_LOOP_ID = 'recur.loops.converge-pr'
export const DOCS_LOOP_ID = 'recur.loops.docs'
export const SECURITY_LOOP_ID = 'recur.loops.security'
export const DEFAULT_POLICY_ID = 'recur.policies.balanced'

export type LoopInfo = {
  id: string
  name: string
  hint?: string
  entry?: string
  bundleName?: string
}

export type PolicyInfo = {
  id: string
  name: string
  hint?: string
  maxRounds?: number
  onMaxRounds?: string
  planApproval?: string
  extendRounds?: number
  bundleName?: string
}

export type LoopSettings = {
  defaultLoop?: string
  defaultPolicy?: string
  disabledBundleLoops?: string[]
  disabledBundlePolicies?: string[]
}

export type LoopWorkspace = {
  loopId?: string
  policyId?: string
  runStarted?: boolean
  type?: string
}

const LIST_LOOPS = 'recur/internal/app.BundleService.ListLoops'
const LIST_POLICIES = 'recur/internal/app.BundleService.ListPolicies'
const CREATE_WITH = 'recur/internal/app.WorkspacesService.CreateWorkspaceWith'
const SET_LOOP_POLICY = 'recur/internal/app.WorkspacesService.SetWorkspaceLoopPolicy'
const ENGAGE_WITH = 'recur/internal/app.WorkspacesService.EngageLoopWith'

function asList<T extends { id?: string }>(raw: unknown): T[] {
  if (!Array.isArray(raw)) return []
  return raw.filter((x): x is T => !!x && typeof x === 'object' && typeof (x as T).id === 'string')
}

export async function listLoops(): Promise<LoopInfo[]> {
  try {
    if (typeof (BundleService as { ListLoops?: () => Promise<LoopInfo[]> }).ListLoops === 'function') {
      return asList(await (BundleService as { ListLoops: () => Promise<LoopInfo[]> }).ListLoops())
    }
    return asList(await Call.ByName(LIST_LOOPS))
  } catch {
    return []
  }
}

export async function listPolicies(): Promise<PolicyInfo[]> {
  try {
    if (typeof (BundleService as { ListPolicies?: () => Promise<PolicyInfo[]> }).ListPolicies === 'function') {
      return asList(await (BundleService as { ListPolicies: () => Promise<PolicyInfo[]> }).ListPolicies())
    }
    return asList(await Call.ByName(LIST_POLICIES))
  } catch {
    return []
  }
}

export function enabledLoops(list: LoopInfo[], set?: LoopSettings | null): LoopInfo[] {
  const disabled = new Set(set?.disabledBundleLoops ?? [])
  return list.filter((l) => l.id && !disabled.has(l.id))
}

export function enabledPolicies(list: PolicyInfo[], set?: LoopSettings | null): PolicyInfo[] {
  const disabled = new Set(set?.disabledBundlePolicies ?? [])
  return list.filter((p) => p.id && !disabled.has(p.id))
}

export function defaultLoopId(set?: LoopSettings | null): string {
  return set?.defaultLoop?.trim() || DEFAULT_LOOP_ID
}

export function defaultPolicyId(set?: LoopSettings | null): string {
  return set?.defaultPolicy?.trim() || DEFAULT_POLICY_ID
}

export function workspaceLoopId(ws?: LoopWorkspace | null, set?: LoopSettings | null): string {
  return ws?.loopId?.trim() || defaultLoopId(set)
}

export function workspacePolicyId(ws?: LoopWorkspace | null, set?: LoopSettings | null): string {
  return ws?.policyId?.trim() || defaultPolicyId(set)
}

export function loopSelectOptions(list: LoopInfo[]) {
  return list.map((l) => ({ value: l.id, label: l.name || l.id, hint: l.hint ?? '' }))
}

export function policySelectOptions(list: PolicyInfo[]) {
  return list.map((p) => ({ value: p.id, label: p.name || p.id, hint: p.hint ?? '' }))
}

/** Map a shipped (or similarly named) policy onto the legacy autonomy field. */
export function autonomyFromPolicyId(id: string): string {
  const s = id.toLowerCase()
  if (s.includes('cautious')) return 'cautious'
  if (s.includes('aggressive')) return 'aggressive'
  return 'balanced'
}

export async function createWorkspaceWith(
  projectId: string,
  name: string, // mode=loop: GoalDisplay label when prompt is expanded wire
  baseBranch: string,
  mode: string,
  prompt: string,
  autonomy: string,
  loopId: string,
  policyId: string,
  simplify?: boolean | null,
  attachments?: { id: string; name: string; mime: string; data: string }[],
): Promise<Workspace> {
  return Call.ByName(CREATE_WITH, projectId, name, baseBranch, mode, prompt, autonomy, loopId, policyId, simplify ?? null, attachments ?? []) as Promise<Workspace>
}

export async function setWorkspaceLoopPolicy(workspaceId: string, loopId: string, policyId: string): Promise<Workspace> {
  return Call.ByName(SET_LOOP_POLICY, workspaceId, loopId, policyId) as Promise<Workspace>
}

const SET_CHAT_LOOP = 'recur/internal/app.WorkspacesService.RememberChatLoopPolicy'

export async function setChatLoopPolicy(
  workspaceId: string,
  code: string,
  loopId: string,
  policyId: string,
): Promise<void> {
  await Call.ByName(SET_CHAT_LOOP, workspaceId, code, loopId, policyId)
}

export async function engageLoopWith(workspaceId: string, goal: string, loopId: string, policyId: string): Promise<string> {
  try {
    return await Call.ByName(ENGAGE_WITH, workspaceId, goal, loopId, policyId) as string
  } catch {
    return WorkspacesService.EngageLoop(workspaceId, goal)
  }
}

const shippedLoopFallback: LoopInfo[] = [
  { id: DEFAULT_LOOP_ID, name: 'To a PR', hint: 'Plan, implement, review, open a PR' },
  { id: CONVERGE_LOOP_ID, name: 'Review & fix', hint: 'Review and fix the current diff until the bar passes' },
  { id: CONVERGE_PR_LOOP_ID, name: 'Review & fix to a PR', hint: 'Review and fix the current diff, then open a PR' },
  { id: DOCS_LOOP_ID, name: 'Docs', hint: 'Plan, write, and edit documentation — no PR' },
  { id: SECURITY_LOOP_ID, name: 'Security review & fix', hint: 'Review and fix auth, secrets, and permissions until the bar passes' },
]
const shippedPolicyFallback: PolicyInfo[] = [
  { id: 'recur.policies.cautious', name: 'Cautious', hint: 'Close every finding; escalate after 2 rounds' },
  { id: DEFAULT_POLICY_ID, name: 'Balanced', hint: 'Fix medium and up; escalate after 4 rounds' },
  { id: 'recur.policies.aggressive', name: 'Aggressive', hint: 'Ship-blockers only; proceed after 6 rounds' },
]

export function fallbackLoops(): LoopInfo[] {
  return shippedLoopFallback
}

export function fallbackPolicies(): PolicyInfo[] {
  return shippedPolicyFallback
}
