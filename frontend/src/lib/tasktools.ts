// Display helpers for Claude Code's TaskCreate / TaskUpdate / TaskGet / TaskList
// chat cards. Keep these pure so the "no JSON dump / friendly label" contracts
// stay testable without mounting AgentChat.
//
// These are NOT the Task subagent tool (description + prompt). TaskCreate's
// `description` is the task body — using it as the collapsed row title is what
// made the transcript dump a truncated JSON blob next to a paragraph.

export type ClaudeTaskView = {
  id: string
  subject: string
  description: string
  status: string
  owner: string
  blocks: string[]
  blockedBy: string[]
}

export type ClaudeTaskArgs = {
  taskId: string
  subject: string
  description: string
  status: string
  owner: string
  addBlocks: string[]
  addBlockedBy: string[]
}

const CLAUDE_TASK_TOOLS = new Set([
  'TaskCreate',
  'TaskUpdate',
  'TaskGet',
  'TaskList',
])

export function isClaudeTaskTool(tool: string | undefined | null): boolean {
  return !!tool && CLAUDE_TASK_TOOLS.has(tool)
}

export function formatTaskId(id: string): string {
  const t = id.trim()
  if (!t) return ''
  return t.startsWith('#') ? t : `#${t}`
}

/** Chip / checklist copy: "in_progress" → "in progress". */
export function prettyTaskStatus(status: string): string {
  return status.replaceAll('_', ' ').trim()
}

/** Map Claude task status onto `.tc-finding-status` variants in `_chat.scss`. */
export function taskStatusChipClass(status: string): string {
  switch (status) {
    case 'in_progress':
      return 'doing'
    case 'completed':
      return 'done'
    case 'deleted':
      return 'invalid'
    default:
      return status
  }
}

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

function strList(v: unknown): string[] {
  if (!Array.isArray(v)) return []
  return v.map((x) => str(x)).filter(Boolean)
}

