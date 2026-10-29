// Bridge between the Svelte stores and the backend services. Every service
// method resolves through one handler keyed by method name; the showcase
// installs it (showcase/transport.ts) before the app boots.
import { projects, workspaces, prs, inbox, briefing, attention, settings, pushToast } from './stores'

type Handler = (method: string, args: unknown[]) => Promise<any>
let handler: Handler = () => Promise.reject(new Error('no service handler installed'))

export function setServiceHandler(h: Handler) {
  handler = h
}

export const service = new Proxy({} as Record<string, (...args: any[]) => Promise<any>>, {
  get: (_, method) => (...args: unknown[]) => handler(String(method), args),
})

// call() wraps a service promise so a rejection shows an error toast instead
// of failing silently. Returns null on failure.
export async function call<T>(p: Promise<T>, failMsg: string): Promise<T | null> {
  try { return await p }
  catch (err: unknown) {
    pushToast('error', `${failMsg}: ${err instanceof Error ? err.message : String(err)}`)
    return null
  }
}

// loadInitial pulls the dataset into the stores on boot.
export async function loadInitial() {
  const [pj, ws, pr, ib, st, att, br] = await Promise.all([
    service.ListProjects(),
    service.ListWorkspaces(),
    service.ListPRs(),
    service.GetInbox(),
    service.GetSettings(),
    service.AttentionCounts(),
    service.GetBriefing(),
  ])
  projects.set(pj)
  workspaces.set(ws)
  prs.set(pr)
  inbox.set(ib)
  settings.set(st)
  attention.set(att)
  briefing.set(br)
}
