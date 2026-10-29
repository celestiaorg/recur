/** Looking-at surface shared with the agent and prompt actions. */
export type ChatSurfaceKind = 'workspace' | 'local_pr' | 'github_pr'

export type ChatSurface = {
  surface: ChatSurfaceKind
  prId?: string
  prNumber?: number
  prSource?: string
  prUrl?: string
  prTitle?: string
  prBase?: string
  prBranch?: string
}

const OPEN = '⟦recur-surface⟧'
const CLOSE = '⟦/recur-surface⟧'

export function chatSurfaceForView(
  view: string,
  pr?: {
    id?: string
    number?: number
    source?: string
    url?: string
    title?: string
    base?: string
    branch?: string
  } | null,
): ChatSurface {
  const id = (pr?.id ?? '').trim()
  if (view === 'prdetail' && id) {
    const source = (pr?.source ?? '').toLowerCase()
    const github = source === 'gh' || source === 'github'
    return {
      surface: github ? 'github_pr' : 'local_pr',
      prId: id,
      prNumber: pr?.number ?? 0,
      prSource: pr?.source ?? '',
      prUrl: pr?.url ?? '',
      prTitle: pr?.title ?? '',
      prBase: pr?.base ?? '',
      prBranch: pr?.branch ?? '',
    }
  }
  return { surface: 'workspace' }
}

/** Prefix the chat wire (not the UI bubble) so the engine can peel the surface. */
export function attachChatSurface(text: string, ctx: ChatSurface): string {
  const payload = JSON.stringify({
    surface: ctx.surface,
    prId: ctx.prId || undefined,
    prNumber: ctx.prNumber || undefined,
    prSource: ctx.prSource || undefined,
    prUrl: ctx.prUrl || undefined,
    prTitle: ctx.prTitle || undefined,
    prBase: ctx.prBase || undefined,
    prBranch: ctx.prBranch || undefined,
  })
  return `${OPEN}${payload}${CLOSE}\n${text}`
}

/** Strip a leading looking-at marker. Invalid markers are left in place. */
export function peelChatSurface(text: string): string {
  const s = text.replace(/^[ \t\r\n]+/, '')
  if (!s.startsWith(OPEN)) return text
  const closeAt = s.indexOf(CLOSE)
  if (closeAt < 0) return text
  const raw = s.slice(OPEN.length, closeAt).trim()
  try {
    const parsed = JSON.parse(raw) as { surface?: string }
    if (!parsed || typeof parsed.surface !== 'string' || !parsed.surface.trim()) return text
  } catch {
    return text
  }
  return s.slice(closeAt + CLOSE.length).replace(/^[ \t\r\n]+/, '')
}
