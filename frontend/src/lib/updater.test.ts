import { beforeEach, describe, expect, it, vi } from 'vitest'

const handlers = new Map<string, (e: any) => void>()
const emit = vi.fn((..._a: unknown[]) => Promise.resolve())

vi.mock('@wailsio/runtime', () => ({
  Events: { On: (name: string, cb: (e: any) => void) => handlers.set(name, cb), Emit: (...a: unknown[]) => emit(...a) },
}))
vi.mock('./stores', () => ({ pushToast: vi.fn() }))

import { pushToast } from './stores'
import { checkForUpdates, wireUpdaterEvents } from './updater'

const fire = (name: string, data?: unknown) => handlers.get(`wails:updater:${name}`)!({ data })
const lastToast = () => vi.mocked(pushToast).mock.calls.at(-1)

describe('updater toasts', () => {
  wireUpdaterEvents()
  beforeEach(() => vi.clearAllMocks())

  it('keeps background checks silent until an update is ready', () => {
    fire('update-available', { version: '1.2.0' })
    fire('no-update')
    fire('error', { message: 'offline' })
    expect(pushToast).not.toHaveBeenCalled()
  })

  it('reports every outcome of a menu check', () => {
    checkForUpdates()
    expect(emit).toHaveBeenCalledWith('updater.check')
    fire('no-update')
    expect(lastToast()?.slice(0, 2)).toEqual(['success', 'Recur is up to date'])

    checkForUpdates()
    fire('error', { message: 'offline' })
    expect(lastToast()?.slice(0, 2)).toEqual(['error', 'Update failed: offline'])
  })

  it('offers a restart once ready and re-shows it instead of re-checking', () => {
    fire('update-ready', { version: '1.2.0' })
    const [kind, text, extra] = lastToast()!
    expect([kind, text]).toEqual(['info', 'Recur 1.2.0 is ready to install'])
    ;(extra as any).action.fn()
    expect(emit).toHaveBeenCalledWith('wails:updater:user:restart')

    emit.mockClear()
    checkForUpdates()
    expect(emit).not.toHaveBeenCalled()
    expect(lastToast()?.[1]).toBe('Recur 1.2.0 is ready to install')
  })
})