function collapse(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

export function parseClaudeTaskArgs(input: Record<string, unknown> | null | undefined): ClaudeTaskArgs {
  const p = input ?? {}
  return {
    taskId: str(p.taskId) || str(p.task_id),
    subject: str(p.subject),
    description: str(p.description),
    status: str(p.status),
    owner: str(p.owner),
    addBlocks: strList(p.addBlocks),
    addBlockedBy: strList(p.addBlockedBy),
  }
}

/** TaskCreate result: "Task #1 created successfully: Guard theme re-apply". */
export function parseTaskCreateId(result: string): string {
  const t = result.trim()
  if (!t) return ''
  const m = /^Task #(\S+) created successfully/i.exec(t)
  if (m) return m[1]
  try {
    const raw = JSON.parse(t)
    if (raw && typeof raw === 'object') {
      const id = str(raw.taskId) || str(raw.id) || str(raw.task?.id)
      if (id) return id
    }
  } catch { /* text ack */ }
  return ''
}

function asTaskView(raw: unknown): ClaudeTaskView | null {
  if (!raw || typeof raw !== 'object') return null
  const o = raw as Record<string, unknown>
  const id = str(o.id) || str(o.taskId)
  const subject = str(o.subject) || str(o.title) || str(o.content)
  if (!id && !subject) return null
  return {
    id,
    subject,
    description: str(o.description),
    status: str(o.status) || 'pending',
    owner: str(o.owner),
    blocks: strList(o.blocks),
    blockedBy: strList(o.blockedBy),
  }
}

/**
 * TaskList result is either JSON `{tasks:[…]}` or `#1 [pending] Subject` lines.
 * `[]` is a known empty list; `null` means no known shape matched (show raw).
 */
export function parseTaskList(result: string): ClaudeTaskView[] | null {
  const t = result.trim()
  if (!t || /^no tasks found$/i.test(t)) return []

  try {
    const raw = JSON.parse(t)
    const list = Array.isArray(raw) ? raw
      : Array.isArray(raw?.tasks) ? raw.tasks
      : Array.isArray(raw?.data?.tasks) ? raw.data.tasks
      : null
    if (list) {
      return (list as unknown[]).map(asTaskView).filter((x): x is ClaudeTaskView => !!x)
    }
    return null
  } catch { /* line format */ }

  const out: ClaudeTaskView[] = []
  for (const line of t.split('\n')) {
    const row = parseTaskListLine(line)
    if (row) out.push(row)
  }
  return out.length ? out : null
}

function parseTaskListLine(line: string): ClaudeTaskView | null {
  const m = /^#(\S+)\s+\[([^\]]+)\]\s+(.*)$/.exec(line.trim())
  if (!m) return null
  let rest = m[3]
  let blockedBy: string[] = []
  const blocked = rest.match(/\s+\[blocked by ([^\]]+)\]\s*$/i)
  if (blocked) {
    blockedBy = blocked[1].split(',').map((s) => s.replace(/^#/, '').trim()).filter(Boolean)
    rest = rest.slice(0, blocked.index).trimEnd()
  }
  let owner = ''
  const own = rest.match(/\s+\(([\w.-]+)\)\s*$/)
  if (own) {
    owner = own[1]
    rest = rest.slice(0, own.index).trimEnd()
  }
  return {
    id: m[1],
    subject: rest,
    description: '',
    status: m[2],
    owner,
    blocks: [],
    blockedBy,
  }
}

/**
 * TaskGet result is either JSON or:
 *   Task #1: Subject
 *   Status: in_progress
 *   Description: …
 *   Blocked by: #2
 *   Blocks: #3
 */
export function parseTaskGet(result: string): ClaudeTaskView | null {
  const t = result.trim()
  if (!t || /^task not found$/i.test(t)) return null

  try {
    const raw = JSON.parse(t)
    const view = asTaskView(raw?.task ?? raw)
    if (view) return view
  } catch { /* text format */ }

  const lines = t.split('\n')
  const head = /^Task #(\S+):\s*(.*)$/.exec(lines[0] ?? '')
  if (!head) return null
  let i = 1
  let status = ''
  if (lines[i]?.startsWith('Status:')) {
    status = lines[i].slice('Status:'.length).trim()
    i++
  }
  let description = ''
  if (lines[i]?.startsWith('Description:')) {
    description = lines[i].slice('Description:'.length)
    i++
    while (i < lines.length && !/^(Blocked by|Blocks):/.test(lines[i])) {
      description += '\n' + lines[i]
      i++
    }
    description = description.trim()
  }
  let blockedBy: string[] = []
  let blocks: string[] = []
  while (i < lines.length) {
    const line = lines[i]
    if (line.startsWith('Blocked by:')) {
      blockedBy = line.slice('Blocked by:'.length).split(',').map((s) => s.replace(/#/g, '').trim()).filter(Boolean)
    } else if (line.startsWith('Blocks:')) {
      blocks = line.slice('Blocks:'.length).split(',').map((s) => s.replace(/#/g, '').trim()).filter(Boolean)
    }
    i++
  }
  return {
    id: head[1],
    subject: head[2].trim(),
    description,
    status: status || 'pending',
    owner: '',
    blocks,
    blockedBy,
  }
}

export function claudeTaskCounts(tasks: ClaudeTaskView[]): { done: number; total: number } {
  return {
    done: tasks.filter((t) => t.status === 'completed').length,
    total: tasks.length,
  }
}

/** Verb for the collapsed row — never the task body, never JSON. */
export function claudeTaskLabel(
  tool: string,
  pending: boolean,
  args: ClaudeTaskArgs,
  list?: { done: number; total: number },
): string {
  switch (tool) {
    case 'TaskCreate':
      return pending ? 'Creating task' : 'Created task'
    case 'TaskUpdate': {
      if (pending) {
        if (args.status === 'in_progress') return 'Starting task'
        if (args.status === 'completed') return 'Completing task'
        if (args.status === 'deleted') return 'Deleting task'
        return 'Updating task'
      }
      if (args.status === 'in_progress') return 'Started task'
      if (args.status === 'completed') return 'Completed task'
      if (args.status === 'deleted') return 'Deleted task'
      return 'Updated task'
    }
    case 'TaskGet':
      return pending ? 'Reading task' : 'Read task'
    case 'TaskList': {
      if (pending) return 'Listing tasks'
      if (list?.total) return `Listed tasks · ${list.done}/${list.total}`
      return 'Listed tasks'
    }
    default:
      return pending ? 'Updating task' : 'Updated task'
  }
}

/** Dim header summary: subject only — #id is a chip, never raw JSON. */
export function claudeTaskMeta(tool: string, args: ClaudeTaskArgs, result = ''): string {
  switch (tool) {
    case 'TaskCreate':
    case 'TaskUpdate':
      return collapse(args.subject)
    case 'TaskGet':
      return collapse(parseTaskGet(result)?.subject || args.subject)
    case 'TaskList': {
      const active = parseTaskList(result)?.find((t) => t.status === 'in_progress')
      return collapse(active?.subject ?? '')
    }
    default:
      return ''
  }
}

/** Status for the header chip (TaskUpdate args, or TaskGet result). */
export function claudeTaskChipStatus(tool: string, args: ClaudeTaskArgs, result = ''): string {
  if (tool === 'TaskUpdate') return args.status
  if (tool === 'TaskGet') return parseTaskGet(result)?.status || ''
  return ''
}

/**
 * True when the expanded card has something the header does not already say.
 * Status-only TaskUpdate / subject-only TaskCreate stay a one-liner.
 */
export function claudeTaskHasBody(
  tool: string,
  args: ClaudeTaskArgs,
  opts: { resultError?: boolean; result?: string; pending?: boolean } = {},
): boolean {
  if (opts.resultError) return true
  switch (tool) {
    case 'TaskCreate':
      return !!args.description
    case 'TaskUpdate':
      return !!(args.description || args.owner || args.addBlocks.length || args.addBlockedBy.length)
    case 'TaskGet': {
      if (opts.pending) return true
      const got = parseTaskGet(opts.result ?? '')
      if (got) {
        return !!(got.description || got.owner || got.blocks.length || got.blockedBy.length)
      }
      return !!(opts.result ?? '').trim()
    }
    case 'TaskList':
      return true
    default:
      return false
  }
}

/** Provider success acks that only restate the header (hide them). */
export function claudeTaskResultIsAck(tool: string, result: string): boolean {
  const t = result.trim()
  if (!t) return true
  if (tool === 'TaskCreate') return /^Task #\S+ created successfully/i.test(t)
  if (tool === 'TaskUpdate') return /^Updated task #/i.test(t)
  return false
}
