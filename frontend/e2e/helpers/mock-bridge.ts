// Mock Wails bridge for the e2e suite.
//
// A Wails v3 app's frontend talks to Go over the native runtime bridge. In a plain
// browser (which is all Playwright can drive) those bound calls 404 — the runtime
// only offers "UI previews". This helper intercepts the bridge endpoint
// (`POST /wails/runtime`) and answers bound-method calls with fixture data, so the
// real Svelte frontend renders and behaves against realistic, backend-shaped data
// with NO live Go backend. That makes the suite deterministic and CI-friendly.
//
// The fixture (e2e/fixtures/seed.json) is the REAL store seed dumped from Go
// (`go run ./cmd/seeddump`), so the tests exercise the actual data contract.
//
// Method dispatch: a bound call carries a numeric `methodID` (a stable hash of the
// Go method). We resolve methodID → method NAME by parsing the generated bindings
// (bindings/recur/internal/app/*.ts) at setup time, then map by NAME — so a binding
// regeneration that changes the IDs does NOT break this map (names are stable).

import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import type { Page, Route } from '@playwright/test'
import { applyChatTabOrder } from '../../src/lib/chat-tab-reorder'
import { peelChatSurface } from '../../src/lib/chat-surface'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(here, '..', '..', '..')
const bindingsDirs = [
  join(here, '..', '..', 'bindings', 'recur', 'internal', 'app'),
  // Official Wails Dock service bindings (SetBadge / RemoveBadge).
  join(here, '..', '..', 'bindings', 'github.com', 'wailsapp', 'wails', 'v3', 'pkg', 'services', 'dock'),
]

// TextMate catalog for e2e. Metadata comes from shipped bundle.toml so ids/paths
// cannot silently drift; file bodies are read only from ReadGrammar/ReadTheme
// (never at import) so a missing theme file fails that call, not suite collection.
const MOCK_GRAMMAR_SCOPES = new Set(['source.go'])
const MOCK_THEME_IDS = new Set(['recur', 'paper', 'cool-glow'])

function readShipped(...parts: string[]): string {
  return readFileSync(join(repoRoot, 'bundles', ...parts), 'utf8')
}

function parseInlineTable(raw: string): Record<string, string> {
  const out: Record<string, string> = {}
  const re = /(\w+)\s*=\s*"([^"]*)"/g
  let m: RegExpExecArray | null
  while ((m = re.exec(raw)) !== null) out[m[1]] = m[2]
  return out
}

