import { writable } from 'svelte/store'

export type KeybindingMenuItem = {
  id: string
  label: string
  category: string
  order: number
}

export type KeybindingMenuState = {
  items: KeybindingMenuItem[]
  onPick: (item: KeybindingMenuItem) => void
}

/** TextMate-style numbered picker (null when closed). */
export const keybindingMenu = writable<KeybindingMenuState | null>(null)

export function openKeybindingMenu(
  items: KeybindingMenuItem[],
  onPick: (item: KeybindingMenuItem) => void,
) {
  keybindingMenu.set({ items, onPick })
}

export function closeKeybindingMenu() {
  keybindingMenu.set(null)
}
