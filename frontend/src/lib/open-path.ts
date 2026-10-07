import { Dialogs } from '@wailsio/runtime'
import { WorkspacesService } from '../../bindings/recur/internal/app'
import type { OpenPathResult } from '../../bindings/recur/internal/domain/models'
import { call, refreshProjects, upsertProject, upsertWorkspace } from './api'
import { observatoryJump, openWorkspace } from './stores'

let picking = false

/** Native file/folder picker, then the same OpenPath path as `recur .`. */
export async function pickAndOpenPath(): Promise<void> {
  if (picking) return
  picking = true
  try {
    let path: unknown
    try {
      path = await Dialogs.OpenFile({
        CanChooseDirectories: true,
        CanChooseFiles: true,
        AllowsMultipleSelection: false,
        Title: 'Open',
        Message: 'Choose a file or folder',
        ButtonText: 'Open',
      })
    } catch {
      return
    }
    if (typeof path !== 'string' || !path) return
    const res = await call(WorkspacesService.OpenPath(path), 'Could not open')
    if (res) applyOpenPathResult(res)
  } finally {
    picking = false
  }
}

export function applyOpenPathResult(raw: OpenPathResult | Record<string, unknown> | null | undefined): void {
  if (!raw || typeof raw !== 'object') return
  const res = raw as OpenPathResult
  if (res.project) upsertProject(res.project)
  if (res.workspace) upsertWorkspace(res.workspace)
  void refreshProjects()
  const wsId = res.workspaceId
  if (!wsId) return
  const jumpPath = (res.file || res.reveal || '').trim()
  if (jumpPath) {
    observatoryJump.set({ path: jumpPath, workspaceId: wsId })
  }
  openWorkspace(wsId)
}