export function parseTomlArrayTables(src: string, name: string): Record<string, unknown>[] {
  const parts = src.split(new RegExp(`(?:^|\\n)\\[\\[${name}\\]\\]\\s*`))
  const tables: Record<string, unknown>[] = []
  for (const part of parts.slice(1)) {
    const body = part.split(/\n\[\[/)[0]
    const rec: Record<string, unknown> = {}
    for (const line of body.split('\n')) {
      const trimmed = line.trim()
      if (!trimmed || trimmed.startsWith('#')) continue
      const eq = trimmed.indexOf('=')
      if (eq < 0) continue
      const key = trimmed.slice(0, eq).trim()
      const raw = trimmed.slice(eq + 1).trim()
      if (raw.startsWith('{')) rec[key] = parseInlineTable(raw)
      else if (raw.startsWith('[')) rec[key] = [...raw.matchAll(/"([^"]*)"/g)].map((x) => x[1])
      else if (raw.startsWith('"') && raw.endsWith('"')) {
        try {
          rec[key] = JSON.parse(raw)
        } catch {
          rec[key] = raw.slice(1, -1)
        }
      }
      else if (raw === 'true' || raw === 'false') rec[key] = raw === 'true'
      else if (/^-?\d+$/.test(raw)) rec[key] = Number(raw)
      else rec[key] = raw
    }
    tables.push(rec)
  }
  return tables
}

type MockGrammar = {
  scopeName: string
  id: string
  path: string
  language?: string
  fileTypes?: string[]
  bundleName: string
  sourceRoot: string
  absPath: string
  lineComment?: string
  blockCommentStart?: string
  blockCommentEnd?: string
  increaseIndentPattern?: string
  decreaseIndentPattern?: string
  indentNextLinePattern?: string
  zeroIndentPattern?: string
  disableIndentCorrections?: boolean
  closeBrackets?: string[]
}
type MockTheme = {
  id: string
  name: string
  path: string
  uiTheme: string
  order?: number
  preview: Record<string, string>
  bundleName: string
  sourceRoot: string
  absPath: string
}

let tmCatalog: { grammars: MockGrammar[]; themes: MockTheme[] } | null = null

function loadTmCatalog(): { grammars: MockGrammar[]; themes: MockTheme[] } {
  if (tmCatalog) return tmCatalog
  const grammars: MockGrammar[] = []
  try {
    for (const g of parseTomlArrayTables(readShipped('Languages', 'bundle.toml'), 'grammar')) {
      const scopeName = String(g.scopeName ?? '')
      if (!MOCK_GRAMMAR_SCOPES.has(scopeName)) continue
      grammars.push({
        scopeName,
        id: String(g.id ?? ''),
        path: String(g.path ?? ''),
        language: g.language ? String(g.language) : undefined,
        fileTypes: Array.isArray(g.fileTypes) ? g.fileTypes.map(String) : undefined,
        bundleName: 'Languages',
        sourceRoot: 'default',
        absPath: join(repoRoot, 'bundles', 'Languages', String(g.path ?? '')),
      })
    }
    try {
      for (const d of parseTomlArrayTables(readShipped('LanguageData', 'bundle.toml'), 'languageData')) {
        const scopeName = String(d.scopeName ?? '')
        const g = grammars.find((x) => x.scopeName === scopeName)
        if (!g) continue
        if (typeof d.lineComment === 'string') g.lineComment = d.lineComment
        if (typeof d.blockCommentStart === 'string') g.blockCommentStart = d.blockCommentStart
        if (typeof d.blockCommentEnd === 'string') g.blockCommentEnd = d.blockCommentEnd
        if (typeof d.increaseIndentPattern === 'string') g.increaseIndentPattern = d.increaseIndentPattern
        if (typeof d.decreaseIndentPattern === 'string') g.decreaseIndentPattern = d.decreaseIndentPattern
        if (typeof d.indentNextLinePattern === 'string') g.indentNextLinePattern = d.indentNextLinePattern
        if (typeof d.zeroIndentPattern === 'string') g.zeroIndentPattern = d.zeroIndentPattern
        if (d.disableIndentCorrections === true) g.disableIndentCorrections = true
        if (Array.isArray(d.closeBrackets)) g.closeBrackets = d.closeBrackets.map(String)
      }
    } catch {
      // LanguageData pack missing — Mod-/ falls back to no commentTokens.
    }
  } catch {
    // Languages manifest missing — editor highlight specs fail at ReadGrammar.
  }
  const themes: MockTheme[] = []
  for (const bundle of [
    { dir: 'Recur', name: 'Recur' },
    { dir: 'Themes', name: 'Themes' },
  ]) {
    try {
      for (const t of parseTomlArrayTables(readShipped(bundle.dir, 'bundle.toml'), 'theme')) {
        const id = String(t.id ?? '')
        if (!MOCK_THEME_IDS.has(id)) continue
        themes.push({
          id,
          name: String(t.name ?? ''),
          path: String(t.path ?? ''),
          uiTheme: String(t.uiTheme ?? ''),
          order: typeof t.order === 'number' ? t.order : undefined,
          preview: t.preview && typeof t.preview === 'object' && !Array.isArray(t.preview)
            ? (t.preview as Record<string, string>)
            : {},
          bundleName: bundle.name,
          sourceRoot: 'default',
          absPath: join(repoRoot, 'bundles', bundle.dir, String(t.path ?? '')),
        })
      }
    } catch {
      // That bundle's themes unavailable — settings picker just omits them.
    }
  }
  tmCatalog = { grammars, themes }
  return tmCatalog
}

function publicGrammar(g: MockGrammar) {
  const { absPath: _, ...info } = g
  return info
}
function publicTheme(t: MockTheme) {
  const { absPath: _, ...info } = t
  return info
}

type MockTermTheme = {
  id: string
  name: string
  path: string
  uiTheme: string
  order?: number
  palette: Record<string, string>
  bundleName: string
  sourceRoot: string
}

let termThemeCatalog: MockTermTheme[] | null = null

function loadTerminalThemes(): MockTermTheme[] {
  if (termThemeCatalog) return termThemeCatalog
  const out: MockTermTheme[] = []
  try {
    for (const t of parseTomlArrayTables(readShipped('Terminal', 'bundle.toml'), 'terminalTheme')) {
      const path = String(t.path ?? '')
      let palette: Record<string, string> = {}
      try {
        palette = JSON.parse(readShipped('Terminal', path)) as Record<string, string>
      } catch {
        continue
      }
      out.push({
        id: String(t.id ?? ''),
        name: String(t.name ?? ''),
        path,
        uiTheme: String(t.uiTheme ?? 'dark'),
        order: typeof t.order === 'number' ? t.order : undefined,
        palette,
        bundleName: 'Terminal',
        sourceRoot: 'default',
      })
    }
  } catch {
    // Terminal pack missing — picker shows Match app only.
  }
  termThemeCatalog = out
  return out
}

export type Seed = {
  projects: any[]
  projectGroups?: any[]
  loops: any[]
  prs: any[]
  inbox: any[]
  policy: any
  settings: any
  resource: any
  loopById: Record<string, any>
  feedByLoop: Record<string, any[]>
  agentsByLoop: Record<string, any[]>
  escByLoop: Record<string, any>
  prById: Record<string, any>
}

export function loadSeed(): Seed {
  const seed: Seed = JSON.parse(readFileSync(join(here, '..', 'fixtures', 'seed.json'), 'utf8'))
  // seeddump leaves WorktreePath empty (demo loops never created a git
  // worktree). Observatory skips ListTree until a path is set, so every
  // editor spec would otherwise render "No files".
  const pathFor = (id: string) => `/mock/worktrees/${id}`
  for (const l of seed.loops) {
    if (!l.worktreePath) l.worktreePath = pathFor(l.id)
  }
  for (const l of Object.values(seed.loopById ?? {})) {
    if (l && !l.worktreePath) l.worktreePath = pathFor(l.id)
  }
  return seed
}

// Build methodID -> methodName from the generated bindings, so the map self-heals
// across `wails3 generate bindings` runs (IDs change, names don't).
function methodIdToName(): Map<number, string> {
  const map = new Map<number, string>()
  for (const dir of bindingsDirs) {
    let files: string[]
    try {
      files = readdirSync(dir).filter((f) => f.endsWith('service.ts'))
    } catch {
      continue
    }
    for (const file of files) {
      const src = readFileSync(join(dir, file), 'utf8')
      // Each exported binding is: export function NAME(...) { ... $Call.ByID(<id> ...
      const re = /export function (\w+)\s*\([^)]*\)[\s\S]*?\$Call\.ByID\((\d+)/g
      let m: RegExpExecArray | null
      while ((m = re.exec(src)) !== null) map.set(Number(m[2]), m[1])
    }
  }
  return map
}

// A resolver receives the call's positional args, the seed, and the effective
// resolver table (base merged with any per-test overrides) so a batched method
// can delegate to its single-item sibling and pick up that test's override.
type Resolver = (
  args: any[],
  seed: Seed,
  table: Record<string, Resolver>,
) => unknown | Promise<unknown>

/** Seed dumps omit worktreePath; Observatory will not ListTree without one. */
function withMockWorktree<T extends { id?: string; archived?: boolean; worktreePath?: string; phase?: string; lastError?: string } | null>(
  ws: T,
): T {
  if (!ws || ws.archived || ws.worktreePath) return ws
  if (ws.phase === 'failed' && ws.lastError) return ws
  return { ...ws, worktreePath: `/tmp/recur-e2e/${ws.id ?? 'ws'}` }
}

function mockBriefing(s: Seed) {
  const workspaces: Record<string, unknown>[] = []
  for (const l of s.loops) {
    if (l.archived || l.isMain) continue
    const chatNote = s.inbox.find((it) => it.unread && it.kind === 'chat' && it.workspaceId === l.id)
    let bucket = ''
    let reason = ''
    let next = ''
    if (l.group === 'needs' || l.state === 'escalated' || l.state === 'failed') {
      bucket = 'needs'
      reason = l.state === 'failed' ? 'failure' : 'question'
      next = 'Answer the prompt in this workspace'
    } else if (chatNote) {
      bucket = 'needs'
      reason = 'unseen_chat'
      next = 'Read the response'
    } else if (l.group === 'active') {
      bucket = 'working'
      reason = 'active_agent'
      next = 'Let the agent finish this turn'
    } else if (l.group === 'ready') {
      bucket = 'pickup'
      reason = 'unfinished_plan'
      next = 'Pick this up where you left off'
    } else {
      continue
    }
    const steps = (l as any).plan?.steps ?? []
    workspaces.push({
      workspaceId: l.id,
      projectId: l.projectId,
      projectName: l.projectId,
      title: l.short || l.goal,
      branch: l.branch,
      bucket,
      reasons: [reason],
      now: l.last || l.statusLabel || l.phase || '',
      next,
      lastActivityAt: Date.now(),
      lastSummary: l.last || '',
      state: l.state,
      phase: l.phase,
      diffAdd: (l as any).diff?.add ?? 0,
      diffDel: (l as any).diff?.del ?? 0,
      planDone: steps.filter((s: any) => s.status === 'done' || s.status === 'skipped').length,
      planTotal: steps.length,
      primary: {
        label: reason === 'unseen_chat' ? 'Read' : 'Open',
        target: reason === 'unseen_chat' && typeof chatNote?.target === 'string'
          ? chatNote.target
          : `workspace:${l.id}`,
        kind: 'open',
      },
      secondary:
        bucket === 'pickup'
          ? [{ label: 'Continue chat', target: `chat:${l.id}:PA`, kind: 'continue' }]
          : undefined,
    })
  }
  return {
    generatedAt: Date.now(),
    counts: {
      needs: workspaces.filter((w) => w.bucket === 'needs').length,
      working: workspaces.filter((w) => w.bucket === 'working').length,
      pickup: workspaces.filter((w) => w.bucket === 'pickup').length,
    },
    workspaces,
  }
}

function listWorkspaceChats(s: Seed, wsId: string) {
  const w = s.loopById?.[wsId]
  const hasRun = !!w && (w.runStarted || (w.type ?? 'loop') === 'loop')
  const extra = ((s as any).chatsByLoop?.[wsId] ?? []) as { code: string; title: string; updatedAt: number; mode?: string; loopId?: string; policyId?: string; promptGroup?: string }[]
  const rest = extra.filter((t) => t.code !== 'PA' && t.code !== 'MA')
  const pa = extra.find((t) => t.code === 'PA')
  const tabs = [{ code: 'PA', title: 'New chat', updatedAt: 0, ...pa }, ...rest]
  const hidden = !!((s as any).hiddenMaestro ?? {})[wsId]
  const unordered = hasRun && !hidden
    ? [{ code: 'MA', title: 'Loop', mode: 'loop', updatedAt: 0 }, ...tabs]
    : tabs
  const order = ((s as any).chatOrderByLoop?.[wsId] ?? []) as string[]
  const mem = ((s as any).chatLoopByKey ?? {}) as Record<string, { loopId?: string; policyId?: string }>
  return applyChatTabOrder(unordered, order).map((tab) => {
    const hit = mem[`${wsId}\0${tab.code}`]
    return hit ? { ...tab, ...hit } : tab
  })
}

// BridgeError lets a test resolver simulate a failed bound call (Wails marshals a
// trailing Go `error` into a rejected promise) — returning one from a resolver
// answers the call with a 500 so the frontend's error toast path is exercised.
export class BridgeError {
  message: string
  constructor(message = 'mock failure') { this.message = message }
}

function readBody(absPath: string, label: string): string | BridgeError {
  try {
    return readFileSync(absPath, 'utf8')
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err)
    return new BridgeError(`${label}: ${msg}`)
  }
}

let commentSeq = 0
const nowComment = (bodyHtml: string) => ({
  id: `c-${Date.now()}-${++commentSeq}`,
  code: 'AR',
  author: 'You',
  verb: 'commented',
  avatarCss: 'background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)',
  time: 'now',
  bodyHtml,
})

// Resolvers keyed by Go method NAME. Getters return fixture data; mutations return a
// sane, shape-correct value so the UI's optimistic updates work without a backend.
export const resolvers: Record<string, Resolver> = {
  // Projects
  ListProjects: (_a, s) => s.projects,
  ListProjectGroups: (_a, s) => s.projectGroups ?? [],
  GetProject: (a, s) => s.projects.find((p) => p.id === a[0]) ?? null,
  AddProject: (a, s) => {
    const name = a[0]?.name || 'new-project'
    const p = {
      ...a[0],
      id: a[0]?.id || name,
      name,
      open: a[0]?.open ?? true,
      baseBranch: a[0]?.baseBranch || 'origin/main',
      isGit: a[0]?.isGit ?? !String(a[0]?.repoPath || '').includes('no-git'),
    }
    s.projects = [...s.projects, p]
    if (p.isGit && !s.loops.some((l) => l.isMain && l.projectId === p.id)) {
      const local = String(p.baseBranch || 'main').replace(/^origin\//, '') || 'main'
      const ws = withMockWorktree({
        id: `MAIN-${p.id}`,
        projectId: p.id,
        type: 'main',
        isMain: true,
        short: 'Working copy',
        goal: 'Working copy',
        branch: local,
        baseBranch: '',
        statusLabel: 'Idle',
        diff: { add: 0, del: 0 },
      })
      s.loops = [...s.loops, ws]
      s.loopById[ws.id] = ws
    }
    return p
  },
  SuggestBaseBranch: (a) => {
    const path = String(a[0] || '')
    if (!path) return 'main'
    // Mirror git.SuggestBaseBranch preference for e2e: remotes win when "git-like".
    return path.includes('no-git') ? 'main' : 'origin/main'
  },
  ListBranchesForPath: (a) => {
    const path = String(a[0] || '')
    if (!path || path.includes('no-git')) return []
    // Real tips only — do not inject empty/stale preferred as a phantom choice.
    const known = ['origin/main', 'develop', 'feature/auth', 'main']
    const preferred = String(a[1] || '')
    const names = (preferred && known.includes(preferred) ? [preferred, ...known] : known).filter(
      (b, i, arr) => arr.indexOf(b) === i,
    )
    const remote = names.filter((n) => n.startsWith('origin/')).map((name) => ({ name, group: 'origin' }))
    const local = names.filter((n) => !n.startsWith('origin/')).map((name) => ({ name, group: 'Local' }))
    return [...remote, ...local]
  },
  AddProjectGroup: (a, s) => {
    const g = {
      id: a[0]?.id || crypto.randomUUID(),
      name: a[0]?.name || 'New group',
      open: a[0]?.open ?? true,
    }
    s.projectGroups = [...(s.projectGroups ?? []), g]
    return g
  },
  UpdateProjectGroup: (a, s) => {
    const g = a[0]
    s.projectGroups = (s.projectGroups ?? []).map((x) => (x.id === g.id ? { ...x, ...g } : x))
    return (s.projectGroups ?? []).find((x) => x.id === g.id) ?? g
  },
  DeleteProjectGroup: (a, s) => {
    const id = a[0]
    s.projectGroups = (s.projectGroups ?? []).filter((x) => x.id !== id)
    s.projects = s.projects.map((p) => (p.groupId === id ? { ...p, groupId: '' } : p))
    return null
  },
  ReorderProjectGroups: (a, s) => {
    const ids: string[] = a[0] ?? []
    const byId = new Map((s.projectGroups ?? []).map((g) => [g.id, g]))
    const next = []
    for (const id of ids) {
      const g = byId.get(id)
      if (g) next.push(g)
    }
    s.projectGroups = next
    return null
  },
  SetProjectGroup: (a, s) => {
    const [projectID, groupID] = a
    s.projects = s.projects.map((p) => (p.id === projectID ? { ...p, groupId: groupID || '' } : p))
    return s.projects.find((p) => p.id === projectID) ?? null
  },
  UpdateProjectRunConfig: (a, s) => {
    const p = a[0]
    s.projects = s.projects.map((x) => (x.id === p.id ? { ...x, ...p } : x))
    return s.projects.find((x) => x.id === p.id) ?? p
  },
  PreviewCopyFiles: (a) => {
    const patterns = String(a[1] ?? '').trim()
    const root = '/Users/josh/code/storefront'
    if (!patterns) return { root, files: [], count: 0 }
    return {
      root,
      files: ['apps/web/.env', 'apps/api/.env', 'deploy/.env.mainnet'],
      count: 3,
    }
  },
  ReorderProjects: (a, s) => {
    const ids: string[] = a[0] ?? []
    const byId = new Map(s.projects.map((p) => [p.id, p]))
    const next = []
    for (const id of ids) {
      const p = byId.get(id)
      if (p) next.push(p)
    }
    s.projects = next
    return null
  },
  DeleteProject: () => null,
  ListBranches: (a, s) => {
    const p = s.projects.find((x) => x.id === a[0])
    // Seed fixtures always have a real base branch; empty list = empty/unborn repo.
    // Extra names give the searchable branch picker something to filter against.
    // Shape matches domain.BranchChoice (remote groups then Local, contiguous).
    if (!p?.baseBranch) return []
    const names = [p.baseBranch, 'origin/main', 'develop', 'feature/auth', 'feature/search', 'hotfix/login'].filter(
      (b, i, arr) => arr.indexOf(b) === i,
    )
    const remote = names.filter((n) => n.startsWith('origin/')).map((name) => ({ name, group: 'origin' }))
    const local = names.filter((n) => !n.startsWith('origin/')).map((name) => ({ name, group: 'Local' }))
    return [...remote, ...local]
  },
  // Workspaces. The Loop* names below are the pre-rename spelling and are kept so
  // older specs keep resolving; the Workspace* names are what the current bindings
  // actually call, and without them the app boots to an empty "No workspaces
  // running" list and every spec that opens one times out.
  //
  // Seed dumps leave worktreePath empty (no real checkout). Observatory skips
  // ListTree until a path exists, so stamp a mock root for non-archived rows.
  ListWorkspaces: (_a, s) => [...s.loops].reverse().map(withMockWorktree),
  GetWorkspace: (a, s) => withMockWorktree(s.loopById[a[0]] ?? null),
  ListOpenInTargets: () => [
    { id: 'finder', name: 'Finder', kind: 'app' },
    { id: 'cursor', name: 'Cursor', kind: 'app' },
    { id: 'terminal', name: 'Terminal', kind: 'app' },
  ],
  OpenWorkspaceIn: () => null,
  GetWorkspaceFeed: (a, s) => s.feedByLoop[a[0]] ?? [],
  LastAssistantText: () => '',
  WatchWorkspace: () => null,
  WorkspaceSessions: () => [],
  WorkspaceChatItems: () => [],
  WorkspaceChatModels: () => [],
  WorkspaceChatSelection: () => ({ model: '', flags: {}, mode: '' }),
  RememberWorkspaceChatSelection: () => null,
  RememberWorkspaceChatSimplify: () => null,
  ApplyProvisionalChatTitles: () => null,
  RememberWorkspaceChatPromptGroup: (a, s) => {
    const wsId = String(a[0] ?? '')
    const code = String(a[1] ?? '')
    const group = String(a[2] ?? '')
    if (!wsId || !code || !group) return null
    const by = ((s as any).chatsByLoop ??= {})
    const list = [...(by[wsId] ?? [])] as { code: string; title: string; updatedAt: number; promptGroup?: string }[]
    const i = list.findIndex((t) => t.code === code)
    if (i >= 0) list[i] = { ...list[i], promptGroup: group }
    else list.push({ code, title: 'New chat', updatedAt: 0, promptGroup: group })
    by[wsId] = list
    return null
  },
  RememberChatLoopPolicy: (a, s) => {
    const wsId = String(a[0] ?? '')
    const code = String(a[1] ?? '')
    const loopId = String(a[2] ?? '')
    const policyId = String(a[3] ?? '')
    const mem = ((s as any).chatLoopByKey ??= {}) as Record<string, { loopId: string; policyId: string }>
    mem[`${wsId}\0${code}`] = { loopId, policyId }
    const by = ((s as any).chatsByLoop ??= {})
    const extra = (by[wsId] ?? []) as { code: string }[]
    const i = extra.findIndex((t) => t.code === code)
    if (i >= 0) extra[i] = { ...extra[i], loopId, policyId }
    else if (code && code !== 'PA' && code !== 'MA') {
      by[wsId] = [...extra, { code, title: 'New chat', updatedAt: 0, loopId, policyId }]
    }
    return null
  },
  SetWorkspaceChatFocus: () => null,
  SetChatUIOccluded: () => null,
  MarkWorkspaceChatSeen: () => null,
  AttentionCounts: () => ({ unseen: 0, inboxUnread: 0, dock: 0, appUnseen: 0, byWorkspace: {}, byProject: {} }),
  GetBriefing: (_a, s) => mockBriefing(s),
  MarkWorkspaceViewed: () => null,
  AppChatItems: () => [],
  AppChatSelection: () => ({ model: '', flags: {} }),
  RememberAppChatSelection: () => null,
  SendAppMessage: () => '',
  CancelAppMessage: () => null,
  SetAppChatFocus: () => null,
  MarkAppChatSeen: () => null,
  ListAppChats: (_a, s) => {
    const extra = ((s as any).appChats ?? []) as { code: string; title: string }[]
    return [{ code: 'APP', title: 'Ask Recur', mode: 'ask' }, ...extra.map((t) => ({ ...t, mode: 'ask' }))]
  },
  NewAppChat: (_a, s) => {
    const extra = ((s as any).appChats ??= []) as { code: string; title: string }[]
    const tab = { code: `AP${extra.length + 2}`, title: 'New chat' }
    extra.push(tab)
    return { ...tab, mode: 'ask' }
  },
  CloseAppChat: (args, s) => {
    const code = String(args[0] ?? '')
    const extra = (((s as any).appChats ?? []) as { code: string; title: string }[]).filter((t) => t.code !== code)
    ;(s as any).appChats = extra
    return [{ code: 'APP', title: 'Ask Recur', mode: 'ask' }, ...extra.map((t) => ({ ...t, mode: 'ask' }))]
  },
  // Wails Dock service (frontend-driven badge; no-op in browser e2e)
  SetBadge: () => null,
  RemoveBadge: () => null,
  GetBadge: () => null,
  SetCustomBadge: () => null,
  ShowAppIcon: () => null,
  HideAppIcon: () => null,
  // Unified chat surface: a workspace with an engaged run leads with the Loop
  // (MA) tab, mirroring Engine.ListWorkspaceChats. Extra tabs from
  // CreateWorkspaceChat are kept so a mid-turn "new chat" is a real idle tab.
  ListWorkspaceChats: (a, s) => listWorkspaceChats(s, a[0]),
  CreateWorkspaceChat: (a, s) => {
    const by = ((s as any).chatsByLoop ??= {})
    const n = (by[a[0]]?.length ?? 0) + 2 // PA is 1; first create → PA2
    const tab = { code: `PA${n}`, title: 'New chat', updatedAt: 0 }
    by[a[0]] = [...(by[a[0]] ?? []), tab]
    return tab
  },
  ForkWorkspaceChat: (a, s) => {
    const by = ((s as any).chatsByLoop ??= {})
    const n = (by[a[0]]?.length ?? 0) + 2
    const tab = { code: `PA${n}`, title: 'Forked chat', updatedAt: Date.now() }
    by[a[0]] = [...(by[a[0]] ?? []), tab]
    return tab
  },
  WorkspaceChatTurnDiff: () => ({ hash: "", message: "", files: [] }),
  ResetWorkspaceChat: () => null,
  ForkWorkspaceChatToWorkspace: (a, s) => {
    const src = s.loopById?.[a[0]]
    const ws = {
      id: 'WS-FORK',
      projectId: src?.projectId,
      type: 'solo',
      goal: 'Forked chat',
      short: 'Forked chat',
      group: 'active',
      statusLabel: 'Creating',
      phase: 'creating',
      branch: 'recur/fork',
      baseBranch: src?.baseBranch || 'main',
    }
    s.loopById[ws.id] = ws
    s.loops = [...(s.loops ?? []), ws]
    return ws
  },
  CloseWorkspaceChat: (a, s) => {
    const wsId = a[0] as string
    const code = a[1] as string
    const by = ((s as any).chatsByLoop ??= {})
    const closed = ((s as any).closedChatsByLoop ??= {})
    const extra = by[wsId] ?? []
    if (code === 'MA') {
      ;((s as any).hiddenMaestro ??= {})[wsId] = true
      const w = s.loopById?.[wsId]
      if (w) w.loopChatCode = ''
      const tab = { code: 'MA', title: 'Loop', mode: 'loop', updatedAt: Date.now() }
      closed[wsId] = [tab, ...(closed[wsId] ?? []).filter((t: any) => t.code !== 'MA')]
    } else {
      const tab = extra.find((t: any) => t.code === code)
      if (tab) {
        const closedTab = { ...tab, updatedAt: tab.updatedAt || Date.now() }
        closed[wsId] = [closedTab, ...(closed[wsId] ?? []).filter((t: any) => t.code !== code)]
        by[wsId] = extra.filter((t: any) => t.code !== code)
      }
    }
    return listWorkspaceChats(s, wsId)
  },
  ListClosedWorkspaceChats: (a, s) => ((s as any).closedChatsByLoop?.[a[0]] ?? []),
  DeleteClosedWorkspaceChat: (a, s) => {
    const wsId = a[0] as string
    const code = a[1] as string
    if (listWorkspaceChats(s, wsId).some((t) => t.code === code)) {
      return ((s as any).closedChatsByLoop?.[wsId] ?? [])
    }
    const closed = ((s as any).closedChatsByLoop ??= {})
    closed[wsId] = (closed[wsId] ?? []).filter((t: any) => t.code !== code)
    return closed[wsId]
  },
  ReopenWorkspaceChat: (a, s) => {
    const wsId = a[0] as string
    const code = a[1] as string
    const by = ((s as any).chatsByLoop ??= {})
    const closed = ((s as any).closedChatsByLoop ??= {})
    const list = closed[wsId] ?? []
    const tab = list.find((t: any) => t.code === code)
    if (tab) {
      closed[wsId] = list.filter((t: any) => t.code !== code)
      if (code === 'MA') {
        delete ((s as any).hiddenMaestro ??= {})[wsId]
        const w = s.loopById?.[wsId]
        if (w && !w.loopChatCode) w.loopChatCode = 'MA'
      } else {
        by[wsId] = [...(by[wsId] ?? []).filter((t: any) => t.code !== tab.code), tab]
      }
    }
    return listWorkspaceChats(s, wsId)
  },
  ReorderWorkspaceChats: (a, s) => {
    const wsId = a[0] as string
    const codes = (a[1] ?? []) as string[]
    const current = listWorkspaceChats(s, wsId)
    if (codes.length !== current.length) {
      return new BridgeError(`reorder chats: got ${codes.length} codes, have ${current.length} tabs`)
    }
    const have = new Set(current.map((t) => t.code))
    const seen = new Set<string>()
    for (const code of codes) {
      if (!have.has(code)) return new BridgeError(`reorder chats: unknown tab ${code}`)
      if (seen.has(code)) return new BridgeError(`reorder chats: duplicate tab ${code}`)
      seen.add(code)
    }
    ;((s as any).chatOrderByLoop ??= {})[wsId] = [...codes]
    return null
  },
  WorkspaceFacts: () => [],
  SendWorkspaceMessage: () => 'Working on it (mock reply).',
  CancelWorkspaceMessage: () => null,
  StartWorkspace: () => 'LOOP-NEW',
  // Unified creation: args are
  // (projectID, name, baseBranch, mode, prompt, autonomy, simplify?); returns the record.
  CreateWorkspace: (a) => (a[3] === 'loop'
    ? { id: 'LOOP-NEW', projectId: a[0], runStarted: true, state: 'queued', statusLabel: 'Creating', phase: 'creating', branch: 'recur/new', baseBranch: a[2] || 'main', loopId: a[6] || 'recur.loops.pr', policyId: a[7] || 'recur.policies.balanced' }
    : { id: 'WS-NEW', projectId: a[0], type: 'solo', statusLabel: 'Creating', phase: 'creating', branch: 'recur/new', baseBranch: a[2] || 'main' }),
  CreateWorkspaceWith: (a) => (a[3] === 'loop'
    ? { id: 'LOOP-NEW', projectId: a[0], runStarted: true, state: 'queued', statusLabel: 'Creating', phase: 'creating', branch: 'recur/new', baseBranch: a[2] || 'main', loopId: a[6] || 'recur.loops.pr', policyId: a[7] || 'recur.policies.balanced' }
    : { id: 'WS-NEW', projectId: a[0], type: 'solo', statusLabel: 'Creating', phase: 'creating', branch: 'recur/new', baseBranch: a[2] || 'main', loopId: a[6], policyId: a[7] }),
  ListLoops: () => [
    { id: 'recur.loops.pr', name: 'To a PR', hint: 'Plan, implement, review, open a PR', entry: 'planning' },
    { id: 'recur.loops.converge', name: 'Review & fix', hint: 'Review and fix the current diff until the bar passes', entry: 'converge' },
    { id: 'recur.loops.converge-pr', name: 'Review & fix to a PR', hint: 'Review and fix the current diff, then open a PR', entry: 'converge' },
    { id: 'recur.loops.docs', name: 'Docs', hint: 'Plan, write, and edit documentation — no PR', entry: 'planning' },
    { id: 'recur.loops.security', name: 'Security review & fix', hint: 'Review and fix auth, secrets, and permissions until the bar passes', entry: 'converge' },
  ],
  ListPolicies: () => [
    { id: 'recur.policies.cautious', name: 'Cautious', hint: 'Close every finding; escalate after 2 rounds' },
    { id: 'recur.policies.balanced', name: 'Balanced', hint: 'Fix medium and up; escalate after 4 rounds' },
    { id: 'recur.policies.aggressive', name: 'Aggressive', hint: 'Ship-blockers only; proceed after 6 rounds' },
  ],
  SetWorkspaceLoopPolicy: (a, s) => {
    const id = a[0] as string
    const w = s.loopById[id]
    if (!w) return new BridgeError(`workspace ${id} not found`)
    const updated = { ...w, loopId: a[1] || w.loopId, policyId: a[2] || w.policyId }
    s.loopById[id] = updated
    s.loops = s.loops.map((l) => (l.id === id ? updated : l))
    return updated
  },
  EngageLoopWith: () => 'Loop engaged — planning has started.',
  SetWorkspaceBaseBranch: (a, s) => {
    const id = a[0] as string
    const base = String(a[1] ?? '').trim()
    const w = s.loopById[id]
    if (!w) return new BridgeError(`workspace ${id} not found`)
    if (w.isMain) return new BridgeError('the main working copy has no merge base')
    if (!base) return new BridgeError('base branch is required')
    if (w.branch && base === w.branch) return new BridgeError(`target branch cannot be the workspace's own branch`)
    const updated = withMockWorktree({ ...w, baseBranch: base })
    s.loopById[id] = updated
    s.loops = s.loops.map((l) => (l.id === id ? updated : l))
    s.prs = s.prs.map((p) =>
      p.workspaceId === id && p.state !== 'Merged' && p.group !== 'merged' ? { ...p, base } : p,
    )
    return updated
  },
  RemoveWorkspace: (a, s) => {
    const id = a[0]
    const updated = { ...(s.loopById[id] ?? {}), archived: true, worktreePath: '' }
    s.loopById[id] = updated
    s.loops = s.loops.map((l) => (l.id === id ? updated : l))
    return updated
  },
  DeleteWorkspace: () => null,
  WorkspaceDiscardRisk: () => '',
  OpenWorkspacePR: (a, s) => s.loopById[a[0]] ?? null,
  PullWorkspacePR: (a, s) => ({ status: 'updated', workspace: withMockWorktree(s.loopById[a[0]] ?? {}), base: 'origin/main' }),
  MergeWorkspace: (a, s) => withMockWorktree({ ...(s.loopById[a[0]] ?? {}), state: 'merged', statusLabel: 'merged' }),
  UpdateWorkspaceWithBase: (a, s) => ({ status: 'updated', workspace: withMockWorktree(s.loopById[a[0]] ?? {}), base: 'main' }),
  UpdateWorkingCopyFromRemote: (a, s) => ({ status: 'updated', workspace: s.loopById[a[0]] ?? {}, base: 'origin/main' }),
  FetchWorkingCopyUpstream: (a, s) => {
    const w = s.loopById?.[a[0]]
    if (w?.isMain) {
      return { behindBase: 0, aheadBase: 0, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    }
    return { behindBase: 0, aheadBase: 0, dirty: false, unmerged: false, base: '' }
  },
  PushWorkingCopy: (a, s) => s.loopById[a[0]] ?? {},
  WorkspaceLandState: (a, s) => {
    const w = s.loopById?.[a[0]]
    if (w?.isMain) {
      return { behindBase: 0, aheadBase: 0, dirty: false, unmerged: false, base: 'origin/main', remote: 'origin' }
    }
    return { behindBase: 0, aheadBase: 0, dirty: false, unmerged: false, base: 'main' }
  },
  OpenPath: (a, s) => {
    const path = String(a[0] ?? '')
    const name = path.split('/').filter(Boolean).pop() || 'opened'
    let p = s.projects.find((x) => x.repoPath === path)
    if (!p) {
      p = { id: `open-${name}`, name, repoPath: path, isGit: true, open: true }
      s.projects = [...s.projects, p]
    }
    const existing = s.loops.find((l) => l.isMain && l.projectId === p.id)
    const ws = existing ?? withMockWorktree({
      id: `MAIN-${p.id}`,
      projectId: p.id,
      type: 'main',
      isMain: true,
      short: 'Working copy',
      goal: 'Working copy',
      statusLabel: 'Idle',
      diff: { add: 0, del: 0 },
    })
    if (!existing) {
      s.loops = [...s.loops, ws]
      s.loopById[ws.id] = ws
    }
    return {
      path,
      projectId: p.id,
      workspaceId: ws.id,
      created: !existing,
      project: p,
      workspace: ws,
    }
  },
  OpenMainWorkspace: (a, s) => {
    const projectId = a[0] as string
    const existing = s.loops.find((l) => l.isMain && l.projectId === projectId)
    if (existing) return withMockWorktree(existing)
    const p = s.projects.find((x) => x.id === projectId)
    const ws = withMockWorktree({
      id: `MAIN-${projectId}`,
      projectId,
      type: 'main',
      isMain: true,
      short: 'Working copy',
      goal: 'Working copy',
      branch: String(p?.baseBranch || 'main').replace(/^origin\//, '') || 'main',
      baseBranch: '',
      statusLabel: 'Idle',
      diff: { add: 0, del: 0 },
    })
    s.loops = [...s.loops, ws]
    s.loopById[ws.id] = ws
    return ws
  },
  UpdatePlan: () => null,
  RetryLoop: () => null,
  RestartLoop: () => 'Restarting',
  // Loops (pre-rename aliases)
  ListLoops: (_a, s) => s.loops,
  GetLoop: (a, s) => s.loopById[a[0]] ?? null,
  GetLoopFeed: (a, s) => s.feedByLoop[a[0]] ?? [],
  StartLoop: () => 'LOOP-NEW',
  PauseLoop: () => null,
  ResumeLoop: () => null,
  StopLoop: () => null,
  ArchiveLoop: (a, s) => ({ ...(s.loopById[a[0]] ?? {}), archived: true, worktreePath: '' }),
  DeleteLoop: () => null,
  // Maestro
  SendMaestroMessage: () => 'On it — keeping within your rails (mock reply).',
  CancelMaestroMessage: () => null,
  GetMaestroChat: () => [],
  // Escalation
  GetEscalation: (a, s) => s.escByLoop[a[0]] ?? {},
  AnswerEscalation: (a, s) => {
    const existing = s.escByLoop[a[0]]
    if (!existing) return {}
    const updated = { ...existing, status: 'answered', answer: a[2] || a[1] || '' }
    s.escByLoop[a[0]] = updated
    return updated
  },
  // Agents
  ListLoopAgents: (a, s) => s.agentsByLoop[a[0]] ?? [],
  GetAgentTrace: (a, s) => (s.agentsByLoop[a[0]] ?? []).find((g: any) => g.code === a[1]) ?? {},
  // PRs
  ListPRs: (_a, s) => s.prs,
  SyncGitHubPRs: (_a, s) => s.prs,
  CreateWorkspaceFromPR: (a, s) => {
    const id = String(a[0] ?? '')
    const pr = s.prById[id]
    if (!pr) return {}
    if (pr.workspaceId) {
      return s.loopById[pr.workspaceId] ?? s.loops.find((l: { id: string }) => l.id === pr.workspaceId) ?? {}
    }
    const host = s.loops.find((l: { hostPrId?: string }) => l.hostPrId === id)
    const ws = host
      ? { ...host, hostPrId: '', type: 'solo', statusLabel: 'Idle', phase: 'editing', worktreePath: '/tmp/from-pr' }
      : {
          id: 'WS-FROM-PR',
          projectId: pr.projectId || 'storefront',
          type: 'solo',
          goal: pr.title || 'PR workspace',
          short: pr.title || 'PR workspace',
          statusLabel: 'Idle',
          phase: 'editing',
          branch: pr.branch || 'recur/from-pr',
          baseBranch: pr.base || 'main',
          worktreePath: '/tmp/from-pr',
        }
    pr.workspaceId = ws.id
    s.prById[id] = pr
    s.prs = s.prs.map((p: { id: string }) => (p.id === id ? { ...p, workspaceId: ws.id } : p))
    s.loopById[ws.id] = ws
    s.loops = host
      ? s.loops.map((l: { id: string }) => (l.id === ws.id ? ws : l))
      : [ws, ...s.loops]
    return ws
  },
  EnsurePRChatHost: (a, s) => {
    const id = String(a[0] ?? '')
    const pr = s.prById[id]
    if (!pr) return {}
    if (pr.workspaceId) {
      return s.loopById[pr.workspaceId] ?? s.loops.find((l: { id: string }) => l.id === pr.workspaceId) ?? {}
    }
    const existing = s.loops.find((l: { hostPrId?: string }) => l.hostPrId === id)
    if (existing) return existing
    const ws = {
      id: `WS-HOST-${id}`,
      projectId: pr.projectId || 'storefront',
      type: 'solo',
      hostPrId: id,
      goal: pr.title || 'PR chat',
      short: pr.title || 'PR chat',
      statusLabel: 'Review',
      phase: 'editing',
      worktreePath: '/tmp/repo',
      baseBranch: pr.base || 'main',
    }
    s.loopById[ws.id] = ws
    s.loops = [ws, ...s.loops]
    return ws
  },
  GetPR: (a, s) => s.prById[a[0]] ?? null,
  GetPRFiles: (a, s) => s.prById[a[0]]?.files ?? [],
  GetPRCommits: (a, s) => s.prById[a[0]]?.commits ?? [],
  GetCommitDiff: (a, s) => {
    const ref = String(a[1] ?? '')
    const commits = s.prById[a[0]]?.commits ?? []
    return commits.find((c: any) => c.hash === ref || c.sha === ref) ?? {}
  },
  AddPRComment: (a, s) => {
    const [id, file, line, body, startLine] = a
    const pr = s.prById[id]
    if (pr && file && line) {
      const side = line < 0 ? 'LEFT' : 'RIGHT'
      const n = Math.abs(Number(line) || 0)
      const startN = Math.abs(Number(startLine) || 0)
      const startSide = Number(startLine) < 0 ? 'LEFT' : 'RIGHT'
      const text = String(body ?? '').trim()
      if (!text) {
        const findingID = String(a[5] ?? '').trim()
        pr.pendingComments = (pr.pendingComments ?? []).filter((c: any) => {
          if (!findingID) return true
          return c.id !== findingID && (c.findingId || '').trim() !== findingID
        })
        return nowComment('')
      }
      const pc: any = { id: 'pend-' + Date.now(), file, line: n, side, body: text }
      const findingID = String(a[5] ?? '').trim()
      if (findingID) pc.findingId = findingID
      if (startN > 0 && startN < n) {
        pc.startLine = startN
        pc.startSide = startSide
      }
      pr.pendingComments = [...(pr.pendingComments ?? []), pc]
      return nowComment(text)
    }
    const loc = file ? `<code>${file}${line > 0 ? ':' + line : ''}</code> — ` : ''
    const c = nowComment(loc + String(body ?? ''))
    c.bodyMd = String(body ?? '')
    c.author = 'You'
    c.authorLogin = pr?.viewerLogin || 'avery'
    if (pr) pr.comments = [...(pr.comments ?? []), c]
    return c
  },
  UpdatePRDetails: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    pr.title = String(a[1] ?? pr.title)
    const body = String(a[2] ?? '')
    const desc = (pr.comments ?? []).find((c: any) => c.kind === 'description')
    if (desc) desc.bodyMd = body
    else pr.comments = [{ id: 'gh-description', kind: 'description', author: pr.authorLogin || 'You', bodyMd: body, bodyHtml: '' }, ...(pr.comments ?? [])]
    return pr
  },
  UpdatePRComment: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    const id = String(a[1] ?? '')
    const body = String(a[2] ?? '')
    const c = (pr.comments ?? []).find((x: any) => x.id === id)
    if (!c) return null
    c.bodyMd = body
    if (!c.path) c.bodyHtml = body
    return c
  },
  DeletePRComment: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    const id = String(a[1] ?? '')
    pr.comments = (pr.comments ?? []).filter((c: any) => c.id !== id)
    return pr
  },
  ReplyPRComment: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    const id = String(a[1] ?? '')
    const body = String(a[2] ?? '')
    const root = (pr.comments ?? []).find((c: any) => c.id === id)
    if (!root) return null
    const reply = {
      ...nowComment(body),
      bodyMd: body,
      inReplyToId: root.id,
      parentId: root.parentId,
      path: root.path,
      line: root.line,
      threadId: root.threadId || root.id,
      resolved: !!root.resolved,
    }
    pr.comments = [...(pr.comments ?? []), reply]
    return reply
  },
  TogglePRCommentReaction: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    const id = String(a[1] ?? '')
    const content = String(a[2] ?? '').trim()
    const order = ['+1', '-1', 'laugh', 'hooray', 'confused', 'heart', 'rocket', 'eyes']
    const n = content.toLowerCase() === 'thumbs_up' ? '+1'
      : content.toLowerCase() === 'thumbs_down' ? '-1'
        : order.includes(content) ? content : content.toLowerCase()
    if (!id || !order.includes(n)) return pr
    pr.comments = (pr.comments ?? []).map((c: any) => {
      if (c.id !== id) return c
      const rs = [...(c.reactions ?? [])]
      const i = rs.findIndex((r: any) => r.content === n)
      if (i >= 0) {
        const cur = rs[i]
        rs[i] = cur.mine
          ? { ...cur, count: cur.count - 1, mine: false }
          : { ...cur, count: cur.count + 1, mine: true }
      } else {
        rs.push({ content: n, count: 1, mine: true })
      }
      return { ...c, reactions: rs.filter((r: any) => (r.count ?? 0) > 0) }
    })
    return pr
  },
  SetPRThreadResolved: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    const key = String(a[1] ?? '')
    const resolved = !!a[2]
    pr.comments = (pr.comments ?? []).map((c: any) => {
      const match = c.threadId === key || c.id === key || c.inReplyToId === key
      return match ? { ...c, resolved, threadId: c.threadId || key } : c
    })
    return pr
  },
  SubmitReview: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return null
    const pending = pr.pendingComments ?? []
    pr.pendingComments = []
    const review = { ...nowComment(String(a[2] ?? '')), bodyMd: String(a[2] ?? '') }
    const kids = pending.map((pc: any) => ({
      ...nowComment(`<code>${pc.file}:${pc.line}</code>`),
      bodyMd: String(pc.body ?? ''),
      parentId: review.id,
      path: pc.file,
      line: pc.line,
      side: pc.side || 'RIGHT',
    }))
    pr.comments = [...(pr.comments ?? []), review, ...kids]
    return pr
  },
  UpdateWithBase: (a, s) => ({ ...(s.prById[a[0]] ?? {}), state: 'In review', group: 'you', behindBase: 0, updatingBase: '' }),
  MergePR: (a, s) => ({ ...(s.prById[a[0]] ?? {}), state: 'Merged', group: 'merged' }),
  SetPRDraft: (a, s) => {
    const pr = s.prById[a[0]]
    if (!pr) return {}
    const draft = !!a[1]
    const next = {
      ...pr,
      draft,
      mergeStateStatus: draft ? 'DRAFT' : (pr.mergeStateStatus === 'DRAFT' ? 'CLEAN' : (pr.mergeStateStatus ?? 'CLEAN')),
    }
    s.prById[pr.id] = next
    const i = s.prs.findIndex((p) => p.id === pr.id)
    if (i >= 0) s.prs[i] = next
    return next
  },
  ReadyPR: (a, s, table) => table.SetPRDraft([a[0], false], s, table),
  SearchPRMentionableUsers: () => [],
  // Policy
  GetPolicy: (_a, s) => s.policy,
  SetPolicy: (a) => a[0],
  // Inbox
  GetInbox: (_a, s) => s.inbox,
  MarkInboxRead: (a, s) => {
    const id = String(a[0] ?? '')
    const chat = parseChatInboxID(id)
    if (chat) {
      s.inbox = s.inbox.filter((it) => it.id !== id)
    } else {
      s.inbox = s.inbox.map((it) => (it.id === id ? { ...it, unread: false } : it))
    }
    return s.inbox.filter((it) => it.unread).length
  },
  MarkAllInboxRead: (_a, s) => {
    s.inbox = s.inbox.filter((it) => it.kind !== 'chat').map((it) => ({ ...it, unread: false }))
    return 0
  },
  // Settings
  DetectAgent: () => 'Claude Code · dev@example.com (Acme)',
  GetClaudeStatus: () => ({
    authenticated: true,
    path: '/usr/local/bin/claude',
    version: '2.1.224',
    provider: 'Anthropic API',
    loginMethod: 'Claude Team account',
    organization: 'Acme',
    email: 'dev@example.com',
    error: '',
  }),
  GetCodexStatus: () => ({
    binInstalled: true,
    binPath: '/opt/homebrew/bin/codex',
    authenticated: false,
    loginMethod: '',
    email: '',
    plan: '',
    error: 'Not logged in. Run `codex login`.',
  }),
  GetChatGPTStatus: () => ({ authenticated: false, accountId: '', email: '', accounts: [], modelCount: 0, error: '' }),
  GetCursorStatus: () => ({
    bridgeInstalled: true,
    bridgePath: '/tmp/cursor-sdk-bridge',
    bridgeVersion: '1.0.31',
    pinnedVersion: '1.0.31',
    updateAvailable: false,
    authenticated: true,
    account: 'dev@example.com',
    apiKeyName: 'Recur laptop',
    email: 'dev@example.com',
    name: 'Dev User',
    modelCount: 12,
    error: '',
  }),
  InstallCursorBridge: () => ({
    bridgeInstalled: true,
    bridgePath: '/tmp/cursor-sdk-bridge',
    bridgeVersion: '1.0.30',
    pinnedVersion: '1.0.30',
    updateAvailable: false,
    authenticated: true,
    account: 'dev@example.com',
    apiKeyName: 'Recur laptop',
    email: 'dev@example.com',
    name: 'Dev User',
    modelCount: 12,
    error: '',
  }),
  GetSettings: (_a, s) => s.settings,
  GetInstanceLabel: () => '',
  GitHubLogin: () => 'mycodecrafting',
  SetSettings: (a, s) => {
    s.settings = a[0]
    return s.settings
  },
  ListModelCatalog: () => {
    const effort = {
      id: 'effort',
      label: 'Effort',
      options: [
        { value: 'low', label: 'Low' },
        { value: 'medium', label: 'Medium' },
        { value: 'high', label: 'High', default: true },
        { value: 'xhigh', label: 'Extra High' },
        { value: 'max', label: 'Max' },
      ],
    }
    const context = {
      id: 'context',
      label: 'Context',
      options: [
        { value: '200k', label: '200K', default: true },
        { value: '1m', label: '1M' },
      ],
    }
    const both = [effort, context]
    return [{
      provider: 'claude-code',
      label: 'Claude Code',
      models: [
        { ref: 'claude-code/claude-fable-5-1', label: 'Fable 5.1', flags: [effort], contextWindow: 1_000_000 },
        { ref: 'claude-code/claude-fable-5', label: 'Fable 5', flags: [effort], contextWindow: 1_000_000 },
        { ref: 'claude-code/claude-opus-5-5', label: 'Opus 5.5', flags: [effort], contextWindow: 1_000_000 },
        { ref: 'claude-code/claude-opus-5', label: 'Opus 5', flags: [effort], contextWindow: 1_000_000 },
        { ref: 'claude-code/claude-opus-4-6', label: 'Opus 4.6', flags: both },
        { ref: 'claude-code/claude-sonnet-5-5', label: 'Sonnet 5.5', flags: [effort], contextWindow: 1_000_000 },
        { ref: 'claude-code/claude-sonnet-5', label: 'Sonnet 5', flags: [effort], contextWindow: 1_000_000 },
        { ref: 'claude-code/claude-sonnet-4-6', label: 'Sonnet 4.6', flags: both },
        { ref: 'claude-code/claude-haiku-4-5-20251001', label: 'Haiku 4.5', flags: [effort], contextWindow: 200_000 },
      ],
    }]
  },
  GetResource: (_a, s) => s.resource,
  GetProviderUsage: () => ({ fetchedAt: 0, stale: false, providers: [] }),
  RefreshProviderUsage: () => ({ fetchedAt: 0, stale: false, providers: [] }),
  ConnectCursorUsage: () => ({ loginUrl: '', pending: false, error: 'not in e2e' }),
  CancelCursorUsageConnect: () => null,
  DisconnectCursorUsage: () => null,
  LogsPath: () => '/tmp/recur-test/logs/recur.log',
  RevealLogs: () => null,
  ClearAllData: () => null,
  ListMonospaceFonts: () => [
    'JetBrains Mono',
    'IBM Plex Mono',
    'Source Code Pro',
    'Inconsolata',
    'Menlo',
    'Monaco',
    'Fira Code',
  ],
  // Code observatory / ADE
  // One level at a time (matches CodeService depth=1 + UI lazy expand).
  ListTree: (a) => {
    const path = (a[2] as string) || ''
    if (!path) return [{ name: 'src', path: 'src', dir: true }]
    if (path === 'src') return [{ name: 'main.go', path: 'src/main.go', dir: false }]
    return []
  },
  // Batched sibling of ListTree (one git status for the whole batch). Delegates
  // to whatever ListTree resolver is in effect so a test that overrides the tree
  // shape only has to override one of them.
  ListTrees: (a, s, table) => {
    const dirs = (a[2] as string[]) ?? []
    const out: Record<string, unknown> = {}
    for (const dir of dirs) out[dir] = table.ListTree([a[0], a[1], dir], s, table)
    return out
  },
  ReadFile: (a) => ({
    path: a[2] || 'src/main.go',
    language: 'go',
    content: 'package main\n\nfunc main() {}\n',
    size: 32,
    writable: !!a[1],
  }),
  WriteFile: () => null,
  // Editor session store (Go-side in production; per-seed here so a session
  // survives a gotoApp "restart" within one test). Mirrors the backend merge:
  // meta replaces wholesale, buffers/scratch upsert by path, drops are
  // explicit, and an empty result deletes the session.
  GetEditorSession: (a, s) => {
    const sess = ((s as any).editorSessions ?? {})[a[0] as string]
    return sess ? structuredClone(sess) : null
  },
  UpdateEditorSession: (a, s) => {
    const by = ((s as any).editorSessions ??= {} as Record<string, any>)
    const p = (a[1] ?? {}) as any
    const cur = by[a[0] as string] ?? { openFiles: [], selectedPath: '' }
    if (p.meta) {
      cur.openFiles = [...(p.meta.openFiles ?? [])]
      cur.selectedPath = p.meta.selectedPath ?? ''
      cur.splitPath = p.meta.splitPath
      cur.scratchSeq = p.meta.scratchSeq
      cur.fileViews = p.meta.fileViews ? structuredClone(p.meta.fileViews) : undefined
      if ('syntaxByPath' in p.meta) {
        const m = p.meta.syntaxByPath
        cur.syntaxByPath = m && Object.keys(m).length ? structuredClone(m) : undefined
      }
        if ('syntaxByFileType' in p.meta) {
          const m = p.meta.syntaxByFileType
          cur.syntaxByFileType = m && Object.keys(m).length ? structuredClone(m) : undefined
        }
        cur.mainSurface = p.meta.mainSurface === 'review' ? 'review' : undefined
        cur.leftKind = p.meta.leftKind || undefined
        cur.rightKind = p.meta.rightKind || undefined
        cur.leftOpen = typeof p.meta.leftOpen === 'boolean' ? p.meta.leftOpen : undefined
        cur.rightOpen = typeof p.meta.rightOpen === 'boolean' ? p.meta.rightOpen : undefined
      }
    for (const [path, buf] of Object.entries(p.buffers ?? {})) (cur.buffers ??= {})[path] = buf
    for (const path of p.dropBuffers ?? []) delete cur.buffers?.[path]
    for (const [path, doc] of Object.entries(p.scratchDocs ?? {})) (cur.scratchDocs ??= {})[path] = doc
    for (const path of p.dropScratchDocs ?? []) delete cur.scratchDocs?.[path]
    const empty = !cur.openFiles?.length
      && !Object.keys(cur.buffers ?? {}).length
      && !Object.keys(cur.scratchDocs ?? {}).length
      && !Object.keys(cur.syntaxByPath ?? {}).length
      && !Object.keys(cur.syntaxByFileType ?? {}).length
      && cur.mainSurface !== 'review'
      && (!cur.leftKind || cur.leftKind === 'maestro')
      && (!cur.rightKind || cur.rightKind === 'files')
      && cur.leftOpen !== false
      && cur.rightOpen !== false
    if (empty) delete by[a[0] as string]
    else by[a[0] as string] = cur
    return null
  },
  ClearEditorSession: (a, s) => {
    delete ((s as any).editorSessions ?? {})[a[0] as string]
    return null
  },
  // Last-open workspace/view. Survives page.reload() within a test because it
  // lives on the seed object the route handler closes over.
  GetShellState: (_a, s) => (s as any).shellState ?? { view: '' },
  SetShellState: (a, s) => {
    (s as any).shellState = a[0] ?? { view: '' }
    return null
  },
  GetExpandedSchedules: (_a, s) => (s as any).expandedSchedules ?? [],
  SetExpandedSchedules: (a, s) => {
    (s as any).expandedSchedules = a[0] ?? []
    return null
  },
  // Recur Bundles
  ListGrammars: () => loadTmCatalog().grammars.map(publicGrammar),
  ReadGrammar: (a) => {
    const scope = String(a[0] ?? '')
    const g = loadTmCatalog().grammars.find((x) => x.scopeName === scope)
    return g ? readBody(g.absPath, `grammar ${scope}`) : new BridgeError(`grammar ${scope} not found`)
  },
  ListThemes: () => loadTmCatalog().themes.map(publicTheme),
  ReadTheme: (a) => {
    const id = String(a[0] ?? '')
    const t = loadTmCatalog().themes.find((x) => x.id === id)
    return t ? readBody(t.absPath, `theme ${id}`) : new BridgeError(`theme ${id} not found`)
  },
  ListTerminalThemes: () => loadTerminalThemes(),
  ListSkippedBundles: () => [],
  ExpandGoal: (a) => ({ prompt: String(a[0] ?? ''), display: '' }),
  ListPromptActions: () => [
    {
      id: 'recur.chat.fix-findings',
      name: 'Fix Findings',
      description: 'Fix or dispute open reviewer findings',
      slash: 'fix-findings',
      mode: 'agent',
      timeoutMs: 0,
      interpreter: 'typescript',
      bundleName: 'Chat',
      sourceRoot: 'default',
      freshSession: true,
      tabGroup: 'ops',
    },
    {
      id: 'recur.chat.commit',
      name: 'Commit',
      description: 'Stage and commit all worktree changes',
      slash: 'commit',
      mode: 'agent',
      timeoutMs: 0,
      interpreter: 'typescript',
      bundleName: 'Chat',
      sourceRoot: 'default',
      freshSession: true,
      tabGroup: 'ops',
    },
    {
      id: 'recur.chat.commit-push',
      name: 'Commit Push',
      description: 'Commit all changes and push to remote',
      slash: 'commit-push',
      mode: 'agent',
      timeoutMs: 0,
      interpreter: 'typescript',
      bundleName: 'Chat',
      sourceRoot: 'default',
      freshSession: true,
      tabGroup: 'ops',
    },
    {
      id: 'recur.chat.open-pr',
      name: 'Open PR',
      description: 'Open a GitHub pull request with an agent-written description',
      slash: 'open-pr',
      mode: 'agent',
      timeoutMs: 15000,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
      freshSession: true,
      tabGroup: 'ops',
    },
    {
      id: 'recur.chat.open-draft-pr',
      name: 'Open draft PR',
      description: 'Open a GitHub draft pull request with an agent-written description',
      slash: 'open-draft-pr',
      mode: 'agent',
      timeoutMs: 15000,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
      freshSession: true,
      tabGroup: 'ops',
    },
    {
      id: 'recur.chat.fix-checks',
      name: 'Fix checks',
      description: 'Investigate and fix failing GitHub checks on this pull request',
      slash: 'fix-checks',
      mode: 'agent',
      timeoutMs: 15000,
      interpreter: 'shebang',
      takesArgs: true,
      hasComplete: true,
      freshSession: true,
      tabGroup: 'ops',
      argPlaceholder: 'all or a failing check',
      args: [{ name: 'check', placeholder: 'all or a failing check', optional: true }],
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
    {
      id: 'recur.chat.review',
      name: 'Review',
      description: 'Review the current surface (workspace or pull request) and file findings',
      slash: 'review',
      mode: 'review',
      timeoutMs: 15000,
      freshSession: true,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
      tabGroup: 'review',
    },
    {
      id: 'recur.chat.re-review',
      name: 'Re-review',
      description: 'Re-review the current surface after fixes',
      slash: 're-review',
      mode: 'review',
      timeoutMs: 15000,
      freshSession: true,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
    {
      id: 'recur.chat.simplify-review',
      name: 'Simplify review',
      description: 'Review the current surface for over-engineering and file findings',
      slash: 'simplify-review',
      mode: 'review',
      timeoutMs: 30000,
      freshSession: true,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
      tabGroup: 'simplify-review',
    },
    {
      id: 'recur.chat.simplify-audit',
      name: 'Simplify audit',
      description: 'Audit the whole repository for over-engineering and file findings',
      slash: 'simplify-audit',
      mode: 'review',
      timeoutMs: 30000,
      freshSession: true,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
      tabGroup: 'simplify-audit',
    },
    {
      id: 'recur.chat.review-and-fix',
      name: 'Review & fix',
      description: 'Review the current worktree and fix until the bar passes',
      slash: 'review-and-fix',
      mode: 'loop',
      loop: 'recur.loops.converge',
      timeoutMs: 0,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
    {
      id: 'recur.chat.review-to-pr',
      name: 'Review & fix to a PR',
      description: 'Review and fix the current diff, then open a PR',
      slash: 'review-to-pr',
      mode: 'loop',
      loop: 'recur.loops.converge-pr',
      timeoutMs: 0,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
    {
      id: 'recur.chat.to-a-pr',
      name: 'To a PR',
      description: 'Plan, implement, review, and open a pull request',
      slash: 'to-a-pr',
      mode: 'loop',
      loop: 'recur.loops.pr',
      timeoutMs: 0,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
    {
      id: 'recur.chat.write-docs',
      name: 'Write docs',
      description: 'Plan, write, and edit documentation — no PR',
      slash: 'write-docs',
      mode: 'loop',
      loop: 'recur.loops.docs',
      timeoutMs: 0,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
    {
      id: 'recur.chat.security-review',
      name: 'Security review & fix',
      description: 'Review and fix auth, secrets, and permissions until the bar passes',
      slash: 'security-review',
      mode: 'loop',
      loop: 'recur.loops.security',
      timeoutMs: 0,
      interpreter: 'shebang',
      bundleName: 'Chat',
      sourceRoot: 'default',
    },
  ],
  RunPromptAction: (a) => {
    const id = a[0] as string
    const surface = String((a[1] as { surface?: string } | undefined)?.surface ?? '')
    if (id === 'recur.chat.review') {
      if (surface === 'github_pr') {
        return {
          prompt:
            'Review this GitHub pull request and file findings. Use gh pr diff — do not treat git diff in cwd as the PR.',
          display: 'Please review this GitHub pull request',
        }
      }
      if (surface === 'local_pr') {
        return {
          prompt: 'Review this Local PR and file findings. Do not make code changes.',
          display: 'Please review this Local PR',
        }
      }
      return {
        prompt:
          'Review the changes in this worktree and file findings. Git context is already gathered — do NOT re-run git status/diff/log.',
        display: 'Please review the changes in this worktree and file any real issues',
      }
    }
    if (id === 'recur.chat.re-review') {
      if (surface === 'github_pr') {
        return {
          prompt: 'Please re-review this GitHub pull request after the recent fixes. Do not make code changes.',
          display: 'Please re-review this GitHub pull request',
        }
      }
      if (surface === 'local_pr') {
        return {
          prompt: 'Please re-review this Local PR after the recent fixes. Do not make code changes.',
          display: 'Please re-review this Local PR',
        }
      }
      return {
        prompt:
          'Please re-review this worktree after the recent fixes. Do not make code changes.',
        display: 'Please re-review this worktree after the recent fixes',
      }
    }
    if (id === 'recur.chat.open-pr') {
      return {
        prompt:
          'The user requested a pull request. Call mcp__recur__open_pr with a title and markdown body of ALL workspace changes versus the base. Do not run gh pr create yourself.',
        display: 'Open a pull request',
      }
    }
    if (id === 'recur.chat.open-draft-pr') {
      return {
        prompt:
          'The user requested a draft pull request. Call mcp__recur__open_pr with draft set to true, a title, and a markdown body of ALL workspace changes versus the base. Do not run gh pr create yourself.',
        display: 'Open a draft pull request',
      }
    }
    if (id === 'recur.chat.fix-checks') {
      const raw = String((a[1] as { args?: string } | undefined)?.args ?? '').trim()
      const args = !raw || raw.toLowerCase() === 'all' ? '' : raw
      return {
        prompt: args
          ? `Investigate and fix the failing GitHub check ${args}. Use gh run view --log-failed.`
          : 'Investigate and fix the failing GitHub checks on this PR. Use gh run view --log-failed.',
        display: args ? `Fix failing check: ${args}` : 'Fix failing GitHub checks',
      }
    }
    if (id === 'recur.chat.review-and-fix') {
      return {
        prompt:
          'The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes. Do not reopen scope, do not plan a new feature, and do not open a PR.',
        display: 'Review & fix',
      }
    }
    if (id === 'recur.chat.review-to-pr') {
      return {
        prompt:
          'The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR. Do not reopen scope and do not plan a new feature.',
        display: 'Review & fix to a PR',
      }
    }
    if (id === 'recur.chat.to-a-pr') {
      return {
        prompt:
          'Ship a correct, reviewable change to a pull request. Plan first when there is no approved plan, then implement and review until the policy bar passes, then open the PR. Do not reopen scope the human already closed.',
        display: 'To a PR',
      }
    }
    if (id === 'recur.chat.write-docs') {
      return {
        prompt:
          'Write the documentation the human asked for. Do not change product code unless the docs require it. Do not open a PR — the write-up lives in this worktree.',
        display: 'Write docs',
      }
    }
    if (id === 'recur.chat.security-review') {
      return {
        prompt:
          'Review the current worktree for auth, secrets, tokens, and permission checks. Fix real findings. Do not reopen product scope, and do not open a PR.',
        display: 'Security review & fix',
      }
    }
    return { prompt: 'hi', display: 'Hi' }
  },
  RunPromptComplete: (a) => {
    const id = a[0] as string
    const req = (a[1] as { args?: string } | undefined) ?? {}
    const term = String(req.args ?? '').trim().toLowerCase()
    if (id === 'recur.chat.fix-checks') {
      const hits: { insert: string; label: string; detail: string; badge: string }[] = []
      if (!term || 'all'.includes(term)) {
        hits.push({ insert: 'all', label: 'All failing checks', detail: 'Every failing GitHub check', badge: 'All' })
      }
      if (!term || 'lint'.includes(term) || term === 'lint') {
        hits.push({ insert: 'lint', label: 'lint', detail: 'fail', badge: 'Check' })
      }
      return { hits, message: '' }
    }
    return { hits: [], message: '' }
  },
  CompiledPromptModule: (a) => {
    const id = a[0] as string
    if (id === 'recur.chat.fix-findings') {
      return `
exports.default = function () {
  return {
    prompt: "Open reviewer findings remain for this workspace. Call mcp__recur__list_findings to load them. Operator notes on a finding are instructions — follow them. For each open finding: verify it against the current code, then either fix it with minimal correct edits and mark it resolved via mcp__recur__update_finding, or mark it invalid with a clear reason if the claim is wrong or out of scope.",
    display: "Fix open reviewer findings"
  };
};
`
    }
    if (id === 'recur.chat.commit') {
      return `
exports.default = function () {
  return {
    prompt: "Commit all changes. Write a clear message from the diff. Do not push or amend unless I ask. If there’s nothing to commit, say so.",
    display: "Commit all changes"
  };
};
`
    }
    if (id === 'recur.chat.commit-push') {
      return `
exports.default = function () {
  return {
    prompt: "Commit all changes, then push to the remote tracking branch (create/set upstream if needed). Write a clear message from the diff. Do not amend unless I ask. If there’s nothing to commit, still push any unpushed commits; if everything is already pushed, say so.",
    display: "Commit all changes and push"
  };
};
`
    }
    return `exports.default = function () { return { prompt: "hi", display: "Hi" }; };`
  },
  ListCommands: () => [
    {
      id: 'recur.text.sort-lines-asc',
      name: 'Sort Lines Ascending',
      category: 'Text',
      order: 10,
      scope: '',
      fileGlob: '',
      input: 'selection',
      fallbackInput: 'document',
      output: 'replaceInput',
      triggers: ['palette'],
      prompts: [],
      keyEquivalent: 'ctrl+f5',
      language: '',
      openInPreview: false,
      interpreter: 'typescript',
      bundleName: 'Text',
      sourceRoot: 'default',
    },
    {
      id: 'recur.text.uppercase',
      name: 'Uppercase',
      category: 'Text',
      order: 20,
      scope: '',
      fileGlob: '',
      input: 'selection',
      fallbackInput: 'document',
      output: 'replaceInput',
      triggers: ['palette'],
      prompts: [],
      keyEquivalent: 'ctrl+u',
      language: '',
      openInPreview: false,
      interpreter: 'typescript',
      bundleName: 'Text',
      sourceRoot: 'default',
    },
    {
      id: 'recur.go.gofmt-on-save',
      name: 'Gofmt on Save',
      category: 'Go',
      order: 10,
      scope: 'source.go',
      fileGlob: '*.go',
      input: 'document',
      fallbackInput: '',
      output: 'replaceDocument',
      triggers: ['willSave'],
      prompts: [],
      keyEquivalent: '',
      language: '',
      openInPreview: false,
      interpreter: 'shebang',
      bundleName: 'Go',
      sourceRoot: 'default',
    },
  ],
  ListSnippets: () => [
    {
      id: 'recur.go.ife',
      name: 'If Err',
      tabTrigger: 'ife',
      scope: 'source.go',
      fileGlob: '*.go',
      body: 'if ${1:err} != nil {\n\t${2:log.Fatal(${1:err})}\n}',
      keyEquivalent: '',
      order: 10,
      bundleName: 'Go',
      sourceRoot: 'default',
    },
    {
      id: 'recur.html.wrap',
      name: 'Wrap Selection in Tag',
      tabTrigger: '',
      scope: 'text.html.basic',
      fileGlob: '*.{html,htm}',
      body: '<${1:p}>${2:$TM_SELECTED_TEXT}</$1>',
      keyEquivalent: 'ctrl+w',
      order: 10,
      bundleName: 'HTML',
      sourceRoot: 'default',
    },
  ],
  ListSymbols: () => [
    { id: 'recur.symbols.source.function', scope: 'entity.name.function', show: true, kind: 'func', bundleName: 'Source', sourceRoot: 'default' },
    { id: 'recur.symbols.source.hide-calls', scope: 'meta.function-call entity.name.function', show: false, bundleName: 'Source', sourceRoot: 'default' },
  ],
  RunCommand: (a) => {
    const rc = a[1] as { input?: string }
    const input = rc?.input ?? ''
    // Mock gofmt: normalize double blank lines to one and ensure trailing newline.
    const formatted = input.replace(/\n{3,}/g, '\n\n').replace(/[ \t]+$/gm, '')
    const out = formatted.endsWith('\n') ? formatted : formatted + '\n'
    return { stdout: out, stderr: '', exitCode: 0, output: 'replaceDocument' }
  },
  // CommonJS shape matching esbuild FormatCommonJS + bundled `@recur/sdk`
  // (Worker runs via AsyncFunction). Includes a minimal Output/replaceEdited.
  CompiledModule: (a) => {
    const id = a[0] as string
    const sdk = `
var Output = Object.freeze({
  replaceSelection: "replaceSelection",
  replaceDocument: "replaceDocument"
});
function replaceEdited(ctx, text) {
  return { text: text, output: ctx.selectedText ? Output.replaceSelection : Output.replaceDocument };
}
`
    if (id === 'recur.text.sort-lines-asc') {
      return sdk + `
exports.default = function (ctx) {
  var ends = ctx.input.endsWith("\\n");
  var lines = ctx.input.replace(/\\n$/, "").split("\\n");
  lines.sort(function (a, b) { return a.localeCompare(b); });
  var text = lines.join("\\n");
  if (ends) text += "\\n";
  return replaceEdited(ctx, text);
};
`
    }
    return sdk + `exports.default = function (ctx) { return { text: ctx.input }; };`
  },
  Search: (a) => [{ path: 'src/main.go', line: 3, col: 1, text: a[2], preview: 'func main() {}' }],
  SearchAdvanced: (a) => ({
    hits: [{ path: 'src/main.go', line: 3, col: 1, text: a[2], preview: 'func main() {}' }],
    skippedLarge: 0,
    truncated: false,
  }),
  ReplaceInFiles: () => ({ filesChanged: 1, replacements: 1 }),
  ListFiles: () => ['src/main.go', 'README.md'],
  ListTerminals: () => [],
  RestoreWorkspaceTerminals: () => [],
  ListTerminalDockChrome: () => [],
  SetTerminalDockChrome: () => null,
  SetTerminalTitle: () => null,
  SetActiveTerminal: () => null,
  ReorderTerminals: () => null,
  // SearchFiles ranks in Go in production; here we filter the same ListFiles
  // fixture (so per-test ListFiles overrides still drive @-mention menus).
  // Exclude is applied at index build time in production (SetFileIndexExclude).
  SearchFiles: (a, _seed, table) => {
    const query = String(a[2] ?? '').trim().toLowerCase()
    const limit = Number(a[3]) > 0 ? Number(a[3]) : 50
    const filesOnly = !!a[4]
    const listFn = table.ListFiles
    const files = (typeof listFn === 'function' ? listFn(a, _seed, table) : ['src/main.go', 'README.md']) as string[]
    const matches = (p: string) => !query || p.toLowerCase().includes(query)
    const fileHits = files.filter(matches).map((path) => ({ path, score: 1 }))
    if (filesOnly) {
      return { hits: fileHits.slice(0, limit), building: false, ready: true }
    }
    const seen = new Set<string>()
    const dirs: string[] = []
    for (const file of files) {
      let p = file
      while (true) {
        const i = p.lastIndexOf('/')
        if (i <= 0) break
        p = p.slice(0, i)
        if (seen.has(p)) break
        seen.add(p)
        dirs.push(p)
      }
    }
    const dirHits = dirs.filter(matches).map((path) => ({ path, score: 1, dir: true as const }))
    return { hits: [...fileHits, ...dirHits].slice(0, limit), building: false, ready: true }
  },
  SetFileIndexExclude: () => null,
  ListSkills: () => [
    { name: 'design-audit', description: 'Audit UI polish', scope: 'workspace', userInvocable: true },
    { name: 'create-rule', description: 'Create a Cursor rule', scope: 'user', userInvocable: true },
  ],
  ListAllSkills: () => [
    { name: 'design-audit', description: 'Audit UI polish', scope: 'workspace', userInvocable: true },
    { name: 'create-rule', description: 'Create a Cursor rule', scope: 'user', userInvocable: true },
  ],
  Outline: () => [{ name: 'main', kind: 'func', line: 3 }],
  FindDefs: (a) => [{ name: a[2], kind: 'func', path: 'src/main.go', line: 3 }],
  FindRefs: (a) => [{ path: 'src/main.go', line: 3, text: 'func main() {}', def: true }],
  OpenInEditor: () => null,
  RevealInFinder: () => null,
  AbsPath: (a) => `/mock/${a[2] || ''}`,
  CreateFile: () => null,
  Mkdir: () => null,
  Rename: () => null,
  Delete: () => null,
  Copy: () => null,
  WorkingStatus: () => ({ 'src/main.go': 'm' }),
  StagedFiles: () => [],
  Stage: () => null,
  Unstage: () => null,
  Commit: () => ({ hash: 'abc1234', committed: true, additions: 1, deletions: 0 }),
  CommitStaged: () => ({ hash: 'abc1234', committed: true, additions: 1, deletions: 0 }),
  DiscardFile: () => null,
  DiscardAll: () => null,
  ListFindings: () => [],
  SetFindingStatus: (a) => ({
    id: String(a[1] || ''),
    workspaceId: String(a[0] || ''),
    status: String(a[2] || ''),
  }),
  CreateFinding: (a) => ({
    id: 'f-user-1',
    workspaceId: String(a[0] || ''),
    file: String(a[1] || ''),
    line: Number(a[2] || 0),
    message: String(a[3] || ''),
    endLine: Number(a[4] || 0),
    status: 'open',
    severity: 'info',
    source: 'user',
    iteration: 1,
  }),
  ListArtifacts: () => [],
  ListPRFindings: () => [],
  ListChangedFiles: () => ['src/main.go'],
  GitStatus: () => ({ 'src/main.go': 'm' }),
  FileAtBase: () => 'package main\n\nfunc main() {}\n',
  FileDiff: (_a, s) => ({
    path: 'src/main.go',
    status: 'm',
    diff: { add: 1, del: 0 },
    lines: [
      { old: '', new: '', type: 'hunk', codeHtml: '@@ -1,1 +1,2 @@' },
      { old: '1', new: '1', type: 'ctx', codeHtml: 'package main' },
      { old: '', new: '2', type: 'add', codeHtml: '// edited' },
    ],
  }),
  DiffVsBase: (_a, s, table) => ({ files: [table.FileDiff([], s, table)] }),
  DiffWorking: (_a, s, table) => ({ files: [table.FileDiff([], s, table)] }),
  ListCommits: () => [{
    hash: 'a3f91c2',
    sha: 'a3f91c2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
    message: 'Add feature',
    author: 'Ada',
    authorRole: 'Author',
    agentCode: 'AD',
    color: 'var(--indigo)',
    iterLabel: '',
    diff: { add: 4, del: 1 },
    when: '2m',
    files: [],
  }, {
    hash: 'b1c20d4',
    sha: 'b1c20d4bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
    message: 'Older work',
    author: 'Ada',
    authorRole: 'Author',
    agentCode: 'AD',
    color: 'var(--indigo)',
    iterLabel: '',
    diff: { add: 2, del: 0 },
    when: '1d',
    files: [],
  }],
  CommitDiff: (_a, s, table) => [table.FileDiff([], s, table)],
}

export type BridgeOptions = {
  // Override or extend resolvers per-test (e.g. to simulate an error or empty state).
  resolvers?: Record<string, Resolver>
  // Collected unresolved method names (for debugging); populated as calls arrive.
  onUnresolved?: (name: string) => void
}

/** Per-page D-attention state so e2e can idle a chat tab the way the engine would. */
export type MockChatAttention = {
  occluded: boolean
  focusWs: string
  focusCode: string
  unseen: Set<string>
  seed: Seed
}

const mockChatAttentionByPage = new WeakMap<Page, MockChatAttention>()

export function mockChatAttention(page: Page): MockChatAttention | undefined {
  return mockChatAttentionByPage.get(page)
}

function chatTabKey(workspaceId: string, code: string) {
  return `${workspaceId}::${code}`
}

function chatInboxID(workspaceId: string, code: string) {
  return `ib-chat-${workspaceId}-${code}`
}

function parseChatInboxID(id: string): { workspaceId: string; code: string } | null {
  if (!id.startsWith('ib-chat-')) return null
  const rest = id.slice('ib-chat-'.length)
  const i = rest.lastIndexOf('-')
  if (i <= 0) return null
  return { workspaceId: rest.slice(0, i), code: rest.slice(i + 1) }
}

function upsertChatInbox(seed: Seed, workspaceId: string, code: string) {
  const id = chatInboxID(workspaceId, code)
  const ws = seed.loopById?.[workspaceId]
  let sub = ws?.short || workspaceId
  const hostPrId = (ws?.hostPrId ?? '').trim()
  if (hostPrId) {
    const pr = seed.prById?.[hostPrId] ?? seed.prs?.find((p: { id: string }) => p.id === hostPrId)
    if (pr) sub = `#${pr.number} · ${pr.title || sub}`
  }
  const row = {
    id,
    group: 'ready',
    unread: true,
    kind: 'chat',
    color: 'var(--cyan)',
    title: `<b>${code === 'MA' ? 'Loop' : 'Chat'}</b> finished a reply`,
    sub,
    time: 'now',
    action: 'View',
    actionClass: 'go',
    target: `chat:${workspaceId}:${code}`,
    workspaceId,
  }
  const i = seed.inbox.findIndex((it) => it.id === id)
  if (i >= 0) seed.inbox[i] = row
  else seed.inbox = [row, ...seed.inbox]
}

function removeChatInbox(seed: Seed, workspaceId: string, code: string) {
  const id = chatInboxID(workspaceId, code)
  const next = seed.inbox.filter((it) => it.id !== id)
  if (next.length === seed.inbox.length) return false
  seed.inbox = next
  return true
}

function attentionRollup(seed: Seed, unseen: Set<string>) {
  const byWorkspace: Record<string, number> = {}
  const byProject: Record<string, number> = {}
  for (const key of unseen) {
    const i = key.indexOf('::')
    const wsId = i >= 0 ? key.slice(0, i) : key
    byWorkspace[wsId] = (byWorkspace[wsId] ?? 0) + 1
    const ws = seed.loopById?.[wsId]
    if (ws?.hostPrId) continue
    const proj = ws?.projectId
    if (proj) byProject[proj] = (byProject[proj] ?? 0) + 1
  }
  const n = unseen.size
  const chatUnread = seed.inbox.filter((it) => it.unread && it.kind === 'chat').length
  const otherUnread = seed.inbox.filter((it) => it.unread && it.kind !== 'chat').length
  return {
    unseen: n,
    inboxUnread: otherUnread + chatUnread,
    dock: n + otherUnread,
    byWorkspace,
    byProject,
  }
}

async function dispatchAttentionUpdated(
  page: Page,
  attn: MockChatAttention,
  workspaceId: string,
  code: string,
  tabUnseen: boolean,
  clearAllUnseen = false,
) {
  const counts = attentionRollup(attn.seed, attn.unseen)
  await page.evaluate(
    ({ attention }) => {
      ;(window as any)._wails.dispatchWailsEvent({ name: 'attention.updated', data: attention })
    },
    { attention: { ...counts, workspaceId, code, tabUnseen, clearAllUnseen } },
  )
}

/**
 * Finish a chat turn the way Engine.updateAgentStatus does: idle the agent, and
 * raise unseen / attention.updated only when the tab is not really visible
 * (unfocused, or SetChatUIOccluded).
 */
export async function simulateChatTurnIdle(page: Page, workspaceId: string, code: string) {
  const attn = mockChatAttentionByPage.get(page)
  if (!attn) throw new Error('simulateChatTurnIdle: installBridge was not called for this page')
  const focused = !attn.occluded && attn.focusWs === workspaceId && attn.focusCode === code
  const key = chatTabKey(workspaceId, code)
  if (focused) {
    attn.unseen.delete(key)
    removeChatInbox(attn.seed, workspaceId, code)
  } else {
    attn.unseen.add(key)
    upsertChatInbox(attn.seed, workspaceId, code)
  }
  await page.evaluate(
    ({ agent }) => {
      ;(window as any)._wails.dispatchWailsEvent({ name: 'agent.updated', data: agent })
    },
    { agent: { workspaceId, agentId: `${workspaceId}-${code}`, status: 'idle' } },
  )
  // Engine only publishes attention.updated when unseen actually changed.
  if (!focused) {
    await dispatchAttentionUpdated(page, attn, workspaceId, code, true)
    await page.evaluate(() => {
      ;(window as any)._wails.dispatchWailsEvent({ name: 'inbox.updated', data: {} })
    })
  }
}

// installBridge wires page.route so every bound call is answered from the seed.
// Resolvers mutate `seed` in place — callers that pass a shared fixture MUST
// clone first, or later tests on the same worker see leftover chat tabs / settings.
export async function installBridge(page: Page, seed: Seed, opts: BridgeOptions = {}) {
  // Playwright has no native Wails host, so Env.Info() is never injected.
  // Seed darwin so System.IsMac() / isCmdOrCtrl match the macOS app (⌘, not Ctrl).
  await page.addInitScript(() => {
    const w = window as Window & { _wails?: { environment?: { OS?: string } } }
    w._wails = w._wails ?? {}
    w._wails.environment = { ...w._wails.environment, OS: 'darwin' }
  })
  const idToName = methodIdToName()
  const table = { ...resolvers, ...(opts.resolvers ?? {}) }

  const attn: MockChatAttention = {
    occluded: false,
    focusWs: '',
    focusCode: '',
    unseen: new Set(),
    seed,
  }
  mockChatAttentionByPage.set(page, attn)
  // Queue a publish until after route.fulfill — evaluate inside the handler can deadlock.
  let pendingPublish: { workspaceId: string; code: string; clearAllUnseen?: boolean } | null = null
  const markFocusedSeen = () => {
    if (attn.occluded || !attn.focusWs || !attn.focusCode) return
    if (attn.unseen.delete(chatTabKey(attn.focusWs, attn.focusCode))) {
      removeChatInbox(attn.seed, attn.focusWs, attn.focusCode)
      pendingPublish = { workspaceId: attn.focusWs, code: attn.focusCode }
    }
  }
  const innerFocus = table.SetWorkspaceChatFocus
  table.SetWorkspaceChatFocus = (a, s, t) => {
    attn.focusWs = String(a[0] ?? '')
    attn.focusCode = String(a[1] ?? '')
    markFocusedSeen()
    return innerFocus?.(a, s, t)
  }
  const innerSendChat = table.SendWorkspaceMessage
  table.SendWorkspaceMessage = (a, s, t) => {
    const next = [...(a ?? [])]
    if (typeof next[2] === 'string') next[2] = peelChatSurface(next[2])
    return innerSendChat?.(next, s, t)
  }
  // Engine.CancelChat also idles a run the cancelled tab owns (idleWalkByChat).
  const pendingEvents: { name: string; data: unknown }[] = []
  const innerCancel = table.CancelWorkspaceMessage
  table.CancelWorkspaceMessage = (a, s, t) => {
    const ws = s.loopById?.[String(a[0] ?? '')]
    if (ws && (ws.loopChatCode || 'MA') === a[1] && ['queued', 'planning', 'coding', 'reviewing', 'verifying'].includes(ws.state)) {
      Object.assign(ws, { state: 'paused', phase: 'idle' })
      const row = s.loops.find((l: { id: string }) => l.id === ws.id)
      if (row) Object.assign(row, { state: 'paused', phase: 'idle' })
      pendingEvents.push({ name: 'workspace.updated', data: { workspaceId: ws.id, state: 'paused', phase: 'idle' } })
    }
    return innerCancel?.(a, s, t)
  }
  const innerOcclude = table.SetChatUIOccluded
  table.SetChatUIOccluded = (a, s, t) => {
    const was = attn.occluded
    attn.occluded = !!a[0]
    if (!attn.occluded && was) markFocusedSeen()
    return innerOcclude?.(a, s, t)
  }
  const innerMarkRead = table.MarkInboxRead
  table.MarkInboxRead = (a, s, t) => {
    const chat = parseChatInboxID(String(a[0] ?? ''))
    if (chat) {
      attn.unseen.delete(chatTabKey(chat.workspaceId, chat.code))
      pendingPublish = { workspaceId: chat.workspaceId, code: chat.code }
    }
    return innerMarkRead?.(a, s, t)
  }
  const innerMarkAll = table.MarkAllInboxRead
  table.MarkAllInboxRead = (a, s, t) => {
    attn.unseen.clear()
    pendingPublish = { workspaceId: '', code: '', clearAllUnseen: true }
    return innerMarkAll?.(a, s, t)
  }
  const innerCounts = table.AttentionCounts
  table.AttentionCounts = (a, s, t) => {
    const base = (innerCounts?.(a, s, t) ?? {}) as {
      unseen?: number
      inboxUnread?: number
      dock?: number
      byWorkspace?: Record<string, number>
      byProject?: Record<string, number>
    }
    const live = attentionRollup(s, attn.unseen)
    return {
      ...base,
      ...live,
      byWorkspace: { ...(base.byWorkspace ?? {}), ...live.byWorkspace },
      byProject: { ...(base.byProject ?? {}), ...live.byProject },
    }
  }

  await page.route('**/wails/runtime', async (route: Route) => {
    let body: any = {}
    try {
      body = JSON.parse(route.request().postData() || '{}')
    } catch {
      /* non-JSON runtime message */
    }
    const call = body?.args
    // A bound method call carries a numeric methodID. Anything else is a runtime
    // message (event subscription, window ops, …) — answer benignly.
    const boundName =
      (call && typeof call.methodName === 'string' && call.methodName.split('.').pop()) ||
      (call && typeof call.methodID === 'number' ? idToName.get(call.methodID) : undefined)
    if (call && boundName) {
      const name = boundName
      const fn = table[name]
      if (!fn) {
        opts.onUnresolved?.(name ?? `#${call.methodID}`)
        return route.fulfill({ status: 200, contentType: 'application/json', body: 'null' })
      }
      const result = await fn(call.args ?? [], seed, table)
      if (result instanceof BridgeError) {
        return route.fulfill({ status: 500, contentType: 'text/plain', body: result.message })
      }
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(result ?? null),
      })
      for (const evt of pendingEvents.splice(0)) {
        await page.evaluate((e) => (window as any)._wails.dispatchWailsEvent(e), evt)
      }
      const pub = pendingPublish
      pendingPublish = null
      if (pub) {
        await dispatchAttentionUpdated(page, attn, pub.workspaceId, pub.code, false, !!pub.clearAllUnseen)
        await page.evaluate(() => {
          ;(window as any)._wails.dispatchWailsEvent({ name: 'inbox.updated', data: {} })
        })
      }
      return
    }
    return route.fulfill({ status: 200, contentType: 'application/json', body: '{}' })
  })
}

// gotoApp navigates to the app root and waits for the shell to mount.
export async function gotoApp(page: Page) {
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await waitForShell(page)
}

/** Simulate an app restart: same mocked backend, fresh frontend boot. */
export async function reloadApp(page: Page) {
  await page.reload({ waitUntil: 'domcontentloaded' })
  await waitForShell(page)
}

async function waitForShell(page: Page) {
  await page.waitForSelector('.rail', { timeout: 15000 })
  // Let loadInitial() resolve + the first reactive render settle.
  await page.waitForTimeout(600)
}
