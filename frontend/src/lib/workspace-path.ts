import { get } from 'svelte/store'
import { CodeService } from './api'
import { copyText } from './clipboard'
import { wsMainBlocked } from './modes'
import { selectedWorkspaceId, workspaces, pushToast } from './stores'

/** Copy the on-screen workspace's folder path to the clipboard. */
export async function copySelectedWorkspacePath(): Promise<void> {
  const id = get(selectedWorkspaceId)
  const ws = get(workspaces).find((w) => w.id === id)
  if (wsMainBlocked(ws)) return
  let path = ws?.worktreePath?.trim() ?? ''
  if (!path && ws) {
    try {
      path = (await CodeService.AbsPath(ws.projectId, ws.id, '')).trim()
    } catch {
      path = ''
    }
  }
  if (!path) {
    pushToast('error', 'This workspace has no folder on disk')
    return
  }
  try {
    await copyText(path)
    pushToast('info', 'Copied path')
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err)
    pushToast('error', `Copy failed: ${msg}`)
  }
}
