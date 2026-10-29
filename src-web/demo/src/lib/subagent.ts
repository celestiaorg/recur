// Display helpers for Claude Code's Agent/Task subagent tool (and SendMessage).
// Keep these pure so "no JSON dump / no internal launch metadata" stays testable
// without mounting AgentChat.
//
// Claude Code renamed the spawn tool from Task → Agent in v2.1.63. Recur's card
// is still Task; the display layer maps the name, and these helpers accept both.

export function isSubagentTool(tool: string | undefined | null): boolean {
  return tool === 'Task' || tool === 'Agent'
}

export function isSendMessageTool(tool: string | undefined | null): boolean {
  return tool === 'SendMessage'
}

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

function collapse(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

export function parseSubagentArgs(input: Record<string, unknown> | null | undefined): {
  description: string
  prompt: string
  subagentType: string
  model: string
  background: boolean
} {
  const p = input ?? {}
  const bg = p.run_in_background
  return {
    description: str(p.description),
    prompt: str(p.prompt),
    subagentType: str(p.subagent_type),
    model: str(p.model),
    background: bg === true || bg === 'true',
  }
}

export function parseSendMessageArgs(input: Record<string, unknown> | null | undefined): {
  description: string
  message: string
} {
  const p = input ?? {}
  return {
    description: str(p.description) || str(p.summary),
    message: str(p.message) || str(p.prompt),
  }
}

/**
 * Claude Code's background-launch ack is instructions for the MODEL
 * (agentId, SendMessage, output_file). Prefix-match so a Read of a file that
 * merely mentions the phrase is not treated as a launch ack. Keep in sync
 * with engine isSubagentLaunchMeta.
 */
export function isSubagentLaunchAck(result: string): boolean {
  return result.trim().startsWith('Async agent launched successfully')
}

/** Collapsed-row meta: type (and model). Never JSON, never the description. */
export function subagentMeta(type: string, model: string): string {
  if (type && model) return `${type} · ${model}`
  return type || model || ''
}

export function sendMessageLabel(pending: boolean, description: string): string {
  if (pending) return description ? `Sending ${description}` : 'Sending message'
  return description ? `Sent ${description}` : 'Sent a message'
}

export function sendMessageMeta(description: string, message: string): string {
  // Description is the row title — meta is a short prompt preview, never JSON
  // and never the agent id (`to` / `task_id`).
  if (description) return ''
  return collapse(message)
}
