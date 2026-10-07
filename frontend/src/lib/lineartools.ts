// Display helpers for Linear MCP chat cards.
// Keep these pure so the "no JSON dump / friendly card" contracts stay testable
// without mounting AgentChat.

export type LinearArgs = {
  query: string
  team: string
  id: string
  issue: string
  name: string
  title: string
  project: string
  user: string
  urlOrId: string
}

export type LinearItem = {
  title: string
  identifier: string
  url: string
  summary: string
  body: string
  color: string
  chips: string[]
}

export type LinearView = {
  kind: 'list' | 'item'
  noun: string
  items: LinearItem[]
  hasNextPage: boolean
}

const COLLECTION_KEYS = [
  'issues', 'projects', 'teams', 'labels', 'users', 'comments', 'cycles',
  'documents', 'initiatives', 'milestones', 'notifications', 'templates',
  'diffs', 'releases', 'releaseNotes', 'release_notes', 'releasePipelines',
  'release_pipelines', 'agentSkills', 'agent_skills', 'statusUpdates',
  'status_updates', 'attachments',
]

const ENTITY_KEYS = [
  'issue', 'project', 'team', 'comment', 'document', 'initiative', 'milestone',
  'user', 'cycle', 'release', 'attachment', 'template', 'diff', 'notification',
  'label',
]

const VERBS: Record<string, [doing: string, done: string]> = {
  list: ['Listing', 'Listed'],
  get: ['Looking up', 'Looked up'],
  save: ['Saving', 'Saved'],
  delete: ['Deleting', 'Deleted'],
  create: ['Creating', 'Created'],
  update: ['Updating', 'Updated'],
  search: ['Searching', 'Searched'],
  mark: ['Marking', 'Marked'],
  share: ['Sharing', 'Shared'],
  unshare: ['Unsharing', 'Unshared'],
  merge: ['Merging', 'Merged'],
  submit: ['Submitting', 'Submitted'],
  resolve: ['Resolving', 'Resolved'],
  retire: ['Retiring', 'Retired'],
  restore: ['Restoring', 'Restored'],
  extract: ['Extracting', 'Extracted'],
  prepare: ['Preparing', 'Prepared'],
}

function asObject(raw: unknown): Record<string, unknown> | null {
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) return raw as Record<string, unknown>
  return null
}

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

function named(v: unknown): string {
  const s = str(v)
  if (s) return s
  const o = asObject(v)
  if (!o) return ''
  return str(o.name) || str(o.key) || str(o.displayName) || str(o.title) || str(o.email)
}

function isIssueId(s: string): boolean {
  return /^[A-Z][A-Z0-9]+-\d+$/.test(s)
}

function isUUID(s: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s)
}

function isHexColor(s: string): boolean {
  return /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(s)
}

