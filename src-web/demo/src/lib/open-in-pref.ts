import { writable } from 'svelte/store'

const KEY = 'recur.openInDefault'

function readDefault(): string {
  try {
    return localStorage.getItem(KEY) ?? ''
  } catch {
    return ''
  }
}

/** Preferred Open in target id (cursor, finder, …). Empty = catalog order. */
export const openInDefault = writable(readDefault())

export function setOpenInDefault(id: string) {
  const next = id.trim()
  if (next === 'copy-path') return
  openInDefault.set(next)
  try {
    if (next) localStorage.setItem(KEY, next)
    else localStorage.removeItem(KEY)
  } catch {
    /* ignore */
  }
}

/** Move the default target to the front; no-op if it isn't in the list. */
export function orderOpenInTargets<T extends { id: string }>(list: T[], defaultId: string): T[] {
  if (!defaultId) return list
  const i = list.findIndex((t) => t.id === defaultId)
  if (i <= 0) return list
  return [list[i], ...list.slice(0, i), ...list.slice(i + 1)]
}
