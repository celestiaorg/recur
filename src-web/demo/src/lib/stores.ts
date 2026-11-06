// Svelte stores for app state shared across views.

import { writable, derived } from 'svelte/store'
import type {
  InboxItem, PR, Project, Settings, Workspace, WorkspaceBrief,
} from './types'
import { resolveEditorThemeKey } from './cm-themes'
import { parseChatRouteTarget, parseWorkspaceRouteTarget } from './chat-route'
import { type PRListLife, type PRListSort, PR_LIST_SEGS, PR_LIST_SORT_DEFAULT, PR_LIST_SORTS } from './pr-list-filter'

export type ViewName = 'detail' | 'pr' | 'prdetail' | 'briefing'

// ---- appearance / theme ----
// Themes are color-only (identity & layout stay constant). 'default' means
// "no data-theme attr".
const THEME_KEYS = ['default', 'paper', 'slate', 'coolglow', 'catppuccin-mocha', 'catppuccin-latte', 'nord', 'gruvbox']
export const theme = writable<string>('default')

/** True for light app-chrome palettes (affects file-icons). */
export function isLightTheme(key: string): boolean {
  return key === 'paper' || key === 'catppuccin-latte'
}

export function setTheme(key: string) {
  const valid = THEME_KEYS.includes(key) ? key : 'default'
  theme.set(valid)
  if (valid === 'default') document.documentElement.removeAttribute('data-theme')
  else document.documentElement.setAttribute('data-theme', valid)
}

// ---- code editor theme (independent of app chrome theme) ----
export const editorTheme = writable<string>('recur')

export function applyEditorTheme(key: string) {
  editorTheme.set(resolveEditorThemeKey(key))
}

export const setEditorTheme = applyEditorTheme

// ---- code editor layout prefs ----
export type EditorWhitespace = 'off' | 'trailing' | 'all'

export type EditorPrefs = {
  fontSize: number
  lineWrapping: boolean
  tabSize: number
  whitespace: EditorWhitespace
  specialChars: boolean
  lineNumbers: boolean
  activeLine: boolean
  foldGutter: boolean
  /** SCM add/modify/delete bars in the code-view gutter (vs FileAtBase). */
  diffGutter: boolean
  bracketMatching: boolean
  scrollPastEnd: boolean
  lineHeight: number
  cursorBlink: boolean
  thickCaret: boolean
  rectangularSelect: boolean
  dropCursor: boolean
  /** Print-margin column; 0 = off. */
  rulerColumn: number
}

export const editorPrefs = writable<EditorPrefs>({
  fontSize: 12,
  lineWrapping: true,
  tabSize: 4,
  whitespace: 'off',
  specialChars: true,
  lineNumbers: true,
  activeLine: true,
  foldGutter: true,
  diffGutter: true,
  bracketMatching: true,
  scrollPastEnd: false,
  lineHeight: 1.55,
  cursorBlink: true,
  thickCaret: false,
  rectangularSelect: true,
  dropCursor: true,
  rulerColumn: 0,
})

export const EDITOR_FONT_STACK = "'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, monospace"

// ---- code observatory panels ----
export type CodePanelKey = 'maestro' | 'findings' | 'files'

export const CODE_PANEL_LABELS: Record<CodePanelKey, string> = {
  maestro: 'Maestro',
  findings: 'Findings',
  files: 'Files',
}

/** VS Code–style globs hidden from the file tree. */
export const explorerPrefs = writable({
  exclude: ['**/.git', '**/.svn', '**/.hg', '**/.DS_Store', '**/Thumbs.db'],
})

// ---- panel sizes (resizable docks) ----
export type PanelKey = 'rail' | 'obsSide' | 'prFileList' | 'maestroDock'

const PANEL_DEFAULTS: Record<PanelKey, number> = {
  rail: 272,
  obsSide: 260,
  prFileList: 250,
  maestroDock: 600,
}

const PANEL_BOUNDS: Record<PanelKey, { min: number; max: number }> = {
  rail: { min: 180, max: 600 },
  // The Observatory docks get a generous ceiling; their real cap is applied at
  // drag time relative to the editor row (ResizeHandle).
  obsSide: { min: 160, max: 2000 },
  prFileList: { min: 160, max: 900 },
  maestroDock: { min: 260, max: 2000 },
}

const PANEL_CSS_VARS: Record<PanelKey, string> = {
  rail: '--rail-width',
  obsSide: '--obs-side-width',
  prFileList: '--pr-filelist-width',
  maestroDock: '--maestro-dock-width',
}

export const panelWidths = writable<Record<PanelKey, number>>({ ...PANEL_DEFAULTS })

export function setPanelWidthLive(key: PanelKey, width: number) {
  panelWidths.update((cur) => {
    const { min, max } = PANEL_BOUNDS[key]
    const next = { ...cur, [key]: Math.round(Math.min(max, Math.max(min, width))) }
    const root = document.documentElement
    for (const k of Object.keys(PANEL_CSS_VARS) as PanelKey[]) {
      root.style.setProperty(PANEL_CSS_VARS[k], `${next[k]}px`)
    }
    return next
  })
}

