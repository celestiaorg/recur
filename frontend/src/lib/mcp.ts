/** Settings → MCP / Project → MCP helpers. Enablement matches domain.MCPGlobalDisabled. */

export const LINEAR_MCP_ID = 'linear'

export type MCPServerRow = {
  id?: string
  name?: string
  preset?: string
  enabled?: boolean
  transport?: string
  url?: string
  headers?: { [k: string]: string | undefined }
  command?: string
  args?: string[]
  env?: { [k: string]: string | undefined }
}

const reserved = new Set(['recur', 'playwright'])

export function validMCPID(id: string): boolean {
  return /^[a-z][a-z0-9-]{0,40}$/.test(id) && !reserved.has(id)
}

export function slugMCPID(name: string): string {
  return name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 41)
}

export function mcpGlobalDisabled(servers: MCPServerRow[] | undefined | null): string[] {
  const on = new Set<string>()
  const extraOff: string[] = []
  for (const s of servers ?? []) {
    const id = (s.id ?? '').trim()
    if (!id) continue
    if (s.enabled) on.add(id)
    else if (id !== LINEAR_MCP_ID) extraOff.push(id)
  }
  const off: string[] = []
  if (!on.has(LINEAR_MCP_ID)) off.push(LINEAR_MCP_ID)
  off.push(...extraOff)
  return off
}

export function linearMCPOn(servers: MCPServerRow[] | undefined | null): boolean {
  return !!(servers ?? []).some((s) => s.id === LINEAR_MCP_ID && s.enabled)
}

export function customMCPServers(servers: MCPServerRow[] | undefined | null): MCPServerRow[] {
  return (servers ?? []).filter((s) => (s.id ?? '') !== LINEAR_MCP_ID && (s.preset ?? '') !== LINEAR_MCP_ID)
}

export function upsertLinearEnabled(servers: MCPServerRow[] | undefined | null, enabled: boolean): MCPServerRow[] {
  const rest = customMCPServers(servers)
  if (!enabled) return rest
  return [{ id: LINEAR_MCP_ID, name: 'Linear', preset: LINEAR_MCP_ID, enabled: true, transport: 'http' }, ...rest]
}

export function setCustomMCPEnabled(servers: MCPServerRow[] | undefined | null, id: string, enabled: boolean): MCPServerRow[] {
  return (servers ?? []).map((s) => (s.id === id ? { ...s, enabled } : s))
}

export function removeMCPServer(servers: MCPServerRow[] | undefined | null, id: string): MCPServerRow[] {
  return (servers ?? []).filter((s) => s.id !== id)
}

// updateMCPServer patches a stored server in place so the id (and project
// override lists keyed on it) stay put. Callers must not pass a different id.
export function updateMCPServer(servers: MCPServerRow[] | undefined | null, id: string, patch: Partial<MCPServerRow>): MCPServerRow[] {
  return (servers ?? []).map((s) => (s.id === id ? { ...s, ...patch, id: s.id } : s))
}
