// Quick-create a blank hand-driven workspace (⇧⌘N): same as submitting the
// New-workspace modal with an empty prompt, always in Agent mode — not the
// last mode remembered by the modal.
import { get } from 'svelte/store'
import { ProjectsService, WorkspacesService, call, refreshProjects, upsertWorkspace, bridgeErrorMessage } from './api'
import { composeMarkedText, isChatInjectedReply, isChatStoppedReply, stripAttachmentMarkers, toSendAttachments } from './agentchat'
import { attachChatSurface } from './chat-surface'
import { waitForWorktree } from './wait-for-worktree'
import {
  closeNewTask,
  commandsOpen,
  gitProjectForNewWorkspace,
  newTaskOpen,
  newProjectOpen,
  openInOpen,
  openNewTask,
  openWorkspace,
  paletteOpen,
  pushToast,
  runMenuOpen,
  shortcutsOpen,
  stopMenuOpen,
  syntaxModeOpen,
  view,
  setWorkspaceSeed,
  takeWorkspaceSeed,
} from './stores'
import type { Project, Workspace } from '../../bindings/recur/internal/domain/models'
import type { ChatAttachmentDraft } from '../components/AgentChat.svelte'

/** Start git worktree add from repoId + baseBranch only. ListProjects /
 *  ListBranches returning new objects, or loadingBranches flipping, must not
 *  cancel an in-flight create. */
export function eagerCreateInput(
  open: boolean,
  loop: boolean,
  projectId: string,
  branch: string,
  claimed: boolean,
): { projectId: string; branch: string } | null {
  if (!open || loop || claimed || !projectId || !branch) return null
  return { projectId, branch }
}

/** Modal eager-create: persist as soon as repo+base are known. Cancel must
 *  delete even when CreateWorkspace is still in flight (pendingId is empty
 *  until the RPC returns; workspace.updated can paint the row earlier). */
export function startEagerCreate(
  create: () => Promise<Workspace | null>,
  onReady: (ws: Workspace) => void,
  abandon: (id: string) => void,
): { promise: Promise<Workspace | null>; cancel: () => void } {
  let cancelled = false
  let id = ''
  const promise = create()
    .then((ws) => {
      if (!ws?.id) return null
      id = ws.id
      if (cancelled) {
        abandon(id)
        return null
      }
      onReady(ws)
      return ws
    })
    .catch(() => null)
  return {
    promise,
    cancel() {
      cancelled = true
      if (id) abandon(id)
    },
  }
}

/** Effect cleanup after eager create. Always drop the modal's pendingId so a
 *  later open cannot submit into the previous row; only abandon if unclaimed. */
export function endEagerCreate(claimed: boolean, cancel: () => void): { pendingId: ''; pendingPromise: null } {
  if (!claimed) cancel()
  return { pendingId: '', pendingPromise: null }
}

/** Prefer the painted pending row; otherwise wait for CreateWorkspace. */
export async function takeEagerWorkspace(
  pendingId: string,
  pendingPromise: Promise<Workspace | null> | null,
  rows: readonly Workspace[],
): Promise<Workspace | null> {
  if (pendingId) {
    const found = rows.find((w) => w.id === pendingId)
    if (found) return found
  }
  if (pendingPromise) return pendingPromise
  return null
}

export type OpeningSeed = {
  id: string
  text: string
  display?: string
  mode?: string
  model?: string
  flags?: Record<string, string>
  simplify?: boolean
  attachments?: { id: string; name: string; mime: string; dataUrl: string }[]
}

const FIRST_CHAT = 'PA'

function openCreatedWorkspace(): boolean {
  return get(view) === 'welcome'
}

/** Open the new workspace from Welcome. Anywhere else, keep this view and send
 *  any opening prompt without mounting the new shell. */
export function revealCreatedWorkspace(seed: OpeningSeed, opts?: { loop?: boolean }): Promise<void> | void {
  if (!openCreatedWorkspace()) {
    if (opts?.loop) return
    return startParkedOpening(seed)
  }
  if (!opts?.loop) setWorkspaceSeed(seed)
  openWorkspace(seed.id)
}

/** Wire prompt + chat-bubble label after New-workspace ExpandGoal. */
export function openingChatSeed(
  typed: string,
  draft: { wire: string; display: string },
  expanded?: { prompt?: string | null; display?: string | null } | null,
): { text: string; display: string } {
  const t = typed.trim()
  if (draft.wire && t === draft.display) {
    return { text: draft.wire, display: draft.display }
  }
  return {
    text: expanded?.prompt?.trim() || t,
    display: (expanded?.display ?? '').trim(),
  }
}

/** Keep inline ⟦attachment:id⟧ layout on the seeded opening message. */
export function openingSeedText(
  marked: string,
  prompt: string,
  drafts: ChatAttachmentDraft[] = [],
): string {
  const stripped = stripAttachmentMarkers(marked).trim()
  if (!prompt.trim() || stripAttachmentMarkers(prompt).trim() === stripped) {
    return marked
  }
  return composeMarkedText(prompt, drafts)
}