// ---- navigation ----
export const view = writable<ViewName>('briefing')
export const selectedWorkspaceId = writable<string>('')
export const selectedPRId = writable<string>('')

export const prListSeg = writable<PRListLife>('open')
/** Status facets (review requested, checks, …). Empty = no extra cut. */
export const prListFacets = writable<string[]>([])
export const prListSort = writable<PRListSort>(PR_LIST_SORT_DEFAULT)
/** Empty = every git project. */
export const prListProjectIds = writable<string[]>([])
export { PR_LIST_SEGS, PR_LIST_SORTS }

export type PRDetailTab = 'conv' | 'commits' | 'files' | 'checks'
export const prDetailTab = writable<PRDetailTab>('conv')

// Deep-link into the code observatory (file:line or a panel).
export type ObsJump = {
  path?: string
  line?: number
  workspaceId?: string
  panel?: 'findings' | 'plan' | 'review'
  preferDiff?: boolean
}
export const observatoryJump = writable<ObsJump | null>(null)

// ---- data ----
export const projects = writable<Project[]>([])
export const workspaces = writable<Workspace[]>([])
export const prs = writable<PR[]>([])
/** PRs visible under the dashboard's project filter (empty selection = all).
 *  Legacy rows with an empty projectId resolve through their workspace. */
export const prListScoped = derived([prs, prListProjectIds, workspaces], ([$prs, $ids, $ws]) => {
  if (!$ids.length) return $prs
  return $prs.filter((p) => {
    const pid = (p.projectId || $ws.find((w) => w.id === p.workspaceId)?.projectId || '').trim()
    return $ids.includes(pid)
  })
})
export const inbox = writable<InboxItem[]>([])

/** Unread notifications per workspace: the read/unread signal on Briefing cards. */
export const inboxByWorkspace = derived(inbox, ($inbox) => {
  const m: Record<string, InboxItem[]> = {}
  for (const i of $inbox) {
    if (i.unread && i.workspaceId) (m[i.workspaceId] ??= []).push(i)
  }
  return m
})

export const briefing = writable<{ generatedAt: number; workspaces: WorkspaceBrief[] }>({ generatedAt: 0, workspaces: [] })

// Unseen finished chat tabs + rollups for sidebar/project badges.
export const attention = writable<{ byWorkspace: Record<string, number>; byProject: Record<string, number> }>({
  byWorkspace: {},
  byProject: {},
})
export const settings = writable<Settings | null>(null)
// Workspace IDs with at least one agent in a live status. Sidebar and fan-out
// rows also pulse for an in-flight loop (workspaceIsRunning).
export const workspacesRunning = writable<ReadonlySet<string>>(new Set())

// One-shot: WorkspaceView selects this agent tab after openWorkspaceChat.
export const pendingChatFocus = writable<{ workspaceId: string; code: string } | null>(null)

export function go(v: ViewName) {
  view.set(v)
}

export function openWorkspace(id: string) {
  selectedWorkspaceId.set(id)
  go('detail')
}

export function openWorkspaceChat(workspaceId: string, agentCode = '') {
  if (agentCode) pendingChatFocus.set({ workspaceId, code: agentCode })
  openWorkspace(workspaceId)
}

export function openPR(id: string) {
  selectedPRId.set(id)
  prDetailTab.set('conv')
  go('prdetail')
}

// routeTarget handles inbox targets like "workspace:LOOP-2218" / "pr:482" /
// "chat:workspaceId:PA2".
export function routeTarget(target: string) {
  const [kind, ...rest] = target.split(':')
  if (kind === 'workspace' || kind === 'loop') {
    const parsed = parseWorkspaceRouteTarget(target)
    if (!parsed) return
    openWorkspace(parsed.workspaceId)
    if (parsed.panel) observatoryJump.set({ workspaceId: parsed.workspaceId, panel: parsed.panel })
  } else if (kind === 'pr') openPR(rest.join(':'))
  else if (kind === 'chat') {
    const parsed = parseChatRouteTarget(target)
    if (parsed) openWorkspaceChat(parsed.workspaceId, parsed.code)
  }
}

// navKey maps a view to the sidebar nav item that should appear active.
export function navKey(v: ViewName): string {
  if (v === 'prdetail') return 'pr'
  if (v === 'detail') return ''
  return v
}

// ---- toasts ----
export type ToastKind = 'success' | 'error' | 'info'
export type Toast = { id: number; kind: ToastKind; text: string; ttlMs: number }
export const toasts = writable<Toast[]>([])
let toastSeq = 0

export function pushToast(kind: ToastKind, text: string) {
  const id = ++toastSeq
  const ttlMs = kind === 'error' ? 8000 : 3500
  toasts.update((arr) => [...arr, { id, kind, text, ttlMs }])
  setTimeout(() => dismissToast(id), ttlMs)
}

export function dismissToast(id: number) {
  toasts.update((arr) => arr.filter((t) => t.id !== id))
}
