import { writable } from 'svelte/store'
import { ShellService } from '../../bindings/recur/internal/app'

/** True when this run draws its own title bar and menu (Linux, native chrome off). */
export const customChrome = writable(false)

/** Read the startup chrome mode once; the saved setting may differ until restart. */
export async function loadChrome(): Promise<void> {
  customChrome.set(await ShellService.HasCustomChrome().catch(() => false))
}
