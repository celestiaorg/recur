// Human-facing labels for tool names shown on permission prompts.
// Keep in sync with permissionToolLabel / recurToolLabels in
// internal/engine/permission.go — the wire field stays the raw tool name
// (remember/approve keys off it); only display goes through here.

const recurToolLabels: Record<string, string> = {
  report_finding: 'Report finding',
  list_findings: 'List findings',
  update_finding: 'Update finding',
  done: 'Complete turn',
  report_result: 'Report result',
  plan: 'Record plan',
  escalate: 'Escalate',
  ask_user: 'Ask user',
  switch_mode: 'Switch mode',
  create_artifact: 'Create artifact',
  list_artifacts: 'List artifacts',
  read_artifact: 'Read artifact',
  present_artifact: 'Present artifact',
  rename_session: 'Rename chat',
  rename_workspace: 'Rename workspace',
  rename_branch: 'Rename branch',
  open_pr: 'Open PR',
  create_workspace: 'Create workspace',
  merge_child: 'Merge child workspace',
  read_child: 'Read child workspace',
  archive_child: 'Archive child workspace',
  message_child: 'Message child workspace',
  attach_evidence: 'Attach evidence',
  edit_plan: 'Edit plan',
  rerun_review: 'Re-run review',
  dispatch_specialist: 'Dispatch specialist',
  extend_iterations: 'Extend iterations',
  resume_loop: 'Resume loop',
  submit_plan: 'Submit plan',
  update_plan_step: 'Update plan step',
  list_skills: 'List skills',
  activate_skill: 'Activate skill',
}

function humanizeSnake(s: string): string {
  const spaced = s.replaceAll('_', ' ').trim()
  if (!spaced) return spaced
  return spaced[0].toUpperCase() + spaced.slice(1)
}

/** Friendly tool title for permission cards (mcp__recur__list_findings → List findings). */
export function permissionToolLabel(tool: string): string {
  const t = (tool ?? '').trim()
  if (!t) return t
  if (t.startsWith('mcp__recur__')) {
    const name = t.slice('mcp__recur__'.length)
    return recurToolLabels[name] ?? humanizeSnake(name)
  }
  if (t.startsWith('mcp__')) {
    const parts = t.split('__')
    return humanizeSnake(parts[parts.length - 1] ?? t)
  }
  return t
}