let creating = false

function dismissOverlays() {
  paletteOpen.set(false)
  shortcutsOpen.set(false)
  openInOpen.set(false)
  commandsOpen.set(false)
  syntaxModeOpen.set(false)
  runMenuOpen.set(false)
  stopMenuOpen.set(false)
  if (get(newTaskOpen)) closeNewTask()
  newProjectOpen.set(false)
}

/** Pick a live branch for ⇧⌘N. Uses ListBranchesForPath (foldMissing=false)
 *  so a configured-but-deleted base is not injected the way ListBranches does
 *  for the modal picker. Prefer the project's base when it still resolves;
 *  otherwise the first real tip; empty/unborn repos keep the configured/main
 *  fallback (worktree add can create it). */
async function resolveBaseBranch(project: Project): Promise<string> {
  const fallback = (project.baseBranch || 'main').trim() || 'main'
  try {
    const list = await ProjectsService.ListBranchesForPath(project.repoPath, fallback)
    if (!list?.length) return fallback
    return list.some((b) => b.name === fallback) ? fallback : (list[0]?.name || fallback)
  } catch {
    return fallback
  }
}

/** Create PA if needed, then persist dock selection so it survives reload. */
async function persistOpeningChat(
  seed: OpeningSeed, mode: string, flags: Record<string, string>, simplify: boolean | null,
): Promise<void> {
  await WorkspacesService.ListWorkspaceChats(seed.id)
  await WorkspacesService.RememberWorkspaceChatSelection(
    seed.id, FIRST_CHAT, seed.model ?? '', flags, mode, simplify,
  )
}

/** Remember dock + send the opening prompt without mounting the workbench. */
export async function startParkedOpening(seed: OpeningSeed): Promise<void> {
  const mode = seed.mode || 'agent'
  const flags = seed.flags ?? {}
  const simplify = typeof seed.simplify === 'boolean' ? seed.simplify : null
  const drafts = seed.attachments ?? []
  const marked = composeMarkedText(seed.text || '', drafts).trim()
  setWorkspaceSeed(seed)
  if (!marked && !drafts.length) {
    await persistOpeningChat(seed, mode, flags, simplify).catch(() => {})
    return
  }

  try {
    await persistOpeningChat(seed, mode, flags, simplify)
    void WorkspacesService.ApplyProvisionalChatTitles(seed.id, FIRST_CHAT, (seed.display ?? '').trim() || marked).catch(() => {})
  } catch (err) {
    pushToast('error', bridgeErrorMessage(err) || 'Could not prepare the workspace chat')
    return
  }

  const ready = await waitForWorktree(seed.id, new AbortController().signal)
  if (!ready) return
  if (!takeWorkspaceSeed(seed.id)) return

  const wire = attachChatSurface(marked, { surface: 'workspace' })
  try {
    const reply = await WorkspacesService.SendWorkspaceMessage(
      seed.id, FIRST_CHAT, wire, seed.model ?? '', flags,
      toSendAttachments(drafts),
      mode,
      (seed.display ?? '').trim(),
      false,
    )
    if (isChatInjectedReply(reply) || isChatStoppedReply(reply)) return
    const text = (reply || '').trim()
    if (!isParkedOpeningFailure(text)) return
    pushToast('error', text)
    setWorkspaceSeed(seed)
  } catch (err) {
    pushToast('error', bridgeErrorMessage(err) || 'The agent hit an error — try again.')
    setWorkspaceSeed(seed)
  }
}

/** Go early-returns before the user row is recorded (workspace_chat.go / maestro.go). */
function isParkedOpeningFailure(text: string): boolean {
  return [
    "I couldn't save that image attachment.",
    "I couldn't run that prompt action:",
    "I couldn't engage the loop:",
    "I can't find that workspace",
    "I can't run right now:",
    "I'm at capacity",
    "There's nothing to retry",
    'A loop is running in this workspace',
    'Create a workspace from this pull request',
  ].some((p) => text.startsWith(p))
}

/** Create a blank Agent-mode workspace in the focused git project. */
export async function createBlankWorkspace(): Promise<void> {
  if (creating) return
  creating = true
  try {
    dismissOverlays()
    await refreshProjects()
    const project = gitProjectForNewWorkspace()
    if (!project) {
      openNewTask()
      return
    }
    const base = await resolveBaseBranch(project)
    const ws = await call(
      WorkspacesService.CreateWorkspace(project.id, '', base, 'agent', '', '', null),
      'Could not create the workspace',
    )
    if (!ws) return
    upsertWorkspace(ws)
    pushToast('success', 'Workspace created')
    void revealCreatedWorkspace({ id: ws.id, text: '', mode: 'agent' })
  } finally {
    creating = false
  }
}
