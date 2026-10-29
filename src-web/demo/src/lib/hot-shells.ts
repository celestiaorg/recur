/**
 * Live DOM budget for workspace workbenches.
 *
 * Snapshots (observatory-cache, CodeMirror parks) remember recent roots in JS
 * heap, byte-capped. The DOM cannot: each Observatory is CodeMirror + xterm + a full chat
 * tree. Keep the visible shell plus the previous one mounted; evict the rest
 * so WKWebView does not accumulate workbenches. A→B→A is then a node move,
 * not a remount.
 *
 * HOT_SHELL_MAX = 2 is the measured cap. Raising the live cap made hot
 * switches slower on WKWebView.
 */
export const HOT_SHELL_MAX = 2

/** Newest-first LRU. `id` becomes the head; entries missing from `live` drop. */
export function touchHotShells(
  hot: readonly string[],
  id: string,
  opts?: { max?: number; live?: ReadonlySet<string> },
): string[] {
  const max = opts?.max ?? HOT_SHELL_MAX
  const live = opts?.live
  const next: string[] = []
  const seen = new Set<string>()
  const push = (x: string) => {
    if (!x || seen.has(x)) return
    if (live && !live.has(x)) return
    seen.add(x)
    next.push(x)
  }
  push(id)
  for (const x of hot) {
    if (next.length >= max) break
    push(x)
  }
  return next
}

export function hotShellsEqual(a: readonly string[], b: readonly string[]): boolean {
  if (a.length !== b.length) return false
  return a.every((id, i) => id === b[i])
}

/** Mounted workbench ids. Archived rows stay in the store but must not keep a shell. */
export function liveWorkspaceIds(workspaces: readonly { id?: string; archived?: boolean }[]): Set<string> {
  const live = new Set<string>()
  for (const w of workspaces) {
    if (w.id && !w.archived) live.add(w.id)
  }
  return live
}
