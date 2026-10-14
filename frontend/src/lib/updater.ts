// In-app updater UI (Go: updater.go, headless Wails updater). Background checks
// stay silent until an update is downloaded; a menu check reports every outcome.
import { Events } from '@wailsio/runtime'
import { pushToast } from './stores'

const replace = 'updater'
let manual = false
let readyVersion = ''

function showReady() {
  pushToast('info', `Recur ${readyVersion} is ready to install`, {
    action: { label: 'Restart', fn: () => void Events.Emit('wails:updater:user:restart').catch(() => {}) },
    ttlMs: 0,
    replace,
  })
}

export function checkForUpdates() {
  if (readyVersion) return showReady()
  manual = true
  pushToast('info', 'Checking for updates…', { replace, ttlMs: 0 })
  void Events.Emit('updater.check').catch(() => {})
}

export function wireUpdaterEvents() {
  Events.On('wails:updater:update-available', (e: any) => {
    if (manual) pushToast('info', `Downloading Recur ${e.data?.version ?? ''}…`, { replace, ttlMs: 0 })
  })
  Events.On('wails:updater:no-update', () => {
    if (manual) pushToast('success', 'Recur is up to date', { replace })
    manual = false
  })
  Events.On('wails:updater:error', (e: any) => {
    if (manual) pushToast('error', `Update failed: ${e.data?.message ?? 'unknown error'}`, { replace })
    manual = false
  })
  Events.On('wails:updater:update-ready', (e: any) => {
    manual = false
    readyVersion = e.data?.version ?? ''
    showReady()
  })
}
