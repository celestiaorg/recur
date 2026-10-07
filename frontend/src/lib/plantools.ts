// Display helpers for mcp__recur__submit_plan / update_plan_step chat cards.
// Keep these pure so the "no JSON dump / friendly label" contracts stay testable
// without mounting AgentChat.

export type PlanStepView = { id: string; title: string; status: string }

export function isSubmitPlanTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__submit_plan') || tool === 'submit_plan'
}

export function isUpdatePlanStepTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__update_plan_step') || tool === 'update_plan_step'
}

export function isEditPlanTool(tool: string | undefined | null): boolean {
  return !!tool?.endsWith('__edit_plan') || tool === 'edit_plan'
}

/** Human row title for submit_plan (progress lives in the label, not meta). */
export function submitPlanLabel(pending: boolean, stepCount: number): string {
  if (pending) return 'Submitting plan'
  return stepCount
    ? `Submitted plan · ${stepCount} step${stepCount === 1 ? '' : 's'}`
    : 'Submitted plan'
}

/** Human row title for update_plan_step. */
export function updatePlanStepLabel(pending: boolean): string {
  return pending ? 'Updating plan step' : 'Updated plan step'
}

/** Human row title for edit_plan (rewrites the loop's definition of done). */
export function editPlanLabel(pending: boolean): string {
  return pending ? 'Editing plan' : 'Edited plan'
}

/** First non-empty line, collapsed whitespace — collapsed-row meta, never JSON. */
export function prosePreview(text: string): string {
  return text.split('\n').find((l) => l.trim())?.replace(/\s+/g, ' ').trim() ?? ''
}

export function parseSubmitPlanBody(raw: unknown): string {
  if (!raw || typeof raw !== 'object') return ''
  const b = (raw as Record<string, unknown>).body
  return typeof b === 'string' ? b.trim() : ''
}

/** Attachment ids from submit_plan args (strings only). */
export function parseSubmitPlanAttachments(raw: unknown): string[] {
  if (!Array.isArray(raw)) return []
  const out: string[] = []
  const seen = new Set<string>()
  for (const a of raw) {
    if (typeof a !== 'string') continue
    const id = a.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    out.push(id)
  }
  return out
}

export function parseEditPlan(raw: unknown): string {
  if (!raw || typeof raw !== 'object') return ''
  const p = (raw as Record<string, unknown>).plan
  return typeof p === 'string' ? p.trim() : ''
}

/** Collapsed-row meta: "S2 → done · optional note" — never raw JSON. */
export function updatePlanStepMeta(stepId: string, status: string, note: string): string {
  const n = note.replace(/\s+/g, ' ').trim()
  const parts: string[] = []
  if (stepId && status) parts.push(`${stepId} → ${status}`)
  else if (stepId) parts.push(stepId)
  else if (status) parts.push(status)
  if (n) parts.push(n)
  return parts.join(' · ')
}

/** Parse submit_plan args.steps into checklist rows (string[] or legacy objects). */
export function parseSubmitPlanSteps(raw: unknown): PlanStepView[] {
  if (!Array.isArray(raw)) return []
  const out: PlanStepView[] = []
  for (const s of raw) {
    if (typeof s === 'string') {
      const title = s.trim()
      if (!title) continue
      out.push({ id: `S${out.length + 1}`, title, status: 'pending' })
      continue
    }
    if (s && typeof s === 'object') {
      const o = s as Record<string, unknown>
      const title = typeof o.thought === 'string' ? o.thought.trim()
        : typeof o.title === 'string' ? o.title.trim()
        : typeof o.content === 'string' ? o.content.trim()
        : ''
      if (!title) continue
      const id = typeof o.step_id === 'string' && o.step_id
        ? o.step_id
        : typeof o.id === 'string' && o.id
          ? o.id
          : `S${out.length + 1}`
      const status = typeof o.status === 'string' && o.status ? o.status : 'pending'
      out.push({ id, title, status })
    }
  }
  return out
}

/** Recover step title from engine ack: "Step S3 (title…) → done." */
export function parseUpdatePlanStepTitle(result: string): string {
  const r = result.trim()
  if (!r) return ''
  const arrow = r.includes('→') ? '→' : '->'
  const close = `) ${arrow} `
  const open = r.indexOf(' (')
  const end = r.lastIndexOf(close)
  if (open < 0 || end <= open) return ''
  return r.slice(open + 2, end).trim()
}
