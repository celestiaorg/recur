/** Parse `chat:<workspaceId>:<code>` inbox targets (workspace ids may contain dots). */
export function parseChatRouteTarget(target: string): { workspaceId: string; code: string } | null {
  if (!target.startsWith('chat:')) return null
  const rest = target.slice('chat:'.length)
  const i = rest.indexOf(':')
  const workspaceId = i < 0 ? rest : rest.slice(0, i)
  const code = i < 0 ? '' : rest.slice(i + 1)
  if (!workspaceId) return null
  return { workspaceId, code }
}

/** Parse `workspace:<id>` / `loop:<id>` with an optional `:plan` / `:findings` / `:review` panel. */
export function parseWorkspaceRouteTarget(target: string): { workspaceId: string; panel?: 'plan' | 'findings' | 'review' } | null {
  const t = target.trim()
  const i = t.indexOf(':')
  if (i < 0) return null
  const kind = t.slice(0, i)
  if (kind !== 'workspace' && kind !== 'loop') return null
  const rest = t.slice(i + 1).trim()
  if (!rest) return null
  const cut = rest.indexOf(':')
  if (cut < 0) return { workspaceId: rest }
  const workspaceId = rest.slice(0, cut)
  const panel = rest.slice(cut + 1)
  if (!workspaceId) return null
  if (panel === 'plan' || panel === 'findings' || panel === 'review') return { workspaceId, panel }
  return { workspaceId }
}