function isTruncatedBlob(s: string): boolean {
  return /\(truncated\b/i.test(s) || /use `get_[a-z_]+` for full/i.test(s)
}

function firstLine(s: string, max = 88): string {
  const t = s.replace(/\s+/g, ' ').trim()
  if (!t) return ''
  if (t.length <= max) return t
  return t.slice(0, max - 1) + '…'
}

function humanKey(k: string): string {
  return k.replace(/[._]/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2').toLowerCase()
}

function singular(noun: string): string {
  if (noun.endsWith('ies')) return noun.slice(0, -3) + 'y'
  if (noun.endsWith('ses')) return noun.slice(0, -2)
  if (noun.endsWith('s') && !noun.endsWith('ss')) return noun.slice(0, -1)
  return noun
}

function linearServer(tool: string): string {
  if (!tool.startsWith('mcp__')) return ''
  const rest = tool.slice('mcp__'.length)
  const i = rest.lastIndexOf('__')
  if (i <= 0) return ''
  return rest.slice(0, i).toLowerCase()
}

/** Cursor/Recur Linear MCP: mcp__linear__list_projects, plugin server ids that contain linear. */
export function isLinearTool(tool: string | undefined | null): boolean {
  const t = (tool ?? '').trim()
  if (!t.startsWith('mcp__')) return false
  const server = linearServer(t)
  return server === 'linear' || server.includes('linear')
}

export function linearToolName(tool: string): string {
  return tool.split('__').pop() || tool
}

function parseJSONValue(raw: string): unknown {
  const t = raw.trim()
  if (!t) return null
  const tryParse = (s: string): unknown => {
    try {
      return JSON.parse(s)
    } catch {
      return null
    }
  }
  const direct = tryParse(t)
  if (direct != null) return direct
  const i = t.search(/[\{\[]/)
  if (i < 0) return null
  return tryParse(t.slice(i))
}

export function parseLinearArgs(raw: unknown): LinearArgs {
  const o = asObject(raw) ?? {}
  return {
    query: str(o.query),
    team: str(o.team) || str(o.teamId),
    id: str(o.id),
    issue: str(o.issue) || str(o.issueId),
    name: str(o.name),
    title: str(o.title),
    project: str(o.project) || str(o.projectId),
    user: str(o.user),
    urlOrId: str(o.urlOrId) || str(o.url_or_id),
  }
}

export function linearSubject(args: LinearArgs): string {
  return args.query || args.id || args.issue || args.name || args.title
    || args.urlOrId || args.project || args.team || args.user
}

export function linearInputBody(raw: unknown): string {
  const o = asObject(raw)
  if (!o) return ''
  return str(o.body) || str(o.description) || str(o.content) || str(o.markdown)
}

function parseItem(raw: unknown): LinearItem | null {
  const o = asObject(raw)
  if (!o) return null
  const id = str(o.identifier) || str(o.id)
  const identifier = isIssueId(id) ? id : (isIssueId(str(o.identifier)) ? str(o.identifier) : '')
  const title = str(o.name) || str(o.title) || identifier
  const url = str(o.url)
  const body = str(o.description) || str(o.body) || str(o.content) || str(o.markdown)
  let summary = str(o.summary) || str(o.subtitle)
  if (!summary && body && !isTruncatedBlob(body)) summary = firstLine(body)
  else if (summary) summary = firstLine(summary)
  if (isTruncatedBlob(summary)) summary = ''
  const color = isHexColor(str(o.color)) ? str(o.color) : ''
  const chips: string[] = []
  const push = (v: string) => {
    if (v && v !== title && v !== identifier && !chips.includes(v)) chips.push(v)
  }
  push(named(o.status))
  push(named(o.priority))
  push(named(o.assignee) || named(o.lead) || named(o.owner) || named(o.author) || named(o.creator))
  push(named(o.team) || named(o.leadTeam))
  push(str(o.visibility))
  push(str(o.state) || named(o.state))
  if (!title && !url && !body && !chips.length && !color) return null
  if (!title && isUUID(id) && !url && !body) return null
  return {
    title: title || (url ? 'Linear' : firstLine(body) || identifier || 'Untitled'),
    identifier,
    url,
    summary,
    body: isTruncatedBlob(body) ? '' : body,
    color,
    chips,
  }
}

export function parseLinearResult(result: string): LinearView | null {
  const v = parseJSONValue(result)
  if (v == null) return null
  if (Array.isArray(v)) {
    const items = v.map(parseItem).filter((x): x is LinearItem => !!x)
    if (!items.length) return null
    return { kind: items.length === 1 ? 'item' : 'list', noun: 'results', items, hasNextPage: false }
  }
  const o = asObject(v)
  if (!o) return null
  const hasNextPage = o.hasNextPage === true
    || asObject(o.pageInfo)?.hasNextPage === true
  if (str(o.identifier) || str(o.title) || str(o.name) || str(o.url)
    || str(o.description) || str(o.body) || str(o.content) || str(o.markdown)) {
    const item = parseItem(o)
    if (item) return { kind: 'item', noun: '', items: [item], hasNextPage: false }
  }
  for (const key of COLLECTION_KEYS) {
    const arr = o[key]
    if (!Array.isArray(arr)) continue
    return {
      kind: 'list',
      noun: humanKey(key),
      items: arr.map(parseItem).filter((x): x is LinearItem => !!x),
      hasNextPage,
    }
  }
  for (const key of ENTITY_KEYS) {
    const nested = o[key]
    const item = parseItem(nested)
    if (item) return { kind: 'item', noun: humanKey(key), items: [item], hasNextPage: false }
  }
  const item = parseItem(o)
  if (item) return { kind: 'item', noun: '', items: [item], hasNextPage: false }
  return null
}

function splitVerb(name: string): { verb: string; rest: string } {
  const i = name.indexOf('_')
  if (i <= 0) return { verb: name, rest: '' }
  return { verb: name.slice(0, i), rest: name.slice(i + 1).replaceAll('_', ' ') }
}

export function linearToolLabel(opts: {
  tool: string
  pending: boolean
  failed: boolean
  args: LinearArgs
  view: LinearView | null
}): string {
  const name = linearToolName(opts.tool)
  const { verb, rest } = splitVerb(name)
  const pair = VERBS[verb]
  if (opts.failed) {
    const noun = name.replaceAll('_', ' ')
    return noun ? `${noun.charAt(0).toUpperCase()}${noun.slice(1)} failed` : 'Linear tool failed'
  }
  const subject = linearSubject(opts.args)
    || (opts.view?.kind === 'item' ? (opts.view.items[0]?.identifier || opts.view.items[0]?.title) : '')
    || ''
  if (!pair) return name.replaceAll('_', ' ')
  const word = opts.pending ? pair[0] : pair[1]
  if (verb === 'list') return rest ? `${word} ${rest}` : word
  if (verb === 'get' || verb === 'search') {
    if (subject) return `${word} ${subject}`
    return rest ? `${word} ${rest}` : word
  }
  if (subject && (verb === 'save' || verb === 'update' || verb === 'create' || verb === 'delete')) {
    return rest ? `${word} ${rest}` : `${word} ${subject}`
  }
  return rest ? `${word} ${rest}` : word
}

export function linearToolMeta(args: LinearArgs, view: LinearView | null, label = ''): string {
  const parts: string[] = []
  const subject = linearSubject(args)
  if (args.query) parts.push(`"${args.query}"`)
  else if (args.team && !label.includes(args.team)) parts.push(args.team)
  else if (subject && !label.includes(subject)) parts.push(subject)
  if (view?.kind === 'list') {
    const n = view.items.length
    if (n === 0) parts.push('none')
    else parts.push(String(n))
  } else if (view?.kind === 'item') {
    const chips = view.items[0]?.chips ?? []
    for (const c of chips.slice(0, 2)) {
      if (!label.includes(c) && !parts.includes(c)) parts.push(c)
    }
  }
  return parts.join(' · ')
}

export function linearCountLabel(view: LinearView): string {
  const n = view.items.length
  const noun = view.noun || 'result'
  if (n === 1) return `1 ${singular(noun)}`
  return `${n} ${noun}`
}

export function linearItemUrl(view: LinearView | null): string {
  if (view?.kind !== 'item') return ''
  return view.items[0]?.url ?? ''
}
